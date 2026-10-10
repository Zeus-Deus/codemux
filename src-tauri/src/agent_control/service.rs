//! Launch sequencing belongs here, above the existing native session owner.
use super::{ControlAccess, ControlCaller, ControlError, NativeControlGuard, NativeControlState};
use crate::agent_provider::{ProviderChatCapabilities, ProviderKind, StartSessionInput, ThreadId};
use crate::commands::agent_chat::{self, ProviderRegistry, SendTurnCommandInput};
use crate::database::{AgentChatSessionConfig, DatabaseStore};
use crate::state::{AppStateStore, WorkspaceSnapshot};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager, Runtime};

pub(crate) fn parse<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, ControlError> {
    if !value.is_object() { return Err(ControlError::new("invalid_arguments", "Arguments must be an object")); }
    serde_json::from_value(value).map_err(|e| ControlError::new("invalid_arguments", e.to_string()))
}

pub(crate) fn identifier(value: &str, name: &str, maximum: usize) -> Result<(), ControlError> {
    if value.is_empty() || value.len() > maximum || value.trim() != value || value.chars().any(char::is_control) {
        return Err(ControlError::new("invalid_arguments", format!("{name} must be a nonempty unchanged identifier (max {maximum} bytes)")));
    }
    Ok(())
}

pub(crate) fn workspace<R: Runtime>(app: &AppHandle<R>, id: &str, mutation: bool) -> Result<WorkspaceSnapshot, ControlError> {
    identifier(id, "workspace_id", 256)?;
    let workspace = app.state::<AppStateStore>().snapshot().workspaces.into_iter().find(|w| w.workspace_id.0 == id)
        .ok_or_else(|| ControlError::new("workspace_not_found", "Select an existing workspace explicitly"))?;
    if mutation && workspace.is_local_import_snapshot_only() {
        return Err(ControlError::new("imported_snapshot_read_only", "Imported workspaces are read-only snapshots"));
    }
    if mutation && (workspace.host_id.is_some() || workspace.remote_cwd.is_some()) {
        return Err(ControlError::new("unsupported_remote_workspace", "Native outside control currently targets local workspaces"));
    }
    Ok(workspace)
}

/// The connector owns authentication; these checks own the worker ceiling.
/// Unknown provider mode semantics fail closed for supervised credentials.
pub(crate) fn permission_ceiling(caller: &ControlCaller, provider: ProviderKind, mode: Option<&str>) -> Result<(), ControlError> {
    if caller.access == ControlAccess::ReadOnly {
        return Err(ControlError::new("permission_denied", "This connection has read-only access"));
    }
    if caller.access == ControlAccess::FullAccess { return Ok(()); }
    let supervised = match provider {
        ProviderKind::Claude => matches!(mode, Some("default" | "acceptEdits")),
        ProviderKind::Codex => matches!(mode, Some("read-only" | "workspace-write")),
        ProviderKind::Cursor | ProviderKind::Grok => matches!(mode, Some("ask" | "plan")),
        ProviderKind::Hermes | ProviderKind::OpenCode => false,
    };
    if supervised { Ok(()) } else {
        Err(ControlError::new("permission_ceiling", "This worker mode needs full-access consent; supervised clients cannot target full-access or unknown modes"))
    }
}

pub(crate) fn authorize<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller) -> Result<(), ControlError> {
    crate::mcp_connector::validate_caller(app,caller)
}

