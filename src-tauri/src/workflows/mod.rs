//! Durable orchestration owned by CodeMux, independent of model-provider sessions.
//! Scheduling intent is committed before dispatch. Ambiguous executions retain
//! admission/write ownership until an executor establishes quiescence.

pub mod artifacts;
pub mod capabilities;
pub mod config;
pub mod executor;
mod scheduler;
pub mod scripts;
mod store;
pub mod tools;
pub mod types;

pub use scheduler::{DryRunDriver, WorkflowDriver};
pub use types::*;

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::{atomic::AtomicBool, Arc, Mutex},
};
use tokio::sync::{broadcast, watch, Notify};

#[derive(Clone)]
pub struct WorkflowService {
    inner: Arc<Inner>,
}
struct Inner {
    store: store::Store,
    global_concurrency: usize,
    route_limits: Mutex<HashMap<String, usize>>,
    provider_limits: Mutex<HashMap<String, usize>>,
    notify: Notify,
    driver_running: AtomicBool,
    shutdown_requested: AtomicBool,
    shutdown: watch::Sender<bool>,
    events: broadcast::Sender<WorkflowEvent>,
}

impl WorkflowService {
    /// Persistent recovery requires exclusive runtime ownership before opening
    /// the database, after the prior owner's execution has ended. The app's
    /// WorkflowState acquires its runtime lock before calling this method.
    pub fn open(path: impl AsRef<Path>, global_concurrency: usize) -> Result<Self, String> {
        if !(1..=256).contains(&global_concurrency) {
            return Err("Global concurrency must be 1–256".into());
        }
        let (events, _) = broadcast::channel(256);
        let (shutdown, _) = watch::channel(false);
        let service = Self {
            inner: Arc::new(Inner {
                store: store::Store::open(path.as_ref())?,
                global_concurrency,
                route_limits: Mutex::new(HashMap::new()),
                provider_limits: Mutex::new(HashMap::new()),
                notify: Notify::new(),
                driver_running: AtomicBool::new(false),
                shutdown_requested: AtomicBool::new(false),
                shutdown,
                events,
            }),
        };
        service.recover()?;
        Ok(service)
    }

    pub(crate) fn with_connection<T>(
        &self,
        f: impl FnOnce(&mut Connection) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut connection = self
            .inner
            .store
            .connection
            .lock()
            .map_err(|_| "Workflow database lock poisoned")?;
        f(&mut connection)
    }

