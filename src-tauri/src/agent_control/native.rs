//! Authenticated in-process transport. Identity never comes from tool arguments.
use super::{
    permissions::Ceiling, service, CallerPrincipal, ControlAccess, ControlCaller, ControlError,
};
use crate::{
    agent_provider::ProviderKind,
    database::DatabaseStore,
    mcp::{
        registry::{HostCaller, HostTools},
        runtime::{prefix_tool_name, McpTool},
    },
    state::AppStateStore,
};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, Runtime};

pub struct OrchestrationHost<R: Runtime>(pub AppHandle<R>);
const TOOLS: &[&str] = &[
    "orchestrator_capabilities",
    "thread_launch",
    "create_threads",
    "thread_send",
    "thread_read",
    "thread_wait",
    "thread_list",
    "thread_search",
    "thread_interrupt",
    "operation_status",
];
pub(crate) fn tool_name(name: &str) -> Option<&str> {
    name.strip_prefix("mcp__codemux__").filter(|n| {
        TOOLS.contains(n)
            || matches!(
                *n,
                "delegate_task"
                    | "task_status"
                    | "task_cancel"
                    | "workspace_launch"
                    | "suggest_tasks"
                    | "delegation_targets"
                    | "delegate_remote_task"
            )
    })
}
impl<R: Runtime> HostTools for OrchestrationHost<R> {
    fn tools_for(&self, caller: &HostCaller) -> Vec<McpTool> {
        if !matches!(caller.provider, ProviderKind::Claude | ProviderKind::Codex)
            || !self
                .0
                .try_state::<DatabaseStore>()
                .is_some_and(|db| crate::commands::delegation::setting_on(&db))
        {
            return vec![];
        }
        let mut tools = super::catalog::native_tools();
        tools.retain(|tool| TOOLS.contains(&tool.name) && tool.name != "agent_capabilities");
        let mut capabilities = super::catalog::native_tools().remove(0);
        capabilities.name = "orchestrator_capabilities";
        capabilities.description = "Discover live configured provider instances, model IDs, model options and inherited permission ceilings. Values are opaque. Chat links use codemux-chat://v1/<thread_id>.";
        tools.push(capabilities);
        let mut result: Vec<_> = tools.into_iter().map(|mut tool| {
            if let Some(required) = tool.input_schema["required"].as_array_mut() { required.retain(|v| v != "workspace_id" && !(tool.name == "thread_launch" && (v == "permission_mode" || v == "provider" || v == "client_request_id"))); }
            if tool.name == "thread_launch" {
                tool.description = "If the user asks to delegate work or wants results back, use delegate_task instead. Start an independent chat only when the user requests a separate conversation. Without workspace_id the chat gets its own workspace, nested under yours in the same folder and titled with title; with workspace_id it opens as a tab in that workspace. Omitted or null permission_mode inherits the caller's effective ceiling; explicit modes cannot exceed it. Worktree strategies are unsupported. Returns an operation receipt; use operation_status.";
                tool.input_schema["properties"]["provider_instance_id"] = json!({"type":"string"});
                tool.input_schema["properties"]["worktree"] = json!({"description":"Unsupported in this version"});
            }
            if tool.name == "thread_read" {
                tool.input_schema["properties"]["text_offset"]=json!({"type":"integer","minimum":0});
                tool.input_schema["properties"]["max_chars"]=json!({"type":"integer","minimum":1,"maximum":16000});
            }
            if tool.name == "thread_send" {
                tool.description = "Send a message to another local chat as this authenticated agent. Auto starts an idle receiver, steers a ready busy receiver, or queues. Queue and steer may be selected explicitly. Use thread_interrupt for selected-run interruption. A queued receipt is not completion. Reuse client_request_id unchanged on retry.";
                tool.input_schema["properties"]["delivery"]["enum"] = json!(["auto","queue","steer"]);
            }
            let description = format!("{}\nUse codemux-chat://v1/<thread_id> for chat links. When asked to report to a coordinator, send the result with thread_send. Do not poll; after async delegation, end the turn and let the result wake you.", tool.description);
            McpTool { name: tool.name.into(), prefixed_name: prefix_tool_name("codemux", tool.name), description: Some(description), input_schema: tool.input_schema, server_id: "codemux".into() }
        }).collect();
        for (name, description, schema) in [
            ("create_threads", "If the user asks to delegate work or wants results back, use delegate_task instead. Start independent chats ONLY when the user requests separate conversations. Without workspace_id each chat gets its own workspace, nested under yours in the same folder; with workspace_id they open as tabs in that workspace. Results do not automatically return.", json!({"type":"object","properties":{"workspace_id":{"type":"string"},"client_request_id":{"type":"string"},"threads":{"type":"array","minItems":1,"maxItems":20,"items":{"type":"object"}}},"required":["threads"],"additionalProperties":false})),
            ("thread_search", "Search bounded safe-visible chat titles and prose. Defaults to the caller's project. Explicit workspace selects that project. Chat links use codemux-chat://v1/<thread_id>.", json!({"type":"object","properties":{"workspace_id":{"type":"string"},"query":{"type":"string","minLength":1,"maxLength":1000},"limit":{"type":"integer","minimum":1,"maximum":50}},"required":["query"],"additionalProperties":false})),
        ] { result.push(McpTool { name:name.into(),prefixed_name:prefix_tool_name("codemux",name),description:Some(description.into()),input_schema:schema,server_id:"codemux".into() }); }
        result
    }
    fn call(&self, _: &HostCaller, _: &str, _: &Value) -> Option<Value> {
        None
    }
}