pub(crate) async fn capabilities<R: Runtime>(app: &AppHandle<R>, provider: ProviderKind) -> Result<ProviderChatCapabilities, ControlError> {
    #[cfg(test)]
    if let Some(state) = app.try_state::<NativeControlState>() {
        if let Some(caps) = state.fixture_capabilities.lock().unwrap().get(&provider).cloned() { return Ok(caps); }
    }
    agent_chat::list_chat_provider_capabilities(
        app.clone(), provider,
        app.state::<std::sync::Arc<crate::agent_provider::codex::capabilities::CodexCapabilityCache>>(),
        app.state::<std::sync::Arc<crate::agent_provider::cursor::capabilities::CursorCapabilityCache>>(),
        app.state::<std::sync::Arc<crate::agent_provider::grok::capabilities::GrokCapabilityCache>>(),
        app.state::<std::sync::Arc<crate::agent_provider::claude::capabilities::ClaudeCapabilityCache>>(),
        app.state::<std::sync::Arc<crate::agent_provider::opencode::OpenCodeServerManager>>(),
    ).await.map_err(ControlError::from)
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Launch {
    workspace_id: String,
    client_request_id: String,
    provider: ProviderKind,
    permission_mode: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    context_window: Option<String>,
    #[serde(default)] fast_mode: bool,
    title: Option<String>,
    message: Option<String>,
    #[serde(default)] select: bool,
    worktree: Option<Value>,
}

pub(crate) async fn launch<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, args: Value) -> Result<Value, ControlError> {
    if !args.as_object().is_some_and(|args| args.contains_key("permission_mode")) {
        return Err(ControlError::new("permission_mode_required", "Select an advertised worker permission mode explicitly; null is allowed only when the provider has no modes"));
    }
    let input: Launch = parse(args.clone())?;
    identifier(&input.client_request_id, "client_request_id", 128)?;
    identifier(&input.workspace_id, "workspace_id", 256)?;
    if let Some(receipt) = retry_receipt(app, caller, "thread_launch", &args)? { return Ok(receipt); }
    let workspace = workspace(app, &input.workspace_id, true)?;
    permission_ceiling(caller, input.provider, input.permission_mode.as_deref())?;
    if let Some(message) = input.message.as_deref() {
        if message.trim().is_empty() || message.len() > 65536 { return Err(ControlError::new("invalid_arguments", "message must contain text and fit in 65536 bytes")); }
    }
    if let Some(title) = input.title.as_deref() { identifier(title, "title", 100)?; }
    if input.worktree.is_some() { return Err(ControlError::new("unsupported_operation", "Worktree launch is not available yet")); }
    agent_chat::feature_flag_on(&app.state::<crate::observability::ObservabilityStore>())?;
    if app.state::<ProviderRegistry>().get(input.provider).await.is_none() {
        return Err(ControlError::new("provider_not_configured", "The selected provider is not configured in CodeMux"));
    }
    let caps = capabilities(app, input.provider).await?;
    if !(caps.permission_modes.is_empty() && input.permission_mode.is_none())
        && !caps.permission_modes.iter().any(|option| Some(&option.value) == input.permission_mode.as_ref()) {
        return Err(ControlError::new("unsupported_permission_mode", "Select an exact mode from agent_capabilities"));
    }
    let model = match &input.model {
        Some(id) => Some(caps.models.iter().find(|model| &model.id == id)
            .ok_or_else(|| ControlError::new("unsupported_model", "Select an exact model from agent_capabilities"))?),
        None => None,
    };
    if let Some(effort) = &input.effort {
        if !model.is_some_and(|model| model.effort_levels.contains(effort)) {
            return Err(ControlError::new("unsupported_effort", "Explicit effort requires a selected model advertising that exact value"));
        }
    }
    if let Some(context) = &input.context_window {
        if !model.is_some_and(|model| model.context_window_options.iter().any(|option| &option.value == context)) {
            return Err(ControlError::new("unsupported_context_window", "The selected model does not advertise that context window"));
        }
    }
    if input.fast_mode && !model.is_some_and(|model| model.supports_fast_mode) {
        return Err(ControlError::new("unsupported_fast_mode", "The selected model does not advertise fast mode"));
    }
    authorize(app, caller)?;
    let epoch = app.state::<NativeControlState>().epoch.clone();
    let fingerprint = format!("{:x}", Sha256::digest(serde_json::to_vec(&args).map_err(|e|ControlError::new("invalid_arguments",e.to_string()))?));
    let thread = uuid::Uuid::new_v4().to_string();
    let admitted = app.state::<DatabaseStore>().admit_control_operation(
        &caller.principal, &input.client_request_id, "thread_launch", &fingerprint, &epoch,
        Some(&workspace.workspace_id.0), Some(&thread),
    ).map_err(|e| ControlError::new(if e.starts_with("request_key_conflict") { "request_key_conflict" } else { "operation_admission_failed" }, e))?;
    let receipt = serde_json::to_value(&admitted.receipt).map_err(|e|ControlError::new("serialization_failed",e.to_string()))?;
    if !admitted.newly_admitted { return Ok(receipt); }
    let app = app.clone();
    let caller = caller.clone();
    let operation = admitted.receipt.operation_id;
    tauri::async_runtime::spawn(async move {
        let db = app.state::<DatabaseStore>();
        if db.update_control_operation(&caller.principal, &operation, &epoch, "running", None, None, None, None).is_err() { return; }
        let outcome = materialize(&app, &caller, input, &thread).await;
        let (state, result, error) = match outcome {
            Ok(result) => ("succeeded", Some(result), None),
            Err(error) => ("failed", None, serde_json::to_value(error).ok()),
        };
        if let Err(error) = db.update_control_operation(&caller.principal, &operation, &epoch, state, None, None, result.as_ref(), error.as_ref()) {
            eprintln!("[codemux::agent_control] could not save operation outcome: {error}");
        }
    });
    Ok(receipt)
}