    pub fn list(&self) -> Result<Vec<RunSnapshot>, String> {
        self.with_connection(|c| store::list_recent(c, None, 200))
    }
    pub fn list_recent(
        &self,
        workspace_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<RunSnapshot>, String> {
        if limit == 0 || limit > 1000 {
            return Err("History page limit must be 1–1000".into());
        }
        self.with_connection(|c| store::list_recent(c, workspace_id, limit))
    }
    pub fn list_summaries(
        &self,
        workspace_id: &str,
        limit: usize,
    ) -> Result<Vec<RunSummary>, String> {
        if limit == 0 || limit > 1000 {
            return Err("History page limit must be 1–1000".into());
        }
        self.with_connection(|c| store::list_summaries(c, workspace_id, limit))
    }
    pub fn snapshot(&self, run_id: &str) -> Result<RunSnapshot, String> {
        self.with_connection(|c| store::load(c, run_id))
    }
    pub fn events(&self, after: i64) -> Result<Vec<WorkflowEvent>, String> {
        self.with_connection(|c| store::events(c, after))
    }
    pub fn subscribe(&self) -> broadcast::Receiver<WorkflowEvent> {
        self.inner.events.subscribe()
    }
    /// Application shutdown checkpoints queued work without cancelling the
    /// whole run. Active attempts lose authority and retain holds until stopped.
    pub fn shutdown_signal(&self) -> Result<(), String> {
        self.latch_shutdown();
        self.pause_all()
    }
    /// Revoke admission and callbacks without waiting for database access.
    pub fn latch_shutdown(&self) {
        // Revoke in-memory admission and callbacks even if the durable
        // checkpoint fails. Existing reservations remain held on that error.
        self.inner
            .shutdown_requested
            .store(true, std::sync::atomic::Ordering::SeqCst);
        self.inner.shutdown.send_replace(true);
        self.inner.notify.notify_one();
    }
    pub(super) fn shutdown_watch(&self) -> watch::Receiver<bool> {
        self.inner.shutdown.subscribe()
    }
    pub(super) fn ensure_runtime_active(&self) -> Result<(), String> {
        if self
            .inner
            .shutdown_requested
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            Err("Workflow host is shutting down".into())
        } else {
            Ok(())
        }
    }
    pub fn pause_all(&self) -> Result<(), String> {
        let changed = self.with_connection(|c| {
            let tx = c
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|e| e.to_string())?;
            let mut runs = store::list_active(&tx)?;
            for run in &mut runs {
                run.pause_requested = true;
                run.status = RunStatus::Paused;
                if let Some(script) = run.script.as_mut() {
                    if matches!(script.status, ScriptStatus::Pending | ScriptStatus::Running) {
                        script.status = ScriptStatus::Paused;
                    }
                }
                for task in &mut run.tasks {
                    if task
                        .current_attempt
                        .as_ref()
                        .is_some_and(|a| a.status.holds_capacity())
                    {
                        cancel_task(task);
                    }
                }
                touch(run);
                settle(run);
                store::save(&tx, run, "checkpoint")?;
            }
            tx.commit().map_err(|e| e.to_string())?;
            Ok(runs)
        })?;
        for run in changed {
            self.changed(&run, "checkpoint");
        }
        Ok(())
    }
    fn changed(&self, run: &RunSnapshot, kind: &str) {
        let _ = self.inner.events.send(WorkflowEvent {
            sequence: 0,
            run_id: run.id.clone(),
            revision: run.revision,
            kind: kind.into(),
            timestamp_ms: run.updated_at_ms,
        });
        self.inner.notify.notify_one();
    }

    pub fn set_route_capacity(&self, route_id: &str, capacity: usize) -> Result<(), String> {
        if capacity == 0 || capacity > 256 {
            return Err("Route capacity must be 1–256".into());
        }
        self.inner
            .route_limits
            .lock()
            .map_err(|_| "Route lock poisoned")?
            .insert(route_id.into(), capacity);
        self.inner.notify.notify_one();
        Ok(())
    }
    pub fn set_provider_capacity(&self, provider: &str, capacity: usize) -> Result<(), String> {
        if provider.is_empty() || provider.len() > 128 || capacity == 0 || capacity > 256 {
            return Err("Provider capacity must be 1–256".into());
        }
        self.inner
            .provider_limits
            .lock()
            .map_err(|_| "Provider limit lock poisoned")?
            .insert(provider.to_ascii_lowercase(), capacity);
        self.inner.notify.notify_one();
        Ok(())
    }

    pub fn create(&self, spec: RunSpec, idempotency_key: &str) -> Result<RunSnapshot, String> {
        self.create_internal(spec, idempotency_key, false)
            .map(|(run, _)| run)
    }
    /// The host pins an immutable baseline before activating a fresh live run.
    /// Idempotent replay returns false and never resumes existing user state.
    pub fn create_paused(
        &self,
        spec: RunSpec,
        idempotency_key: &str,
    ) -> Result<(RunSnapshot, bool), String> {
        self.create_internal(spec, idempotency_key, true)
    }
    fn create_internal(
        &self,
        mut spec: RunSpec,
        idempotency_key: &str,
        paused: bool,
    ) -> Result<(RunSnapshot, bool), String> {
        for route in &mut spec.routes {
            route.provider = route.provider.to_ascii_lowercase();
        }
        validate_id(idempotency_key)?;
        validate_run(&spec)?;
        let payload = serde_json::to_value(&spec).map_err(|e| e.to_string())?;
        let (result, fresh) = self.with_connection(|c| {
            let tx = c
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|e| e.to_string())?;
            if let Some(id) = store::operation(&tx, "create", idempotency_key, &payload)? {
                return Ok((store::load(&tx, &id)?, false));
            }
            let now = now_ms();
            let mut limits = spec.limits.clone();
            limits.concurrency = if limits.concurrency == 0 {
                4.min(self.inner.global_concurrency)
            } else {
                limits.concurrency.min(self.inner.global_concurrency)
            };
            let mut run = RunSnapshot {
                id: uuid::Uuid::new_v4().to_string(),
                status: if paused {
                    RunStatus::Paused
                } else {
                    RunStatus::Running
                },
                revision: 1,
                created_at_ms: now,
                updated_at_ms: now,
                tasks: spec.tasks.iter().cloned().map(new_task).collect(),
                script: spec.script.as_ref().map(|_| ScriptSnapshot {
                    status: if paused {
                        ScriptStatus::Paused
                    } else {
                        ScriptStatus::Pending
                    },
                    result: None,
                    error: None,
                    phase: None,
                }),
                spec,
                usage: Usage {
                    tokens_unknown: false,
                    cost_usd: Some(0.0),
                    cost_unknown: false,
                    ..Usage::default()
                },
                resolved_limits: limits,
                cancel_requested: false,
                pause_requested: paused,
                error: None,
            };
            if !paused {
                settle(&mut run);
            }
            store::save(&tx, &run, "created")?;
            store::record_operation(&tx, "create", idempotency_key, &payload, &run.id)?;
            tx.commit().map_err(|e| e.to_string())?;
            Ok((run, true))
        })?;
        if fresh {
            self.changed(&result, "created");
        }
        Ok((result, fresh))
    }

    fn mutate(
        &self,
        id: &str,
        kind: &str,
        key: Option<&str>,
        payload: Value,
        f: impl FnOnce(&mut RunSnapshot) -> Result<(), String>,
    ) -> Result<RunSnapshot, String> {
        self.mutate_checked(id, kind, key, payload, None, f)
    }

    fn mutate_checked(
        &self,
        id: &str,
        kind: &str,
        key: Option<&str>,
        payload: Value,
        authority: Option<&Dispatch>,
        f: impl FnOnce(&mut RunSnapshot) -> Result<(), String>,
    ) -> Result<RunSnapshot, String> {
        if let Some(key) = key {
            validate_id(key)?;
        }
        let result = self.with_connection(|c| {
            let tx = c
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|e| e.to_string())?;
            let scope = format!("{id}/{kind}");
            let mut run = store::load(&tx, id)?;
            if let Some(dispatch) = authority {
                self.ensure_runtime_active()?;
                authorize_in_run(&run, dispatch)?;
            }
            if let Some(key) = key {
                if store::operation(&tx, &scope, key, &payload)?.is_some() {
                    return Ok(run);
                }
            }
            f(&mut run)?;
            touch(&mut run);
            settle(&mut run);
            store::save(&tx, &run, kind)?;
            if let Some(key) = key {
                store::record_operation(&tx, &scope, key, &payload, id)?;
            }
            tx.commit().map_err(|e| e.to_string())?;
            Ok(run)
        })?;
        self.changed(&result, kind);
        Ok(result)
    }

    pub fn add_tasks(
        &self,
        id: &str,
        tasks: Vec<TaskSpec>,
        parent: Option<&str>,
        key: &str,
    ) -> Result<RunSnapshot, String> {
        let payload = json!({"tasks": tasks, "parent": parent});
        self.mutate(id, "add_tasks", Some(key), payload, |run| {
            ensure_mutable(run)?;
            add_to_run(run, tasks, parent)?;
            Ok(())
        })
    }

    pub fn pause(&self, id: &str) -> Result<RunSnapshot, String> {
        self.mutate(id, "pause", None, Value::Null, |run| {
            ensure_mutable(run)?;
            run.status = RunStatus::Paused;
            run.pause_requested = true;
            if let Some(script) = run.script.as_mut() {
                if matches!(script.status, ScriptStatus::Pending | ScriptStatus::Running) {
                    script.status = ScriptStatus::Paused;
                }
            }
            Ok(())
        })
    }

    pub fn resume(&self, id: &str) -> Result<RunSnapshot, String> {
        self.mutate(id, "resume", None, Value::Null, |run| {
            if run.tasks.iter().any(|t| t.status == TaskStatus::Unknown) {
                return Err("Reconcile unknown attempts before resuming".into());
            }
            if run.cancel_requested
                || run.error.is_some()
                || matches!(run.status, RunStatus::Completed | RunStatus::Cancelled)
            {
                return Err("This run cannot resume".into());
            }
            run.status = RunStatus::Running;
            run.pause_requested = false;
            if let Some(script) = run.script.as_mut() {
                if script.status == ScriptStatus::Paused {
                    script.status = ScriptStatus::Pending;
                }
            }
            Ok(())
        })
    }

    pub fn cancel(&self, id: &str) -> Result<RunSnapshot, String> {
        self.mutate(id, "cancel", None, Value::Null, |run| {
            run.cancel_requested = true;
            for task in &mut run.tasks {
                cancel_task(task);
            }
            if let Some(script) = run.script.as_mut() {
                if !matches!(
                    script.status,
                    ScriptStatus::Completed | ScriptStatus::Failed
                ) {
                    script.status = ScriptStatus::Paused;
                }
            }
            Ok(())
        })
    }

    pub fn retire(&self, id: &str, task_id: &str) -> Result<RunSnapshot, String> {
        self.mutate(id, "retire", None, json!(task_id), |run| {
            retire_in_run(run, task_id)
        })
    }
    pub fn cancel_task(&self, id: &str, task_id: &str) -> Result<RunSnapshot, String> {
        self.mutate(id, "cancel_task", None, json!(task_id), |run| {
            cancel_branch(run, task_id, false)
        })
    }

    pub fn retry(&self, id: &str, task_id: &str) -> Result<RunSnapshot, String> {
        self.mutate(id, "retry", None, json!(task_id), |run| {
            if run.cancel_requested { return Err("Cancelled runs cannot retry".into()); }
            if run.script.as_ref().is_some_and(|s|s.status==ScriptStatus::Failed) {return Err("Failed pinned scripts require a new run".into());}
            let task = task_mut(run, task_id)?;
            if !matches!(task.status, TaskStatus::Failed | TaskStatus::Cancelled | TaskStatus::Blocked) {
                return Err("Only terminal, quiescent tasks can retry; unknown attempts require reconciliation".into());
            }
            task.status = TaskStatus::Queued; task.error = None; task.result = None;
            task.retired=false;
            run.error = None; run.status = RunStatus::Running;
            Ok(())
        })
    }

    pub fn replace(
        &self,
        id: &str,
        task_id: &str,
        spec: TaskSpec,
        key: &str,
    ) -> Result<RunSnapshot, String> {
        self.mutate(
            id,
            "replace",
            Some(key),
            json!({"task_id":task_id,"spec":spec}),
            |run| {
                if run.cancel_requested {
                    return Err("Cancelled runs cannot replace tasks".into());
                }
                replace_in_run(run, task_id, spec)?;
                run.error = None;
                run.status = RunStatus::Running;
                Ok(())
            },
        )
    }

    pub fn message(&self, id: &str, task_id: &str, text: &str) -> Result<RunSnapshot, String> {
        if text.is_empty() || text.len() > 8192 {
            return Err("Messages must contain 1–8192 bytes".into());
        }
        self.mutate(
            id,
            "message",
            None,
            json!({"task_id":task_id,"text":text}),
            |run| {
                ensure_mutable(run)?;
                append_message(run, task_id, text)
            },
        )
    }

    pub fn set_script_status(
        &self,
        id: &str,
        status: ScriptStatus,
        result: Option<Value>,
        error: Option<String>,
    ) -> Result<RunSnapshot, String> {
        if matches!(
            status,
            ScriptStatus::Running | ScriptStatus::Completed | ScriptStatus::Failed
        ) {
            self.ensure_runtime_active()?;
        }
        self.mutate(id, "script", None, json!(status), |run| {
            if matches!(
                status,
                ScriptStatus::Running | ScriptStatus::Completed | ScriptStatus::Failed
            ) {
                self.ensure_runtime_active()?;
            }
            if run.cancel_requested {
                return Err("Cancelled run cannot execute a script".into());
            }
            if let Some(value) = result.as_ref() {
                validate_output(value, run.resolved_limits.max_output_bytes)?;
                let prior = run
                    .script
                    .as_ref()
                    .and_then(|s| s.result.as_ref())
                    .map(encoded_bytes)
                    .transpose()?
                    .unwrap_or(0);
                if retained_output_bytes(run)?
                    .saturating_sub(prior)
                    .saturating_add(encoded_bytes(value)?)
                    > MAX_RETAINED_OUTPUT_BYTES
                {
                    return Err("Workflow retained output budget exceeded".into());
                }
            }
            if error.as_ref().is_some_and(|e| e.len() > 8192) {
                return Err("Script error exceeds the byte limit".into());
            }
            let script = run.script.as_mut().ok_or("Run has no script")?;
            script.status = status;
            script.result = result;
            script.error = error;
            if status == ScriptStatus::Running {
                run.status = RunStatus::Running;
            }
            if status == ScriptStatus::Failed {
                run.error = script
                    .error
                    .clone()
                    .or_else(|| Some("Workflow script failed".into()));
                for task in &mut run.tasks {
                    cancel_task(task);
                }
            }
            Ok(())
        })
    }

    pub fn set_phase(&self, id: &str, phase: &str) -> Result<RunSnapshot, String> {
        if phase.len() > 256 {
            return Err("Phase label is too long".into());
        }
        self.mutate(id, "phase", None, json!(phase), |run| {
            ensure_mutable(run)?;
            run.script.as_mut().ok_or("Run has no script")?.phase = Some(phase.to_owned());
            Ok(())
        })
    }

    pub fn authorize_attempt(&self, dispatch: &Dispatch) -> Result<(), String> {
        self.ensure_runtime_active()?;
        let run = self.snapshot(&dispatch.run_id)?;
        self.ensure_runtime_active()?;
        authorize_in_run(&run, dispatch).map(|_| ())
    }

    pub(crate) fn observe_successful_dependencies(
        &self,
        dispatch: &Dispatch,
        observations: &[(String, u64)],
    ) -> Result<(), String> {
        if observations.is_empty() {
            return self.authorize_attempt(dispatch);
        }
        let changed = self.with_connection(|connection| {
            self.ensure_runtime_active()?;
            let tx = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| error.to_string())?;
            let mut run = store::load(&tx, &dispatch.run_id)?;
            let before = authorize_in_run(&run, dispatch)?
                .accepted_dependencies
                .len();
            for (id, _) in observations {
                authorize_target(&run, &dispatch.task_id, id)?;
                if id == &dispatch.task_id {
                    return Err("A task cannot consume its own result".into());
                }
            }
            capture_successful_dependencies(&mut run, &dispatch.task_id, observations)?;
            // Re-reading a ready child revalidates its authority without
            // rewriting the whole run or growing the event journal.
            if task_mut(&mut run, &dispatch.task_id)?
                .accepted_dependencies
                .len()
                == before
            {
                return Ok(None);
            }
            touch(&mut run);
            settle(&mut run);
            store::save(&tx, &run, "dependency_observed")?;
            tx.commit().map_err(|error| error.to_string())?;
            Ok(Some(run))
        })?;
        if let Some(run) = changed {
            self.changed(&run, "dependency_observed");
        }
        Ok(())
    }
    /// Keep generation/cancellation checks and artifact application under the
    /// same service lock. The closure must not re-enter service methods.
    pub fn with_accepted_attempt<T>(
        &self,
        run_id: &str,
        task_id: &str,
        attempt_id: &str,
        generation: u64,
        apply: impl FnOnce(&TaskSnapshot, &AttemptSnapshot) -> Result<T, String>,
    ) -> Result<T, String> {
        self.with_connection(|c| {
            self.ensure_runtime_active()?;
            let run = store::load(c, run_id)?;
            if run.cancel_requested {
                return Err("Run authority was revoked".into());
            }
            let task = run
                .tasks
                .iter()
                .find(|t| t.spec.id == task_id)
                .ok_or("Unknown task")?;
            let attempt = task
                .current_attempt
                .as_ref()
                .ok_or("Missing accepted attempt")?;
            if task.generation != generation
                || attempt.generation != generation
                || attempt.id != attempt_id
                || task.status != TaskStatus::Succeeded
                || task.retired
                || attempt.status != AttemptStatus::Succeeded
                || attempt.cancel_requested
            {
                return Err("Artifact acceptance is stale or revoked".into());
            }
            validate_consumed_dependencies(&run, task)?;
            apply(task, attempt)
        })
    }

    pub fn add_tasks_as(
        &self,
        dispatch: &Dispatch,
        mut tasks: Vec<TaskSpec>,
        key: &str,
    ) -> Result<RunSnapshot, String> {
        self.mutate_checked(
            &dispatch.run_id,
            "add_tasks_as",
            Some(key),
            json!({"parent":dispatch.task_id,"tasks":tasks}),
            Some(dispatch),
            |run| {
                let parent = authorize_in_run(run, dispatch)?.spec.clone();
                for child in &mut tasks {
                    if child.scope.is_empty() && !parent.scope.is_empty() {
                        child.scope = parent.scope.clone();
                    }
                    if child.access == TaskAccess::Write && parent.access != TaskAccess::Write {
                        return Err("Read-only workers cannot delegate writes".into());
                    }
                    if !scope_contains(&parent.scope, &child.scope) {
                        return Err("Delegation cannot widen the parent's scope".into());
                    }
                }
                add_to_run(run, tasks, Some(&dispatch.task_id))
            },
        )
    }

    pub fn retire_as(&self, dispatch: &Dispatch, target: &str) -> Result<RunSnapshot, String> {
        self.mutate_checked(
            &dispatch.run_id,
            "retire_as",
            None,
            json!({"attempt":dispatch.attempt_id,"target":target}),
            Some(dispatch),
            |run| {
                authorize_in_run(run, dispatch)?;
                authorize_target(run, &dispatch.task_id, target)?;
                retire_in_run(run, target)
            },
        )
    }
    pub fn cancel_task_as(&self, dispatch: &Dispatch, target: &str) -> Result<RunSnapshot, String> {
        self.mutate_checked(
            &dispatch.run_id,
            "cancel_task_as",
            None,
            json!({"attempt":dispatch.attempt_id,"target":target}),
            Some(dispatch),
            |run| {
                authorize_target(run, &dispatch.task_id, target)?;
                cancel_branch(run, target, false)
            },
        )
    }

    pub fn replace_as(
        &self,
        dispatch: &Dispatch,
        target: &str,
        mut spec: TaskSpec,
    ) -> Result<RunSnapshot, String> {
        self.mutate_checked(
            &dispatch.run_id,
            "replace_as",
            None,
            json!({"attempt":dispatch.attempt_id,"target":target,"spec":spec}),
            Some(dispatch),
            |run| {
                let parent = authorize_in_run(run, dispatch)?.spec.clone();
                authorize_target(run, &dispatch.task_id, target)?;
                if spec.scope.is_empty() && !parent.scope.is_empty() {
                    spec.scope = parent.scope.clone();
                }
                if spec.access == TaskAccess::Write && parent.access != TaskAccess::Write {
                    return Err("Read-only workers cannot delegate writes".into());
                }
                if !scope_contains(&parent.scope, &spec.scope) {
                    return Err("Replacement cannot widen the parent's scope".into());
                }
                replace_in_run(run, target, spec)
            },
        )
    }

    pub fn message_as(
        &self,
        dispatch: &Dispatch,
        target: &str,
        text: &str,
    ) -> Result<RunSnapshot, String> {
        if text.is_empty() || text.len() > 8192 {
            return Err("Messages must contain 1–8192 bytes".into());
        }
        self.mutate_checked(
            &dispatch.run_id,
            "message_as",
            None,
            json!({"attempt":dispatch.attempt_id,"target":target,"text":text}),
            Some(dispatch),
            |run| {
                authorize_in_run(run, dispatch)?;
                authorize_target(run, &dispatch.task_id, target)?;
                append_message(run, target, text)
            },
        )
    }

    pub fn record_external_ref(
        &self,
        id: &str,
        attempt_id: &str,
        value: Value,
    ) -> Result<RunSnapshot, String> {
        validate_output(&value, 16_384)?;
        self.mutate(id, "external_ref", None, Value::Null, |run| {
            let task = run
                .tasks
                .iter_mut()
                .find(|t| {
                    t.current_attempt
                        .as_ref()
                        .is_some_and(|a| a.id == attempt_id)
                })
                .ok_or("Unknown active attempt")?;
            let attempt = task.attempts.last_mut().ok_or("Missing attempt")?;
            if !matches!(
                attempt.status,
                AttemptStatus::Dispatching | AttemptStatus::Running
            ) || attempt.cancel_requested
            {
                return Err("Attempt authority is revoked".into());
            }
            attempt.external_ref = Some(value);
            task.current_attempt = Some(attempt.clone());
            Ok(())
        })
    }

    fn recover(&self) -> Result<(), String> {
        self.with_connection(|c| {
            let tx = c
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|e| e.to_string())?;
            for mut run in store::list_active(&tx)? {
                let mut changed = false;
                let mut released_reservations = 0u64;
                for task in &mut run.tasks {
                    if let Some(attempt) = task.attempts.last_mut() {
                        if matches!(
                            attempt.status,
                            AttemptStatus::Dispatching
                                | AttemptStatus::Running
                                | AttemptStatus::Stopping
                        ) {
                            if attempt.external_execution_fenced && attempt.external_ref.is_none() {
                                // Production acquires exclusive runtime ownership before
                                // opening this store. A fenced driver cannot launch until
                                // its durable external intent exists; no predecessor can
                                // continue and create that intent after this handoff.
                                attempt.status = if run.cancel_requested || attempt.cancel_requested {
                                    AttemptStatus::Cancelled
                                } else {
                                    AttemptStatus::Failed
                                };
                                attempt.error = Some("Execution interrupted before external startup; explicitly retry and resume to run this task".into());
                                attempt.finished_at_ms = Some(now_ms());
                                attempt.output = None;
                                attempt.artifacts.clear();
                                attempt.usage = Usage { tokens_unknown:false, cost_unknown:false, cost_usd:Some(0.0), ..Usage::default() };
                                released_reservations = released_reservations.saturating_add(attempt.reserved_tokens);
                                task.status = if attempt.status == AttemptStatus::Cancelled { TaskStatus::Cancelled } else { TaskStatus::Failed };
                                task.result = None;
                                run.pause_requested = true;
                            } else {
                                attempt.status = AttemptStatus::Unknown;
                                attempt.error = Some(
                                    "Execution interrupted; external outcome requires reconciliation"
                                        .into(),
                                );
                                task.status = TaskStatus::Unknown;
                            }
                            task.current_attempt = Some(attempt.clone());
                            task.error = attempt.error.clone();
                            changed = true;
                        }
                    }
                }
                run.usage.reserved_tokens = run.usage.reserved_tokens.saturating_sub(released_reservations);
                if let Some(script) = run.script.as_mut() {
                    if matches!(script.status, ScriptStatus::Pending | ScriptStatus::Running) {
                        script.status = ScriptStatus::Paused;
                        run.pause_requested = true;
                        changed = true;
                    }
                }
                if changed {
                    touch(&mut run);
                    settle(&mut run);
                    store::save(&tx, &run, "recovered")?;
                }
            }
            tx.commit().map_err(|e| e.to_string())
        })
    }

    pub fn journal_get(
        &self,
        id: &str,
        command_id: &str,
        payload: &Value,
    ) -> Result<Option<Value>, String> {
        self.with_connection(|c| journal_get(c, id, command_id, payload))
    }
    pub fn journal_begin(&self, id: &str, command_id: &str, payload: &Value) -> Result<(), String> {
        validate_id(command_id)?;
        self.with_connection(|c| {
            let tx = c
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|e| e.to_string())?;
            let run = store::load(&tx, id)?;
            validate_output(payload, run.resolved_limits.max_output_bytes)?;
            let existing: Option<String> = tx
                .query_row(
                    "SELECT payload FROM workflow_journal WHERE run_id=?1 AND command_id=?2",
                    params![id, command_id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| e.to_string())?;
            if let Some(encoded) = existing {
                if serde_json::from_str::<Value>(&encoded).map_err(|e| e.to_string())? != *payload {
                    return Err("Journal command payload changed during replay".into());
                }
            } else {
                let encoded = serde_json::to_string(payload).map_err(|e| e.to_string())?;
                store::reserve_journal(
                    &tx,
                    id,
                    encoded.len().saturating_add(command_id.len()),
                    1,
                    journal_entry_limit(&run),
                )?;
                tx.execute(
                    "INSERT INTO workflow_journal(run_id,command_id,payload) VALUES(?1,?2,?3)",
                    params![id, command_id, encoded],
                )
                .map_err(|e| e.to_string())?;
            }
            tx.commit().map_err(|e| e.to_string())
        })
    }
    pub fn journal_commit(
        &self,
        id: &str,
        command_id: &str,
        payload: &Value,
        result: &Value,
    ) -> Result<Value, String> {
        self.journal_begin(id, command_id, payload)?;
        self.with_connection(|c| {
            let tx = c
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|e| e.to_string())?;
            let run = store::load(&tx, id)?;
            validate_output(result, run.resolved_limits.max_output_bytes)?;
            let existing = journal_get(&tx, id, command_id, payload)?;
            if let Some(value) = existing {
                if value != *result {
                    return Err("Journal result already committed differently".into());
                }
                return Ok(value);
            }
            let encoded = serde_json::to_string(result).map_err(|e| e.to_string())?;
            store::reserve_journal(&tx, id, encoded.len(), 0, journal_entry_limit(&run))?;
            tx.execute(
                "UPDATE workflow_journal SET result=?3 WHERE run_id=?1 AND command_id=?2",
                params![id, command_id, encoded],
            )
            .map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
            Ok(result.clone())
        })
    }
    pub fn read_or_execute(
        &self,
        id: &str,
        command_id: &str,
        payload: &Value,
        f: impl FnOnce() -> Result<Value, String>,
    ) -> Result<Value, String> {
        validate_id(command_id)?;
        let completed=self.with_connection(|c|{
            let tx=c.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e|e.to_string())?;
            if let Some(value)=journal_get(&tx,id,command_id,payload)? {return Ok(Some(value));}
            let present:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM workflow_journal WHERE run_id=?1 AND command_id=?2)",params![id,command_id],|r|r.get(0)).map_err(|e|e.to_string())?;
            if present {return Err("Journal command outcome is unknown; reconcile before executing again".into());}
            let run=store::load(&tx,id)?;validate_output(payload,run.resolved_limits.max_output_bytes)?;
            let encoded=serde_json::to_string(payload).map_err(|e|e.to_string())?;
            store::reserve_journal(&tx,id,encoded.len().saturating_add(command_id.len()),1,journal_entry_limit(&run))?;
            tx.execute("INSERT INTO workflow_journal(run_id,command_id,payload) VALUES(?1,?2,?3)",params![id,command_id,encoded]).map_err(|e|e.to_string())?;
            tx.commit().map_err(|e|e.to_string())?;Ok(None)
        })?;
        if let Some(value) = completed {
            return Ok(value);
        }
        match f() {
            Ok(value) => self.journal_commit(id, command_id, payload, &value),
            Err(error) => {
                let error = scripts::bounded_error(error);
                self.with_connection(|c| {
                    let tx = c
                        .transaction_with_behavior(TransactionBehavior::Immediate)
                        .map_err(|e| e.to_string())?;
                    if journal_get(&tx, id, command_id, payload)?.is_some() {
                        return Err("Journal outcome was already committed differently".into());
                    }
                    let run = store::load(&tx, id)?;
                    store::reserve_journal(&tx, id, error.len(), 0, journal_entry_limit(&run))?;
                    tx.execute(
                        "UPDATE workflow_journal SET error=?3 WHERE run_id=?1 AND command_id=?2",
                        params![id, command_id, error],
                    )
                    .map_err(|e| e.to_string())?;
                    tx.commit().map_err(|e| e.to_string())
                })?;
                Err(error)
            }
        }
    }
}

