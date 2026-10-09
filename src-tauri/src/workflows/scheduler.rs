use async_trait::async_trait;
use futures_util::FutureExt;
use rusqlite::TransactionBehavior;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
use tokio::{sync::watch, task::JoinSet};

use super::*;

/// Returning a known disposition certifies execution has stopped. Unknown
/// keeps its admission/write holds; a successful provider interrupt alone is
/// insufficient evidence for a known disposition.
#[async_trait]
pub trait WorkflowDriver: Send + Sync {
    async fn execute(
        &self,
        dispatch: Dispatch,
        service: WorkflowService,
        cancellation: watch::Receiver<bool>,
    ) -> ExecutionReport;
}

#[derive(Debug, Clone)]
pub struct DryRunDriver {
    pub delay: Duration,
    pub fail_tasks: HashSet<String>,
}
impl Default for DryRunDriver {
    fn default() -> Self {
        Self {
            delay: Duration::from_millis(25),
            fail_tasks: HashSet::new(),
        }
    }
}
#[async_trait]
impl WorkflowDriver for DryRunDriver {
    async fn execute(
        &self,
        dispatch: Dispatch,
        _service: WorkflowService,
        mut cancellation: watch::Receiver<bool>,
    ) -> ExecutionReport {
        if *cancellation.borrow() {
            return dry_run_report(ExecutionReport::cancelled());
        }
        tokio::select! {
            _=tokio::time::sleep(self.delay)=>{},
            _=cancellation.changed()=>{return dry_run_report(ExecutionReport::cancelled());}
        }
        if self.fail_tasks.contains(&dispatch.task_id) {
            return dry_run_report(ExecutionReport::failed(
                "Configured dry-run failure; no model was called",
            ));
        }
        let output=dispatch.task.output_schema.as_ref().map(|schema|sample(schema,0))
            .unwrap_or_else(||json!({"dry_run":true,"task_id":dispatch.task_id,"message":"Simulated execution; no model was called"}));
        dry_run_report(ExecutionReport::success(output))
    }
}

fn dry_run_report(mut report: ExecutionReport) -> ExecutionReport {
    report.usage.tokens_unknown = false;
    report.usage.cost_usd = Some(0.0);
    report.usage.cost_unknown = false;
    report
}

fn sample(schema: &Value, depth: usize) -> Value {
    if depth > 32 {
        return Value::Null;
    }
    if let Some(value) = schema.get("const").or_else(|| schema.get("default")) {
        return value.clone();
    }
    if let Some(value) = schema
        .get("enum")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
    {
        return value.clone();
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("object") => {
            let fields = schema
                .get("properties")
                .and_then(Value::as_object)
                .map(|m| {
                    m.iter()
                        .map(|(key, value)| (key.clone(), sample(value, depth + 1)))
                        .collect()
                })
                .unwrap_or_default();
            Value::Object(fields)
        }
        Some("array") => {
            let count = schema
                .get("minItems")
                .and_then(Value::as_u64)
                .unwrap_or(0)
                .min(32);
            Value::Array(
                (0..count)
                    .map(|_| {
                        sample(
                            &schema.get("items").cloned().unwrap_or(Value::Null),
                            depth + 1,
                        )
                    })
                    .collect(),
            )
        }
        Some("integer") | Some("number") => schema.get("minimum").cloned().unwrap_or(json!(0)),
        Some("boolean") => json!(true),
        Some("null") => Value::Null,
        _ => json!("simulated"),
    }
}