async fn materialize<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, input: Launch, thread: &str) -> Result<Value, ControlError> {
    authorize(app, caller)?;
    let workspace = workspace(app, &input.workspace_id, true)?;
    permission_ceiling(caller, input.provider, input.permission_mode.as_deref())?;
    let provider_name = serde_json::to_value(input.provider).map_err(|e|ControlError::new("serialization_failed",e.to_string()))?;
    let db = app.state::<DatabaseStore>();
    // Save configuration before publishing a bound pane. A mounting renderer
    // cannot see an unbound draft and start a competing provider session.
    db.upsert_agent_chat_session(thread, &input.workspace_id, Some(&workspace.cwd), provider_name.as_str().unwrap_or_default())?;
    db.update_agent_chat_session_config(thread, &AgentChatSessionConfig {
        model: Some(input.model.clone()), effort: Some(input.effort.clone()),
        context_window: Some(input.context_window.clone()), permission_mode: Some(input.permission_mode.clone()),
        fast_mode: Some(input.fast_mode),
    })?;
    if let Some(title) = &input.title { db.set_agent_chat_title(thread, title)?; }
    let pane = agent_chat::agent_chat_create_pane(
        app.clone(), app.state(), app.state(), input.workspace_id.clone(), Some(input.provider),
        Some(workspace.cwd.clone()), Some(crate::presets::LaunchMode::NewTab), Some(thread.into()), Some(input.select),
    )?;
    let start = StartSessionInput {
        thread_id: ThreadId(thread.into()), cwd: workspace.cwd.into(), model: input.model,
        resume_cursor: None, fresh_session: true, permission_mode: input.permission_mode,
        effort: input.effort, context_window: input.context_window, fast_mode: input.fast_mode,
        additional_directories: vec![], env: None, workspace_id: None, extra: Value::Null, recorded_usage_baseline: None,
    };
    let control = NativeControlGuard { caller:caller.clone(),workspace_id:input.workspace_id.clone(),thread_id:thread.into(),provider:input.provider,admission:None }.retain_admission(app);
    let actual = agent_chat::agent_chat_start_session_guarded(app.clone(), pane.clone(), input.provider, start, Some(thread.into()), Some(control.clone())).await?;
    if actual.0 != thread { return Err(ControlError::new("unexpected_thread_binding", "Provider changed the native thread ID; inspect the pane before retrying")); }
    authorize(app, caller)?;
    let turn = if let Some(message) = input.message {
        let command: SendTurnCommandInput = serde_json::from_value(json!({
            "thread_id": thread, "text": message, "delivery":"queue", "model_override":null,
            "client_nonce": input.client_request_id,
        })).map_err(|e| ControlError::new("invalid_arguments", e.to_string()))?;
        Some(agent_chat::send_turn_with_control(app.clone(), input.provider, command, Some(control)).await?)
    } else { None };
    Ok(json!({"workspace_id":input.workspace_id,"thread_id":thread,"pane_id":pane,"turn":turn}))
}

pub(crate) fn operation_status<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, args: Value) -> Result<Value, ControlError> {
    #[derive(Deserialize)] #[serde(deny_unknown_fields)]
    struct Args { operation_id: String }
    let args: Args = parse(args)?;
    identifier(&args.operation_id, "operation_id", 256)?;
    let epoch = app.state::<NativeControlState>().epoch.clone();
    let receipt = app.state::<DatabaseStore>().control_operation(&caller.principal, &args.operation_id, &epoch)?
        .ok_or_else(|| ControlError::new("operation_not_found", "No operation belongs to this client and ID"))?;
    serde_json::to_value(receipt).map_err(|e| ControlError::new("serialization_failed",e.to_string()))
}

