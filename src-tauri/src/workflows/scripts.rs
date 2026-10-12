//! Restricted orchestration JavaScript. QuickJS receives only journaled host
//! functions: no Node, browser, filesystem, network, credentials or loaders.
//! Completed observations replay in their original delivery order; mutations
//! with an uncertain durable intent fail closed instead of being reissued.

use rquickjs::{
    prelude::{Async, Func},
    AsyncContext, AsyncRuntime, Promise,
};
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{HashSet, VecDeque},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::sync::{watch, Notify};

use super::{RunSnapshot, RunStatus, ScriptStatus, TaskSpec, TaskStatus, WorkflowService};

pub const SCRIPT_API_VERSION: u32 = 1;
pub const MAX_SCRIPT_BYTES: usize = 128 * 1024;
const MAX_HOST_CALLS: u64 = 10_000;
const MAX_JS_MEMORY: usize = 32 * 1024 * 1024;
const MAX_INTERRUPTS: u64 = 100_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptTemplate {
    pub id: String,
    pub workspace_id: String,
    pub title: String,
    pub source: String,
    pub updated_at_ms: i64,
}

#[derive(Clone, Default)]
pub struct WorkflowScriptRuntime {
    running: Arc<Mutex<HashSet<String>>>,
    idle: Arc<Notify>,
}

impl WorkflowScriptRuntime {
    pub fn is_running(&self, id: &str) -> bool {
        self.running.lock().is_ok_and(|set| set.contains(id))
    }

    /// Observe real completion of owned script threads; the caller chooses a
    /// deadline. A blocked thread is never treated as safely aborted.
    pub async fn drain_active(&self) -> Result<(), String> {
        loop {
            let idle = self.idle.notified();
            tokio::pin!(idle);
            idle.as_mut().enable();
            if self
                .running
                .lock()
                .map_err(|_| "Script runtime unavailable")?
                .is_empty()
            {
                return Ok(());
            }
            idle.await;
        }
    }

    pub fn start(&self, service: WorkflowService, id: &str) -> Result<(), String> {
        service.ensure_runtime_active()?;
        let run = service.snapshot(id)?;
        if run.cancel_requested
            || run.script.as_ref().is_some_and(|script| {
                matches!(
                    script.status,
                    ScriptStatus::Completed | ScriptStatus::Failed
                )
            })
        {
            return Ok(());
        }
        let script = run.spec.script.as_ref().ok_or("Run has no script")?;
        validate_source(&script.source)?;
        if script.api_version != SCRIPT_API_VERSION {
            return Err("Unsupported workflow script API version".into());
        }
        initialize(&service)?;
        {
            let mut running = self
                .running
                .lock()
                .map_err(|_| "Script runtime unavailable")?;
            if running.contains(id) {
                return Ok(());
            }
            if running.len() >= 4 {
                return Err("Four scripts are already active. Finish or cancel a run before starting another.".into());
            }
            service.ensure_runtime_active()?;
            running.insert(id.to_owned());
        }
        let runtime = self.clone();
        let id = id.to_owned();
        tauri::async_runtime::spawn_blocking(move || {
            let cleanup = RunningScript {
                running: runtime.running.clone(),
                idle: runtime.idle.clone(),
                id: id.clone(),
            };
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| e.to_string())
                    .and_then(|executor| {
                        executor.block_on(execute(service.clone(), &id, MAX_INTERRUPTS))
                    })
            }))
            .unwrap_or_else(|_| Err("Workflow script runtime panicked".into()));
            // The shutdown checkpoint owns durable paused state. A late script
            // result must neither revive it nor wait for database publication.
            if service.ensure_runtime_active().is_err() {
                drop(cleanup);
                return;
            }
            let state = service.snapshot(&id);
            match (result, state) {
                (Ok(result), Ok(run)) if !run.cancel_requested => {
                    if let Err(error) =
                        service.set_script_status(&id, ScriptStatus::Completed, Some(result), None)
                    {
                        let _ = service.set_script_status(
                            &id,
                            ScriptStatus::Failed,
                            None,
                            Some(bounded_error(error)),
                        );
                    }
                }
                (Err(error), Ok(run)) if !run.cancel_requested => {
                    let status = if matches!(run.status, RunStatus::Paused | RunStatus::Unknown | RunStatus::Stopping) {
                        ScriptStatus::Paused
                    } else {
                        ScriptStatus::Failed
                    };
                    let _ =
                        service.set_script_status(&id, status, None, Some(bounded_error(error)));
                }
                _ => {}
            }
            drop(cleanup);
        });
        Ok(())
    }
}

struct RunningScript {
    running: Arc<Mutex<HashSet<String>>>,
    idle: Arc<Notify>,
    id: String,
}
impl Drop for RunningScript {
    fn drop(&mut self) {
        if let Ok(mut running) = self.running.lock() {
            running.remove(&self.id);
        }
        self.idle.notify_waiters();
    }
}
struct MonitorGuard(tokio::task::JoinHandle<()>);
impl Drop for MonitorGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub fn validate_source(source: &str) -> Result<(), String> {
    if source.is_empty() || source.len() > MAX_SCRIPT_BYTES {
        return Err("Script must contain 1–131072 bytes".into());
    }
    Ok(())
}

pub(crate) fn bounded_error(mut error: String) -> String {
    let mut length = error.len().min(8192);
    while !error.is_char_boundary(length) {
        length -= 1;
    }
    error.truncate(length);
    error
}