fn journal_entry_limit(run: &RunSnapshot) -> usize {
    run.resolved_limits
        .max_attempts
        .saturating_add(run.resolved_limits.max_tasks)
        .saturating_mul(4)
        .max(1000)
}

fn journal_get(
    c: &Connection,
    id: &str,
    command_id: &str,
    payload: &Value,
) -> Result<Option<Value>, String> {
    let row: Option<(String, Option<String>, Option<String>)> = c
        .query_row(
            "SELECT payload,result,error FROM workflow_journal WHERE run_id=?1 AND command_id=?2",
            params![id, command_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    if let Some((encoded, result, error)) = row {
        if serde_json::from_str::<Value>(&encoded).map_err(|e| e.to_string())? != *payload {
            return Err("Journal command payload changed during replay".into());
        }
        if let Some(error) = error {
            return Err(error);
        }
        result
            .map(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
            .transpose()
    } else {
        Ok(None)
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
fn touch(run: &mut RunSnapshot) {
    run.revision += 1;
    run.updated_at_ms = now_ms();
}
fn new_task(spec: TaskSpec) -> TaskSnapshot {
    TaskSnapshot {
        spec,
        retired: false,
        generation: 1,
        status: TaskStatus::Queued,
        depth: 0,
        parent_task_id: None,
        current_attempt: None,
        attempts: vec![],
        result: None,
        error: None,
        waiting_for: vec![],
        accepted_dependencies: Default::default(),
        messages: vec![],
    }
}
fn task_mut<'a>(run: &'a mut RunSnapshot, id: &str) -> Result<&'a mut TaskSnapshot, String> {
    run.tasks
        .iter_mut()
        .find(|t| t.spec.id == id)
        .ok_or_else(|| format!("Unknown task: {id}"))
}
fn ensure_mutable(run: &RunSnapshot) -> Result<(), String> {
    if run.cancel_requested
        || run.error.is_some()
        || matches!(
            run.status,
            RunStatus::Completed | RunStatus::Cancelled | RunStatus::Unknown | RunStatus::Stopping
        )
    {
        Err("Run is terminal, stopping, or awaiting reconciliation".into())
    } else {
        Ok(())
    }
}
fn authorize_in_run<'a>(
    run: &'a RunSnapshot,
    dispatch: &Dispatch,
) -> Result<&'a TaskSnapshot, String> {
    // Stopping can describe one cancelled branch. Global revocation remains
    // controlled by run flags, and each admitted attempt keeps its own fence.
    if run.cancel_requested
        || run.error.is_some()
        || !matches!(run.status, RunStatus::Running | RunStatus::Paused | RunStatus::Stopping)
    {
        return Err("Run authority is paused or revoked".into());
    }
    let task = run
        .tasks
        .iter()
        .find(|t| t.spec.id == dispatch.task_id)
        .ok_or("Unknown task")?;
    let attempt = task.current_attempt.as_ref().ok_or("Missing attempt")?;
    if task.generation != dispatch.generation
        || task.retired
        || attempt.id != dispatch.attempt_id
        || attempt.cancel_requested
        || !matches!(
            attempt.status,
            AttemptStatus::Dispatching | AttemptStatus::Running
        )
    {
        return Err("Attempt authority is stale or revoked".into());
    }
    Ok(task)
}
fn authorize_target(run: &RunSnapshot, parent: &str, target: &str) -> Result<(), String> {
    let mut id = target;
    for _ in 0..=run.resolved_limits.max_depth {
        if id == parent {
            return Ok(());
        }
        let task = run
            .tasks
            .iter()
            .find(|t| t.spec.id == id)
            .ok_or("Unknown task")?;
        id = task
            .parent_task_id
            .as_deref()
            .ok_or("Worker can only control itself and its descendants")?;
    }
    Err("Worker can only control itself and its descendants".into())
}
fn scope_contains(parent: &[String], child: &[String]) -> bool {
    if parent.is_empty() {
        return true;
    }
    if child.is_empty() {
        return false;
    }
    child.iter().all(|child| {
        parent.iter().any(|parent| {
            parent == "." || child == parent || child.starts_with(&format!("{parent}/"))
        })
    })
}
fn add_to_run(
    run: &mut RunSnapshot,
    tasks: Vec<TaskSpec>,
    parent: Option<&str>,
) -> Result<(), String> {
    if run.tasks.len().saturating_add(tasks.len()) > run.resolved_limits.max_tasks {
        return Err("Task limit reached".into());
    }
    let depth = if let Some(parent) = parent {
        run.tasks
            .iter()
            .find(|t| t.spec.id == parent)
            .ok_or("Unknown parent task")?
            .depth
            + 1
    } else {
        0
    };
    if depth > run.resolved_limits.max_depth {
        return Err("Delegation depth limit reached".into());
    }
    for spec in tasks {
        if spec.access == TaskAccess::Write && !run.spec.allow_writes {
            return Err("Writes require explicit run authorization".into());
        }
        if run.tasks.iter().any(|t| t.spec.id == spec.id) {
            return Err(format!("Duplicate task id: {}", spec.id));
        }
        let mut task = new_task(spec);
        task.depth = depth;
        task.parent_task_id = parent.map(str::to_owned);
        run.tasks.push(task);
    }
    validate_graph(&run.tasks, &run.spec.routes)?;
    run.spec.tasks = run.tasks.iter().map(|t| t.spec.clone()).collect();
    Ok(())
}
fn retire_in_run(run: &mut RunSnapshot, target: &str) -> Result<(), String> {
    cancel_branch(run, target, true)
}
fn cancel_branch(run: &mut RunSnapshot, target: &str, retire: bool) -> Result<(), String> {
    if !run.tasks.iter().any(|t| t.spec.id == target) {
        return Err("Unknown task".into());
    }
    let mut cancelled = HashSet::from([target.to_owned()]);
    loop {
        let before = cancelled.len();
        for task in &run.tasks {
            if task
                .parent_task_id
                .as_ref()
                .is_some_and(|p| cancelled.contains(p))
            {
                cancelled.insert(task.spec.id.clone());
            }
        }
        if before == cancelled.len() {
            break;
        }
    }
    for task in &mut run.tasks {
        if cancelled.contains(&task.spec.id) {
            if retire {
                task.retired = true;
            }
            cancel_task(task);
        }
    }
    Ok(())
}
fn replace_in_run(run: &mut RunSnapshot, id: &str, spec: TaskSpec) -> Result<(), String> {
    if spec.id != id {
        return Err("Replacement must preserve task identity".into());
    }
    if spec.access == TaskAccess::Write && !run.spec.allow_writes {
        return Err("Writes require explicit run authorization".into());
    }
    let mut invalid = HashSet::from([id.to_owned()]);
    loop {
        let before = invalid.len();
        for task in &run.tasks {
            if task.spec.dependencies.iter().any(|d| invalid.contains(d))
                || task
                    .accepted_dependencies
                    .keys()
                    .any(|d| invalid.contains(d))
            {
                invalid.insert(task.spec.id.clone());
            }
        }
        if before == invalid.len() {
            break;
        }
    }
    for task in &mut run.tasks {
        if !invalid.contains(&task.spec.id) {
            continue;
        }
        if task.retired && task.spec.id != id {
            continue;
        }
        if task
            .current_attempt
            .as_ref()
            .is_some_and(|a| a.status.holds_capacity())
        {
            return Err("Stop dependent active attempts before changing their inputs".into());
        }
        if task.spec.id == id {
            task.spec = spec.clone();
            task.waiting_for.clear();
        }
        task.generation += 1;
        task.accepted_dependencies.clear();
        task.retired = false;
        task.status = TaskStatus::Queued;
        task.result = None;
        task.error = None;
    }
    validate_graph(&run.tasks, &run.spec.routes)?;
    run.spec.tasks = run.tasks.iter().map(|t| t.spec.clone()).collect();
    Ok(())
}
fn capture_successful_dependencies(
    run: &mut RunSnapshot,
    consumer: &str,
    observations: &[(String, u64)],
) -> Result<(), String> {
    for (id, generation) in observations {
        let dependency = run
            .tasks
            .iter()
            .find(|task| &task.spec.id == id)
            .ok_or("Unknown observed dependency")?;
        if dependency.retired
            || dependency.status != TaskStatus::Succeeded
            || dependency.generation != *generation
            || dependency.current_attempt.as_ref().is_none_or(|attempt| {
                attempt.generation != *generation
                    || attempt.cancel_requested
                    || attempt.status != AttemptStatus::Succeeded
            })
        {
            return Err("Observed dependency result is stale or revoked".into());
        }
        let task = task_mut(run, consumer)?;
        if task
            .accepted_dependencies
            .get(id)
            .is_some_and(|previous| previous != generation)
        {
            return Err("Consumed dependency generation changed".into());
        }
        task.accepted_dependencies.insert(id.clone(), *generation);
    }
    Ok(())
}

fn validate_consumed_dependencies(
    run: &RunSnapshot,
    consumer: &TaskSnapshot,
) -> Result<(), String> {
    for (id, generation) in &consumer.accepted_dependencies {
        let dependency = run
            .tasks
            .iter()
            .find(|task| &task.spec.id == id)
            .ok_or("Unknown consumed dependency")?;
        if dependency.retired
            || dependency.status != TaskStatus::Succeeded
            || dependency.generation != *generation
            || dependency.current_attempt.as_ref().is_none_or(|attempt| {
                attempt.generation != *generation
                    || attempt.cancel_requested
                    || attempt.status != AttemptStatus::Succeeded
            })
        {
            return Err("Consumed dependency result is stale or revoked".into());
        }
    }
    for id in &consumer.spec.dependencies {
        if run
            .tasks
            .iter()
            .find(|task| &task.spec.id == id)
            .is_none_or(|dependency| {
                dependency.retired || dependency.status != TaskStatus::Succeeded
            })
        {
            return Err("Declared dependency result is stale or revoked".into());
        }
    }
    Ok(())
}

fn cancel_task(task: &mut TaskSnapshot) {
    if let Some(attempt) = task
        .attempts
        .last_mut()
        .filter(|a| a.status.holds_capacity())
    {
        attempt.cancel_requested = true;
        if attempt.status != AttemptStatus::Unknown {
            attempt.status = AttemptStatus::Stopping;
            task.status = TaskStatus::Stopping;
        }
        task.current_attempt = Some(attempt.clone());
    } else if !matches!(
        task.status,
        TaskStatus::Succeeded | TaskStatus::Failed | TaskStatus::Cancelled
    ) {
        task.status = TaskStatus::Cancelled;
    }
}

fn settle(run: &mut RunSnapshot) {
    // A topological pass propagates failures and repairs through the entire DAG.
    if let Ok(order) = dag_order(&run.tasks) {
        let positions: HashMap<String, usize> = run
            .tasks
            .iter()
            .enumerate()
            .map(|(i, t)| (t.spec.id.clone(), i))
            .collect();
        for index in order {
            let task = &run.tasks[index];
            if !matches!(
                task.status,
                TaskStatus::Queued | TaskStatus::Blocked | TaskStatus::Waiting
            ) {
                continue;
            }
            let dependencies: Vec<&TaskSnapshot> = task
                .spec
                .dependencies
                .iter()
                .filter_map(|id| positions.get(id).map(|i| &run.tasks[*i]))
                .collect();
            let waited: Vec<&TaskSnapshot> = task
                .waiting_for
                .iter()
                .filter_map(|id| positions.get(id).map(|i| &run.tasks[*i]))
                .collect();
            let broken = dependencies.iter().any(|dependency| {
                dependency.retired
                    || matches!(
                        dependency.status,
                        TaskStatus::Failed | TaskStatus::Cancelled | TaskStatus::Blocked
                    )
            }) || waited.iter().any(|dependency| dependency.retired);
            let waiting = !task.waiting_for.is_empty()
                && !waited
                    .iter()
                    .all(|dependency| task_terminal(dependency.status));
            let task = &mut run.tasks[index];
            if broken {
                task.status = TaskStatus::Blocked;
                task.error = Some("A required dependency did not succeed".into());
            } else {
                task.error = None;
                task.status = if waiting {
                    TaskStatus::Waiting
                } else {
                    TaskStatus::Queued
                };
            }
        }
    }
    if run.tasks.iter().any(|t| t.status == TaskStatus::Unknown) {
        run.status = RunStatus::Unknown;
    } else if run.tasks.iter().any(|t| t.status == TaskStatus::Stopping) {
        run.status = RunStatus::Stopping;
    } else if run.cancel_requested {
        run.status = RunStatus::Cancelled;
    } else if run.error.is_some() {
        run.status = RunStatus::Failed;
    } else if run
        .script
        .as_ref()
        .is_some_and(|s| s.status == ScriptStatus::Failed)
    {
        run.status = RunStatus::Failed;
    } else if run.script.as_ref().is_some_and(|s| {
        matches!(
            s.status,
            ScriptStatus::Pending | ScriptStatus::Running | ScriptStatus::Paused
        )
    }) {
        run.status = if run.pause_requested
            || run
                .script
                .as_ref()
                .is_some_and(|s| s.status == ScriptStatus::Paused)
        {
            RunStatus::Paused
        } else {
            RunStatus::Running
        };
    } else if run.tasks.iter().any(|t| {
        matches!(
            t.status,
            TaskStatus::Queued | TaskStatus::Running | TaskStatus::Waiting
        )
    }) {
        run.status = if run.pause_requested {
            RunStatus::Paused
        } else {
            RunStatus::Running
        };
    } else if run
        .tasks
        .iter()
        .any(|t| t.spec.required && !t.retired && t.status != TaskStatus::Succeeded)
    {
        run.status = RunStatus::Failed;
    } else {
        run.status = RunStatus::Completed;
    }
}
fn task_terminal(status: TaskStatus) -> bool {
    matches!(
        status,
        TaskStatus::Succeeded
            | TaskStatus::Failed
            | TaskStatus::Cancelled
            | TaskStatus::Blocked
            | TaskStatus::Unknown
    )
}

fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 160
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./:".contains(&b))
    {
        return Err("Identifiers must contain 1–160 ASCII letters, digits, or -_./:".into());
    }
    Ok(())
}

