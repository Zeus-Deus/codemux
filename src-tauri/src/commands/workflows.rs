//! Desktop entry points for managed workflows.
//! The worker runtime receives authority from admitted dispatches, not IPC.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tokio::sync::watch;

use crate::{
    agent_provider::ProviderKind,
    observability::ObservabilityStore,
    state::AppStateStore,
    workflows::{
        self,
        artifacts::{ArtifactPreview, ArtifactStore, IntegrationReport},
        capabilities::{self, WorkflowCapability},
        config::HostPolicy,
        executor::LiveWorkflowDriver,
        scripts::{ScriptTemplate, WorkflowScriptRuntime},
        Dispatch, DryRunDriver, ExecutionReport, RunMode, RunSnapshot, RunSpec, ScriptSpec,
        ScriptStatus, TaskSpec, WorkflowDriver, WorkflowService,
    },
};

pub struct WorkflowState {
    service: Result<WorkflowService, String>,
    scripts: WorkflowScriptRuntime,
    artifacts: Arc<ArtifactStore>,
    started: AtomicBool,
    policy: HostPolicy,
    _ownership: Option<RuntimeOwnership>,
    live: Mutex<Option<Arc<LiveWorkflowDriver>>>,
}

impl WorkflowState {
    pub fn open_default() -> Self {
        let directory = dirs::config_dir().map(|root| root.join(crate::APP_DIR_NAME));
        let policy = directory
            .as_ref()
            .ok_or("Could not determine the workflow data directory".to_owned())
            .and_then(|root| HostPolicy::load(&root.join("workflows.json")));
        let mut ownership = None;
        let service = policy.as_ref().map_err(Clone::clone).and_then(|policy| {
            let root = directory
                .as_ref()
                .ok_or("Workflow data directory is unavailable")?;
            ownership = Some(lock_runtime(root)?);
            let service =
                WorkflowService::open(root.join("workflows.db"), policy.global_concurrency)?;
            policy.configure(&service)?;
            Ok(service)
        });
        // An unavailable persistent database disables workflows. It must not
        // silently lose execution ownership by switching to an empty DB.
        Self {
            service,
            scripts: WorkflowScriptRuntime::default(),
            artifacts: Arc::new(ArtifactStore::new(
                directory.unwrap_or_default().join("workflow-artifacts"),
            )),
            started: AtomicBool::new(false),
            policy: policy.unwrap_or_default(),
            _ownership: ownership,
            live: Mutex::new(None),
        }
    }
    fn service(&self) -> Result<WorkflowService, String> {
        self.service.clone()
    }

    pub fn request_shutdown(&self) {
        if let Ok(service) = &self.service {
            if let Err(error) = service.shutdown_signal() {
                eprintln!("[codemux::workflows] shutdown checkpoint failed: {error}");
            }
        }
    }
}

struct RuntimeOwnership(std::fs::File);
impl Drop for RuntimeOwnership {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

fn lock_runtime(root: &std::path::Path) -> Result<RuntimeOwnership, String> {
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("workflows.lock"))
        .map_err(|e| e.to_string())?;
    file.try_lock().map_err(|_|"Another CodeMux process owns managed workflows. Close that process before running workflows here.".to_string())?;
    Ok(RuntimeOwnership(file))
}

fn gate(store: &ObservabilityStore) -> Result<(), String> {
    super::agent_chat::feature_flag_on(store)
}

fn workspace<R: Runtime>(app: &AppHandle<R>, id: &str) -> Result<PathBuf, String> {
    let state = app.state::<AppStateStore>();
    let workspace = state
        .find_workspace(id)
        .ok_or("The workflow workspace is no longer available")?;
    if workspace.remote_cwd.is_some() || workspace.host_id.is_some() || workspace.attach_only {
        return Err("Managed workflows currently require a local workspace".into());
    }
    Ok(PathBuf::from(workspace.cwd))
}