impl WorkflowService {
    /// Claiming commits dispatch intent before the driver sees any work.
    pub fn claim_next(&self) -> Result<Option<Dispatch>, String> {
        let route_limits = self
            .inner
            .route_limits
            .lock()
            .map_err(|_| "Route lock poisoned")?
            .clone();
        let provider_limits = self
            .inner
            .provider_limits
            .lock()
            .map_err(|_| "Provider limit lock poisoned")?
            .clone();
        let (dispatch,changes)=self.with_connection(|c|{
            let tx=c.transaction_with_behavior(TransactionBehavior::Immediate).map_err(|e|e.to_string())?;
            let mut runs=store::list_active(&tx)?;
            let mut changes=vec![];
            for run in &mut runs {
                if (run.status==RunStatus::Running || (run.status==RunStatus::Paused && run.tasks.iter().any(|t|t.current_attempt.as_ref().is_some_and(|a|a.status.holds_capacity()))))
                    && now_ms().saturating_sub(run.created_at_ms) as u64>=run.resolved_limits.wall_time_ms {
                    run.error=Some("Workflow wall-time limit reached".into());
                    for task in &mut run.tasks {cancel_task(task);}
                    touch(run);settle(run);store::save(&tx,run,"time_limit")?;changes.push((run.clone(),"time_limit"));
                }
            }
            let active:Vec<_>=runs.iter().flat_map(|run|run.tasks.iter().filter_map(move |task|
                task.current_attempt.as_ref().filter(|a|a.status.holds_capacity()).map(|a|(run,task,a)))).collect();
            if active.len()>=self.inner.global_concurrency {
                tx.commit().map_err(|e|e.to_string())?;return Ok((None,changes));
            }
            let route_counts:HashMap<String,usize>=active.iter().fold(HashMap::new(),|mut counts,(_,_,a)|{
                *counts.entry(a.route_id.clone()).or_default()+=1;counts
            });
            let provider_counts:HashMap<String,usize>=active.iter().fold(HashMap::new(),|mut counts,(run,_,attempt)| {
                if let Some(route)=run.spec.routes.iter().find(|r|r.id==attempt.route_id) {*counts.entry(route.provider.to_ascii_lowercase()).or_default()+=1;}
                counts
            });
            // Give other runs a turn rather than monopolizing the global pool.
            let mut order:Vec<usize>=(0..runs.len()).collect();
            order.sort_by_key(|i|(active.iter().filter(|(r,_,_)|r.id==runs[*i].id).count(),runs[*i].updated_at_ms));
            let mut selected=None;
            for run_index in order {
                let run=&runs[run_index];
                if run.status!=RunStatus::Running {continue;}
                if active.iter().filter(|(r,_,_)|r.id==run.id).count()>=run.resolved_limits.concurrency {continue;}
                let attempts:usize=run.tasks.iter().map(|t|t.attempts.len()).sum();
                let exhausted=attempts>=run.resolved_limits.max_attempts || run.resolved_limits.token_budget
                    .is_some_and(|budget|run.usage.total_tokens.saturating_add(run.usage.estimated_tokens).saturating_add(run.usage.reserved_tokens)>=budget);
                if exhausted {
                    if !active.iter().any(|(r,_,_)|r.id==run.id) && run.tasks.iter().any(|t|matches!(t.status,TaskStatus::Queued|TaskStatus::Waiting)) {
                        selected=Some((run_index,None));break;
                    }
                    continue;
                }
                for (task_index,task) in run.tasks.iter().enumerate() {
                    if task.status!=TaskStatus::Queued || !task.spec.dependencies.iter()
                        .all(|id|run.tasks.iter().any(|t|&t.spec.id==id && t.status==TaskStatus::Succeeded))
                        || !task.waiting_for.iter().all(|id|run.tasks.iter().any(|t|&t.spec.id==id && task_terminal(t.status))) {continue;}
                    let route=task.spec.route_id.as_ref().and_then(|id|run.spec.routes.iter().find(|r|&r.id==id))
                        .unwrap_or(&run.spec.routes[task_index%run.spec.routes.len()]);
                    if *route_counts.get(&route.id).unwrap_or(&0)>=*route_limits.get(&route.id).unwrap_or(&self.inner.global_concurrency) {continue;}
                    if *provider_counts.get(&route.provider.to_ascii_lowercase()).unwrap_or(&0)>=*provider_limits.get(&route.provider.to_ascii_lowercase()).unwrap_or(&self.inner.global_concurrency) {continue;}
                    if active.iter().any(|(other,held,_)|other.spec.workspace_id==run.spec.workspace_id
                        && (task.spec.access==TaskAccess::Write || held.spec.access==TaskAccess::Write)
                        && scopes_overlap(&task.spec.scope,&held.spec.scope)) {continue;}
                    selected=Some((run_index,Some((task_index,route.clone()))));break;
                }
                if selected.is_some(){break;}
            }
            // Drop borrowed active census before modifying the selected snapshot.
            drop(active);
            let dispatch=if let Some((run_index,selection))=selected {
                let run=&mut runs[run_index];
                if let Some((task_index,route))=selection {
                    let reservation=if run.spec.mode==RunMode::DryRun {0} else {run.resolved_limits.token_budget
                        .map(|b|b.saturating_sub(run.usage.total_tokens.saturating_add(run.usage.estimated_tokens).saturating_add(run.usage.reserved_tokens)).min(4096)).unwrap_or(4096)};
                    let task=&mut run.tasks[task_index];
                    let attempt=AttemptSnapshot {id:uuid::Uuid::new_v4().to_string(),generation:task.generation,
                        operation_id:uuid::Uuid::new_v4().to_string(),status:AttemptStatus::Dispatching,route_id:route.id.clone(),
                        started_at_ms:now_ms(),finished_at_ms:None,cancel_requested:false,reserved_tokens:reservation,
                        external_ref:None,output:None,error:None,usage:Usage::default(),artifacts:vec![]};
                    task.status=TaskStatus::Running;task.error=None;task.attempts.push(attempt.clone());task.current_attempt=Some(attempt.clone());
                    let dispatch=Dispatch {run_id:run.id.clone(),task_id:task.spec.id.clone(),generation:task.generation,
                        attempt_id:attempt.id,operation_id:attempt.operation_id,task:task.spec.clone(),route,
                        mode:run.spec.mode,workspace_id:run.spec.workspace_id.clone(),goal:run.spec.goal.clone(),
                        messages:task.messages.clone(),dependency_results:vec![]};
                    let needed:Vec<_>=task.spec.dependencies.iter().cloned().collect();
                    let waited=task.waiting_for.clone();
                    let consumer=task.spec.id.clone();
                    let mut dispatch=dispatch;
                    dispatch.dependency_results=needed.iter().filter_map(|id|run.tasks.iter().find(|t|&t.spec.id==id)
                        .and_then(|t|t.result.clone()).map(|output|(id.clone(),output))).collect();
                    dispatch.dependency_results.extend(waited.iter().filter_map(|id|run.tasks.iter().find(|t|&t.spec.id==id)
                        .map(|t|(id.clone(),json!({"id":id,"status":t.status,"generation":t.generation,"result":t.result,"error":t.error})))));
                    let observations:Vec<_>=waited.iter().filter_map(|id|run.tasks.iter().find(|t|&t.spec.id==id && t.status==TaskStatus::Succeeded && !t.retired)
                        .map(|t|(id.clone(),t.generation))).collect();
                    capture_successful_dependencies(run,&consumer,&observations)?;
                    run.usage.reserved_tokens=run.usage.reserved_tokens.saturating_add(reservation);
                    touch(run);store::save(&tx,run,"dispatch_intent")?;changes.push((run.clone(),"dispatch_intent"));Some(dispatch)
                } else {
                    run.error=Some("Workflow attempt or token budget exhausted".into());
                    for task in &mut run.tasks {cancel_task(task);}
                    touch(run);settle(run);store::save(&tx,run,"budget_limit")?;changes.push((run.clone(),"budget_limit"));None
                }
            } else {None};
            tx.commit().map_err(|e|e.to_string())?;Ok((dispatch,changes))
        })?;
        for (run, kind) in changes {
            self.changed(&run, kind);
        }
        Ok(dispatch)
    }