pub(crate) fn thread_list<R: Runtime>(app: &AppHandle<R>, args: Value) -> Result<Value, ControlError> {
    #[derive(Deserialize)] #[serde(deny_unknown_fields)]
    struct Args { workspace_id: String, cursor: Option<String>, limit: Option<u32> }
    let args: Args = parse(args)?;
    workspace(app, &args.workspace_id, false)?;
    let limit = args.limit.unwrap_or(20);
    if !(1..=100).contains(&limit) { return Err(ControlError::new("invalid_arguments", "limit must be between 1 and 100")); }
    if let Some(cursor) = &args.cursor { identifier(cursor, "cursor", 256)?; }
    let db = app.state::<DatabaseStore>();
    let mut ids = db.control_thread_ids(&args.workspace_id, args.cursor.as_deref(), limit + 1)?;
    let has_more = ids.len() > limit as usize;
    if has_more { ids.truncate(limit as usize); }
    let next_cursor = if has_more { ids.last().cloned() } else { None };
    let state = app.state::<AppStateStore>();
    let threads: Vec<_> = ids.iter().filter_map(|id| db.get_agent_chat_session(id)).map(|record| json!({
        "thread_id": record.thread_id,
        "workspace_id": record.workspace_id,
        "provider": record.provider,
        "title": record.title,
        "last_active_at": record.last_active_at,
        "permission_mode": record.permission_mode,
        "imported_snapshot": record.imported_from.is_some(),
        "pane_id": state.agent_chat_pane_id_for_thread(&record.thread_id),
    })).collect();
    Ok(json!({"threads":threads,"has_more":has_more,"next_cursor":next_cursor}))
}