fn validate_run(spec: &RunSpec) -> Result<(), String> {
    if spec.workspace_id.is_empty()
        || spec.title.is_empty()
        || spec.title.len() > 2048
        || spec.goal.len() > 65536
    {
        return Err("Invalid workflow workspace/title/goal".into());
    }
    let l = &spec.limits;
    if l.concurrency > 256
        || l.max_tasks == 0
        || l.max_tasks > 10000
        || l.max_attempts == 0
        || l.max_attempts > 50000
        || l.max_depth > 32
        || l.max_output_bytes == 0
        || l.max_output_bytes > 16_777_216
        || l.wall_time_ms == 0
        || l.wall_time_ms > 604_800_000
    {
        return Err("Workflow limits are outside supported bounds".into());
    }
    if spec.tasks.len() > l.max_tasks {
        return Err("Task limit reached".into());
    }
    if !spec.allow_writes && spec.tasks.iter().any(|t| t.access == TaskAccess::Write) {
        return Err("Writes require explicit run authorization".into());
    }
    if spec.routes.is_empty() || spec.routes.len() > 64 {
        return Err("Select 1–64 workflow routes".into());
    }
    let mut ids = HashSet::new();
    for route in &spec.routes {
        validate_id(&route.id)?;
        if route.provider.is_empty() || route.provider.len() > 128 || !ids.insert(&route.id) {
            return Err("Invalid or duplicate workflow route".into());
        }
    }
    if let Some(script) = &spec.script {
        if script.source.len() > 131072 || script.api_version != 1 {
            return Err("Unsupported or oversized workflow script".into());
        }
        validate_output(&script.args, l.max_output_bytes)?;
    }
    validate_graph(
        &spec.tasks.iter().cloned().map(new_task).collect::<Vec<_>>(),
        &spec.routes,
    )
}