struct ModeDriver {
    dry: DryRunDriver,
    live: Arc<LiveWorkflowDriver>,
}
#[async_trait]
impl WorkflowDriver for ModeDriver {
    async fn execute(
        &self,
        dispatch: Dispatch,
        service: WorkflowService,
        cancellation: watch::Receiver<bool>,
    ) -> ExecutionReport {
        match dispatch.mode {
            RunMode::DryRun => self.dry.execute(dispatch, service, cancellation).await,
            RunMode::Live => self.live.execute(dispatch, service, cancellation).await,
        }
    }
}

fn start_runtime<R: Runtime>(app: &AppHandle<R>, state: &WorkflowState) -> Result<(), String> {
    let service = state.service()?;
    if state.started.swap(true, Ordering::AcqRel) {
        return Ok(());
    }
    let live = Arc::new(live_driver(app, state));
    *state
        .live
        .lock()
        .map_err(|_| "Workflow driver registry is unavailable")? = Some(Arc::clone(&live));
    let driver = Arc::new(ModeDriver {
        dry: DryRunDriver::default(),
        live,
    });
    let driver_service = service.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = driver_service.run_driver(driver).await {
            eprintln!("[codemux::workflows] driver stopped: {error}");
        }
    });
    let mut events = service.subscribe();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            match events.recv().await {
                Ok(event) => {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    let mut last = event;
                    while let Ok(event) = events.try_recv() {
                        last = event;
                    }
                    let _ = app.emit(
                        "workflow-changed",
                        serde_json::json!({"run_id":null,"revision":last.revision}),
                    );
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    let _ = app.emit(
                        "workflow-changed",
                        serde_json::json!({"run_id":null,"revision":null}),
                    );
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    Ok(())
}

fn live_driver<R: Runtime>(app: &AppHandle<R>, state: &WorkflowState) -> LiveWorkflowDriver {
    let resolver_app = app.clone();
    let lookup_app = app.clone();
    LiveWorkflowDriver::new(
        Arc::new(move |id| workspace(&resolver_app, id)),
        Arc::new(move |kind| {
            let app = lookup_app.clone();
            Box::pin(async move {
                app.state::<super::agent_chat::ProviderRegistry>()
                    .get(kind)
                    .await
            })
        }),
        Arc::new(workflows::tools::graph_tools),
        Arc::clone(&state.artifacts),
    )
}

async fn preflight<R: Runtime>(app: &AppHandle<R>, spec: &RunSpec) -> Result<(), String> {
    workspace(app, &spec.workspace_id)?;
    if spec.mode == RunMode::DryRun {
        return Ok(());
    }
    for route in &spec.routes {
        let kind = capabilities::provider_kind(&route.provider)?;
        capabilities::validate_route(kind, route)?;
        let provider = app
            .state::<super::agent_chat::ProviderRegistry>()
            .get(kind)
            .await
            .ok_or_else(|| format!("{} is not configured", route.provider))?;
        let capability = capabilities::describe(kind, provider.managed_capabilities());
        if !capability.live {
            return Err(format!(
                "{} cannot run managed workflows: {}",
                route.provider,
                capability.reason.unwrap_or_default()
            ));
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn workflow_capabilities<R: Runtime>(
    app: AppHandle<R>,
    flags: State<'_, ObservabilityStore>,
) -> Result<Vec<WorkflowCapability>, String> {
    gate(&flags)?;
    app.state::<WorkflowState>().service()?;
    let mut output = vec![];
    for kind in [
        ProviderKind::Claude,
        ProviderKind::Codex,
        ProviderKind::Cursor,
        ProviderKind::Grok,
        ProviderKind::Hermes,
        ProviderKind::OpenCode,
    ] {
        let provider = app
            .state::<super::agent_chat::ProviderRegistry>()
            .get(kind)
            .await;
        let mut capability = capabilities::describe(
            kind,
            provider
                .as_ref()
                .map(|provider| provider.managed_capabilities())
                .unwrap_or_default(),
        );
        if provider.is_none() {
            capability.reason = Some("Provider is not configured".into());
        }
        output.push(capability);
    }
    Ok(output)
}

#[tauri::command]
pub fn workflow_list(
    workspace_id: String,
    state: State<'_, WorkflowState>,
    flags: State<'_, ObservabilityStore>,
) -> Result<Vec<workflows::RunSummary>, String> {
    gate(&flags)?;
    state.service()?.list_summaries(&workspace_id, 100)
}

#[tauri::command]
pub fn workflow_get(
    run_id: String,
    state: State<'_, WorkflowState>,
    flags: State<'_, ObservabilityStore>,
) -> Result<RunSnapshot, String> {
    gate(&flags)?;
    state.service()?.snapshot(&run_id)
}

#[tauri::command]
pub async fn workflow_create<R: Runtime>(
    app: AppHandle<R>,
    mut spec: RunSpec,
    idempotency_key: String,
    state: State<'_, WorkflowState>,
    flags: State<'_, ObservabilityStore>,
) -> Result<RunSnapshot, String> {
    gate(&flags)?;
    let path = workspace(&app, &spec.workspace_id)?;
    state.policy.constrain(&mut spec, &path)?;
    preflight(&app, &spec).await?;
    let service = state.service()?;
    let (run, fresh) = service.create_paused(spec, &idempotency_key)?;
    if fresh {
        pin_artifacts(&app, &state, &run).await?;
        service.resume(&run.id)?;
    }
    start_runtime(&app, &state)?;
    if fresh && run.spec.script.is_some() {
        if let Err(error) = state.scripts.start(service.clone(), &run.id) {
            let _ = service.set_script_status(
                &run.id,
                ScriptStatus::Failed,
                None,
                Some(workflows::scripts::bounded_error(error.clone())),
            );
            return Err(error);
        }
    }
    service.snapshot(&run.id)
}

#[derive(Deserialize)]
pub struct ScriptExecutionInput {
    pub spec: RunSpec,
    pub source: String,
    #[serde(default)]
    pub args: Value,
    pub idempotency_key: String,
}

#[tauri::command]
pub async fn workflow_script_execute<R: Runtime>(
    app: AppHandle<R>,
    mut input: ScriptExecutionInput,
    state: State<'_, WorkflowState>,
    flags: State<'_, ObservabilityStore>,
) -> Result<RunSnapshot, String> {
    gate(&flags)?;
    workflows::scripts::validate_source(&input.source)?;
    input.spec.script = Some(ScriptSpec {
        source: input.source,
        args: input.args,
        api_version: workflows::scripts::SCRIPT_API_VERSION,
    });
    let path = workspace(&app, &input.spec.workspace_id)?;
    state.policy.constrain(&mut input.spec, &path)?;
    preflight(&app, &input.spec).await?;
    let service = state.service()?;
    let (run, fresh) = service.create_paused(input.spec, &input.idempotency_key)?;
    if fresh {
        pin_artifacts(&app, &state, &run).await?;
        service.resume(&run.id)?;
    }
    start_runtime(&app, &state)?;
    if fresh {
        if let Err(error) = state.scripts.start(service.clone(), &run.id) {
            let _ = service.set_script_status(
                &run.id,
                ScriptStatus::Failed,
                None,
                Some(workflows::scripts::bounded_error(error.clone())),
            );
            return Err(error);
        }
    }
    Ok(service.snapshot(&run.id)?)
}

#[tauri::command]
pub fn workflow_pause(
    run_id: String,
    state: State<'_, WorkflowState>,
    flags: State<'_, ObservabilityStore>,
) -> Result<RunSnapshot, String> {
    gate(&flags)?;
    state.service()?.pause(&run_id)
}

#[tauri::command]
pub async fn workflow_resume<R: Runtime>(
    app: AppHandle<R>,
    run_id: String,
    state: State<'_, WorkflowState>,
    flags: State<'_, ObservabilityStore>,
) -> Result<RunSnapshot, String> {
    gate(&flags)?;
    let service = state.service()?;
    let before = service.snapshot(&run_id)?;
    preflight(&app, &before.spec).await?;
    pin_artifacts(&app, &state, &before).await?;
    let run = service.resume(&run_id)?;
    start_runtime(&app, &state)?;
    if run.spec.script.is_some()
        && run.script.as_ref().is_some_and(|script| {
            matches!(script.status, ScriptStatus::Pending | ScriptStatus::Paused)
        })
    {
        if state.scripts.is_running(&run_id) {
            service.set_script_status(&run_id, ScriptStatus::Running, None, None)?;
        } else {
            if let Err(error) = state.scripts.start(service.clone(), &run_id) {
                let _ = service.pause(&run_id);
                let _ = service.set_script_status(
                    &run_id,
                    ScriptStatus::Paused,
                    None,
                    Some(workflows::scripts::bounded_error(error.clone())),
                );
                return Err(error);
            }
        }
    }
    service.snapshot(&run_id)
}

#[tauri::command]
pub fn workflow_cancel(
    run_id: String,
    state: State<'_, WorkflowState>,
    flags: State<'_, ObservabilityStore>,
) -> Result<RunSnapshot, String> {
    gate(&flags)?;
    state.service()?.cancel(&run_id)
}

#[tauri::command]
pub fn workflow_cancel_task(
    run_id: String,
    task_id: String,
    state: State<'_, WorkflowState>,
    flags: State<'_, ObservabilityStore>,
) -> Result<RunSnapshot, String> {
    gate(&flags)?;
    state.service()?.cancel_task(&run_id, &task_id)
}

#[tauri::command]
pub fn workflow_retry<R: Runtime>(
    app: AppHandle<R>,
    run_id: String,
    task_id: String,
    state: State<'_, WorkflowState>,
    flags: State<'_, ObservabilityStore>,
) -> Result<RunSnapshot, String> {
    gate(&flags)?;
    let run = state.service()?.retry(&run_id, &task_id)?;
    start_runtime(&app, &state)?;
    Ok(run)
}

/// A request to check recovery is not evidence of a stop. Only the managed
/// executor's process identity and drained tool callbacks release this hold.
#[tauri::command]
pub async fn workflow_reconcile<R: Runtime>(
    app: AppHandle<R>,
    run_id: String,
    task_id: String,
    state: State<'_, WorkflowState>,
    flags: State<'_, ObservabilityStore>,
) -> Result<RunSnapshot, String> {
    gate(&flags)?;
    let service = state.service()?;
    let run = service.snapshot(&run_id)?;
    let task = run
        .tasks
        .iter()
        .find(|task| task.spec.id == task_id)
        .ok_or("Unknown task")?;
    let attempt = task
        .current_attempt
        .as_ref()
        .filter(|attempt| attempt.status == workflows::AttemptStatus::Unknown)
        .ok_or("Only an unknown current attempt needs reconciliation")?;
    if run.spec.mode == RunMode::Live {
        let reference = attempt
            .external_ref
            .as_ref()
            .ok_or("Runtime identity is missing; stop cannot be established")?;
        start_runtime(&app, &state)?;
        let driver = state
            .live
            .lock()
            .map_err(|_| "Workflow driver registry is unavailable")?
            .clone()
            .ok_or("Workflow driver has not initialized")?;
        driver.reconcile_owned_runtime(reference).await?;
    }
    let route = run
        .spec
        .routes
        .iter()
        .find(|route| route.id == attempt.route_id)
        .ok_or("Attempt route no longer exists")?;
    let dispatch = Dispatch {
        run_id: run_id.clone(),
        task_id,
        generation: task.generation,
        attempt_id: attempt.id.clone(),
        operation_id: attempt.operation_id.clone(),
        task: task.spec.clone(),
        route: route.clone(),
        mode: run.spec.mode,
        workspace_id: run.spec.workspace_id.clone(),
        goal: run.spec.goal.clone(),
        messages: task.messages.clone(),
        dependency_results: vec![],
    };
    let mut report = if run.cancel_requested || attempt.cancel_requested {
        ExecutionReport::cancelled()
    } else {
        ExecutionReport::failed(
            "Interrupted execution is confirmed stopped; retry the task to execute it again",
        )
    };
    report.usage = attempt.usage.clone();
    service.reconcile_attempt(&dispatch, report)
}

#[tauri::command]
pub fn workflow_replace(
    run_id: String,
    task_id: String,
    spec: TaskSpec,
    idempotency_key: String,
    state: State<'_, WorkflowState>,
    flags: State<'_, ObservabilityStore>,
) -> Result<RunSnapshot, String> {
    gate(&flags)?;
    state
        .service()?
        .replace(&run_id, &task_id, spec, &idempotency_key)
}

#[tauri::command]
pub fn workflow_retire(
    run_id: String,
    task_id: String,
    state: State<'_, WorkflowState>,
    flags: State<'_, ObservabilityStore>,
) -> Result<RunSnapshot, String> {
    gate(&flags)?;
    state.service()?.retire(&run_id, &task_id)
}

#[tauri::command]
pub fn workflow_message(
    run_id: String,
    task_id: String,
    text: String,
    state: State<'_, WorkflowState>,
    flags: State<'_, ObservabilityStore>,
) -> Result<RunSnapshot, String> {
    gate(&flags)?;
    state.service()?.message(&run_id, &task_id, &text)
}

#[tauri::command]
pub fn workflow_script_save<R: Runtime>(
    app: AppHandle<R>,
    workspace_id: String,
    name: String,
    source: String,
    state: State<'_, WorkflowState>,
    flags: State<'_, ObservabilityStore>,
) -> Result<ScriptTemplate, String> {
    gate(&flags)?;
    workspace(&app, &workspace_id)?;
    workflows::scripts::save_template(&state.service()?, &workspace_id, &name, &source)
}

#[tauri::command]
pub fn workflow_script_list(
    workspace_id: String,
    state: State<'_, WorkflowState>,
    flags: State<'_, ObservabilityStore>,
) -> Result<Vec<ScriptTemplate>, String> {
    gate(&flags)?;
    workflows::scripts::list_templates(&state.service()?, &workspace_id)
}

#[tauri::command]
pub async fn workflow_integrate_artifact<R: Runtime>(
    app: AppHandle<R>,
    run_id: String,
    task_id: String,
    attempt_id: String,
    digest: String,
    state: State<'_, WorkflowState>,
    flags: State<'_, ObservabilityStore>,
) -> Result<IntegrationReport, String> {
    gate(&flags)?;
    let run = state.service()?.snapshot(&run_id)?;
    if !run.spec.allow_writes || run.spec.mode != RunMode::Live {
        return Err("This run has no live file-change permission".into());
    }
    let generation = run
        .tasks
        .iter()
        .find(|task| task.spec.id == task_id)
        .ok_or("Unknown task")?
        .generation;
    let artifact = state.artifacts.inspect(&attempt_id)?;
    if artifact.run_id != run_id
        || artifact.source
            != workspace(&app, &run.spec.workspace_id)?
                .canonicalize()
                .map_err(|e| e.to_string())?
    {
        return Err("Artifact workspace ownership changed".into());
    }
    let store = Arc::clone(&state.artifacts);
    let service = state.service()?;
    tauri::async_runtime::spawn_blocking(move || {
        service.with_accepted_attempt(&run_id, &task_id, &attempt_id, generation, |_, attempt| {
            if !attempt.artifacts.iter().any(|artifact| {
                artifact.get("digest").and_then(Value::as_str) == Some(&digest)
                    && artifact.get("attempt_id").and_then(Value::as_str) == Some(&attempt_id)
            }) {
                return Err("Artifact digest is not the accepted task result".into());
            }
            store.integrate(&attempt_id, &digest)
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

async fn pin_artifacts<R: Runtime>(
    app: &AppHandle<R>,
    state: &WorkflowState,
    run: &RunSnapshot,
) -> Result<(), String> {
    if run.spec.mode != RunMode::Live {
        return Ok(());
    }
    let source = workspace(app, &run.spec.workspace_id)?;
    let artifacts = Arc::clone(&state.artifacts);
    let run_id = run.id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || artifacts.pin_run(&run_id, &source))
        .await
        .map_err(|e| e.to_string())?;
    if let Err(error) = result {
        let _ = state.service()?.cancel(&run.id);
        return Err(format!("Could not pin the workflow workspace: {error}"));
    }
    Ok(())
}

#[tauri::command]
pub async fn workflow_artifact_preview<R: Runtime>(
    app: AppHandle<R>,
    run_id: String,
    task_id: String,
    attempt_id: String,
    digest: String,
    path: String,
    state: State<'_, WorkflowState>,
    flags: State<'_, ObservabilityStore>,
) -> Result<ArtifactPreview, String> {
    gate(&flags)?;
    let service = state.service()?;
    let run = service.snapshot(&run_id)?;
    let task = run
        .tasks
        .iter()
        .find(|task| task.spec.id == task_id)
        .ok_or("Unknown task")?;
    let attempt = task
        .attempts
        .iter()
        .find(|attempt| attempt.id == attempt_id)
        .ok_or("Artifact attempt does not belong to this task")?;
    if !attempt.artifacts.iter().any(|artifact| {
        artifact.get("digest").and_then(Value::as_str) == Some(&digest)
            && artifact.get("attempt_id").and_then(Value::as_str) == Some(&attempt_id)
    }) {
        return Err("Artifact digest is not an accepted task result".into());
    }
    let artifact = state.artifacts.inspect(&attempt_id)?;
    if artifact.run_id != run_id
        || artifact.source
            != workspace(&app, &run.spec.workspace_id)?
                .canonicalize()
                .map_err(|e| e.to_string())?
    {
        return Err("Artifact workspace ownership changed".into());
    }
    let store = Arc::clone(&state.artifacts);
    tauri::async_runtime::spawn_blocking(move || store.preview(&attempt_id, &digest, &path))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn workflow_runtime_has_one_process_owner() {
        let directory = tempfile::tempdir().unwrap();
        let owner = lock_runtime(directory.path()).unwrap();
        assert!(lock_runtime(directory.path()).is_err());
        drop(owner);
        assert!(lock_runtime(directory.path()).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn workflow_commands_ipc_execute_persisted_dry_run_without_provider_registry() {
        use serde_json::json;
        use std::time::{Duration, Instant};
        use tauri::ipc::{CallbackFn, InvokeResponseBody};
        use tauri::test::{mock_builder, mock_context, noop_assets, INVOKE_KEY};
        use tauri::webview::InvokeRequest;

        fn invoke(
            view: &tauri::WebviewWindow<tauri::test::MockRuntime>,
            command: &str,
            arguments: Value,
        ) -> Result<Value, Value> {
            let response = tauri::test::get_ipc_response(
                view,
                InvokeRequest {
                    cmd: command.into(),
                    callback: CallbackFn(0),
                    error: CallbackFn(1),
                    url: view.url().unwrap(),
                    body: arguments.into(),
                    headers: Default::default(),
                    invoke_key: INVOKE_KEY.into(),
                },
            )?;
            match response {
                InvokeResponseBody::Json(value) => Ok(serde_json::from_str(&value).unwrap()),
                InvokeResponseBody::Raw(_) => {
                    panic!("Workflow IPC returned an unexpected raw response")
                }
            }
        }
        fn completed(view: &tauri::WebviewWindow<tauri::test::MockRuntime>, id: &str) -> Value {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let run = invoke(view, "workflow_get", json!({"runId":id})).unwrap();
                if run["status"] == "completed" {
                    return run;
                }
                assert!(
                    Instant::now() < deadline,
                    "Workflow did not complete: {run}"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        struct OwnedDriver {
            shutdown: watch::Sender<bool>,
            task: Option<tauri::async_runtime::JoinHandle<Result<(), String>>>,
        }
        impl Drop for OwnedDriver {
            fn drop(&mut self) {
                let _ = self.shutdown.send(true);
                if let Some(task) = self.task.take() {
                    task.abort();
                }
            }
        }

        let directory = tempfile::tempdir().unwrap();
        let workspace_path = directory.path().join("workspace");
        std::fs::create_dir_all(workspace_path.join(".codemux")).unwrap();
        std::fs::write(
            workspace_path.join(".codemux/workflows.json"),
            r#"{"concurrency":1,"max_tasks":3,"max_attempts":6,"deny_writes":true}"#,
        )
        .unwrap();
        let policy_path = directory.path().join("policy.json");
        std::fs::write(&policy_path,
            r#"{"global_concurrency":2,"run_caps":{"concurrency":2,"max_tasks":8,"max_attempts":12}}"#).unwrap();
        let policy = HostPolicy::load(&policy_path).unwrap();
        let database = directory.path().join("workflows.db");
        let service = WorkflowService::open(&database, policy.global_concurrency).unwrap();
        let state = AppStateStore::default();
        let local = state.create_empty_workspace_at_path(workspace_path.clone());
        let remote = state.create_empty_workspace_at_path(workspace_path);
        state.set_workspace_host_id(&remote.0, Some(17)).unwrap();
        let artifact_root = directory.path().join("artifacts");
        let app = mock_builder()
            .manage(state)
            .manage(ObservabilityStore::default())
            .manage(WorkflowState {
                service: Ok(service.clone()),
                scripts: WorkflowScriptRuntime::default(),
                artifacts: Arc::new(ArtifactStore::new(artifact_root.clone())),
                // Own and join the actual dry driver below. No provider registry
                // is installed: any accidental model lookup makes this fail.
                started: AtomicBool::new(true),
                policy,
                _ownership: Some(lock_runtime(directory.path()).unwrap()),
                live: Mutex::new(None),
            })
            .invoke_handler(tauri::generate_handler![
                crate::commands::workflows::workflow_create,
                crate::commands::workflows::workflow_get,
                crate::commands::workflows::workflow_list,
                crate::commands::workflows::workflow_script_execute,
                crate::commands::workflows::workflow_script_save,
                crate::commands::workflows::workflow_script_list,
            ])
            .build(mock_context(noop_assets()))
            .unwrap();
        let view = tauri::WebviewWindowBuilder::new(&app, "workflow-test", Default::default())
            .build()
            .unwrap();
        let (shutdown, receiver) = watch::channel(false);
        let driver_service = service.clone();
        let mut driver = OwnedDriver {
            shutdown,
            task: Some(tauri::async_runtime::spawn(async move {
                driver_service
                    .run_driver_until(Arc::new(DryRunDriver::default()), receiver)
                    .await
            })),
        };
        let input = json!({"workspace_id":local.0,"title":"IPC fixture","goal":"No model calls",
            "mode":"dry_run","routes":[{"id":"claude","provider":"claude"},{"id":"codex","provider":"codex"}],
            "limits":{"concurrency":4,"max_tasks":16,"max_attempts":16,"wall_time_ms":5000},"tasks":[]});
        for (id, expected) in [
            ("missing-workspace", "no longer available"),
            (remote.0.as_str(), "local workspace"),
        ] {
            let mut invalid = input.clone();
            invalid["workspace_id"] = json!(id);
            assert!(invoke(
                &view,
                "workflow_create",
                json!({"spec":invalid,"idempotencyKey":id})
            )
            .unwrap_err()
            .as_str()
            .unwrap()
            .contains(expected));
        }
        let mut denied = input.clone();
        denied["allow_writes"] = json!(true);
        assert!(invoke(
            &view,
            "workflow_create",
            json!({"spec":denied,"idempotencyKey":"write-denied"})
        )
        .unwrap_err()
        .as_str()
        .unwrap()
        .contains("disabled by workflow policy"));

        let mut plain = input.clone();
        plain["tasks"] =
            json!([{"id":"plain","title":"Plain","prompt":"Inspect","route_id":"claude"}]);
        let created = invoke(
            &view,
            "workflow_create",
            json!({"spec":plain,"idempotencyKey":"plain-create"}),
        )
        .unwrap();
        let plain_run = completed(&view, created["id"].as_str().unwrap());
        assert_eq!(
            plain_run["tasks"][0]["attempts"].as_array().unwrap().len(),
            1
        );

        let source = r#"
            const results=await parallel(['claude','codex'],route_id=>agent('Inspect '+route_id,{id:'inspect-'+route_id,route_id}));
            const final=await agent('Summarize',{id:'final',route_id:'codex'});
            return {results,final,mark:args.mark};
        "#;
        let saved = invoke(
            &view,
            "workflow_script_save",
            json!({"workspaceId":local.0,"name":"Reusable","source":"return 1;"}),
        )
        .unwrap();
        let updated = invoke(
            &view,
            "workflow_script_save",
            json!({"workspaceId":local.0,"name":"Reusable","source":source}),
        )
        .unwrap();
        assert_eq!(saved["id"], updated["id"]);
        let templates = invoke(
            &view,
            "workflow_script_list",
            json!({"workspaceId":local.0}),
        )
        .unwrap();
        assert_eq!(templates.as_array().unwrap().len(), 1);
        assert_eq!(templates[0]["source"], source);
        let request = json!({"input":{"spec":input,"source":source,"args":{"mark":"persisted"},"idempotency_key":"script-execute"}});
        let launched = invoke(&view, "workflow_script_execute", request.clone()).unwrap();
        let id = launched["id"].as_str().unwrap();
        let finished = completed(&view, id);
        assert_eq!(finished["resolved_limits"]["concurrency"], 1);
        assert_eq!(finished["resolved_limits"]["max_tasks"], 3);
        assert_eq!(finished["resolved_limits"]["max_attempts"], 6);
        assert_eq!(finished["script"]["status"], "completed");
        assert_eq!(finished["script"]["result"]["mark"], "persisted");
        assert_eq!(
            finished["script"]["result"]["results"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert!(finished["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|task| task["status"] == "succeeded"
                && task["attempts"].as_array().unwrap().len() == 1));
        assert_eq!(finished["usage"]["total_tokens"], 0);
        assert_eq!(finished["usage"]["cost_usd"], 0.0);
        assert_eq!(finished["usage"]["cost_unknown"], false);
        let replay = invoke(&view, "workflow_script_execute", request).unwrap();
        assert_eq!(replay["id"], finished["id"]);
        assert_eq!(replay["revision"], finished["revision"]);
        let history = invoke(&view, "workflow_list", json!({"workspaceId":local.0})).unwrap();
        assert_eq!(history.as_array().unwrap().len(), 2);
        assert!(history
            .as_array()
            .unwrap()
            .iter()
            .all(|summary| summary.get("tasks").is_none()));
        assert!(
            !artifact_root.exists(),
            "Dry runs must not prepare live artifacts"
        );
        let _ = driver.shutdown.send(true);
        tauri::async_runtime::block_on(async {
            tokio::time::timeout(Duration::from_secs(2), driver.task.take().unwrap())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
        });
        let persisted = WorkflowService::open(database, 2)
            .unwrap()
            .snapshot(id)
            .unwrap();
        assert_eq!(persisted.status, workflows::RunStatus::Completed);
        assert_eq!(persisted.tasks.len(), 3);
        assert_eq!(
            persisted.script.unwrap().result.unwrap()["mark"],
            "persisted"
        );
    }
}