pub(crate) fn thread_read<R: Runtime>(app: &AppHandle<R>, args: Value) -> Result<Value, ControlError> {
    #[derive(Deserialize)] #[serde(deny_unknown_fields)]
    struct Args { workspace_id: String, thread_id: String, cursor: Option<i64>, limit: Option<u32> }
    let args: Args = parse(args)?;
    workspace(app, &args.workspace_id, false)?;
    identifier(&args.thread_id, "thread_id", 256)?;
    let limit = args.limit.unwrap_or(20);
    if !(1..=100).contains(&limit) || args.cursor.is_some_and(|cursor| cursor < 0) {
        return Err(ControlError::new("invalid_arguments", "limit must be 1–100 and cursor must be nonnegative"));
    }
    let db = app.state::<DatabaseStore>();
    let record = db.get_agent_chat_session(&args.thread_id)
        .ok_or_else(|| ControlError::new("thread_not_found", "No native thread has this ID"))?;
    if record.workspace_id != args.workspace_id {
        return Err(ControlError::new("thread_outside_workspace", "This thread does not belong to the selected workspace"));
    }
    let page = db.read_agent_chat_history_page(&args.workspace_id, &args.thread_id, args.cursor, limit)?;
    serde_json::to_value(page).map_err(|e| ControlError::new("serialization_failed", e.to_string()))
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Target { pub workspace_id: String, pub thread_id: String }

pub(crate) fn resolve_thread<R: Runtime>(app: &AppHandle<R>, target: &Target, mutation: bool) -> Result<(crate::database::AgentChatSessionRecord, ProviderKind), ControlError> {
    workspace(app, &target.workspace_id, mutation)?;
    identifier(&target.thread_id, "thread_id", 256)?;
    let record = app.state::<DatabaseStore>().get_agent_chat_session(&target.thread_id)
        .ok_or_else(|| ControlError::new("thread_not_found", "No native thread has this ID"))?;
    if record.workspace_id != target.workspace_id { return Err(ControlError::new("thread_outside_workspace", "This thread does not belong to the selected workspace")); }
    let provider: ProviderKind = serde_json::from_value(json!(record.provider))
        .map_err(|_| ControlError::new("unsupported_provider", "This persisted thread has an unsupported provider"))?;
    if mutation {
        if record.imported_from.is_some() { return Err(ControlError::new("imported_snapshot_read_only", "Imported threads are read-only snapshots")); }
        let state = app.state::<AppStateStore>();
        let pane = state.agent_chat_pane_id_for_thread(&record.thread_id)
            .ok_or_else(|| ControlError::new("thread_not_bound", "Reopen this thread in CodeMux before controlling it"))?;
        if state.workspace_id_for_pane(&pane).as_deref() != Some(target.workspace_id.as_str())
            || state.agent_chat_pane_thread(&pane) != Some((provider,target.thread_id.clone())) {
            return Err(ControlError::new("stale_thread_binding", "The pane no longer belongs to this workspace/provider/thread"));
        }
    }
    Ok((record, provider))
}

pub(crate) async fn thread_status<R: Runtime>(app: &AppHandle<R>, args: Value) -> Result<Value, ControlError> {
    let target: Target = parse(args)?;
    let (record, provider) = resolve_thread(app, &target, false)?;
    let implementation = app.state::<ProviderRegistry>().get(provider).await;
    let runtime_live = match &implementation { Some(provider) => provider.has_session(&ThreadId(target.thread_id.clone())).await, None => false };
    let active = if runtime_live {
        agent_chat::agent_chat_turn_active(app.clone(), provider, ThreadId(target.thread_id.clone())).await?
    } else { false };
    let latest = app.state::<DatabaseStore>().control_last_thread_run_event(&target.thread_id)?;
    let terminal = latest.as_ref().filter(|event| event["type"] == "turn_completed");
    let observed = app.state::<NativeControlState>().thread_runtime(&target.thread_id);
    let pending: Vec<_> = observed.as_ref().map(|view|view.pending_approvals.iter().map(|request|
        json!({"request_id":request.outside_handle,"turn_id":request.turn_id,"request_kind":request.request_kind})
    ).collect()).unwrap_or_default();
    let queued = observed.as_ref().map(|view|view.queued_ids.clone()).unwrap_or_default();
    let busy = active || !pending.is_empty() || !queued.is_empty()
        || observed.as_ref().is_some_and(|view|matches!(view.phase.as_str(),"starting"|"running"|"waiting_approval"));
    let phase = if !pending.is_empty() { "waiting_approval" }
    else if let Some(view) = observed.as_ref().filter(|view|view.phase != "unknown") { view.phase.as_str() }
    else if active { "running" } else if let Some(event) = terminal {
        match event["status"]["kind"].as_str() { Some("success") => "completed", Some("interrupted") => "interrupted", _ => "error" }
    } else if runtime_live { "ready" } else { "unknown" };
    let last_turn = observed.as_ref().and_then(|view|view.last_turn.clone()).or_else(||terminal.map(|event|
        json!({"type":"turn_completed","turn_id":event["turn_id"],"status":{"kind":event["status"]["kind"]}})));
    let settled = !busy && matches!(phase,"completed"|"interrupted"|"error") && last_turn.is_some();
    let turn_id = observed.as_ref().and_then(|view|view.turn_id.clone()).or_else(||terminal.and_then(|event|event["turn_id"].as_str().map(str::to_owned)));
    Ok(json!({
        "workspace_id":target.workspace_id,"thread_id":target.thread_id,"provider":provider,
        "pane_id":app.state::<AppStateStore>().agent_chat_pane_id_for_thread(&record.thread_id),
        "runtime_live":runtime_live,"phase":phase,"settled":settled,
        "turn_id":turn_id,"pending_approvals":pending,"queued_ids":queued,"last_turn":last_turn,
        "imported_snapshot":record.imported_from.is_some(),
    }))
}

pub(crate) async fn thread_send<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, args: Value) -> Result<Value, ControlError> {
    #[derive(Deserialize)] #[serde(deny_unknown_fields)]
    struct Args {
        workspace_id: String, thread_id: String, client_request_id: String, message: String,
        #[serde(default)] delivery: crate::agent_provider::types::MessageDelivery,
    }
    let input: Args = parse(args.clone())?;
    identifier(&input.client_request_id, "client_request_id", 128)?;
    if input.message.trim().is_empty() || input.message.len() > 65536 {
        return Err(ControlError::new("invalid_arguments", "message must contain text and fit in 65536 bytes"));
    }
    let target = Target { workspace_id: input.workspace_id.clone(), thread_id: input.thread_id.clone() };
    if let Some(receipt) = retry_receipt(app, caller, "thread_send", &args)? { return Ok(receipt); }
    let (record, provider) = resolve_thread(app, &target, true)?;
    permission_ceiling(caller, provider, record.permission_mode.as_deref())?;
    let admission = admit_mutation(app, caller, "thread_send", &args, &target)?;
    let receipt = serde_json::to_value(&admission.receipt).map_err(|e|ControlError::new("serialization_failed",e.to_string()))?;
    if !admission.newly_admitted { return Ok(receipt); }
    let owned_app = app.clone();
    let owned_caller = caller.clone();
    let control = NativeControlGuard { caller:caller.clone(),workspace_id:target.workspace_id.clone(),thread_id:target.thread_id.clone(),provider,admission:None }.retain_admission(app);
    spawn_operation(app, caller, admission.receipt, async move {
        authorize(&owned_app,&owned_caller)?;
        let (record, current_provider) = resolve_thread(&owned_app, &target, true)?;
        if provider != current_provider { return Err(ControlError::new("stale_thread_binding", "The thread changed provider after admission")); }
        permission_ceiling(&owned_caller,current_provider,record.permission_mode.as_deref())?;
        let command: SendTurnCommandInput = serde_json::from_value(json!({
            "thread_id":input.thread_id,"text":input.message,"delivery":input.delivery,
            "model_override":null,"client_nonce":input.client_request_id,
        })).map_err(|e|ControlError::new("invalid_arguments",e.to_string()))?;
        let turn = agent_chat::send_turn_with_control(owned_app.clone(), provider, command, Some(control)).await?;
        Ok(json!({"workspace_id":target.workspace_id,"thread_id":target.thread_id,"accepted_as":if turn.queued_id.is_some() {"queued"} else if turn.steered {"steered"} else {"dispatched"},"turn":turn}))
    });
    Ok(receipt)
}