pub(crate) async fn qualify<R: Runtime>(
    app: &AppHandle<R>,
    actor: &crate::delegation::app::NativeActor,
) -> Result<ControlCaller, ControlError> {
    qualify_for(app, actor, true).await
}

async fn qualify_for<R: Runtime>(app: &AppHandle<R>, actor: &crate::delegation::app::NativeActor, orchestration: bool) -> Result<ControlCaller, ControlError> {
    actor
        .permit
        .wait_ready()
        .await
        .map_err(|e| ControlError::new("stale_caller", e))?;
    let provider = app
        .state::<crate::commands::agent_chat::ProviderRegistry>()
        .get(actor.provider)
        .await
        .ok_or_else(|| ControlError::new("stale_caller", "Calling provider is unavailable"))?;
    if !provider
        .list_sessions()
        .await
        .map_err(|e| ControlError::from(e.to_string()))?
        .iter()
        .any(|s| s.thread_id.0 == actor.thread_id && s.session_id.0 == actor.session_id)
    {
        return Err(ControlError::new(
            "stale_caller",
            "Calling session was replaced",
        ));
    }
    let authority = actor
        .permit
        .identity()
        .map_err(|e| ControlError::new("stale_caller", e))?;
    let record = app
        .state::<DatabaseStore>()
        .get_agent_chat_session(&actor.thread_id)
        .ok_or_else(|| ControlError::new("stale_caller", "Calling chat no longer exists"))?;
    let native =
        Ceiling::for_mode(actor.provider, actor.permission_mode.as_deref()).ok_or_else(|| {
            ControlError::new(
                "unsupported_permission_ceiling",
                "Effective native mode is unknown",
            )
        })?;
    let canonical = Ceiling::for_mode(actor.provider, record.permission_mode.as_deref())
        .ok_or_else(|| {
            ControlError::new("unsupported_permission_ceiling", "Stored mode is unknown")
        })?;
    let caller = ControlCaller {
        admitted: None,
        identity: CallerPrincipal::Thread {
            thread_id: actor.thread_id.clone(),
            provider_instance_id: crate::commands::delegation::provider_id(actor.provider).into(),
            provider: actor.provider,
            session_id: actor.session_id.clone(),
            authority,
            ceiling: native.intersection(canonical),
        },
        created_by: crate::agent_provider::message::CreatedBy::Agent,
        principal: format!("thread:{}", actor.thread_id),
        access: ControlAccess::Supervised,
        grant_id: None,
    };
    service::authorize_native(app, &caller, orchestration)?;
    Ok(caller)
}

pub(crate) async fn call<R: Runtime>(
    app: AppHandle<R>,
    actor: crate::delegation::app::NativeActor,
    name: String,
    args: Value,
) -> Result<Value, String> {
    call_with_id(app, actor, name, args, None).await
}