fn validate_graph(tasks: &[TaskSnapshot], routes: &[RouteSpec]) -> Result<(), String> {
    let mut ids = HashSet::new();
    let mut input_bytes = 0usize;
    for task in tasks {
        input_bytes = input_bytes.saturating_add(
            serde_json::to_vec(&task.spec)
                .map_err(|e| e.to_string())?
                .len(),
        );
        if input_bytes > 16_777_216 {
            return Err("Workflow task specifications exceed the aggregate byte limit".into());
        }
        validate_id(&task.spec.id)?;
        if !ids.insert(task.spec.id.clone()) {
            return Err("Duplicate task identifier".into());
        }
        if task.spec.title.is_empty()
            || task.spec.title.len() > 2048
            || task.spec.prompt.len() > 65536
        {
            return Err("Invalid task title or prompt length".into());
        }
        if task
            .spec
            .route_id
            .as_ref()
            .is_some_and(|id| !routes.iter().any(|r| &r.id == id))
        {
            return Err("Task selects an unknown route".into());
        }
        if task.spec.dependencies.len() > 256
            || task.waiting_for.len() > 500
            || task.spec.scope.len() > 128
        {
            return Err("Task dependency/scope limit exceeded".into());
        }
        if task.spec.access == TaskAccess::Write && task.spec.scope.is_empty() {
            return Err("Write tasks require explicit scopes".into());
        }
        for scope in &task.spec.scope {
            if scope.is_empty()
                || scope.len() > 4096
                || Path::new(scope).is_absolute()
                || scope.contains([':', '\\'])
                || (scope != "."
                    && scope
                        .split('/')
                        .any(|s| s.is_empty() || s == "." || s == ".." || s == ".git"))
            {
                return Err(
                    "Task scope must stay inside the workspace and outside Git metadata".into(),
                );
            }
        }
        if let Some(schema) = &task.spec.output_schema {
            validate_schema(schema)?;
        }
    }
    dag_order(tasks).map(|_| ())
}