fn retry_receipt<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, tool: &str, args: &Value) -> Result<Option<Value>, ControlError> {
    authorize(app, caller)?;
    for field in ["workspace_id", "thread_id"] {
        if let Some(value) = args.get(field) {
            identifier(value.as_str().ok_or_else(|| ControlError::new("invalid_arguments", field))?, field, 256)?;
        }
    }
    let key = args["client_request_id"].as_str().ok_or_else(|| ControlError::new("invalid_arguments", "client_request_id is required"))?;
    identifier(key, "client_request_id", 128)?;
    let hash = format!("{:x}", Sha256::digest(serde_json::to_vec(args).map_err(|e| ControlError::new("invalid_arguments", e.to_string()))?));
    app.state::<DatabaseStore>().control_operation_by_key(&caller.principal, key, tool, &hash, &app.state::<NativeControlState>().epoch)
        .map_err(|e| ControlError::new(if e.starts_with("request_key_conflict") { "request_key_conflict" } else { "operation_lookup_failed" }, e))?
        .map(serde_json::to_value).transpose().map_err(|e| ControlError::new("serialization_failed", e.to_string()))
}

fn admit_mutation<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, tool: &str, args: &Value, target: &Target) -> Result<crate::database::agent_control::Admission, ControlError> {
    authorize(app,caller)?;
    let key = args["client_request_id"].as_str().ok_or_else(||ControlError::new("invalid_arguments","client_request_id is required"))?;
    identifier(key,"client_request_id",128)?;
    let hash = format!("{:x}",Sha256::digest(serde_json::to_vec(args).map_err(|e|ControlError::new("invalid_arguments",e.to_string()))?));
    let epoch = app.state::<NativeControlState>().epoch.clone();
    app.state::<DatabaseStore>().admit_control_operation(&caller.principal,key,tool,&hash,&epoch,Some(&target.workspace_id),Some(&target.thread_id))
        .map_err(|e|ControlError::new(if e.starts_with("request_key_conflict") {"request_key_conflict"} else {"operation_admission_failed"},e))
}

fn spawn_operation<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, receipt: crate::database::agent_control::OperationReceipt, work: impl std::future::Future<Output=Result<Value,ControlError>> + Send + 'static) {
    let app = app.clone();
    let caller = caller.clone();
    let epoch = app.state::<NativeControlState>().epoch.clone();
    #[cfg(test)]
    let barrier = app.state::<NativeControlState>().fixture_operation_barrier.lock().unwrap().take();
    tauri::async_runtime::spawn(async move {
        #[cfg(test)]
        if let Some(barrier) = barrier { barrier.acquire().await.unwrap().forget(); }
        let db = app.state::<DatabaseStore>();
        if db.update_control_operation(&caller.principal,&receipt.operation_id,&epoch,"running",None,None,None,None).is_err() { return; }
        let (state,result,error) = match work.await {
            Ok(result)=>("succeeded",Some(result),None),
            Err(error)=>("failed",None,serde_json::to_value(error).ok()),
        };
        if let Err(error) = db.update_control_operation(&caller.principal,&receipt.operation_id,&epoch,state,None,None,result.as_ref(),error.as_ref()) {
            eprintln!("[codemux::agent_control] could not save operation outcome: {error}");
        }
    });
}

pub(crate) async fn agent_capabilities<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, args: Value) -> Result<Value, ControlError> {
    #[derive(Deserialize)] #[serde(deny_unknown_fields)]
    struct Args { workspace_id: String, provider: Option<ProviderKind> }
    let args: Args = parse(args)?;
    workspace(app,&args.workspace_id,false)?;
    let registry = app.state::<ProviderRegistry>();
    let registered = match args.provider {
        Some(kind) => vec![(kind,registry.get(kind).await.ok_or_else(||ControlError::new("provider_not_configured","The selected provider is not configured in CodeMux"))?)],
        None => registry.all().await,
    };
    let mut providers = Vec::new();
    for (kind,implementation) in registered {
        let operations = implementation.capabilities();
        match capabilities(app,kind).await {
            Ok(capabilities) => {
                let allowed_modes: Vec<_> = capabilities.permission_modes.iter()
                    .filter(|mode|permission_ceiling(caller,kind,Some(&mode.value)).is_ok())
                    .map(|mode|mode.value.clone()).collect();
                providers.push(json!({"provider":kind,"configured":true,"capabilities":capabilities,"operations":operations,"allowed_permission_modes":allowed_modes,"discovery_error":null}));
            }
            Err(error) => providers.push(json!({"provider":kind,"configured":true,"capabilities":null,"operations":operations,"allowed_permission_modes":[],"discovery_error":error})),
        }
    }
    authorize(app,caller)?;
    Ok(json!({"workspace_id":args.workspace_id,"access":caller.access,"providers":providers}))
}