pub(crate) async fn call_with_id<R: Runtime>(
    app: AppHandle<R>, actor: crate::delegation::app::NativeActor,
    name: String, mut args: Value, call_id: Option<String>,
) -> Result<Value, String> {
    if name == "suggest_tasks" {
        actor.permit.wait_ready().await?;
        actor.permit.identity()?.verify(&actor.thread_id)?;
        let record = app.state::<DatabaseStore>().get_agent_chat_session(&actor.thread_id).ok_or("stale_caller: calling chat missing")?;
        let caller = HostCaller {control:None, thread_id:actor.thread_id.clone(), provider:actor.provider, workspace_id:Some(record.workspace_id), permission_mode:actor.permission_mode.clone()};
        return Ok(crate::commands::follow_ups::call(&app, &caller, &args));
    }
    let orchestration = tool_name(&prefix_tool_name("codemux", &name)).is_some() && name != "workspace_launch";
    let caller = qualify_for(&app, &actor, orchestration).await.map_err(|e| e.to_string())?;
    if !orchestration && name != "workspace_launch" {
        if !crate::mcp::registry::native_relay_allowed(&name, caller.ceiling().is_some_and(|c| c.full_access())) {
            return Err("provider_control_denied: this self-tool requires full access".into());
        }
        return app.state::<crate::mcp::registry::McpRegistry>().dispatch_tool_call(&prefix_tool_name("codemux", &name), args, actor.workspace_id.as_deref()).await;
    }
    if matches!(name.as_str(), "delegation_targets" | "delegate_remote_task") {
        return crate::delegation::app::native_call(
            app,
            actor,
            if name == "delegate_remote_task" {
                "delegate_task".into()
            } else {
                name
            },
            args,
        )
        .await;
    }
    if matches!(name.as_str(), "task_status" | "task_cancel") {
        if args
            .as_object()
            .is_none_or(|o| o.keys().any(|k| k != "task_id" && k != "cursor"))
        {
            return Err("invalid_arguments: task tools accept task_id and cursor only".into());
        }
        if name == "task_cancel" {
            if let Some(task) = app
                .state::<DatabaseStore>()
                .delegation_task(args["task_id"].as_str().ok_or("task_id required")?)?
            {
                if task.parent_thread_id != actor.thread_id { return Err("Task is not owned by this parent".into()); }
                let provider: ProviderKind = serde_json::from_value(json!(task.provider))
                    .map_err(|_| "unsupported_provider")?;
                service::permission_ceiling(&caller, provider, task.permission_mode.as_deref())
                    .map_err(|e| e.to_string())?;
            }
        }
    }
    if matches!(
        name.as_str(),
        "delegate_task" | "task_status" | "task_cancel" | "suggest_tasks"
    ) {
        let host = crate::commands::delegation::DelegationHost::new(app.clone());
        let record = app
            .state::<DatabaseStore>()
            .get_agent_chat_session(&actor.thread_id)
            .ok_or("Calling chat missing")?;
        let host_caller = HostCaller {
            control: Some(caller),
            thread_id: actor.thread_id.clone(),
            provider: actor.provider,
            workspace_id: Some(record.workspace_id),
            permission_mode: actor.permission_mode.clone(),
        };
        if let Some(result) = host
            .call_async(
                &host_caller,
                &prefix_tool_name("codemux", &name),
                &args,
                call_id.as_deref(),
                actor.permit.identity()?.run_id.as_deref(),
            )
            .await
        {
            return Ok(result);
        }
        return crate::delegation::app::native_call(app, actor, name, args).await;
    }
    if name == "workspace_launch" {
        let record = app.state::<DatabaseStore>().get_agent_chat_session(&actor.thread_id).ok_or("Calling chat missing")?;
        let host_caller = HostCaller {control:Some(caller), thread_id:actor.thread_id, provider:actor.provider, workspace_id:Some(record.workspace_id), permission_mode:actor.permission_mode};
        return crate::commands::workspace_launch::WorkspaceLaunchHost::new(app).call(&host_caller, "mcp__codemux__workspace_launch", &args).ok_or("Workspace launch is unavailable".into());
    }
    if !TOOLS.contains(&name.as_str()) {
        return Err(
            "provider_control_denied: tool is unavailable to authenticated chat callers".into(),
        );
    }
    if !args.is_object() {
        return Err("invalid_arguments: pass an object".into());
    }
    let mut child_workspace = false;
    if matches!(
        name.as_str(),
        "thread_send" | "thread_read" | "thread_wait" | "thread_interrupt"
    ) {
        let id = args["thread_id"].as_str().ok_or("thread_id required")?;
        let target = app
            .state::<DatabaseStore>()
            .get_agent_chat_session(id)
            .ok_or("thread_not_found")?;
        if args.get("workspace_id").is_none_or(Value::is_null) {
            args["workspace_id"] = json!(target.workspace_id);
        }
    } else if name != "operation_status" && args.get("workspace_id").is_none_or(Value::is_null) {
        let record = app
            .state::<DatabaseStore>()
            .get_agent_chat_session(&actor.thread_id)
            .ok_or("Calling chat missing")?;
        // A launch without a workspace places each chat in its own child
        // workspace of the caller's; it never picks another existing one.
        child_workspace = matches!(name.as_str(), "thread_launch" | "create_threads");
        args["workspace_id"] = json!(record.workspace_id);
    }
    let result = match name.as_str() {
        "orchestrator_capabilities" => {
            let mut result = service::agent_capabilities(&app, &caller, args)
                .await
                .map_err(|e| e.to_string())?;
            if let Some(providers) = result["providers"].as_array_mut() {
                for provider in providers {
                    provider["provider_instance_id"] = provider["provider"].clone();
                    provider["models"] = provider["capabilities"]["models"].clone();
                    if matches!(provider["provider"].as_str(), Some("claude" | "codex")) {
                        if let Some(modes) = provider["allowed_permission_modes"].as_array_mut() {
                            modes.push(json!("plan"));
                        }

                        if let Some(modes) =
                            provider["capabilities"]["permission_modes"].as_array_mut()
                        {
                            modes.push(json!({"value":"plan","label":"Plan","description":"Read and plan without executing changes","is_default":false}));
                        }
                    }
                    provider["inherited_permission_mode"] =
                        serde_json::from_value::<ProviderKind>(provider["provider"].clone())
                            .ok()
                            .and_then(|kind| {
                                caller
                                    .ceiling()
                                    .and_then(|ceiling| ceiling.inherited(kind).ok())
                            })
                            .map_or(Value::Null, Value::String);
                    provider["native_orchestration"] = json!(matches!(
                        provider["provider"].as_str(),
                        Some("claude" | "codex")
                    ));
                }
            }
            result["limits"] =
                json!({"depth":4,"executing_per_root":3,"admissions_per_human_root":6});
            result
        }
        "thread_list" | "thread_search" => {
            project_inventory(&app, &name, args).map_err(|e| e.to_string())?
        }
        "create_threads" => {
            #[derive(serde::Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Batch {
                workspace_id: String,
                client_request_id: Option<String>,
                threads: Vec<Value>,
            }
            use sha2::{Digest, Sha256};
            // A retry without an explicit key must recover the same per-entry
            // receipts, including after a partially successful batch.
            let implicit_key = format!("batch:{:x}", Sha256::digest(serde_json::to_vec(&json!({"session":actor.session_id,"run":actor.permit.identity()?.run_id,"args":args,"child_workspace":child_workspace})).map_err(|e|e.to_string())?));
            let input: Batch = service::parse(args).map_err(|e| e.to_string())?;
            if input.threads.is_empty() || input.threads.len() > 20 {
                return Err("invalid_arguments: batch size must be 1–20".into());
            }
            let key = input
                .client_request_id
                .unwrap_or(implicit_key);
            service::identifier(&key, "client_request_id", 128).map_err(|e| e.to_string())?;
            // Keep the returned caller key intact while bounding every entry
            // key, including its suffix, in the launch journal's 128 bytes.
            let mut receipts = vec![];
            for (index, mut entry) in input.threads.into_iter().enumerate() {
                let result = async {
                    if !entry.is_object() { return Err("invalid_arguments: each thread must be an object".into()); }
                    entry["workspace_id"] = json!(input.workspace_id);
                    let entry_key = format!("{key}:{index}");
                    entry["client_request_id"] = json!(if entry_key.len() <= 128 {
                        entry_key
                    } else { format!("batch:{:x}:{index}", Sha256::digest(key.as_bytes())) });
                    prepare_launch(&caller, &actor, &mut entry)?;
                    service::launch_placed(&app, &caller, entry, child_workspace).await.map_err(|e| e.to_string())
                }.await;
                match result {
                    Ok(receipt) => receipts.push(receipt),
                    Err(error) => return Ok(json!({"threads":receipts,"client_request_id":key,"failed_index":index,"error":error})),
                }
            }
            json!({"threads":receipts,"client_request_id":key})
        }
        "thread_launch" => {
            prepare_launch(&caller, &actor, &mut args)?;
            service::launch_placed(&app, &caller, args, child_workspace)
                .await
                .map_err(|e| e.to_string())?
        }
        "thread_read" => read_bounded(&app, &caller, args).map_err(|e| e.to_string())?,
        "thread_send" if args["delivery"] == "interrupt" => {
            return Err(
                "invalid_arguments: use thread_interrupt for selected-run interruption".into(),
            )
        }
        _ => super::execute(&app, &caller, &name, args)
            .await
            .map_err(|e| e.to_string())?,
    };
    Ok(with_chat_links(result))
}
fn prepare_launch(
    caller: &ControlCaller,
    actor: &crate::delegation::app::NativeActor,
    args: &mut Value,
) -> Result<(), String> {
    if let Some(instance) = args
        .as_object_mut()
        .and_then(|o| o.remove("provider_instance_id"))
        .filter(|instance| !instance.is_null())
    {
        if args.get("provider").is_some_and(|p| !p.is_null() && p != &instance) {
            return Err("invalid_arguments: conflicting provider selection".into());
        }
        args["provider"] = instance;
    }
    args["select"] = json!(false);
    if args.get("provider").is_none_or(Value::is_null) {
        args["provider"] = json!(actor.provider);
    }
    let provider: ProviderKind =
        serde_json::from_value(args["provider"].clone()).map_err(|_| "unsupported_provider")?;
    if !matches!(provider, ProviderKind::Claude | ProviderKind::Codex) {
        return Err("unsupported_provider: native launch injection is currently available for Claude and Codex".into());
    }
    if args.get("permission_mode").is_none_or(Value::is_null) {
        args["permission_mode"] = json!(caller
            .ceiling()
            .ok_or("Missing caller ceiling")?
            .inherited(provider)
            .map_err(|e| e.to_string())?);
    }
    if args.get("client_request_id").is_none_or(Value::is_null) {
        args["client_request_id"] = json!(uuid::Uuid::new_v4().to_string());
    }
    Ok(())
}
fn project_inventory<R: Runtime>(
    app: &AppHandle<R>,
    tool: &str,
    args: Value,
) -> Result<Value, ControlError> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Args {
        workspace_id: String,
        cursor: Option<String>,
        limit: Option<u32>,
        query: Option<String>,
    }
    let input: Args = service::parse(args)?;
    let selected = service::workspace(app, &input.workspace_id, false)?;
    let key = project_key(&selected);
    let workspaces: Vec<_> = app
        .state::<AppStateStore>()
        .snapshot()
        .workspaces
        .into_iter()
        .filter(|w| project_key(w) == key)
        .collect();
    let db = app.state::<DatabaseStore>();
    let limit = input.limit.unwrap_or(20);
    if limit == 0 || limit > if tool == "thread_search" { 50 } else { 100 } {
        return Err(ControlError::new("invalid_arguments", "Invalid limit"));
    }
    if tool == "thread_search" {
        let query = input
            .query
            .filter(|q| !q.trim().is_empty() && q.len() <= 1000)
            .ok_or_else(|| {
                ControlError::new("invalid_arguments", "query required, max 1000 bytes")
            })?;
        let ids: Vec<_> = workspaces
            .iter()
            .map(|w| w.workspace_id.0.clone())
            .collect();
        return Ok(json!({"matches":db.search_agent_chat(&query,&ids,limit)?}));
    }
    let workspaces: Vec<_> = workspaces
        .iter()
        .map(|w| w.workspace_id.0.clone())
        .collect();
    let mut ids = db.control_project_thread_ids(&workspaces, input.cursor.as_deref(), limit + 1)?;
    let more = ids.len() > limit as usize;
    ids.truncate(limit as usize);
    let next = if more { ids.last().cloned() } else { None };
    let threads:Vec<_>=ids.iter().filter_map(|id|db.get_agent_chat_session(id)).map(|record|json!({"thread_id":record.thread_id,"workspace_id":record.workspace_id,"provider":record.provider,"title":record.title,"last_active_at":record.last_active_at,"permission_mode":record.permission_mode})).collect();
    Ok(json!({"threads":threads,"has_more":more,"next_cursor":next}))
}