fn dag_order(tasks: &[TaskSnapshot]) -> Result<Vec<usize>, String> {
    let positions: HashMap<&str, usize> = tasks
        .iter()
        .enumerate()
        .map(|(i, t)| (t.spec.id.as_str(), i))
        .collect();
    let mut edges = vec![Vec::new(); tasks.len()];
    let mut degrees = vec![0usize; tasks.len()];
    for (index, task) in tasks.iter().enumerate() {
        let mut seen = HashSet::new();
        for dependency in task.spec.dependencies.iter().chain(&task.waiting_for) {
            let parent = *positions
                .get(dependency.as_str())
                .ok_or_else(|| format!("Unknown dependency: {dependency}"))?;
            if seen.insert(parent) {
                edges[parent].push(index);
                degrees[index] += 1;
            }
        }
    }
    let mut queue: std::collections::VecDeque<usize> = degrees
        .iter()
        .enumerate()
        .filter_map(|(i, n)| (*n == 0).then_some(i))
        .collect();
    let mut order = vec![];
    while let Some(index) = queue.pop_front() {
        order.push(index);
        for next in &edges[index] {
            degrees[*next] -= 1;
            if degrees[*next] == 0 {
                queue.push_back(*next);
            }
        }
    }
    if order.len() != tasks.len() {
        return Err("Task graph contains a cycle".into());
    }
    Ok(order)
}