pub(crate) async fn thread_interrupt<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, args: Value) -> Result<Value, ControlError> {
    #[derive(Deserialize)] #[serde(deny_unknown_fields)]
    struct Args { workspace_id: String, thread_id: String, client_request_id: String, turn_id: Option<String> }
    let input: Args = parse(args.clone())?;
    identifier(&input.client_request_id,"client_request_id",128)?;
    if let Some(turn) = &input.turn_id { identifier(turn,"turn_id",256)?; }
    if let Some(receipt) = retry_receipt(app, caller, "thread_interrupt", &args)? { return Ok(receipt); }
    let target = Target { workspace_id:input.workspace_id,thread_id:input.thread_id };
    let (record, provider) = resolve_thread(app,&target,true)?;
    permission_ceiling(caller,provider,record.permission_mode.as_deref())?;
    let implementation = app.state::<ProviderRegistry>().get(provider).await
        .ok_or_else(||ControlError::new("provider_not_configured","The selected provider is not configured in CodeMux"))?;
    if !implementation.capabilities().supports_interrupt { return Err(ControlError::new("unsupported_operation","This provider does not support interruption")); }
    let admission = admit_mutation(app,caller,"thread_interrupt",&args,&target)?;
    let receipt = serde_json::to_value(&admission.receipt).map_err(|e|ControlError::new("serialization_failed",e.to_string()))?;
    if !admission.newly_admitted { return Ok(receipt); }
    let owned_app = app.clone();
    let owned_caller = caller.clone();
    spawn_operation(app,caller,admission.receipt,async move {
        authorize(&owned_app,&owned_caller)?;
        let (record,current_provider) = resolve_thread(&owned_app,&target,true)?;
        if provider != current_provider { return Err(ControlError::new("stale_thread_binding","The thread changed provider after admission")); }
        permission_ceiling(&owned_caller,provider,record.permission_mode.as_deref())?;
        let control = NativeControlGuard { caller:owned_caller.clone(),workspace_id:target.workspace_id.clone(),thread_id:target.thread_id.clone(),provider,admission:None };
        let reached = agent_chat::agent_chat_interrupt_turn_guarded(owned_app.clone(),provider,ThreadId(target.thread_id.clone()),input.turn_id.clone().map(crate::agent_provider::TurnId),Some(control)).await?;
        Ok(json!({"workspace_id":target.workspace_id,"thread_id":target.thread_id,"reached_provider":reached,"requested_turn_id":input.turn_id}))
    });
    Ok(receipt)
}

pub(crate) async fn thread_stop<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, args: Value) -> Result<Value, ControlError> {
    #[derive(Deserialize)] #[serde(deny_unknown_fields)]
    struct Args { workspace_id: String, thread_id: String, client_request_id: String }
    let input: Args = parse(args.clone())?;
    identifier(&input.client_request_id,"client_request_id",128)?;
    if let Some(receipt) = retry_receipt(app, caller, "thread_stop", &args)? { return Ok(receipt); }
    let target = Target { workspace_id:input.workspace_id,thread_id:input.thread_id };
    let (record, provider) = resolve_thread(app,&target,true)?;
    permission_ceiling(caller,provider,record.permission_mode.as_deref())?;
    let admission = admit_mutation(app,caller,"thread_stop",&args,&target)?;
    let receipt = serde_json::to_value(&admission.receipt).map_err(|e|ControlError::new("serialization_failed",e.to_string()))?;
    if !admission.newly_admitted { return Ok(receipt); }
    let owned_app = app.clone();
    let owned_caller = caller.clone();
    spawn_operation(app,caller,admission.receipt,async move {
        authorize(&owned_app,&owned_caller)?;
        let (record,current_provider) = resolve_thread(&owned_app,&target,true)?;
        if provider != current_provider { return Err(ControlError::new("stale_thread_binding","The thread changed provider after admission")); }
        permission_ceiling(&owned_caller,provider,record.permission_mode.as_deref())?;
        let control = NativeControlGuard { caller:owned_caller.clone(),workspace_id:target.workspace_id.clone(),thread_id:target.thread_id.clone(),provider,admission:None };
        agent_chat::agent_chat_stop_session_guarded(owned_app.clone(),provider,ThreadId(target.thread_id.clone()),Some(control)).await?;
        Ok(json!({"workspace_id":target.workspace_id,"thread_id":target.thread_id,"stopped":true}))
    });
    Ok(receipt)
}