fn initialize(service: &WorkflowService) -> Result<(), String> {
    service.with_connection(|connection| {
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS workflow_script_observations (
            run_id TEXT NOT NULL, command_id INTEGER NOT NULL, payload TEXT NOT NULL,
            response TEXT NOT NULL, delivery_order INTEGER NOT NULL,
            PRIMARY KEY(run_id,command_id), UNIQUE(run_id,delivery_order));
         CREATE TABLE IF NOT EXISTS workflow_script_templates (
            id TEXT PRIMARY KEY, workspace_id TEXT NOT NULL, title TEXT NOT NULL,
            source TEXT NOT NULL, updated_at_ms INTEGER NOT NULL,
            UNIQUE(workspace_id,title));",
            )
            .map_err(|e| e.to_string())?;
        let has_counters: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='workflow_script_observation_counters')",
            [], |row| row.get(0),
        ).map_err(|e| e.to_string())?;
        if !has_counters {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|e| e.to_string())?;
            tx.execute_batch("CREATE TABLE workflow_script_observation_counters (
                run_id TEXT PRIMARY KEY, retained_bytes INTEGER NOT NULL, delivery_order INTEGER NOT NULL);
                INSERT INTO workflow_script_observation_counters(run_id,retained_bytes,delivery_order)
                    SELECT run_id,COALESCE(SUM(length(CAST(payload AS BLOB))+length(CAST(response AS BLOB))),0),
                        COALESCE(MAX(delivery_order),0) FROM workflow_script_observations GROUP BY run_id;")
                .map_err(|e| e.to_string())?;
            tx.commit().map_err(|e| e.to_string())?;
        }
        Ok(())
    })
}

fn record_observation(
    service: &WorkflowService,
    run_id: &str,
    command_id: u64,
    payload: &Value,
    response: &Value,
) -> Result<(), String> {
    let payload = serde_json::to_string(payload).map_err(|e| e.to_string())?;
    let response = serde_json::to_string(response).map_err(|e| e.to_string())?;
    let additional = payload.len().saturating_add(response.len());
    service.with_connection(|connection| {
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|e| e.to_string())?;
        let (bytes, last_order): (usize, u64) = tx.query_row(
            "SELECT retained_bytes,delivery_order FROM workflow_script_observation_counters WHERE run_id=?1",
            [run_id], |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional().map_err(|e| e.to_string())?.unwrap_or((0,0));
        if bytes.saturating_add(additional) > 32 * 1024 * 1024 {
            return Err("Workflow observation journal exceeds 32 MiB".into());
        }
        if last_order >= MAX_HOST_CALLS {
            return Err("Workflow observation count limit reached".into());
        }
        let order = last_order + 1;
        tx.execute(
            "INSERT INTO workflow_script_observations(run_id,command_id,payload,response,delivery_order) VALUES(?1,?2,?3,?4,?5)",
            params![run_id,command_id,payload,response,order],
        ).map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO workflow_script_observation_counters(run_id,retained_bytes,delivery_order) VALUES(?1,?2,?3)
                ON CONFLICT(run_id) DO UPDATE SET retained_bytes=excluded.retained_bytes,delivery_order=excluded.delivery_order",
            params![run_id,bytes + additional,order],
        ).map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())
    })
}

pub fn save_template(
    service: &WorkflowService,
    workspace_id: &str,
    title: &str,
    source: &str,
) -> Result<ScriptTemplate, String> {
    initialize(service)?;
    validate_source(source)?;
    let title = title.trim();
    if workspace_id.is_empty() || title.is_empty() || title.len() > 128 {
        return Err("A workspace and script name of 1–128 bytes are required".into());
    }
    service.with_connection(|c| {
        let count: i64 = c.query_row("SELECT COUNT(*) FROM workflow_script_templates WHERE workspace_id=?1", [workspace_id], |r|r.get(0)).map_err(|e|e.to_string())?;
        let existing: Option<String> = c.query_row("SELECT id FROM workflow_script_templates WHERE workspace_id=?1 AND title=?2", params![workspace_id,title], |r|r.get(0)).optional().map_err(|e|e.to_string())?;
        if count >= 100 && existing.is_none() { return Err("This workspace already has 100 saved scripts".into()); }
        let template = ScriptTemplate { id:existing.unwrap_or_else(||uuid::Uuid::new_v4().to_string()),workspace_id:workspace_id.into(),title:title.into(),source:source.into(),updated_at_ms:super::now_ms() };
        c.execute("INSERT INTO workflow_script_templates(id,workspace_id,title,source,updated_at_ms) VALUES(?1,?2,?3,?4,?5)
            ON CONFLICT(workspace_id,title) DO UPDATE SET source=excluded.source,updated_at_ms=excluded.updated_at_ms",
            params![template.id,template.workspace_id,template.title,template.source,template.updated_at_ms]).map_err(|e|e.to_string())?;
        Ok(template)
    })
}

