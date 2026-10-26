//! Project-scoped plugin operations over the same durable domain services used by native control.
use super::{
    platform::{string_field, PlatformService},
    platform_authority::{self, PlatformCaller},
    platform_store::{digest, EffectReceipt, InstallationRecord},
};
use crate::{
    agent_control::{self, selection::ProviderSelection, service, NativeControlState},
    agent_provider::ProviderKind,
    database::{AgentChatSessionRecord, DatabaseStore},
    state::{AppStateStore, WorkspaceSnapshot},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};
use tauri::{AppHandle, Manager, Runtime};

pub fn project_id(workspace: &WorkspaceSnapshot) -> String {
    workspace.project_uid.clone().unwrap_or_else(|| {
        workspace
            .project_root
            .clone()
            .unwrap_or_else(|| workspace.cwd.clone())
    })
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Workspace {
    id: String,
    project_id: String,
    title: String,
    cwd: String,
    project_root: String,
    branch: Option<String>,
    worktree_path: Option<String>,
    archived_at: Option<u64>,
}
fn workspaces<R: Runtime>(app: &AppHandle<R>) -> Vec<Workspace> {
    let snapshot = app.state::<AppStateStore>().snapshot();
    let mut rows: Vec<_> = snapshot
        .workspaces
        .iter()
        .map(|w| Workspace {
            id: w.workspace_id.0.clone(),
            project_id: project_id(w),
            title: w.title.clone(),
            cwd: w.cwd.clone(),
            project_root: w.project_root.clone().unwrap_or_else(|| w.cwd.clone()),
            branch: w.git_branch.clone(),
            worktree_path: w.worktree_path.clone(),
            archived_at: None,
        })
        .collect();
    rows.extend(snapshot.archived_workspaces.into_iter().map(|w| {
        Workspace {
            id: w.workspace_id,
            project_id: w
                .project_uid
                .unwrap_or_else(|| w.project_root.clone().unwrap_or_else(|| w.cwd.clone())),
            title: w.title,
            cwd: w.cwd.clone(),
            project_root: w.project_root.unwrap_or(w.cwd),
            branch: w.git_branch,
            worktree_path: w.worktree_path,
            archived_at: Some(w.archived_at),
        }
    }));
    rows.sort_by(|a, b| a.id.cmp(&b.id));
    rows.dedup_by(|a, b| a.id == b.id);
    rows
}
pub(crate) fn project_options<R: Runtime>(app: &AppHandle<R>) -> Vec<Value> {
    let mut projects = BTreeMap::new();
    for workspace in workspaces(app) {
        projects.entry(workspace.project_id.clone()).or_insert_with(|| json!({
            "id":workspace.project_id,
            "title":Path::new(&workspace.project_root).file_name().map(|name|name.to_string_lossy().into_owned()).unwrap_or(workspace.title),
            "workspaceRoot":workspace.project_root,
        }));
    }
    projects.into_values().collect()
}
pub(crate) fn recovery_workspaces<R: Runtime>(app: &AppHandle<R>) -> Vec<(String, String)> {
    workspaces(app)
        .into_iter()
        .map(|workspace| (workspace.id, workspace.project_id))
        .collect()
}
fn granted(record: &InstallationRecord, caller: &PlatformCaller, project: &str) -> bool {
    record.grants.project_ids.iter().any(|p| p == project)
        && caller
            .project_ids
            .as_ref()
            .is_none_or(|ids| ids.iter().any(|p| p == project))
}
fn page_args(params: &Value) -> Result<(usize, usize), String> {
    let integer = |key: &str, default: u64| -> Result<u64, String> {
        match params.get(key) {
            None => Ok(default),
            Some(v) => v
                .as_u64()
                .ok_or_else(|| format!("invalid_input: {key} must be a nonnegative integer")),
        }
    };
    let cursor = integer("cursor", 0)?;
    let limit = integer("limit", 50)?;
    if cursor > 100_000 || !(1..=200).contains(&limit) {
        return Err("invalid_input: cursor must be <=100000 and limit 1–200".into());
    }
    Ok((cursor as usize, limit as usize))
}
fn page<T: Serialize>(
    items: Vec<T>,
    cursor: usize,
    limit: usize,
    key: &str,
) -> Result<Value, String> {
    let next = (cursor.saturating_add(limit) < items.len()).then_some(cursor + limit);
    let selected: Vec<_> = items.into_iter().skip(cursor).take(limit).collect();
    Ok(json!({key:selected,"nextCursor":next}))
}
fn target<R: Runtime>(
    app: &AppHandle<R>,
    thread: &str,
) -> Result<(AgentChatSessionRecord, Workspace), String> {
    let record = app
        .state::<DatabaseStore>()
        .get_agent_chat_session(thread)
        .ok_or("not_found: Thread not found")?;
    let workspace = workspaces(app)
        .into_iter()
        .find(|w| w.id == record.workspace_id)
        .ok_or("not_found: Thread workspace is unavailable")?;
    Ok((record, workspace))
}
pub(crate) fn thread_project<R: Runtime>(
    app: &AppHandle<R>,
    thread: &str,
) -> Result<String, String> {
    Ok(target(app, thread)?.1.project_id)
}
fn lineage<R: Runtime>(
    platform: &PlatformService,
    app: &AppHandle<R>,
    caller: &PlatformCaller,
    db: &DatabaseStore,
    thread: &str,
) -> Result<Value, String> {
    let mut cursor = thread.to_string();
    let mut seen = BTreeSet::new();
    let mut parent = None;
    let mut relation = None;
    for _ in 0..64 {
        if !seen.insert(cursor.clone())
            || thread_project(app, &cursor)
                .and_then(|project| {
                    platform
                        .authorize(app, caller, "threads.read", Some(&project))
                        .map(|_| ())
                })
                .is_err()
        {
            return Ok(json!({"unresolved":true}));
        }
        let next = if let Some(task) = db.delegation_task_for_child(&cursor)? {
            Some((task.parent_thread_id, "subagent"))
        } else {
            db.agent_chat_fork_origin(&cursor)
                .map(|origin| (origin.source_thread_id, "fork"))
        };
        let Some((next, kind)) = next else {
            return Ok(
                json!({"parentThreadId":parent,"relationshipToParent":relation,"rootThreadId":cursor}),
            );
        };
        if parent.is_none() {
            parent = Some(next.clone());
            relation = Some(kind);
        }
        cursor = next;
    }
    Ok(json!({"unresolved":true}))
}
fn inherited_options(record: &AgentChatSessionRecord) -> BTreeMap<String, Value> {
    let mut options = BTreeMap::new();
    if let Some(effort) = &record.effort {
        options.insert("effort".into(), json!(effort));
    }
    if let Some(window) = &record.context_window {
        options.insert("context_window".into(), json!(window));
    }
    if record.service_tier != "default" {
        options.insert("service_tier".into(), json!(record.service_tier));
    }
    if record.fast_mode {
        options.insert("fast_mode".into(), json!(true));
    }
    options
}
fn metadata<R: Runtime>(
    platform: &PlatformService,
    caller: &PlatformCaller,
    app: &AppHandle<R>,
    record: &AgentChatSessionRecord,
    workspace: &Workspace,
) -> Result<Value, String> {
    let db = app.state::<DatabaseStore>();
    let provider: ProviderKind = serde_json::from_value(json!(record.provider))
        .map_err(|_| "incompatible: Thread provider is unavailable")?;
    let instance =
        agent_control::selection::current_instance(&db, &record.thread_id, provider).ok();
    let runtime = app
        .try_state::<NativeControlState>()
        .and_then(|state| state.thread_runtime(&record.thread_id));
    let options = inherited_options(record);
    let mode = record.permission_mode.as_deref();
    let runtime_mode = match (provider, mode) {
        (ProviderKind::Claude, Some("default")) => Some("approval-required"),
        (ProviderKind::Claude, Some("acceptEdits"))
        | (ProviderKind::Codex, Some("workspace-write")) => Some("auto-accept-edits"),
        (_, Some("plan" | "read-only" | "ask")) => Some("approval-required"),
        _ => agent_control::permissions::Ceiling::for_mode(provider, mode)
            .filter(|c| c.full_access())
            .map(|_| "full-access"),
    };
    let mut result = json!({"id":record.thread_id,"projectId":workspace.project_id,"workspaceId":workspace.id,"title":record.title,
        "modelSelection":{"instanceId":instance,"model":record.model,"options":options.into_iter().map(|(id,value)|json!({"id":id,"value":value})).collect::<Vec<_>>()},"nativePermissionMode":record.permission_mode,
        "runtimeMode":runtime_mode,"interactionMode":if matches!(mode,Some("plan"|"read-only")){"plan"}else{"default"},
        "createdAt":record.created_at,"updatedAt":record.last_active_at,"archivedAt":workspace.archived_at,
        "branch":workspace.branch,"worktreePath":workspace.worktree_path,"importedSnapshot":record.imported_from.is_some(),
        "lineage":lineage(platform,app,caller,&db,&record.thread_id)?,"isRunning":runtime.as_ref().is_some_and(|s|matches!(s.phase.as_str(),"starting"|"running"|"waiting_approval")),
        "needsAttention":runtime.as_ref().is_some_and(|s|!s.pending_approvals.is_empty()),
        "settledAt":null,"settledOverride":null,"snoozedUntil":null,"snoozedAt":null,"pinnedAt":null});
    let installation = platform.store.read(&caller.installation_id)?;
    if let Some(source) = installation
        .receipts
        .values()
        .find(|receipt| {
            receipt.method == "threads.create"
                && receipt
                    .result
                    .as_ref()
                    .is_some_and(|result| result["thread_id"].as_str() == Some(&record.thread_id))
        })
        .and_then(|receipt| receipt.source_thread_id.as_deref())
    {
        result["launchedFrom"] = json!({"threadId":source});
    }
    Ok(result)
}

pub async fn dispatch<R: Runtime>(
    platform: &PlatformService,
    app: &AppHandle<R>,
    caller: &PlatformCaller,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    match method {
        "projects.list" | "workspaces.list" => {
            let record = platform.authorize(app, caller, "workflow.query", None)?;
            platform.authorize(app, caller, "threads.read", None)?;
            let (cursor, limit) = page_args(&params)?;
            let rows: Vec<_> = workspaces(app)
                .into_iter()
                .filter(|w| granted(&record, caller, &w.project_id))
                .collect();
            if method == "workspaces.list" {
                return page(rows, cursor, limit, "workspaces");
            }
            let mut projects = BTreeMap::new();
            for w in rows {
                projects.entry(w.project_id.clone()).or_insert(json!({"id":w.project_id,"title":Path::new(&w.project_root).file_name().map(|n|n.to_string_lossy().into_owned()).unwrap_or(w.title),"workspaceRoot":w.project_root}));
            }
            page(projects.into_values().collect(), cursor, limit, "projects")
        }
        "threads.list" => {
            let project = string_field(&params, "projectId", 4096)?;
            platform.authorize(app, caller, "workflow.query", Some(project))?;
            platform.authorize(app, caller, "threads.read", Some(project))?;
            let (cursor, limit) = page_args(&params)?;
            let workspaces: Vec<_> = workspaces(app)
                .into_iter()
                .filter(|w| w.project_id == project)
                .collect();
            let workspace_ids: Vec<_> = workspaces.iter().map(|w| w.id.clone()).collect();
            let db = app.state::<DatabaseStore>();
            let mut rows = Vec::new();
            let mut cut = None;
            // The DB owns pagination over persisted rows, including closed/cursorless chats.
            while rows.len() <= cursor + limit {
                let ids = db.control_project_thread_ids(&workspace_ids, cut.as_deref(), 200)?;
                for id in &ids {
                    if let Some(record) = db.get_agent_chat_session(id) {
                        if let Some(workspace) =
                            workspaces.iter().find(|w| w.id == record.workspace_id)
                        {
                            rows.push(metadata(platform, caller, app, &record, workspace)?);
                        }
                    }
                }
                if ids.len() < 200 {
                    break;
                }
                cut = ids.last().cloned();
            }
            page(rows, cursor, limit, "threads")
        }
        "threads.metadata" | "threads.read" => {
            let thread = string_field(&params, "threadId", 256)?;
            let (record, workspace) = target(app, thread)?;
            platform.authorize(app, caller, "threads.read", Some(&workspace.project_id))?;
            if method == "threads.metadata" {
                platform.authorize(app, caller, "workflow.query", Some(&workspace.project_id))?;
                return metadata(platform, caller, app, &record, &workspace);
            }
            let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(30);
            if !(1..=100).contains(&limit) {
                return Err("invalid_input: limit must be 1–100".into());
            }
            let cursor = match params.get("cursor") {
                None => None,
                Some(v) => Some(
                    v.as_i64()
                        .filter(|v| *v >= 0)
                        .ok_or("invalid_input: Invalid history cursor")?,
                ),
            };
            let page = app.state::<DatabaseStore>().read_agent_chat_history_page(
                &record.workspace_id,
                thread,
                cursor,
                limit as u32,
            )?;
            Ok(
                json!({"threadId":thread,"projectId":workspace.project_id,"title":record.title,"messages":page.messages.into_iter().map(|message|json!({"id":message.message_id,"role":message.role,"text":message.content.chars().take(4096).collect::<String>(),"createdAt":message.created_at})).collect::<Vec<_>>(),"nextCursor":page.next_cursor,"totalVisibleMessages":page.total_visible_messages}),
            )
        }
        "models.list" => models(platform, app, caller, params).await,
        "threads.status" => {
            let thread = string_field(&params, "threadId", 256)?;
            let (record, workspace) = target(app, thread)?;
            platform.authorize(app, caller, "threads.read", Some(&workspace.project_id))?;
            platform.authorize(app, caller, "workflow.query", Some(&workspace.project_id))?;
            let result = agent_control::execute(
                app,
                &caller.control,
                "thread_status",
                json!({"thread_id":thread,"workspace_id":record.workspace_id}),
            )
            .await
            .map_err(|e| e.to_string())?;
            platform.authorize(app, caller, "threads.read", Some(&workspace.project_id))?;
            Ok(json!({"threadId":thread,"projectId":workspace.project_id,"status":result}))
        }
        "operations.read" => {
            let operation = string_field(&params, "operationId", 128)?;
            let record = platform.store.read(&caller.installation_id)?;
            platform_authority::assert_active(&record, caller)?;
            let receipt = record
                .receipts
                .values()
                .find(|receipt| {
                    receipt.principal == caller.control.principal
                        && receipt.result.as_ref().is_some_and(|result| {
                            result["operation_id"].as_str() == Some(operation)
                        })
                })
                .or_else(|| {
                    // A crash can occur after core admission and before the plugin result
                    // write. Membership still derives from its durable command binding.
                    record
                        .receipts
                        .values()
                        .filter(|receipt| {
                            receipt.principal == caller.control.principal
                                && receipt.result.is_none()
                        })
                        .find(|receipt| {
                            recover_receipt(app, receipt).is_ok_and(|result| {
                                result["operation_id"].as_str() == Some(operation)
                            })
                        })
                })
                .ok_or(
                    "not_found: Operation is not owned by this plugin and authenticated caller",
                )?;
            authorize_receipt(platform, app, caller, receipt)?;
            recover_receipt(app, receipt)
        }
        "workflow.query" => {
            let query = string_field(&params, "query", 64)?;
            let mapped = match query {
                "projects" => "projects.list",
                "workspaces" => "workspaces.list",
                "threads" => "threads.list",
                "metadata" => "threads.metadata",
                "models" => "models.list",
                "thread-status" => "threads.status",
                "operation" => "operations.read",
                "effect" => "effects.read",
                _ => return Err("invalid_input: Unknown workflow query".into()),
            };
            let mut input = params;
            input
                .as_object_mut()
                .ok_or("invalid_input: Query must be an object")?
                .remove("query");
            Box::pin(dispatch(platform, app, caller, mapped, input)).await
        }
        "threads.send" | "threads.create" => mutate(platform, app, caller, method, params).await,
        "effects.read" => {
            let record = platform.store.read(&caller.installation_id)?;
            platform_authority::assert_active(&record, caller)?;
            let key = string_field(&params, "idempotencyKey", 128)?;
            let command_key = scoped_command_key(caller, key)?;
            let receipt = record
                .receipts
                .get(&command_key)
                .ok_or("not_found: Effect receipt not found")?;
            if receipt.principal != caller.control.principal {
                return Err("permission_denied: Effect belongs to another caller".into());
            }
            authorize_receipt(platform, app, caller, receipt)?;
            recover_receipt(app, receipt)
        }
        _ => Err(format!(
            "unsupported_operation: Unknown plugin host method {method}"
        )),
    }
}
async fn models<R: Runtime>(
    platform: &PlatformService,
    app: &AppHandle<R>,
    caller: &PlatformCaller,
    params: Value,
) -> Result<Value, String> {
    let project = string_field(&params, "projectId", 4096)?;
    platform.authorize(app, caller, "workflow.query", Some(project))?;
    platform.authorize(app, caller, "threads.read", Some(project))?;
    let (cursor, limit) = page_args(&params)?;
    let workspace = workspaces(app)
        .into_iter()
        .find(|w| w.project_id == project && w.archived_at.is_none())
        .ok_or("not_found: Open project workspace required for model discovery")?;
    let catalog = agent_control::execute(
        app,
        &caller.control,
        "agent_capabilities",
        json!({"workspace_id":workspace.id}),
    )
    .await
    .map_err(|e| e.to_string())?;
    let mut models = Vec::new();
    for provider in catalog["providers"].as_array().into_iter().flatten() {
        let Some(instance) = provider["provider_instance_id"].as_str() else {
            continue;
        };
        for model in provider["capabilities"]["models"]
            .as_array()
            .into_iter()
            .flatten()
        {
            let Some(id) = model["id"].as_str() else {
                continue;
            };
            models.push(json!({"modelSelection":{"instanceId":instance,"model":id},"title":model["label"],"providerTitle":provider["provider"],"options":model_descriptors(provider,model),"nativeCapabilities":model,"permissionModes":provider["allowed_permission_modes"]}));
        }
    }
    platform.authorize(app, caller, "workflow.query", Some(project))?;
    platform.authorize(app, caller, "threads.read", Some(project))?;
    page(models, cursor, limit, "models")
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ModelSelection {
    #[serde(alias = "provider")]
    instance_id: String,
    model: String,
    #[serde(default, deserialize_with = "model_options")]
    options: BTreeMap<String, Value>,
}
fn model_options<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, Value>, D::Error> {
    let value = Value::deserialize(deserializer)?;
    let pairs = match value {
        Value::Object(values) => values.into_iter().collect::<Vec<_>>(),
        Value::Array(values) => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Selection {
                id: String,
                value: Value,
            }
            values
                .into_iter()
                .map(|value| serde_json::from_value::<Selection>(value).map(|v| (v.id, v.value)))
                .collect::<Result<Vec<_>, _>>()
                .map_err(serde::de::Error::custom)?
        }
        _ => {
            return Err(serde::de::Error::custom(
                "Model options must be an array of {id,value} selections or a legacy object",
            ))
        }
    };
    if pairs.len() > 32 {
        return Err(serde::de::Error::custom("Model option limit exceeded"));
    }
    let mut options = BTreeMap::new();
    for (id, value) in pairs {
        if id.is_empty()
            || id.len() > 256
            || id.chars().any(char::is_control)
            || !(value.is_boolean()
                || value
                    .as_str()
                    .is_some_and(|v| !v.is_empty() && v.len() <= 4096))
            || options.insert(id, value).is_some()
        {
            return Err(serde::de::Error::custom(
                "Invalid or duplicate model option selection",
            ));
        }
    }
    Ok(options)
}
fn model_descriptors(provider: &Value, model: &Value) -> Vec<Value> {
    let mut descriptors = Vec::new();
    for option in provider["options"].as_array().into_iter().flatten() {
        if option["category"] == "mode"
            || matches!(option["id"].as_str(), Some("mode" | "permission_mode"))
        {
            continue;
        }
        let Some(id) = option["id"].as_str() else {
            continue;
        };
        let mut mapped = json!({"id":id,"label":option["name"],"type":option["type"]});
        if option["type"] == "select" {
            mapped["options"] = json!(option["options"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|choice| json!({"id":choice["value"],"label":choice["name"]}))
                .collect::<Vec<_>>());
        }
        if let Some(current) = option.get("current_value") {
            mapped["currentValue"] = current.clone();
        }
        descriptors.push(mapped);
    }
    for (id, label, field, value_key, label_key) in [
        ("effort", "Reasoning effort", "effort_levels", "", ""),
        (
            "context_window",
            "Context window",
            "context_window_options",
            "value",
            "label",
        ),
        (
            "service_tier",
            "Service tier",
            "service_tiers",
            "value",
            "label",
        ),
    ] {
        let choices: Vec<_> = model[field]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|choice| {
                let value = if value_key.is_empty() {
                    choice.as_str()?
                } else {
                    choice[value_key].as_str()?
                };
                let title = if label_key.is_empty() {
                    value
                } else {
                    choice[label_key].as_str().unwrap_or(value)
                };
                Some(json!({"id":value,"label":title}))
            })
            .collect();
        if !choices.is_empty() && !descriptors.iter().any(|descriptor| descriptor["id"] == id) {
            let mut descriptor = json!({"id":id,"label":label,"type":"select","options":choices});
            if id == "effort" {
                descriptor["promptInjectedValues"] = model
                    .get("prompt_injected_effort_levels")
                    .cloned()
                    .unwrap_or_else(|| json!([]));
            }
            descriptors.push(descriptor);
        }
    }
    if model["supports_fast_mode"] == true
        && !descriptors
            .iter()
            .any(|descriptor| descriptor["id"] == "fast_mode")
    {
        descriptors.push(json!({"id":"fast_mode","label":"Fast mode","type":"boolean"}));
    }
    descriptors
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateInput {
    project_id: String,
    idempotency_key: String,
    text: String,
    title: Option<String>,
    model_selection: Option<ModelSelection>,
    runtime_mode: Option<String>,
    interaction_mode: Option<String>,
    native_permission_mode: Option<String>,
    workspace_id: Option<String>,
    workspace_strategy: Option<WorkspaceStrategy>,
}
#[derive(Clone, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
enum WorkspaceStrategy {
    Root,
    Worktree {
        branch: Option<String>,
        #[serde(rename = "baseRef")]
        base_ref: String,
        #[serde(rename = "startFromOrigin", default)]
        start_from_origin: bool,
    },
}

fn native_mode(
    provider: ProviderKind,
    runtime: Option<&str>,
    interaction: Option<&str>,
    explicit: Option<&str>,
    source: Option<&AgentChatSessionRecord>,
) -> Result<Option<String>, String> {
    if interaction.is_some_and(|v| !matches!(v, "plan" | "default")) {
        return Err("invalid_input: Unknown interaction mode".into());
    }
    if let Some(explicit) = explicit {
        if runtime.is_some() {
            return Err(
                "invalid_input: Select runtimeMode or nativePermissionMode, not both".into(),
            );
        }
        if interaction == Some("plan") && !matches!(explicit, "plan" | "read-only") {
            return Err("permission_denied: The native mode does not preserve Plan intent".into());
        }
        return Ok(Some(explicit.into()));
    }
    if interaction == Some("plan") {
        return match provider {
            ProviderKind::Claude
            | ProviderKind::Codex
            | ProviderKind::Cursor
            | ProviderKind::OpenCode => Ok(Some("plan".into())),
            _ => Err("unsupported_operation: Provider has no reviewed Plan mode".into()),
        };
    }
    let Some(runtime) = runtime else {
        if let Some(source) =
            source.filter(|s| s.provider == crate::commands::delegation::provider_id(provider))
        {
            return Ok(source.permission_mode.clone());
        }
        return native_mode(provider, Some("approval-required"), interaction, None, None);
    };
    use ProviderKind::*;
    let mode=match(provider,runtime){(Claude,"approval-required")=>"default",(Claude,"auto-accept-edits")=>"acceptEdits",(Claude,"full-access")=>"bypassPermissions",(Codex,"approval-required")=>"read-only",(Codex,"auto-accept-edits")=>"workspace-write",(Codex,"full-access")=>"danger-full-access",(Cursor|Grok,"approval-required")=>"ask",(Cursor|Grok,"full-access")=>"agent",(OpenCode|Hermes,"full-access")=>"default",_=>return Err("unsupported_operation: Runtime mode has no reviewed native policy mapping; select an advertised nativePermissionMode".into())};
    Ok(Some(mode.into()))
}
pub(crate) fn effect_identity(
    installation: &str,
    operation: &str,
    principal: &str,
    key: &str,
    method: &str,
    request: &Value,
) -> (String, String) {
    (
        format!(
            "plugin-{}",
            digest(&json!([installation, operation, principal, key]))
        ),
        digest(&json!([method, request])),
    )
}
fn operation_id(caller: &PlatformCaller) -> Result<&str, String> {
    caller
        .operation_id
        .as_deref()
        .filter(|id| super::platform_manifest::valid_identifier(id))
        .ok_or_else(|| "invalid_input: A host-verified operationId is required for effects".into())
}
fn scoped_command_key(caller: &PlatformCaller, key: &str) -> Result<String, String> {
    Ok(effect_identity(
        &caller.installation_id,
        operation_id(caller)?,
        &caller.control.principal,
        key,
        "",
        &Value::Null,
    )
    .0)
}
fn authorize_receipt<R: Runtime>(
    platform: &PlatformService,
    app: &AppHandle<R>,
    caller: &PlatformCaller,
    receipt: &EffectReceipt,
) -> Result<(), String> {
    if receipt.principal != caller.control.principal {
        return Err("permission_denied: Effect belongs to another caller".into());
    }
    platform.authorize(app, caller, &receipt.method, Some(&receipt.project_id))?;
    for capability in &receipt.required_capabilities {
        platform.authorize(app, caller, capability, Some(&receipt.project_id))?;
    }
    if let Some(args) = &receipt.core_args {
        if receipt.method == "threads.send" {
            let thread = string_field(args, "thread_id", 256)?;
            let (target, workspace) = target(app, thread)?;
            platform.authorize(app, caller, &receipt.method, Some(&workspace.project_id))?;
            let provider: ProviderKind = serde_json::from_value(json!(target.provider))
                .map_err(|_| "incompatible: Effect provider is unavailable")?;
            service::permission_ceiling(
                &caller.control,
                provider,
                target.permission_mode.as_deref(),
            )
            .map_err(|e| e.to_string())?;
        } else if receipt.method == "threads.create" {
            let provider: ProviderKind = serde_json::from_value(args["provider"].clone())
                .map_err(|_| "incompatible: Effect provider is unavailable")?;
            service::permission_ceiling(
                &caller.control,
                provider,
                args["permission_mode"].as_str(),
            )
            .map_err(|e| e.to_string())?;
        }
    }
    if receipt.method == "threads.create" {
        if let Some(source) = &receipt.source_thread_id {
            let (_, workspace) = target(app, source)?;
            platform.authorize(app, caller, "threads.read", Some(&workspace.project_id))?;
        }
    }
    Ok(())
}
fn recover_receipt<R: Runtime>(
    app: &AppHandle<R>,
    receipt: &EffectReceipt,
) -> Result<Value, String> {
    if let (Some(tool), Some(args)) = (&receipt.core_tool, &receipt.core_args) {
        let epoch = app.state::<NativeControlState>().epoch.clone();
        if let Some(core) = app.state::<DatabaseStore>().control_operation_by_key(
            &receipt.principal,
            &receipt.command_key,
            tool,
            &digest(args),
            &epoch,
        )? {
            return serde_json::to_value(core)
                .map(present_receipt)
                .map_err(|e| e.to_string());
        }
    }
    if let Some(result) = &receipt.result {
        return Ok(present_receipt(result.clone()));
    }
    Err(format!(
        "outcome_unknown: Effect is reserved and will not be redispatched; {}",
        receipt
            .error
            .as_deref()
            .unwrap_or("inspect core history before recovery")
    ))
}
fn present_receipt(mut receipt: Value) -> Value {
    for (native, wire) in [
        ("thread_id", "threadId"),
        ("workspace_id", "workspaceId"),
        ("operation_id", "operationId"),
        ("delivery_id", "deliveryId"),
    ] {
        if let Some(value) = receipt.get(native).filter(|v| !v.is_null()).cloned() {
            receipt[wire] = value;
        }
    }
    receipt
}
async fn dispatch_core_effect<R: Runtime>(
    platform: &PlatformService,
    app: &AppHandle<R>,
    caller: &PlatformCaller,
    key: &str,
    tool: &str,
    args: Value,
) -> Result<Value, String> {
    let receipt = platform
        .store
        .read(&caller.installation_id)?
        .receipts
        .get(key)
        .cloned()
        .ok_or("storage_failed: Effect reservation disappeared")?;
    authorize_receipt(platform, app, caller, &receipt)?;
    let mut control = caller.control.clone();
    control.created_by = crate::agent_provider::message::CreatedBy::System;
    let result = agent_control::execute(app, &control, tool, args)
        .await
        .map(present_receipt)
        .map_err(|e| e.to_string());
    platform.store.update(&caller.installation_id, |record| {
        let receipt = record
            .receipts
            .get_mut(key)
            .ok_or("storage_failed: Effect reservation disappeared")?;
        match &result {
            Ok(result) => {
                receipt.state = "admitted".into();
                receipt.result = Some(result.clone());
            }
            Err(error) => {
                receipt.state = "outcome_unknown".into();
                receipt.error = Some(error.clone());
            }
        }
        Ok(())
    })?;
    result
}
async fn mutate<R: Runtime>(
    platform: &PlatformService,
    app: &AppHandle<R>,
    caller: &PlatformCaller,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let key = string_field(&params, "idempotencyKey", 128)?.to_string();
    if !super::platform_manifest::valid_identifier(&key) {
        return Err("invalid_input: Invalid idempotencyKey".into());
    }
    let (command_key, fingerprint) = effect_identity(
        &caller.installation_id,
        operation_id(caller)?,
        &caller.control.principal,
        &key,
        method,
        &params,
    );
    let project = if method == "threads.send" {
        let thread = string_field(&params, "threadId", 256)?;
        let (_, workspace) = target(app, thread)?;
        workspace.project_id
    } else {
        string_field(&params, "projectId", 4096)?.into()
    };
    platform.authorize(app, caller, method, Some(&project))?;
    let _effect = platform.begin_effect(&command_key)?;
    let record = platform.authorize(app, caller, method, Some(&project))?;
    if let Some(receipt) = record.receipts.get(&command_key) {
        authorize_receipt(platform, app, caller, receipt)?;
        if receipt.fingerprint != fingerprint {
            return Err(
                "idempotency_conflict: This key already identifies different inputs".into(),
            );
        }
        if receipt.principal != caller.control.principal {
            return Err("permission_denied: This effect belongs to another caller".into());
        }
        match recover_receipt(app, receipt) {
            Ok(result) => return Ok(result),
            Err(error) if error.starts_with("outcome_unknown:") => {
                if let (Some(tool), Some(args)) = (&receipt.core_tool, &receipt.core_args) {
                    // Core admission is atomic and precedes every provider effect. The
                    // in-process lease excludes a concurrent still-uncommitted admission.
                    // Therefore absence of its durable receipt permits this same command.
                    platform.authorize(app, caller, method, Some(&project))?;
                    return dispatch_core_effect(
                        platform,
                        app,
                        caller,
                        &command_key,
                        tool,
                        args.clone(),
                    )
                    .await;
                }
                return Err(error);
            }
            Err(error) => return Err(error),
        }
    }
    let mut control = caller.control.clone();
    control.created_by = crate::agent_provider::message::CreatedBy::System;
    let mut verified_source = None;
    let mut root_preparation = None;
    let mut _root_lease = None;
    let (core_tool, mut core_args, strategy) = if method == "threads.send" {
        if params.as_object().is_some_and(|map| {
            map.keys()
                .any(|k| !["threadId", "projectId", "text", "idempotencyKey"].contains(&k.as_str()))
        }) {
            return Err("invalid_input: Unknown send fields".into());
        }
        let thread = string_field(&params, "threadId", 256)?;
        let (record, _workspace) = target(app, thread)?;
        if params
            .get("projectId")
            .is_some_and(|v| v.as_str() != Some(&project))
        {
            return Err("permission_denied: Target project mismatch".into());
        }
        let text = params
            .get("text")
            .and_then(Value::as_str)
            .filter(|v| !v.trim().is_empty() && v.len() <= 64000)
            .ok_or("invalid_input: text must contain 1–64000 bytes")?;
        (
            "thread_send",
            json!({"workspace_id":record.workspace_id,"thread_id":thread,"client_request_id":command_key,"message":format!("[Plugin: {}]\n{text}",platform.store.read(&caller.installation_id)?.manifest.name),"delivery":"queue"}),
            None::<(CreateInput, WorkspaceSnapshot)>,
        )
    } else {
        let input: CreateInput =
            serde_json::from_value(params.clone()).map_err(|e| format!("invalid_input: {e}"))?;
        if input.project_id != project
            || input.idempotency_key != key
            || input.text.trim().is_empty()
            || input.text.len() > 64000
            || input
                .title
                .as_ref()
                .is_some_and(|t| t.trim().is_empty() || t.len() > 100)
        {
            return Err("invalid_input: Invalid creation text/title".into());
        }
        let candidates: Vec<_> = app
            .state::<AppStateStore>()
            .snapshot()
            .workspaces
            .into_iter()
            .filter(|w| project_id(w) == project)
            .collect();
        let root_requested = matches!(input.workspace_strategy, Some(WorkspaceStrategy::Root))
            || (input.workspace_strategy.is_none() && input.workspace_id.is_none());
        let root_workspace = |workspace: &&WorkspaceSnapshot| {
            if workspace.worktree_path.is_some() {
                return false;
            }
            let root = workspace.project_root.as_deref().unwrap_or(&workspace.cwd);
            match (
                Path::new(root).canonicalize(),
                Path::new(&workspace.cwd).canonicalize(),
            ) {
                (Ok(root), Ok(cwd)) => root == cwd,
                _ => root == workspace.cwd,
            }
        };
        let mut workspace = if let Some(id) = &input.workspace_id {
            candidates
                .iter()
                .filter(|workspace| !root_requested || root_workspace(workspace))
                .find(|workspace| &workspace.workspace_id.0 == id)
        } else if root_requested {
            candidates
                .iter()
                .find(root_workspace)
                .or(candidates.first())
        } else {
            candidates
                .iter()
                .find(root_workspace)
                .or(candidates.first())
        }
        .cloned()
        .ok_or(if root_requested {
            "not_found: Open the actual project root workspace before requesting a root launch"
        } else {
            "not_found: Open target project workspace required"
        })?;
        service::workspace(app, &workspace.workspace_id.0, true).map_err(|e| e.to_string())?;
        if root_requested && !root_workspace(&&workspace) {
            let root = workspace
                .project_root
                .clone()
                .ok_or("not_found: Actual project root is unavailable")?;
            if !Path::new(&root).is_absolute() || !Path::new(&root).is_dir() {
                return Err("not_found: Actual project root directory is unavailable".into());
            }
            _root_lease =
                Some(platform.begin_effect(&format!("root-workspace-{}", digest(&json!(root))))?);
            workspace.cwd = root;
            workspace.worktree_path = None;
            root_preparation = Some(workspace.clone());
        }
        // Context targets may supply inheritance only when the plugin can actually read them.
        let source = caller
            .source_thread_id()
            .and_then(|thread| target(app, thread).ok())
            .filter(|(_, w)| {
                platform
                    .authorize(app, caller, "threads.read", Some(&w.project_id))
                    .is_ok()
            })
            .map(|(r, _)| r);
        verified_source = source.as_ref().map(|source| source.thread_id.clone());
        let selected = match input.model_selection.clone() {
            Some(selection) => selection,
            None => {
                let source=source.as_ref().ok_or("invalid_input: Creation requires modelSelection or a readable verified source thread")?;
                let provider: ProviderKind = serde_json::from_value(json!(source.provider))
                    .map_err(|_| "unsupported_operation: Source provider unavailable")?;
                ModelSelection {
                    instance_id: agent_control::selection::current_instance(
                        &app.state::<DatabaseStore>(),
                        &source.thread_id,
                        provider,
                    )?,
                    model: source
                        .model
                        .clone()
                        .ok_or("invalid_input: Source has no explicit model selection")?,
                    options: inherited_options(source),
                }
            }
        };
        let selection = ProviderSelection {
            instance_id: selected.instance_id,
            model: Some(selected.model),
            options: selected.options,
        };
        let (resolved, caps, _) =
            agent_control::selection::resolve(app, selection.clone(), &workspace.cwd, true)
                .await
                .map_err(|e| e.to_string())?;
        let mode = native_mode(
            resolved.provider,
            input.runtime_mode.as_deref(),
            input.interaction_mode.as_deref(),
            input.native_permission_mode.as_deref(),
            source.as_ref(),
        )?;
        service::permission_ceiling(&control, resolved.provider, mode.as_deref())
            .map_err(|e| e.to_string())?;
        service::validate_permission_mode(resolved.provider, mode.as_deref(), &caps)
            .map_err(|e| e.to_string())?;
        platform.authorize(app, caller, method, Some(&project))?;
        let args = json!({"workspace_id":workspace.workspace_id.0,"client_request_id":command_key,"provider":resolved.provider,"target":selection,"permission_mode":mode,"title":input.title,"message":format!("[Plugin: {}]\n{}",record.manifest.name,input.text),"select":false});
        ("thread_launch", args, Some((input, workspace)))
    };
    let is_worktree = strategy.as_ref().is_some_and(|(input, _)| {
        matches!(
            input.workspace_strategy,
            Some(WorkspaceStrategy::Worktree { .. })
        )
    });
    if is_worktree {
        platform.authorize(app, caller, "worktrees.create", Some(&project))?;
    }
    let fresh = platform.store.update(&caller.installation_id, |record| {
        platform_authority::capability(record, caller, method, Some(&project))?;
        if let Some(prior) = record.receipts.get(&command_key) {
            if prior.fingerprint != fingerprint {
                return Err("idempotency_conflict: This key identifies different inputs".into());
            }
            return Ok(false);
        }
        if let Some(delivery) = &caller.delivery_id {
            let event = platform
                .events
                .next(record, caller)?
                .ok_or("permission_denied: Event delivery is not current")?;
            if &event.delivery_id != delivery {
                return Err("permission_denied: Event identity is not current".into());
            }
            if event.project_id != project {
                return Err("permission_denied: Automatic effects are restricted to the finalized run's project".into());
            }
            if !event.effects_allowed {
                return Err("permission_denied: This run has no committed input provenance; automatic thread effects are unavailable".into());
            }
            let effects = record.event_effects.entry(delivery.clone()).or_default();
            if effects.len() >= 3 {
                return Err(
                    "permission_denied: A finalized event can admit at most three thread effects"
                        .into(),
                );
            }
            effects.push(command_key.clone());
        }
        record.receipts.insert(
            command_key.clone(),
            EffectReceipt {
                fingerprint: fingerprint.clone(),
                command_key: command_key.clone(),
                method: method.into(),
                principal: control.principal.clone(),
                state: "reserved".into(),
                result: None,
                error: None,
                source_thread_id: verified_source.clone(),
                project_id: project.clone(),
                core_tool: Some(core_tool.into()),
                core_args: (!is_worktree && root_preparation.is_none()).then_some(core_args.clone()),
                required_capabilities: if is_worktree { vec!["worktrees.create".into()] } else { Vec::new() },
            },
        );
        Ok(true)
    })?;
    if !fresh {
        return recover_receipt(
            app,
            platform
                .store
                .read(&caller.installation_id)?
                .receipts
                .get(&command_key)
                .ok_or("storage_failed: Effect reservation missing")?,
        );
    }
    if let Some(root) = root_preparation {
        platform.authorize(app, caller, method, Some(&project))?;
        let state = app.state::<AppStateStore>();
        let existing = state.snapshot().workspaces.into_iter().find(|workspace| {
            workspace.cwd == root.cwd
                && workspace.worktree_path.is_none()
                && workspace.host_id.is_none()
                && !workspace.is_local_import_snapshot_only()
        });
        let id = if let Some(existing) = existing {
            if project_id(&existing) != project {
                return Err("conflict: Root workspace has a different project identity".into());
            }
            existing.workspace_id.0
        } else {
            let id = state
                .create_empty_workspace_at_path_with_selection(root.cwd.clone().into(), false)
                .0;
            // The root shell is background-only and has no terminals or setup scripts.
            // Keep its authoritative project identity aligned with the open worktree.
            state.set_workspace_project_root(&id, root.cwd.clone());
            state.set_workspace_project_identity(&id, Some(project.clone()), None);
            crate::state::emit_app_state(app);
            id
        };
        core_args["workspace_id"] = json!(id);
        platform.store.update(&caller.installation_id, |record| {
            platform_authority::capability(record, caller, method, Some(&project))?;
            record
                .receipts
                .get_mut(&command_key)
                .ok_or("storage_failed: Effect receipt missing")?
                .core_args = Some(core_args.clone());
            Ok(())
        })?;
    }
    if is_worktree {
        let (input, workspace) = strategy.ok_or("invalid_input: Missing worktree strategy")?;
        match prepare_worktree(
            platform,
            app,
            caller,
            &project,
            &command_key,
            &input,
            &workspace,
        )
        .await
        {
            Ok(workspace_id) => {
                core_args["workspace_id"] = json!(workspace_id);
                platform.store.update(&caller.installation_id, |record| {
                    let receipt = record
                        .receipts
                        .get_mut(&command_key)
                        .ok_or("storage_failed: Effect receipt missing")?;
                    receipt.core_args = Some(core_args.clone());
                    Ok(())
                })?;
            }
            Err(error) => {
                let _ = platform.store.update(&caller.installation_id, |record| {
                    if let Some(receipt) = record.receipts.get_mut(&command_key) {
                        receipt.error = Some(error.clone());
                    }
                    Ok(())
                });
                return Err(error);
            }
        }
    }
    platform.authorize(app, caller, method, Some(&project))?;
    dispatch_core_effect(platform, app, caller, &command_key, core_tool, core_args).await
}

async fn prepare_worktree<R: Runtime>(
    platform: &PlatformService,
    app: &AppHandle<R>,
    caller: &PlatformCaller,
    project: &str,
    command: &str,
    input: &CreateInput,
    workspace: &WorkspaceSnapshot,
) -> Result<String, String> {
    let Some(WorkspaceStrategy::Worktree {
        branch,
        base_ref,
        start_from_origin,
    }) = &input.workspace_strategy
    else {
        return Err("invalid_input: Missing worktree strategy".into());
    };
    platform.authorize(app, caller, "worktrees.create", Some(project))?;
    if caller.control.ceiling().is_some_and(|c| c.plan) {
        return Err("permission_denied: Plan callers cannot create worktrees".into());
    }
    let repo = workspace
        .project_root
        .clone()
        .unwrap_or_else(|| workspace.cwd.clone());
    let branch = branch
        .clone()
        .unwrap_or_else(|| format!("plugins/{}", &command[7..23]));
    for (name, value) in [("branch", branch.as_str()), ("baseRef", base_ref.as_str())] {
        if value.is_empty()
            || value.len() > 256
            || value.starts_with('-')
            || value.chars().any(char::is_control)
        {
            return Err(format!("invalid_input: Invalid {name}"));
        }
    }
    let git = |args: Vec<String>| {
        let repo = repo.clone();
        async move {
            let args: Vec<_> = args.iter().map(String::as_str).collect();
            super::platform_package::git_run_bounded(Path::new(&repo), &args, 4096, false)
                .await
                .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_string())
        }
    };
    if git(vec![
        "check-ref-format".into(),
        "--branch".into(),
        branch.clone(),
    ])
    .await?
        != branch
    {
        return Err("invalid_input: Worktree branch must be literal".into());
    }
    let exists = super::platform_package::git_run_bounded(
        Path::new(&repo),
        &[
            "for-each-ref",
            "--count=1",
            "--format=%(refname)",
            &format!("refs/heads/{branch}"),
        ],
        4096,
        false,
    )
    .await?;
    if !exists.is_empty()
        || crate::git::conventional_worktree_path(Path::new(&repo), &branch)
            .is_some_and(|path| path.exists())
    {
        return Err(
            "conflict: Worktree branch/path already exists; no existing checkout is adopted".into(),
        );
    }
    let base_revision = if *start_from_origin {
        let remote_branch = base_ref.strip_prefix("origin/").unwrap_or(base_ref);
        if git(vec![
            "check-ref-format".into(),
            "--branch".into(),
            remote_branch.into(),
        ])
        .await?
            != remote_branch
        {
            return Err("invalid_input: A remote base must name a literal origin branch".into());
        }
        let fetch_repo = repo.clone();
        let fetch_branch = remote_branch.to_string();
        let fetched = tokio::task::spawn_blocking(move || {
            crate::git::fetch_origin_branch(Path::new(&fetch_repo), &fetch_branch)
        })
        .await
        .map_err(|_| "operation_failed: Origin fetch worker failed")?;
        if !fetched {
            return Err("operation_failed: The requested origin base could not be fetched; no stale/local fallback was used".into());
        }
        format!("refs/remotes/origin/{remote_branch}")
    } else {
        base_ref.clone()
    };
    let commit = git(vec![
        "rev-parse".into(),
        "--verify".into(),
        "--end-of-options".into(),
        format!("{base_revision}^{{commit}}"),
    ])
    .await?;
    let base = format!("refs/codemux/plugin-worktrees/{command}");
    platform.authorize(app, caller, "worktrees.create", Some(project))?;
    // An absolute retained ref freezes the reviewed commit, without the shared
    // workspace helper's implicit best-effort origin fetch for a local base.
    git(vec![
        "update-ref".into(),
        base.clone(),
        commit,
        String::new(),
    ])
    .await?;
    platform.authorize(app, caller, "worktrees.create", Some(project))?;
    let created = crate::commands::workspace::create_worktree_workspace_impl_with_selection(
        app.clone(),
        &app.state::<AppStateStore>(),
        &app.state::<DatabaseStore>(),
        &app.state::<crate::terminal::PtyState>(),
        &app.state::<crate::presets::PresetStoreState>(),
        repo,
        branch,
        true,
        Some(base),
        "empty".into(),
        None,
        None,
        None,
        None,
        false,
        None,
    )
    .await?;
    if created.adopted {
        return Err(
            "conflict: Checkout was claimed concurrently; no plugin prompt was sent".into(),
        );
    }
    // The native helper recomputes identity from today's origin URL. The
    // admitted project may predate that remote or carry a synced identity;
    // its new checkout must stay in the project whose grant admitted it.
    app.state::<AppStateStore>().set_workspace_project_identity(
        &created.workspace_id,
        Some(project.into()),
        None,
    );
    crate::state::emit_app_state(app);
    Ok(created.workspace_id)
}