pub(crate) async fn thread_wait<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, args: Value) -> Result<Value, ControlError> {
    #[derive(Deserialize)] #[serde(deny_unknown_fields)]
    struct Args { workspace_id: String, thread_id: String, turn_id: Option<String>, timeout_ms: Option<u64> }
    let args: Args = parse(args)?;
    let timeout = args.timeout_ms.unwrap_or(1000);
    if !(1..=20000).contains(&timeout) { return Err(ControlError::new("invalid_arguments","timeout_ms must be between 1 and 20000")); }
    if let Some(turn) = &args.turn_id { identifier(turn,"turn_id",256)?; }
    let target = json!({"workspace_id":args.workspace_id,"thread_id":args.thread_id});
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(timeout);
    loop {
        authorize(app,caller)?;
        let status = thread_status(app,target.clone()).await?;
        let selected = if let Some(turn) = &args.turn_id {
            app.state::<DatabaseStore>().control_turn_outcome(&args.thread_id,turn)?
        } else { None };
        let settled = match &args.turn_id {
            None => status["settled"] == true,
            Some(turn) => selected.is_some() && (
                status["settled"] == true
                || status["turn_id"].as_str().is_some_and(|current|current != turn)
            ),
        };
        if settled || tokio::time::Instant::now() >= deadline {
            let selected_turn = selected.map(|event|json!({"turn_id":event["turn_id"],"outcome":event["status"]["kind"]}));
            return Ok(json!({"timed_out":!settled,"settled":settled,"status":status,"selected_turn":selected_turn}));
        }
        tokio::time::sleep_until((tokio::time::Instant::now()+std::time::Duration::from_millis(100)).min(deadline)).await;
    }
}

pub(crate) async fn thread_respond<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, args: Value) -> Result<Value, ControlError> {
    #[derive(Deserialize)] #[serde(rename_all="snake_case")]
    enum Decision { AllowOnce, AllowAlways, Deny }
    #[derive(Deserialize)] #[serde(deny_unknown_fields)]
    struct Args { workspace_id: String, thread_id: String, client_request_id: String, request_id: String, decision: Decision }
    let input: Args = parse(args.clone())?;
    identifier(&input.client_request_id,"client_request_id",128)?;
    identifier(&input.request_id,"request_id",256)?;
    if let Some(receipt) = retry_receipt(app, caller, "thread_respond", &args)? { return Ok(receipt); }
    let target = Target { workspace_id:input.workspace_id,thread_id:input.thread_id };
    let (record,provider) = resolve_thread(app,&target,true)?;
    permission_ceiling(caller,provider,record.permission_mode.as_deref())?;
    if caller.access != ControlAccess::FullAccess && !matches!(input.decision,Decision::Deny) {
        return Err(ControlError::new("permission_ceiling","Supervised clients cannot approve worker requests"));
    }
    let decision = match input.decision {
        Decision::AllowOnce => crate::agent_provider::ApprovalDecision::Allow { updated_input:None,updated_permissions:None },
        Decision::AllowAlways => crate::agent_provider::ApprovalDecision::AllowForSession,
        Decision::Deny => crate::agent_provider::ApprovalDecision::Deny { message:"Denied by the outside assistant".into() },
    };
    let admission = admit_mutation(app,caller,"thread_respond",&args,&target)?;
    let receipt = serde_json::to_value(&admission.receipt).map_err(|e|ControlError::new("serialization_failed",e.to_string()))?;
    if !admission.newly_admitted { return Ok(receipt); }
    let owned_app = app.clone();
    let control = NativeControlGuard { caller:caller.clone(),workspace_id:target.workspace_id.clone(),thread_id:target.thread_id.clone(),provider,admission:None };
    spawn_operation(app,caller,admission.receipt,async move {
        let provider_request = owned_app.state::<NativeControlState>()
            .provider_request_for_handle(&target.thread_id, &input.request_id)
            .ok_or_else(|| ControlError::new("stale_provider_callback", "No current-process callback matches this outside handle"))?;
        agent_chat::agent_chat_respond_to_request_guarded(owned_app,provider,ThreadId(target.thread_id.clone()),
            crate::agent_provider::RequestId(provider_request),decision,Some(control)).await?;
        Ok(json!({"workspace_id":target.workspace_id,"thread_id":target.thread_id,"request_id":input.request_id,"delivered":true}))
    });
    Ok(receipt)
}