    pub fn mark_running(&self, dispatch: &Dispatch) -> Result<RunSnapshot, String> {
        self.mutate(&dispatch.run_id, "started", None, Value::Null, |run| {
            let task = task_mut(run, &dispatch.task_id)?;
            let attempt = task.attempts.last_mut().ok_or("Missing attempt")?;
            if attempt.id != dispatch.attempt_id
                || attempt.generation != dispatch.generation
                || attempt.cancel_requested
                || attempt.status != AttemptStatus::Dispatching
            {
                return Err("Dispatch authority is stale or revoked".into());
            }
            attempt.status = AttemptStatus::Running;
            task.current_attempt = Some(attempt.clone());
            Ok(())
        })
    }

    pub fn finish_attempt(
        &self,
        dispatch: &Dispatch,
        report: ExecutionReport,
    ) -> Result<RunSnapshot, String> {
        self.finish(dispatch, report, false)
    }

    /// Host executors may resolve an ambiguous attempt only using actual
    /// native-session/process or immutable-artifact evidence, never a retry.
    pub fn reconcile_attempt(
        &self,
        dispatch: &Dispatch,
        report: ExecutionReport,
    ) -> Result<RunSnapshot, String> {
        self.finish(dispatch, report, true)
    }

    fn finish(
        &self,
        dispatch: &Dispatch,
        mut report: ExecutionReport,
        reconcile: bool,
    ) -> Result<RunSnapshot, String> {
        self.mutate(
            &dispatch.run_id,
            if reconcile { "reconciled" } else { "finished" },
            None,
            Value::Null,
            |run| {
                let position = run
                    .tasks
                    .iter()
                    .position(|t| t.spec.id == dispatch.task_id)
                    .ok_or("Unknown task")?;
                let task = &run.tasks[position];
                let attempt = task.current_attempt.as_ref().ok_or("Missing attempt")?;
                if attempt.id != dispatch.attempt_id || task.generation != dispatch.generation {
                    return Err("Stale attempt result rejected".into());
                }
                if !attempt.status.holds_capacity() {
                    return Err("Attempt is already terminal".into());
                }
                if attempt.status == AttemptStatus::Unknown && !reconcile {
                    return Err("Recovered attempt requires explicit reconciliation".into());
                }
                if let Some(value) = report.output.as_ref() {
                    if let Err(error) = validate_output(value, run.resolved_limits.max_output_bytes)
                    {
                        reject_report(&mut report, error);
                    }
                }
                if let Err(error) = validate_output(
                    &json!(&report.artifacts),
                    run.resolved_limits.max_output_bytes,
                ) {
                    reject_report(&mut report, error);
                }
                if report.disposition == ExecutionDisposition::Succeeded {
                    let valid =
                        validate_consumed_dependencies(run, &run.tasks[position]).and_then(|_| {
                            report
                                .output
                                .as_ref()
                                .ok_or_else(|| {
                                    "Successful attempt did not return a result".to_owned()
                                })
                                .and_then(|output| {
                                    if let Some(schema) = &run.tasks[position].spec.output_schema {
                                        let validator = jsonschema::validator_for(schema)
                                            .map_err(|e| e.to_string())?;
                                        validator.validate(output).map_err(|e| {
                                            format!("Task output failed its schema: {e}")
                                        })
                                    } else {
                                        Ok(())
                                    }
                                })
                        });
                    if let Err(error) = valid {
                        reject_report(&mut report, error);
                    }
                }
                if report.disposition == ExecutionDisposition::Waiting {
                    if report.waiting_for.is_empty() {
                        reject_report(&mut report, "Waiting requires child task ids".into());
                    } else {
                        let mut graph = run.tasks.clone();
                        graph[position].waiting_for = report.waiting_for.clone();
                        if let Err(error) = validate_graph(&graph, &run.spec.routes) {
                            reject_report(&mut report, error);
                        }
                    }
                }
                if let Some(error) = report.error.as_mut() {
                    let mut length = error.len().min(8192);
                    while !error.is_char_boundary(length) {
                        length -= 1;
                    }
                    error.truncate(length);
                }
                if !report_fits_retained_budget(run, &run.tasks[position], &report)? {
                    reject_report(
                        &mut report,
                        "Workflow retained output budget exceeded".into(),
                    );
                }
                let task = &mut run.tasks[position];
                let attempt = task.attempts.last_mut().ok_or("Missing attempt")?;
                if attempt.cancel_requested && report.disposition != ExecutionDisposition::Unknown {
                    report.disposition = ExecutionDisposition::Cancelled;
                }
                if reconcile {
                    report.usage.input_tokens =
                        report.usage.input_tokens.max(attempt.usage.input_tokens);
                    report.usage.output_tokens =
                        report.usage.output_tokens.max(attempt.usage.output_tokens);
                    report.usage.total_tokens =
                        report.usage.total_tokens.max(attempt.usage.total_tokens);
                    report.usage.estimated_tokens = report
                        .usage
                        .estimated_tokens
                        .max(attempt.usage.estimated_tokens);
                    report.usage.tokens_unknown &= attempt.usage.tokens_unknown;
                    report.usage.cost_usd = match (report.usage.cost_usd, attempt.usage.cost_usd) {
                        (Some(a), Some(b)) => Some(a.max(b)),
                        (a, b) => a.or(b),
                    };
                    report.usage.cost_unknown &= attempt.usage.cost_unknown;
                }
                report.usage.total_tokens = report.usage.total_tokens.max(
                    report
                        .usage
                        .input_tokens
                        .saturating_add(report.usage.output_tokens),
                );
                if report.disposition != ExecutionDisposition::Unknown
                    && report.usage.tokens_unknown
                    && run.spec.mode == RunMode::Live
                {
                    report.usage.estimated_tokens = report.usage.estimated_tokens.max(
                        attempt
                            .reserved_tokens
                            .saturating_sub(report.usage.total_tokens),
                    );
                }
                attempt.output = report.output.clone();
                attempt.error = report.error.clone();
                attempt.artifacts = report.artifacts;
                attempt.usage = report.usage.clone();
                attempt.status = match report.disposition {
                    ExecutionDisposition::Succeeded => AttemptStatus::Succeeded,
                    ExecutionDisposition::Failed => AttemptStatus::Failed,
                    ExecutionDisposition::Cancelled => AttemptStatus::Cancelled,
                    ExecutionDisposition::Waiting => AttemptStatus::Waiting,
                    ExecutionDisposition::Unknown => AttemptStatus::Unknown,
                };
                if report.disposition != ExecutionDisposition::Unknown {
                    attempt.finished_at_ms = Some(now_ms());
                    run.usage.reserved_tokens = run
                        .usage
                        .reserved_tokens
                        .saturating_sub(attempt.reserved_tokens);
                    run.usage.input_tokens = run
                        .usage
                        .input_tokens
                        .saturating_add(report.usage.input_tokens);
                    run.usage.output_tokens = run
                        .usage
                        .output_tokens
                        .saturating_add(report.usage.output_tokens);
                    run.usage.total_tokens = run.usage.total_tokens.saturating_add(
                        report.usage.total_tokens.max(
                            report
                                .usage
                                .input_tokens
                                .saturating_add(report.usage.output_tokens),
                        ),
                    );
                    if let Some(cost) = report.usage.cost_usd.filter(|c| c.is_finite() && *c >= 0.0)
                    {
                        run.usage.cost_usd = Some(run.usage.cost_usd.unwrap_or(0.0) + cost);
                    }
                    run.usage.estimated_tokens = run
                        .usage
                        .estimated_tokens
                        .saturating_add(report.usage.estimated_tokens);
                    run.usage.tokens_unknown |= report.usage.tokens_unknown;
                    run.usage.cost_unknown |= report.usage.cost_unknown;
                }
                task.status = match attempt.status {
                    AttemptStatus::Succeeded => TaskStatus::Succeeded,
                    AttemptStatus::Failed => TaskStatus::Failed,
                    AttemptStatus::Cancelled => TaskStatus::Cancelled,
                    AttemptStatus::Waiting => TaskStatus::Waiting,
                    _ => TaskStatus::Unknown,
                };
                task.error = attempt.error.clone();
                task.current_attempt = Some(attempt.clone());
                if task.status == TaskStatus::Succeeded {
                    task.result = report.output;
                    task.waiting_for.clear();
                } else if task.status == TaskStatus::Waiting {
                    task.waiting_for = report.waiting_for;
                }
                Ok(())
            },
        )
    }