#[cfg(test)]
mod platform_policy_tests {
    use super::*;

    #[test]
    fn platform_mode_conversion_keeps_plan_intent_and_refuses_unknown_policy() {
        assert_eq!(
            native_mode(
                ProviderKind::Claude,
                Some("full-access"),
                Some("plan"),
                None,
                None
            )
            .unwrap(),
            Some("plan".into())
        );
        assert!(native_mode(
            ProviderKind::Claude,
            None,
            Some("plan"),
            Some("bypassPermissions"),
            None
        )
        .is_err());
        assert!(native_mode(ProviderKind::Grok, None, Some("plan"), None, None).is_err());
        assert!(native_mode(ProviderKind::Codex, Some("auto"), None, None, None).is_err());
        assert_eq!(
            native_mode(
                ProviderKind::Codex,
                Some("auto-accept-edits"),
                None,
                None,
                None
            )
            .unwrap(),
            Some("workspace-write".into())
        );
    }
    #[test]
    fn platform_model_options_accept_t3_selections_without_dropping_or_overwriting_values() {
        let parse = |options| {
            serde_json::from_value::<ModelSelection>(
                json!({"instanceId":"codex","model":"model","options":options}),
            )
        };
        assert_eq!(
            parse(json!([{"id":"effort","value":"high"},{"id":"fast_mode","value":true}]))
                .unwrap()
                .options,
            parse(json!({"effort":"high","fast_mode":true}))
                .unwrap()
                .options
        );
        assert!(
            parse(json!([{"id":"effort","value":"high"},{"id":"effort","value":"low"}])).is_err()
        );
        assert!(parse(json!({"effort":{"unknown":"nested"}})).is_err());
        assert!(parse(json!({"fast_mode":1})).is_err());
        let descriptors = model_descriptors(
            &json!({"options":[{"id":"mode","category":"mode","type":"select","name":"Mode"}]}),
            &json!({"effort_levels":["high"],"supports_fast_mode":true}),
        );
        assert!(!descriptors
            .iter()
            .any(|descriptor| descriptor["id"] == "mode"));
        assert!(descriptors
            .iter()
            .any(|descriptor| descriptor["id"] == "fast_mode" && descriptor["type"] == "boolean"));
    }
}