pub fn list_templates(
    service: &WorkflowService,
    workspace_id: &str,
) -> Result<Vec<ScriptTemplate>, String> {
    initialize(service)?;
    service.with_connection(|c| {
        let mut statement=c.prepare("SELECT id,workspace_id,title,source,updated_at_ms FROM workflow_script_templates WHERE workspace_id=?1 ORDER BY updated_at_ms DESC LIMIT 100").map_err(|e|e.to_string())?;
        let rows=statement.query_map([workspace_id],|r|Ok(ScriptTemplate{id:r.get(0)?,workspace_id:r.get(1)?,title:r.get(2)?,source:r.get(3)?,updated_at_ms:r.get(4)?})).map_err(|e|e.to_string())?;
        rows.collect::<Result<Vec<_>,_>>().map_err(|e|e.to_string())
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HostCall {
    id: u64,
    op: String,
    args: Value,
}

struct ScriptHost {
    service: WorkflowService,
    run_id: String,
    snapshot: watch::Receiver<Arc<RunSnapshot>>,
    snapshots: watch::Sender<Arc<RunSnapshot>>,
    replay: Mutex<VecDeque<u64>>,
    replay_changed: Notify,
    stopped: Arc<AtomicBool>,
}

impl ScriptHost {
    async fn call(&self, raw: &str) -> Value {
        let outcome = self.call_inner(raw).await;
        match outcome {
            Ok(value) => json!({"ok":true,"value":value}),
            Err(error) => json!({"ok":false,"error":error}),
        }
    }

    async fn call_inner(&self, raw: &str) -> Result<Value, String> {
        if raw.len() > 256 * 1024 {
            return Err("Workflow host request is too large".into());
        }
        let call: HostCall =
            serde_json::from_str(raw).map_err(|e| format!("Invalid workflow host request: {e}"))?;
        if call.id == 0 || call.id > MAX_HOST_CALLS {
            return Err("Workflow host-call limit reached".into());
        }
        let payload = json!({"op":call.op,"args":call.args});
        let recorded=self.service.with_connection(|c| {
            let row:Option<(String,String)>=c.query_row("SELECT payload,response FROM workflow_script_observations WHERE run_id=?1 AND command_id=?2",params![self.run_id,call.id],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(|e|e.to_string())?;
            match row {
                Some((old,response))=>{
                    if serde_json::from_str::<Value>(&old).map_err(|e|e.to_string())? != payload { return Err(format!("Script replay diverged at command {}",call.id)); }
                    Ok(Some(serde_json::from_str::<Value>(&response).map_err(|e|e.to_string())?))
                },None=>Ok(None)
            }
        })?;
        if let Some(response) = recorded {
            loop {
                let notified = self.replay_changed.notified();
                self.check_running().await?;
                let delivered = {
                    let mut replay = self.replay.lock().map_err(|_| "Replay gate unavailable")?;
                    if replay.front() == Some(&call.id) {
                        replay.pop_front();
                        true
                    } else if !replay.contains(&call.id) {
                        return Err("Script observation was delivered twice".into());
                    } else {
                        false
                    }
                };
                if delivered {
                    self.replay_changed.notify_waiters();
                    return decode_response(response);
                }
                tokio::select! { _=notified=>{}, _=tokio::time::sleep(Duration::from_millis(100))=>{} }
            }
        }
        self.check_running().await?;
        let result = self.perform(&call, &payload).await;
        if matches!(
            call.op.as_str(),
            "add_tasks" | "cancel" | "retire" | "replace" | "message" | "phase"
        ) {
            // Read-your-writes must precede delivery of a mutation response.
            // The background event consumer may still be processing its wake.
            let latest = self.service.snapshot(&self.run_id)?;
            self.snapshots.send_if_modified(|current| {
                if current.revision >= latest.revision {
                    return false;
                }
                *current = Arc::new(latest);
                true
            });
        }
        // Newly completed observations follow all replayed deliveries. This
        // preserves Promise.race / completion-dependent branches on restart.
        loop {
            let notified = self.replay_changed.notified();
            if self
                .replay
                .lock()
                .map_err(|_| "Replay gate unavailable")?
                .is_empty()
            {
                break;
            }
            self.check_running().await?;
            tokio::select! { _=notified=>{}, _=tokio::time::sleep(Duration::from_millis(100))=>{} }
        }
        let response = match &result {
            Ok(value) => json!({"ok":true,"value":value}),
            Err(error) => json!({"ok":false,"error":error}),
        };
        record_observation(&self.service, &self.run_id, call.id, &payload, &response)?;
        result
    }

    async fn check_running(&self) -> Result<(), String> {
        let mut snapshots = self.snapshot.clone();
        loop {
            self.service.ensure_runtime_active()?;
            let state = snapshots.borrow().clone();
            if state.cancel_requested || self.stopped.load(Ordering::Relaxed) {
                return Err("Workflow was cancelled or stopped".into());
            }
            if state.error.is_some() || matches!(
                state.status,
                RunStatus::Unknown | RunStatus::Failed | RunStatus::Cancelled
            ) {
                return Err(format!("Workflow cannot continue while {:?}", state.status));
            }
            if !matches!(state.status, RunStatus::Paused | RunStatus::Stopping) {
                return Ok(());
            }
            snapshots
                .changed()
                .await
                .map_err(|_| "Workflow observation stream closed")?;
        }
    }

    async fn perform(&self, call: &HostCall, payload: &Value) -> Result<Value, String> {
        let key = format!("script-{}", call.id);
        match call.op.as_str() {
            "add_tasks" => {
                let tasks: Vec<TaskSpec> =
                    serde_json::from_value(call.args.clone()).map_err(|e| e.to_string())?;
                let ids = tasks.iter().map(|task| task.id.clone()).collect::<Vec<_>>();
                self.service
                    .read_or_execute(&self.run_id, &key, payload, || {
                        self.service.add_tasks(&self.run_id, tasks, None, &key)?;
                        Ok(json!(ids))
                    })
            }
            "wait" => {
                let id = argument_id(&call.args)?;
                let mut snapshots = self.snapshot.clone();
                loop {
                    self.check_running().await?;
                    let run = snapshots.borrow().clone();
                    let task = run
                        .tasks
                        .iter()
                        .find(|t| t.spec.id == id)
                        .ok_or("Unknown workflow task")?;
                    if matches!(
                        task.status,
                        TaskStatus::Succeeded
                            | TaskStatus::Failed
                            | TaskStatus::Cancelled
                            | TaskStatus::Blocked
                            | TaskStatus::Unknown
                    ) {
                        return Ok(task_result(task));
                    }
                    snapshots
                        .changed()
                        .await
                        .map_err(|_| "Workflow observation stream closed")?;
                }
            }
            "list" => Ok(task_list(&self.snapshot.borrow())),
            "cancel" | "retire" | "replace" | "message" | "phase" | "log" => self
                .service
                .read_or_execute(&self.run_id, &key, payload, || {
                    let run = match call.op.as_str() {
                        "cancel" => self
                            .service
                            .cancel_task(&self.run_id, argument_id(&call.args)?)?,
                        "retire" => self
                            .service
                            .retire(&self.run_id, argument_id(&call.args)?)?,
                        "replace" => {
                            let spec: TaskSpec = serde_json::from_value(
                                call.args
                                    .get("spec")
                                    .cloned()
                                    .ok_or("Replacement task spec is required")?,
                            )
                            .map_err(|e| e.to_string())?;
                            self.service.replace(
                                &self.run_id,
                                argument_id(&call.args)?,
                                spec,
                                &key,
                            )?
                        }
                        "message" => self.service.message(
                            &self.run_id,
                            argument_id(&call.args)?,
                            argument_text(&call.args)?,
                        )?,
                        "phase" => self
                            .service
                            .set_phase(&self.run_id, argument_text(&call.args)?)?,
                        "log" => {
                            let text = argument_text(&call.args)?;
                            if text.len() > 4096 {
                                return Err("Workflow log line exceeds 4096 bytes".into());
                            }
                            // A journaled log is evidence; it does not become a
                            // provider turn or a broadcast of private transcripts.
                            return Ok(json!({"text":text}));
                        }
                        _ => return Err("Unknown workflow operation".into()),
                    };
                    Ok(json!({"revision":run.revision}))
                }),
            _ => Err(format!("Unknown workflow operation: {}", call.op)),
        }
    }
}

fn argument_id(value: &Value) -> Result<&str, String> {
    value
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(|| "Task id is required".into())
}
fn argument_text(value: &Value) -> Result<&str, String> {
    value
        .get("text")
        .and_then(Value::as_str)
        .ok_or_else(|| "Text is required".into())
}
fn decode_response(response: Value) -> Result<Value, String> {
    if response.get("ok") == Some(&Value::Bool(true)) {
        Ok(response.get("value").cloned().unwrap_or(Value::Null))
    } else {
        Err(response
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("Recorded workflow operation failed")
            .to_owned())
    }
}

pub(crate) fn task_result(task: &super::TaskSnapshot) -> Value {
    json!({"id":task.spec.id,"generation":task.generation,"status":task.status,"output":task.result,"error":task.error,
        "artifacts":task.current_attempt.as_ref().map(|attempt|attempt.artifacts.clone()).unwrap_or_default()})
}
pub(crate) fn task_list(run: &RunSnapshot) -> Value {
    json!(run.tasks.iter().map(|task|json!({"id":task.spec.id,"title":task.spec.title,"status":task.status,"generation":task.generation,"route_id":task.current_attempt.as_ref().map(|a|a.route_id.as_str()).or(task.spec.route_id.as_deref()),"dependencies":task.spec.dependencies})).collect::<Vec<_>>())
}

async fn execute(service: WorkflowService, id: &str, max_interrupts: u64) -> Result<Value, String> {
    service.ensure_runtime_active()?;
    let shutdown = service.shutdown_watch();
    let run = service.snapshot(id)?;
    let script = run.spec.script.clone().ok_or("Missing workflow script")?;
    service.set_script_status(id, ScriptStatus::Running, None, None)?;
    let mut events = service.subscribe();
    let (snapshots, receiver) = watch::channel(Arc::new(service.snapshot(id)?));
    let host_snapshots = snapshots.clone();
    let stopped = Arc::new(AtomicBool::new(false));
    let monitor_stop = stopped.clone();
    let monitor_service = service.clone();
    let monitor_id = id.to_owned();
    let monitor_shutdown = shutdown.clone();
    let monitor = MonitorGuard(tokio::spawn(async move {
        loop {
            let event = tokio::select! {
                biased;
                _=wait_for_shutdown(monitor_shutdown.clone())=>{
                    monitor_stop.store(true, Ordering::Relaxed);
                    break;
                },
                event=events.recv()=>event,
            };
            match event {
                Ok(event) if event.run_id != monitor_id => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                _ => {}
            }
            match monitor_service.snapshot(&monitor_id) {
                Ok(run) => {
                    if run.cancel_requested {
                        monitor_stop.store(true, Ordering::Relaxed);
                    }
                    snapshots.send_if_modified(|current| {
                        if current.revision >= run.revision {
                            return false;
                        }
                        *current = Arc::new(run);
                        true
                    });
                }
                Err(_) => {
                    monitor_stop.store(true, Ordering::Relaxed);
                    break;
                }
            }
        }
    }));
    let replay=service.with_connection(|c| {
        let mut statement=c.prepare("SELECT command_id FROM workflow_script_observations WHERE run_id=?1 ORDER BY delivery_order").map_err(|e|e.to_string())?;
        let rows=statement.query_map([id],|r|r.get::<_,u64>(0)).map_err(|e|e.to_string())?;
        rows.collect::<Result<VecDeque<_>,_>>().map_err(|e|e.to_string())
    })?;
    let host = Arc::new(ScriptHost {
        service: service.clone(),
        run_id: id.into(),
        snapshot: receiver,
        snapshots: host_snapshots,
        replay: Mutex::new(replay),
        replay_changed: Notify::new(),
        stopped: stopped.clone(),
    });
    let runtime = AsyncRuntime::new().map_err(|e| e.to_string())?;
    runtime.set_memory_limit(MAX_JS_MEMORY).await;
    runtime.set_max_stack_size(512 * 1024).await;
    let remaining = run
        .resolved_limits
        .wall_time_ms
        .saturating_sub(super::now_ms().saturating_sub(run.created_at_ms) as u64);
    let deadline = Instant::now() + Duration::from_millis(remaining);
    let interrupts = Arc::new(AtomicU64::new(0));
    let interrupt_service = service.clone();
    runtime
        .set_interrupt_handler(Some(Box::new(move || {
            stopped.load(Ordering::Relaxed)
                || interrupt_service.ensure_runtime_active().is_err()
                || Instant::now() >= deadline
                || interrupts.fetch_add(1, Ordering::Relaxed) >= max_interrupts
        })))
        .await;
    let context = AsyncContext::full(&runtime)
        .await
        .map_err(|e| e.to_string())?;
    let host_for_js = host.clone();
    let globals = json!({"args":script.args,"goal":run.spec.goal,"routes":run.spec.routes,"allow_writes":run.spec.allow_writes});
    let code=format!("{BOOTSTRAP}\nObject.assign(globalThis, JSON.parse({}));\n(async()=>{{\n{}\n}})().then(__workflowComplete)",serde_json::to_string(&globals.to_string()).map_err(|e|e.to_string())?,script.source);
    let evaluation =
        context.async_with(async move |ctx| {
            ctx.globals().set("__workflowHost",Func::from(Async(move |raw:String| {
            let host=host_for_js.clone();
            async move { Ok::<String,rquickjs::Error>(host.call(&raw).await.to_string()) }
        }))).map_err(|e|e.to_string())?;
            let promise: Promise = ctx.eval(code).map_err(|e| js_error(&ctx, e))?;
            promise
                .into_future::<String>()
                .await
                .map_err(|e| js_error(&ctx, e))
        });
    // A suspended user-created Promise does not execute QuickJS instructions,
    // so its deadline and cancellation need a host-side wake as well.
    let result = tokio::select! {
        biased;
        _=wait_for_shutdown(shutdown)=>Err("Workflow host is shutting down".into()),
        result=evaluation=>result,
        _=tokio::time::sleep_until(tokio::time::Instant::from_std(deadline))=>Err("Workflow script wall-time limit reached".into()),
        error=wait_for_stop(host.snapshot.clone())=>Err(error),
    };
    drop(monitor);
    let serialized = result?;
    if serialized.len() > run.resolved_limits.max_output_bytes {
        return Err("Workflow script result exceeds the output limit".into());
    }
    if !host
        .replay
        .lock()
        .map_err(|_| "Replay gate unavailable")?
        .is_empty()
    {
        return Err("Script finished before replaying its recorded observations".into());
    }
    serde_json::from_str(&serialized)
        .map_err(|e| format!("Workflow result must be JSON serializable: {e}"))
}

async fn wait_for_shutdown(mut shutdown: watch::Receiver<bool>) {
    loop {
        let stopped = *shutdown.borrow();
        if stopped {
            return;
        }
        if shutdown.changed().await.is_err() {
            return;
        }
    }
}

async fn wait_for_stop(mut snapshots: watch::Receiver<Arc<RunSnapshot>>) -> String {
    loop {
        let run = snapshots.borrow().clone();
        if run.cancel_requested || run.error.is_some()
            || matches!(
                run.status,
                RunStatus::Unknown | RunStatus::Cancelled | RunStatus::Failed
            )
        {
            return "Workflow was cancelled or interrupted".into();
        }
        if snapshots.changed().await.is_err() {
            return "Workflow observation stream closed".into();
        }
    }
}

fn js_error(ctx: &rquickjs::Ctx<'_>, error: rquickjs::Error) -> String {
    if error.is_exception() {
        let exception = ctx.catch();
        if let Some(object) = exception.as_object() {
            if let Ok(message) = object.get::<_, String>("message") {
                return message;
            }
        }
    }
    error.to_string()
}

const BOOTSTRAP: &str = r#"
(function(){'use strict';
let __sequence=0, __taskSequence=0, __outstanding=0;
const __host=__workflowHost;
delete globalThis.__workflowHost;
async function __call(op,args) {
  __outstanding++;
  try {
    const response=JSON.parse(await __host(JSON.stringify({id:++__sequence,op,args})));
    if(!response.ok) throw new Error(response.error);
    return response.value;
  } finally { __outstanding--; }
}
function __task(spec) {
  if(!spec||typeof spec!=='object') throw new Error('A task spec is required');
  return {id:spec.id||`agent-${++__taskSequence}`,title:spec.title||'Agent task',prompt:spec.prompt||'',dependencies:spec.dependencies||[],route_id:spec.route_id||null,access:spec.access||'read_only',scope:spec.scope||[],output_schema:spec.output_schema||spec.schema||null,required:spec.required!==false};
}
const workflow=Object.freeze({
  async addTask(spec) {const ids=await __call('add_tasks',[__task(spec)]);return ids[0];},
  addTasks(specs) {return __call('add_tasks',specs.map(__task));},
  wait(id) {return __call('wait',{id});},
  list() {return __call('list',{});},
  cancelTask(id) {return __call('cancel',{id});},
  retire(id) {return __call('retire',{id});},
  replace(id,spec) {return __call('replace',{id,spec:__task({...spec,id})});},
  message(id,text) {return __call('message',{id,text});},
  phase(text) {return __call('phase',{text});},
  log(text) {return __call('log',{text});}
});
async function agent(prompt,options={}) {const id=await workflow.addTask({...options,prompt});return workflow.wait(id);}
async function parallel(items,mapper) {
  if(!Array.isArray(items)||items.length>4096) throw new Error('Fan-out requires at most 4096 items');
  return Promise.all(items.map(mapper||((item)=>typeof item==='function'?item():item)));
}
const pipeline=parallel;
function __nondeterministic(){throw new Error('Time and randomness are unavailable; pass stable values through args');}
Object.defineProperty(Math,'random',{value:__nondeterministic,writable:false,configurable:false});
function UnavailableDate(){return __nondeterministic();}
Object.defineProperty(UnavailableDate,'now',{value:__nondeterministic});
Object.defineProperty(globalThis,'Date',{value:UnavailableDate,writable:false,configurable:false});
Object.defineProperty(globalThis,'eval',{value:undefined,writable:false,configurable:false});
for(const [name,value] of Object.entries({workflow,agent,parallel,pipeline,phase:workflow.phase,log:workflow.log,__workflowComplete:result=>{
  if(__outstanding!==0)throw new Error('Await every workflow operation before returning');
  return JSON.stringify(result===undefined?null:result);
}}))Object.defineProperty(globalThis,name,{value,writable:false,configurable:false});
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workflows::{RouteSpec, RunMode, RunSpec, ScriptSpec, WorkflowLimits};

    fn run(service: &WorkflowService, source: &str) -> RunSnapshot {
        service
            .create(
                RunSpec {
                    workspace_id: "demo".into(),
                    title: "Script test".into(),
                    goal: "Inspect".into(),
                    mode: RunMode::DryRun,
                    allow_writes: false,
                    routes: vec![RouteSpec {
                        id: "claude".into(),
                        provider: "claude".into(),
                        model: None,
                        effort: None,
                    }],
                    limits: WorkflowLimits {
                        wall_time_ms: 10_000,
                        ..Default::default()
                    },
                    tasks: vec![],
                    script: Some(ScriptSpec {
                        source: source.into(),
                        args: json!({"stable":123}),
                        api_version: 1,
                    }),
                },
                &uuid::Uuid::new_v4().to_string(),
            )
            .unwrap()
    }
    #[tokio::test]
    async fn script_environment_has_no_io_and_rejects_nondeterminism() {
        let service = WorkflowService::open(":memory:", 2).unwrap();
        initialize(&service).unwrap();
        for source in [
            "return Math.random();",
            "return Date.now();",
            "return new Date();",
            "return fetch('https://example.com');",
            "return import('fs');",
        ] {
            let run = run(&service, source);
            assert!(
                execute(service.clone(), &run.id, 1000).await.is_err(),
                "{source}"
            );
        }
        let run=run(&service,"return {node:typeof process,network:typeof fetch,timers:typeof setTimeout,stable:args.stable};");
        assert_eq!(
            execute(service.clone(), &run.id, 1000).await.unwrap(),
            json!({"node":"undefined","network":"undefined","timers":"undefined","stable":123})
        );
    }
    #[tokio::test]
    async fn oversized_script_exception_persists_failure_instead_of_orphaning_vm() {
        let service = WorkflowService::open(":memory:", 2).unwrap();
        let run = run(&service, "throw new Error('🦀'.repeat(3000));");
        let runtime = WorkflowScriptRuntime::default();
        runtime.start(service.clone(), &run.id).unwrap();
        let failed = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let state = service.snapshot(&run.id).unwrap();
                if state.script.as_ref().unwrap().status == ScriptStatus::Failed {
                    return state;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("script exception did not persist its terminal state");
        assert_eq!(failed.status, RunStatus::Failed);
        let error = failed.script.unwrap().error.unwrap();
        assert_eq!(error.len(), 8192);
        assert!(error.chars().all(|character| character == '🦀'));
    }

    #[tokio::test]
    async fn cpu_loop_is_interrupted_without_provider_calls() {
        let service = WorkflowService::open(":memory:", 2).unwrap();
        initialize(&service).unwrap();
        let run = run(&service, "while(true) {}");
        assert!(execute(service, &run.id, 10).await.is_err());
    }
    #[tokio::test]
    async fn host_observations_replay_and_divergence_is_rejected() {
        let service = WorkflowService::open(":memory:", 2).unwrap();
        initialize(&service).unwrap();
        let run = run(
            &service,
            "await workflow.phase('Inspect'); return await workflow.list();",
        );
        let first = execute(service.clone(), &run.id, 1000).await.unwrap();
        assert_eq!(
            execute(service.clone(), &run.id, 1000).await.unwrap(),
            first
        );
        service.with_connection(|c| {c.execute("UPDATE workflow_script_observations SET payload=?1 WHERE run_id=?2 AND command_id=1",params![json!({"op":"phase","args":{"text":"Changed"}}).to_string(),run.id]).map_err(|e|e.to_string())?;Ok(())}).unwrap();
        assert!(execute(service, &run.id, 1000)
            .await
            .unwrap_err()
            .contains("diverged"));
    }
    #[test]
    fn templates_upsert_without_losing_identity() {
        let service = WorkflowService::open(":memory:", 2).unwrap();
        let first = save_template(&service, "demo", "Audit", "return 1;").unwrap();
        let second = save_template(&service, "demo", "Audit", "return 2;").unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(list_templates(&service, "demo").unwrap().len(), 1);
    }

    #[test]
    fn observation_budget_counts_utf8_bytes_and_rolls_back_atomically() {
        let service = WorkflowService::open(":memory:", 2).unwrap();
        initialize(&service).unwrap();
        let run = run(&service, "return 1;");
        let payload = json!({"text":"🦀"});
        let response = json!({"ok":true});
        let bytes = payload.to_string().len() + response.to_string().len();
        service.with_connection(|c| {
            c.execute("INSERT INTO workflow_script_observation_counters(run_id,retained_bytes,delivery_order) VALUES(?1,?2,0)",
                params![run.id,32*1024*1024-bytes]).map_err(|e|e.to_string())?;
            Ok(())
        }).unwrap();
        record_observation(&service, &run.id, 1, &payload, &response).unwrap();
        assert!(
            record_observation(&service, &run.id, 2, &json!({}), &json!({}))
                .unwrap_err()
                .contains("32 MiB")
        );
        service.with_connection(|c| {
            let counters: (usize,u64) = c.query_row(
                "SELECT retained_bytes,delivery_order FROM workflow_script_observation_counters WHERE run_id=?1",
                [&run.id], |row| Ok((row.get(0)?,row.get(1)?)),
            ).map_err(|e|e.to_string())?;
            assert_eq!(counters,(32*1024*1024,1));
            let count: usize = c.query_row("SELECT count(*) FROM workflow_script_observations WHERE run_id=?1",
                [&run.id], |row| row.get(0)).map_err(|e|e.to_string())?;
            assert_eq!(count,1);
            Ok(())
        }).unwrap();
    }

    #[test]
    fn observation_counter_migration_preserves_unicode_size_and_delivery_order() {
        let service = WorkflowService::open(":memory:", 2).unwrap();
        initialize(&service).unwrap();
        let run = run(&service, "return 1;");
        let payload = json!({"text":"🦀"});
        let response = json!({"ok":true,"value":"é"});
        record_observation(&service, &run.id, 1, &payload, &response).unwrap();
        service
            .with_connection(|c| {
                c.execute_batch("DROP TABLE workflow_script_observation_counters")
                    .map_err(|e| e.to_string())
            })
            .unwrap();
        initialize(&service).unwrap();
        record_observation(&service, &run.id, 2, &json!({}), &json!({})).unwrap();
        service.with_connection(|c| {
            let counters: (usize,u64) = c.query_row(
                "SELECT retained_bytes,delivery_order FROM workflow_script_observation_counters WHERE run_id=?1",
                [&run.id], |row| Ok((row.get(0)?,row.get(1)?)),
            ).map_err(|e|e.to_string())?;
            assert_eq!(counters,(payload.to_string().len()+response.to_string().len()+4,2));
            Ok(())
        }).unwrap();
    }

    #[tokio::test]
    async fn dynamic_fanout_runs_and_replays_without_duplicate_attempts() {
        let service = WorkflowService::open(":memory:", 2).unwrap();
        initialize(&service).unwrap();
        let run = run(
            &service,
            r#"
            await workflow.phase('Inspect');
            const results=await parallel(['one','two','three'],(id)=>agent('Inspect '+id,{id}));
            await workflow.phase('Adapt');
            const final=await agent('Synthesize '+JSON.stringify(results),{id:'final'});
            return {results,final};
        "#,
        );
        let driver_service = service.clone();
        let (shutdown, receiver) = watch::channel(false);
        let driver = tokio::spawn(async move {
            driver_service
                .run_driver_until(Arc::new(super::super::DryRunDriver::default()), receiver)
                .await
        });
        let first = tokio::time::timeout(
            Duration::from_secs(5),
            execute(service.clone(), &run.id, 1000),
        )
        .await
        .unwrap()
        .unwrap();
        let replay = execute(service.clone(), &run.id, 1000).await.unwrap();
        assert_eq!(first, replay);
        let snapshot = service.snapshot(&run.id).unwrap();
        assert_eq!(snapshot.tasks.len(), 4);
        assert!(snapshot
            .tasks
            .iter()
            .all(|task| task.attempts.len() == 1 && task.status == TaskStatus::Succeeded));
        assert_eq!(snapshot.usage.total_tokens, 0);
        assert_eq!(snapshot.usage.estimated_tokens, 0);
        shutdown.send(true).unwrap();
        driver.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn unawaited_operations_and_large_final_results_are_rejected() {
        let service = WorkflowService::open(":memory:", 2).unwrap();
        initialize(&service).unwrap();
        let dangling = run(&service, "const id=await workflow.addTask({id:'pending',prompt:'Inspect'});workflow.wait(id);return 'early';");
        assert!(execute(service.clone(), &dangling.id, 1000)
            .await
            .unwrap_err()
            .contains("Await every"));
        let large = run(&service, "return 'x'.repeat(1048577);");
        assert!(execute(service, &large.id, 1000)
            .await
            .unwrap_err()
            .contains("output limit"));
    }

    #[tokio::test]
    async fn never_settled_promise_obeys_wall_time_and_cancellation() {
        let service = WorkflowService::open(":memory:", 2).unwrap();
        initialize(&service).unwrap();
        let short = run(&service, "return await new Promise(()=>{});");
        service
            .with_connection(|connection| {
                let mut snapshot = super::super::store::load(connection, &short.id)?;
                snapshot.resolved_limits.wall_time_ms = 25;
                super::super::store::save(connection, &snapshot, "test_limit")
            })
            .unwrap();
        let error = tokio::time::timeout(
            Duration::from_secs(1),
            execute(service.clone(), &short.id, 1000),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert!(error.contains("wall-time"));
        let cancelled = run(&service, "return await new Promise(()=>{});");
        let stop_service = service.clone();
        let id = cancelled.id.clone();
        let stop = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(25)).await;
            stop_service.cancel(&id).unwrap();
        });
        let error = tokio::time::timeout(
            Duration::from_secs(1),
            execute(service, &cancelled.id, 1000),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert!(error.contains("cancelled"));
        stop.await.unwrap();
    }

    #[tokio::test]
    async fn workflow_script_shutdown_stops_pending_promise_and_fences_completion() {
        for checkpoint_failure in [false, true] {
            let service = WorkflowService::open(":memory:", 2).unwrap();
            let run = run(
                &service,
                "await workflow.phase('Promise ready');return await new Promise(()=>{});",
            );
            let scripts = WorkflowScriptRuntime::default();
            scripts.start(service.clone(), &run.id).unwrap();
            tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    let snapshot = service.snapshot(&run.id).unwrap();
                    if snapshot.script.as_ref().unwrap().phase.as_deref() == Some("Promise ready") {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            }).await.unwrap();
            assert!(scripts.is_running(&run.id));
            if checkpoint_failure {
                service.with_connection(|connection| {
                    connection.execute_batch(
                        "CREATE TEMP TRIGGER fail_script_checkpoint BEFORE UPDATE ON workflow_runs
                         BEGIN SELECT RAISE(ABORT, 'injected script checkpoint failure'); END;",
                    ).map_err(|error| error.to_string())
                }).unwrap();
            }
            let checkpoint = service.shutdown_signal();
            if checkpoint_failure {
                assert!(checkpoint.unwrap_err().contains("injected script checkpoint failure"));
            } else {
                checkpoint.unwrap();
            }
            tokio::time::timeout(Duration::from_secs(1), scripts.drain_active())
                .await.unwrap().unwrap();
            assert!(!scripts.is_running(&run.id));
            let stopped = service.snapshot(&run.id).unwrap();
            let script = stopped.script.unwrap();
            assert_eq!(script.status, if checkpoint_failure { ScriptStatus::Running } else { ScriptStatus::Paused });
            assert!(script.result.is_none());
            for status in [ScriptStatus::Running, ScriptStatus::Completed, ScriptStatus::Failed] {
                assert!(service.set_script_status(&run.id, status, Some(json!({"late":true})), None)
                    .unwrap_err().contains("shutting down"));
            }
            assert!(scripts.start(service.clone(), &run.id).unwrap_err().contains("shutting down"));
        }
    }

    #[tokio::test]
    async fn workflow_script_shutdown_interrupts_cpu_loop_without_database_event() {
        let service = WorkflowService::open(":memory:", 2).unwrap();
        initialize(&service).unwrap();
        let run = run(&service, "await workflow.phase('CPU ready');while(true){}");
        service.with_connection(|connection| {
            let mut snapshot = super::super::store::load(connection, &run.id)?;
            snapshot.resolved_limits.wall_time_ms = 5000;
            super::super::store::save(connection, &snapshot, "bounded-cpu-fixture")
        }).unwrap();
        let execute_service = service.clone();
        let id = run.id.clone();
        let mut task = tokio::task::spawn_blocking(move || {
            tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
                .block_on(execute(execute_service, &id, u64::MAX))
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let snapshot = service.snapshot(&run.id).unwrap();
                if snapshot.script.as_ref().unwrap().phase.as_deref() == Some("CPU ready") {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }).await.unwrap();
        service.latch_shutdown();
        let joined = tokio::time::timeout(Duration::from_secs(1), &mut task).await;
        let promptly = joined.is_ok();
        // Even a failing regression joins its owned blocking thread after the
        // fixture's finite wall-time limit; aborting it would not stop QuickJS.
        let result = match joined {
            Ok(result) => result.unwrap(),
            Err(_) => tokio::time::timeout(Duration::from_secs(6), task).await.unwrap().unwrap(),
        };
        assert!(promptly, "pure JS ignored the host shutdown latch");
        assert!(result.is_err());
        assert!(service.snapshot(&run.id).unwrap().script.unwrap().result.is_none());
    }

    #[tokio::test]
    async fn promise_race_replays_original_delivery_order() {
        struct RacingDriver;
        #[async_trait::async_trait]
        impl super::super::WorkflowDriver for RacingDriver {
            async fn execute(
                &self,
                dispatch: super::super::Dispatch,
                service: WorkflowService,
                cancel: watch::Receiver<bool>,
            ) -> super::super::ExecutionReport {
                super::super::DryRunDriver {
                    delay: Duration::from_millis(if dispatch.task_id == "slow" { 50 } else { 1 }),
                    fail_tasks: HashSet::new(),
                }
                .execute(dispatch, service, cancel)
                .await
            }
        }
        let service = WorkflowService::open(":memory:", 2).unwrap();
        initialize(&service).unwrap();
        let run = run(
            &service,
            r#"
            const ids=await workflow.addTasks([
              {id:'slow',prompt:'Inspect',title:'Slow'},
              {id:'fast',prompt:'Inspect',title:'Fast'}
            ]);
            const winner=await Promise.race(ids.map(id=>workflow.wait(id)));
            await workflow.wait('slow');await workflow.wait('fast');
            return winner.id;
        "#,
        );
        let (shutdown, receiver) = watch::channel(false);
        let driver_service = service.clone();
        let driver = tokio::spawn(async move {
            driver_service
                .run_driver_until(Arc::new(RacingDriver), receiver)
                .await
        });
        let first = execute(service.clone(), &run.id, 1000).await.unwrap();
        assert_eq!(first, json!("fast"));
        // Both tasks are already terminal; replay must still choose the original
        // winner rather than whichever cached wait is evaluated first now.
        assert_eq!(
            execute(service.clone(), &run.id, 1000).await.unwrap(),
            first
        );
        assert!(service
            .snapshot(&run.id)
            .unwrap()
            .tasks
            .iter()
            .all(|task| task.attempts.len() == 1));
        shutdown.send(true).unwrap();
        driver.await.unwrap().unwrap();
    }
}