    pub fn cancellation_requested(&self, dispatch: &Dispatch) -> bool {
        self.snapshot(&dispatch.run_id)
            .ok()
            .and_then(|run| {
                run.tasks
                    .into_iter()
                    .find(|t| t.spec.id == dispatch.task_id)
            })
            .and_then(|t| t.current_attempt)
            .is_none_or(|a| a.id != dispatch.attempt_id || a.cancel_requested)
    }

    /// One driver per service. Its handles stay owned until a terminal report;
    /// process interruption is intentionally not translated into fake success.
    pub async fn run_driver(&self, driver: Arc<dyn WorkflowDriver>) -> Result<(), String> {
        let (_keepalive, shutdown) = watch::channel(false);
        self.run_driver_until(driver, shutdown).await
    }

    /// Stop admission, revoke active authority, and keep execution futures
    /// owned until they report. A caller may enforce its own shutdown deadline;
    /// aborted or unknown executions remain durable reconciliation holds.
    pub async fn run_driver_until(
        &self,
        driver: Arc<dyn WorkflowDriver>,
        mut shutdown: watch::Receiver<bool>,
    ) -> Result<(), String> {
        if self.inner.driver_running.swap(true, Ordering::SeqCst) {
            return Err("Workflow driver is already running".into());
        }
        struct Reset(Arc<Inner>);
        impl Drop for Reset {
            fn drop(&mut self) {
                self.0.driver_running.store(false, Ordering::SeqCst);
            }
        }
        let _reset = Reset(self.inner.clone());
        let mut joins = JoinSet::new();
        let mut running: HashMap<String, (Dispatch, watch::Sender<bool>)> = HashMap::new();
        let mut stopping = false;
        let mut shutdown_revoked = HashSet::new();
        loop {
            stopping |= *shutdown.borrow() || self.inner.shutdown_requested.load(Ordering::SeqCst);
            if stopping {
                for (dispatch, cancel) in running.values() {
                    if !shutdown_revoked.insert(dispatch.attempt_id.clone()) {
                        continue;
                    }
                    self.mutate(
                        &dispatch.run_id,
                        "driver_shutdown",
                        None,
                        Value::Null,
                        |run| {
                            run.status = RunStatus::Paused;
                            run.pause_requested = true;
                            if let Some(script) = run.script.as_mut() {
                                if matches!(
                                    script.status,
                                    ScriptStatus::Pending | ScriptStatus::Running
                                ) {
                                    script.status = ScriptStatus::Paused;
                                }
                            }
                            let task = task_mut(run, &dispatch.task_id)?;
                            if task
                                .current_attempt
                                .as_ref()
                                .is_some_and(|a| a.id == dispatch.attempt_id && !a.cancel_requested)
                            {
                                cancel_task(task);
                            }
                            Ok(())
                        },
                    )?;
                    let _ = cancel.send(true);
                }
                if running.is_empty() {
                    return Ok(());
                }
            }
            // Load each active run once, even when it owns many workers.
            let snapshots: HashMap<String, Option<RunSnapshot>> = running
                .values()
                .map(|(dispatch, _)| dispatch.run_id.clone())
                .collect::<HashSet<_>>()
                .into_iter()
                .map(|id| {
                    let snapshot = self.snapshot(&id).ok();
                    (id, snapshot)
                })
                .collect();
            for (dispatch, cancel) in running.values() {
                let cancelled = snapshots
                    .get(&dispatch.run_id)
                    .and_then(Option::as_ref)
                    .and_then(|run| {
                        run.tasks
                            .iter()
                            .find(|task| task.spec.id == dispatch.task_id)
                    })
                    .and_then(|task| task.current_attempt.as_ref())
                    .is_none_or(|attempt| {
                        attempt.id != dispatch.attempt_id || attempt.cancel_requested
                    });
                if cancelled {
                    let _ = cancel.send(true);
                }
            }
            while !stopping {
                let Some(dispatch) = self.claim_next()? else {
                    break;
                };
                let (cancel, receiver) = watch::channel(false);
                running.insert(dispatch.attempt_id.clone(), (dispatch.clone(), cancel));
                let service = self.clone();
                let driver = driver.clone();
                joins.spawn(async move {
                    if service.mark_running(&dispatch).is_err() {
                        let mut report = ExecutionReport::cancelled();
                        report.usage.tokens_unknown = false;
                        return (dispatch, report);
                    }
                    let report = std::panic::AssertUnwindSafe(driver.execute(
                        dispatch.clone(),
                        service,
                        receiver,
                    ))
                    .catch_unwind()
                    .await
                    .unwrap_or_else(|_| {
                        ExecutionReport::unknown("Executor panicked without a quiescence report")
                    });
                    (dispatch, report)
                });
            }
            tokio::select! {
                Some(result)=joins.join_next(),if !joins.is_empty()=>{
                    match result {Ok((dispatch,report))=>{
                        running.remove(&dispatch.attempt_id);
                        if let Err(error)=self.finish_attempt(&dispatch,report) {log::warn!("Workflow result rejected: {error}");}
                    },Err(error)=>log::error!("Workflow driver task failed: {error}")}
                },
                _=self.inner.notify.notified()=>{},
                changed=shutdown.changed(),if !stopping=>{if changed.is_err() {stopping=true;}},
                _=tokio::time::sleep(if running.is_empty(){Duration::from_secs(1)}else{Duration::from_millis(100)})=>{},
            }
        }
    }
}

fn scopes_overlap(left: &[String], right: &[String]) -> bool {
    if left.is_empty() || right.is_empty() {
        return true;
    }
    fn normalize(scope: &str) -> String {
        scope.replace('\\', "/").trim_end_matches('/').to_owned()
    }
    left.iter().any(|a| {
        right.iter().any(|b| {
            let a = normalize(a);
            let b = normalize(b);
            a == "."
                || b == "."
                || a == b
                || a.starts_with(&format!("{b}/"))
                || b.starts_with(&format!("{a}/"))
        })
    })
}

fn reject_report(report: &mut ExecutionReport, error: String) {
    if report.disposition != ExecutionDisposition::Unknown {
        report.disposition = ExecutionDisposition::Failed;
    }
    report.output = None;
    report.artifacts.clear();
    report.waiting_for.clear();
    report.error = Some(error);
}