fn project_key(workspace: &crate::state::WorkspaceSnapshot) -> std::path::PathBuf {
    let root = std::path::Path::new(workspace.project_root.as_deref().unwrap_or(&workspace.cwd));
    crate::git::git_canonical_root(root)
        .map(|info| info.common_dir)
        .unwrap_or_else(|| root.canonicalize().unwrap_or_else(|_| root.into()))
}
fn with_chat_links(mut value: Value) -> Value {
    match &mut value {
        Value::Object(object) => {
            if let Some(id) = object
                .get("thread_id")
                .and_then(Value::as_str)
                .map(str::to_string)
            {
                object.insert("chat_url".into(), json!(format!("codemux-chat://v1/{id}")));
            }
            for entry in object.values_mut() {
                *entry = with_chat_links(entry.take());
            }
        }
        Value::Array(entries) => {
            for entry in entries {
                *entry = with_chat_links(entry.take());
            }
        }
        _ => {}
    }
    value
}

fn read_bounded<R: Runtime>(
    app: &AppHandle<R>,
    caller: &ControlCaller,
    mut args: Value,
) -> Result<Value, ControlError> {
    let offset = args
        .as_object_mut()
        .and_then(|o| o.remove("text_offset"))
        .map(|v| {
            v.as_u64().ok_or_else(|| {
                ControlError::new("invalid_arguments", "text_offset must be nonnegative")
            })
        })
        .transpose()?
        .unwrap_or(0) as usize;
    let maximum = args
        .as_object_mut()
        .and_then(|o| o.remove("max_chars"))
        .map(|v| {
            v.as_u64()
                .ok_or_else(|| ControlError::new("invalid_arguments", "max_chars must be positive"))
        })
        .transpose()?
        .unwrap_or(16000) as usize;
    if maximum == 0 || maximum > 16000 {
        return Err(ControlError::new(
            "invalid_arguments",
            "max_chars must be 1–16000",
        ));
    }
    let cursor = args["cursor"].as_i64().unwrap_or(0);
    let mut result = service::thread_read(app, caller, args)?;
    let child = result["conversation_id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let db = app.state::<DatabaseStore>();
    let tasks = db.delegation_tasks(caller.thread_id())?;
    let original = result["messages"].as_array().cloned().unwrap_or_default();
    let mut messages = vec![];
    let mut remaining = maximum;
    let mut next_cursor = result["next_cursor"].clone();
    let mut next_offset = 0;
    for (index, mut message) in original.into_iter().enumerate() {
        let text = message["content"].as_str().unwrap_or_default().to_string();
        let length = text.chars().count();
        let start = if index == 0 { offset } else { 0 };
        if start > length {
            return Err(ControlError::new(
                "invalid_cursor",
                "text_offset is beyond the message",
            ));
        }
        let count = (length - start).min(remaining);
        let end = start + count;
        message["content"] = json!(text.chars().skip(start).take(count).collect::<String>());
        message["text_offset"] = json!(start);
        message["truncated"] = json!(end < length);
        if let Some(parent) = caller.thread_id() {
            for task in &tasks {
                if task.child_thread_id.as_deref() == Some(&child)
                    && task.result.as_ref().is_some_and(|published| {
                        let matching_run = published.run_id.as_ref().is_none_or(|run| {
                            db.get_agent_chat_message(message["message_id"].as_i64().unwrap_or(-1))
                                .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
                                .is_some_and(|raw| raw["turn_id"].as_str() == Some(run.as_str()))
                        });
                        matching_run && published.text == text && message["role"] == "assistant"
                    })
                {
                    if db.delegation_read_coverage(parent, &task.task_id, start, end, length)? {
                        caller.admit(|| db.acknowledge_delegation_result(parent, &task.task_id))?;
                    }
                }
            }
        }
        let id = message["message_id"].as_i64().unwrap_or(cursor);
        messages.push(message);
        remaining -= count;
        if end < length {
            next_cursor = json!(id - 1);
            next_offset = end;
            break;
        }
        if remaining == 0 {
            next_cursor = json!(id);
            break;
        }
    }
    result["messages"] = json!(messages);
    result["next_cursor"] = next_cursor;
    result["next_text_offset"] = json!(next_offset);
    Ok(result)
}