fn validate_schema(schema: &Value) -> Result<(), String> {
    validate_output(schema, 131072)?;
    fn walk(
        value: &Value,
        root: &Value,
        depth: usize,
        refs: &mut HashSet<String>,
        visits: &mut usize,
    ) -> Result<(), String> {
        *visits += 1;
        if *visits > 100_000 {
            return Err("Schema expansion limit exceeded".into());
        }
        if depth > 64 {
            return Err("Schema recursion limit exceeded".into());
        }
        match value {
            Value::Object(map) => {
                for keyword in ["$ref", "$dynamicRef", "$recursiveRef"] {
                    if let Some(reference) = map.get(keyword).and_then(Value::as_str) {
                        if !reference.starts_with('#') {
                            return Err("External schema references are disabled".into());
                        }
                        if !refs.insert(reference.into()) {
                            return Err("Cyclic schema references are unsupported".into());
                        }
                        let target = root
                            .pointer(&reference[1..])
                            .ok_or("Schema reference cannot be resolved")?;
                        walk(target, root, depth + 1, refs, visits)?;
                        refs.remove(reference);
                    }
                }
                for (key, v) in map {
                    if !matches!(key.as_str(), "$ref" | "$dynamicRef" | "$recursiveRef") {
                        walk(v, root, depth + 1, refs, visits)?;
                    }
                }
            }
            Value::Array(array) => {
                for v in array {
                    walk(v, root, depth + 1, refs, visits)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    walk(schema, schema, 0, &mut HashSet::new(), &mut 0)?;
    jsonschema::validator_for(schema).map_err(|e| format!("Invalid task output schema: {e}"))?;
    Ok(())
}

fn validate_output(value: &Value, max_bytes: usize) -> Result<(), String> {
    if encoded_bytes(value)? > max_bytes {
        return Err("Workflow output exceeds the byte limit".into());
    }
    fn depth(value: &Value, n: usize) -> bool {
        n <= 64
            && match value {
                Value::Object(m) => m.values().all(|v| depth(v, n + 1)),
                Value::Array(a) => a.iter().all(|v| depth(v, n + 1)),
                _ => true,
            }
    }
    if !depth(value, 0) {
        return Err("Workflow output exceeds the nesting limit".into());
    }
    Ok(())
}

const MAX_RETAINED_OUTPUT_BYTES: usize = 32 * 1024 * 1024;
const MAX_RETAINED_MESSAGE_BYTES: usize = 4 * 1024 * 1024;
fn append_message(run: &mut RunSnapshot, target: &str, text: &str) -> Result<(), String> {
    let retained = run
        .tasks
        .iter()
        .flat_map(|task| task.messages.iter())
        .fold(0usize, |bytes, message| bytes.saturating_add(message.len()));
    if retained.saturating_add(text.len()) > MAX_RETAINED_MESSAGE_BYTES {
        return Err("Run message byte budget reached".into());
    }
    let task = task_mut(run, target)?;
    if task.messages.len() >= 128 {
        return Err("Task message limit reached".into());
    }
    task.messages.push(text.into());
    Ok(())
}
fn encoded_bytes<T: serde::Serialize>(value: &T) -> Result<usize, String> {
    struct Counter(usize);
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len());
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, value).map_err(|e| e.to_string())?;
    Ok(counter.0)
}
fn optional_bytes(value: Option<&Value>) -> Result<usize, String> {
    value.map(encoded_bytes).transpose().map(|v| v.unwrap_or(0))
}
fn attempt_payload_bytes(attempt: &AttemptSnapshot) -> Result<usize, String> {
    Ok(optional_bytes(attempt.output.as_ref())?.saturating_add(encoded_bytes(&attempt.artifacts)?))
}
fn retained_output_bytes(run: &RunSnapshot) -> Result<usize, String> {
    let mut size = run
        .script
        .as_ref()
        .and_then(|s| s.result.as_ref())
        .map(encoded_bytes)
        .transpose()?
        .unwrap_or(0);
    for task in &run.tasks {
        size = size.saturating_add(optional_bytes(task.result.as_ref())?);
        if let Some(attempt) = task.current_attempt.as_ref() {
            size = size.saturating_add(attempt_payload_bytes(attempt)?);
        }
        for attempt in &task.attempts {
            size = size.saturating_add(attempt_payload_bytes(attempt)?);
        }
    }
    Ok(size)
}
fn report_fits_retained_budget(
    run: &RunSnapshot,
    task: &TaskSnapshot,
    report: &ExecutionReport,
) -> Result<bool, String> {
    let old = task
        .current_attempt
        .as_ref()
        .map(attempt_payload_bytes)
        .transpose()?
        .unwrap_or(0);
    let new =
        optional_bytes(report.output.as_ref())?.saturating_add(encoded_bytes(&report.artifacts)?);
    let mut size = retained_output_bytes(run)?
        .saturating_sub(old.saturating_mul(2))
        .saturating_add(new.saturating_mul(2));
    if report.disposition == ExecutionDisposition::Succeeded {
        size = size
            .saturating_sub(optional_bytes(task.result.as_ref())?)
            .saturating_add(optional_bytes(report.output.as_ref())?);
    }
    Ok(size <= MAX_RETAINED_OUTPUT_BYTES)
}

#[cfg(test)]
mod runtime_integration_tests;
#[cfg(test)]
mod tests;
