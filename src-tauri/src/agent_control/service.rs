//! Launch sequencing belongs here, above the existing native session owner.
use super::{Access, CallerKind, ControlAccess, ControlCaller, ControlError, ErrorCode, NativeControlGuard, NativeControlState};
use crate::agent_provider::{ProviderChatCapabilities, ProviderKind, StartSessionInput, ThreadId};
use crate::commands::agent_chat::{self, ProviderRegistry, SendTurnCommandInput};
use crate::database::{AgentChatSessionConfig, DatabaseStore};
use crate::state::{AppStateStore, WorkspaceId, WorkspaceSnapshot};
use crate::workspace_service::{self as workspaces, NotControllable, OriginPolicy, WorkspaceService, WorktreeBase, WorktreeRequest};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager, Runtime};

pub(crate) fn parse<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, ControlError> {
    if !value.is_object() { return Err(ControlError::new(ErrorCode::InvalidArguments, "Arguments must be an object")); }
    serde_json::from_value(value).map_err(|e| ControlError::new(ErrorCode::InvalidArguments, e.to_string()))
}

/// For a defaulted argument: `null` means absent, as the schema check treats
/// every optional `null`. Pair it with `#[serde(default)]`.
pub(super) fn null_as_default<'de, D: serde::Deserializer<'de>, T: Deserialize<'de> + Default>(value: D) -> Result<T, D::Error> {
    Option::<T>::deserialize(value).map(Option::unwrap_or_default)
}

pub(crate) fn identifier(value: &str, name: &str, maximum: usize) -> Result<(), ControlError> {
    if value.is_empty() || value.len() > maximum || value.trim() != value || value.chars().any(char::is_control) {
        return Err(ControlError::new(ErrorCode::InvalidArguments, format!("{name} must be a nonempty unchanged identifier (max {maximum} bytes)")));
    }
    Ok(())
}

pub(crate) fn workspace<R: Runtime>(app: &AppHandle<R>, id: &str, mutation: bool) -> Result<WorkspaceSnapshot, ControlError> {
    identifier(id, "workspace_id", 256)?;
    let workspace = app.state::<AppStateStore>().snapshot().workspaces.into_iter().find(|w| w.workspace_id.0 == id)
        .ok_or_else(|| ControlError::new(ErrorCode::WorkspaceNotFound, "Select an existing workspace explicitly"))?;
    if mutation {
        workspaces::native_control(&workspace).map_err(|refusal| match refusal {
            NotControllable::ImportedSnapshot => ControlError::new(ErrorCode::ImportedSnapshotReadOnly, "Imported workspaces are read-only snapshots"),
            NotControllable::Remote => ControlError::new(ErrorCode::UnsupportedRemoteWorkspace, "Native outside control currently targets local workspaces"),
        })?;
    }
    Ok(workspace)
}

/// The connector owns authentication; these checks own the worker ceiling.
/// Unknown provider mode semantics fail closed for supervised credentials.
pub(crate) fn permission_ceiling(caller: &ControlCaller, provider: ProviderKind, mode: Option<&str>) -> Result<(), ControlError> {
    if let Some(ceiling) = caller.ceiling() {
        return if super::permissions::Ceiling::for_mode(provider, mode).is_some_and(|target| ceiling.permits(target)) {
            Ok(())
        } else { Err(ControlError::new(ErrorCode::PermissionCeiling, "Target mode exceeds this chat's effective native policy")) };
    }
    if caller.access == ControlAccess::ReadOnly {
        return Err(ControlError::new(ErrorCode::PermissionDenied, "This connection has read-only access"));
    }
    if caller.access == ControlAccess::FullAccess { return Ok(()); }
    let supervised = match provider {
        ProviderKind::Claude => matches!(mode, Some("default" | "acceptEdits" | "plan")),
        ProviderKind::Codex => matches!(mode, Some("read-only" | "workspace-write" | "plan")),
        ProviderKind::Cursor | ProviderKind::Grok => matches!(mode, Some("ask" | "plan")),
        ProviderKind::Hermes | ProviderKind::Acp | ProviderKind::OpenCode => false,
    };
    if supervised { Ok(()) } else {
        Err(ControlError::new(ErrorCode::PermissionCeiling, "This worker mode needs full-access consent; supervised clients cannot target full-access or unknown modes"))
    }
}

pub(crate) fn authorize<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller) -> Result<(), ControlError> {
    authorize_native(app, caller, true)
}

pub(crate) fn authorize_native<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, orchestration: bool) -> Result<(), ControlError> {
    authorize_with_delivery_retry(app, caller, orchestration, false)
}

pub(crate) fn authorize_delivery_retry<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller) -> Result<(), ControlError> {
    authorize_with_delivery_retry(app, caller, true, true)
}

fn authorize_with_delivery_retry<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, orchestration: bool, retry_held: bool) -> Result<(), ControlError> {
    if let super::CallerPrincipal::RemoteClient { session_id } = &caller.identity {
        let approved = app.state::<DatabaseStore>().web_remote_get_session(session_id)
            .is_some_and(|session| session.approved && !session.revoked);
        let connected = app.try_state::<crate::web_remote::WebRemoteState>()
            .is_some_and(|state| state.shared().connections.session_live(session_id));
        if !approved || !connected {
            return Err(ControlError::new(ErrorCode::StaleCaller, "The remote client disconnected or its session was revoked"));
        }
    }
    if let super::CallerPrincipal::Thread { thread_id, provider,
        provider_instance_id, authority, ceiling, .. } = &caller.identity {
        let db = app.state::<DatabaseStore>();
        let mut orchestration = orchestration;
        match &caller.admitted {
            None => {
                authority.verify(thread_id).map_err(|e| ControlError::new(ErrorCode::StaleCaller, e))?;
                db.verify_delivery_generation(thread_id, authority.stop_generation).map_err(|e| ControlError::new(ErrorCode::StaleCaller, e))?;
            }
            Some(super::AdmittedAuthority::Operation {operation_id}) => {
                let receipt=db.control_operation(&caller.principal,operation_id,&app.state::<NativeControlState>().epoch)?.ok_or_else(||ControlError::new(ErrorCode::StaleOperation,"Accepted operation missing"))?;
                if receipt.tool == "workspace_launch" { orchestration = false; }
                if !matches!(receipt.tool.as_str(),"thread_launch"|"workspace_launch") || !matches!(receipt.state.as_str(),"accepted"|"running") {return Err(ControlError::new(ErrorCode::StaleOperation,"Accepted launch ended or was revoked"));}
            }
            Some(super::AdmittedAuthority::Task {task_id}) => {
                let task=db.delegation_task(task_id)?.ok_or_else(||ControlError::new(ErrorCode::StaleTask,"Accepted task missing"))?;
                if task.parent_thread_id!=*thread_id || task.cancellation_generation>0 || task.status.terminal() {return Err(ControlError::new(ErrorCode::StaleTask,"Accepted task was stopped or ended"));}
            }
            Some(super::AdmittedAuthority::Delivery {delivery_id}) => {
                let row=db.message_delivery(delivery_id)?.ok_or_else(||ControlError::new(ErrorCode::StaleDelivery,"Accepted message missing"))?;
                let retryable = retry_held && !row.attempted && matches!(row.state.as_str(), "held" | "rejected");
                if row.payload["caller"]["principal"]!=caller.principal || (!retryable && matches!(row.state.as_str(),"cancelled"|"disposed"|"held"|"held_unknown"|"rejected")) {return Err(ControlError::new(ErrorCode::StaleDelivery,"Accepted message authority was revoked"));}
                // Workspace startup remains authorized if collaboration is
                // switched off after admission, including its queued prompt.
                if let Ok(super::AdmittedAuthority::Operation { operation_id }) =
                    serde_json::from_value(row.payload["caller"]["admitted"].clone())
                {
                    let receipt = db.control_operation(&caller.principal, &operation_id, &app.state::<NativeControlState>().epoch)?;
                    if receipt.is_some_and(|r| r.tool == "workspace_launch" && matches!(r.state.as_str(), "accepted" | "running" | "succeeded")) {
                        orchestration = false;
                    }
                }
            }
        }
        if orchestration && !crate::commands::delegation::setting_on(&db) { return Err(ControlError::new(ErrorCode::OrchestrationDisabled, "Agent collaboration is off in Settings")); }
        let record = db.get_agent_chat_session(thread_id).ok_or_else(|| ControlError::new(ErrorCode::StaleCaller, "Calling chat no longer exists"))?;
        let bound = app.state::<AppStateStore>().agent_chat_pane_id_for_thread(thread_id)
            .and_then(|pane| app.state::<AppStateStore>().agent_chat_pane_thread(&pane));
        if (caller.admitted.is_none() && bound != Some((*provider, thread_id.clone()))) || record.provider != crate::commands::delegation::provider_id(*provider) || record.imported_from.is_some() {
            return Err(ControlError::new(ErrorCode::StaleCaller, "Calling chat binding changed"));
        }
        if super::selection::current_instance(&db, thread_id, *provider)? != *provider_instance_id {
            return Err(ControlError::new(
                ErrorCode::StaleCaller,
                "Calling provider instance changed",
            ));
        }
        let canonical = super::permissions::Ceiling::for_record(&db, &record, *provider)
            .ok_or_else(|| {
                ControlError::new(ErrorCode::UnsupportedPermissionCeiling, "Canonical chat mode is unknown")
            })?;
        if let Some(effective)=crate::delegation::authority::NativeAuthority::effective_ceiling(thread_id).map_err(|e|ControlError::new(ErrorCode::UnsupportedPermissionCeiling,e))? {
            if !effective.permits(*ceiling){return Err(ControlError::new(ErrorCode::PermissionCeiling,"Effective native mode narrowed after admission"));}
        }
        if !canonical.permits(*ceiling) { return Err(ControlError::new(ErrorCode::PermissionCeiling, "Calling chat policy was narrowed after admission")); }
        workspace(app, &record.workspace_id, true)?;
        return Ok(());
    }
    crate::mcp_connector::validate_caller(app,caller)
}

pub(crate) async fn capabilities<R: Runtime>(app: &AppHandle<R>, provider: ProviderKind) -> Result<ProviderChatCapabilities, ControlError> {
    capabilities_for_account(app, provider, None).await
}

pub(crate) async fn capabilities_for_account<R: Runtime>(
    app: &AppHandle<R>,
    provider: ProviderKind,
    account_id: Option<String>,
) -> Result<ProviderChatCapabilities, ControlError> {
    #[cfg(test)]
    if let Some(state) = app.try_state::<NativeControlState>() {
        if let Some(caps) = state.fixture_capabilities.lock().unwrap().get(&provider).cloned() { return Ok(caps); }
    }
    agent_chat::list_chat_provider_capabilities(
        app.clone(), provider, None, account_id,
        app.state::<std::sync::Arc<crate::agent_provider::codex::capabilities::CodexCapabilityCache>>(),
        app.state::<std::sync::Arc<crate::agent_provider::cursor::capabilities::CursorCapabilityCache>>(),
        app.state::<std::sync::Arc<crate::agent_provider::grok::capabilities::GrokCapabilityCache>>(),
        app.state::<std::sync::Arc<crate::agent_provider::claude::capabilities::ClaudeCapabilityCache>>(),
        app.state::<std::sync::Arc<crate::agent_provider::opencode::OpenCodeServerManager>>(),
    ).await.map_err(ControlError::from)
}

/// The composer's cached catalog for `provider`, if any. Never spawns.
pub(crate) async fn cached_capabilities<R: Runtime>(
    app: &AppHandle<R>,
    provider: ProviderKind,
    account_id: Option<String>,
) -> Result<Option<ProviderChatCapabilities>, ControlError> {
    #[cfg(test)]
    if let Some(state) = app.try_state::<NativeControlState>() {
        if state.fixture_uncached.lock().unwrap().contains(&provider) { return Ok(None); }
        if let Some(caps) = state.fixture_capabilities.lock().unwrap().get(&provider).cloned() { return Ok(Some(caps)); }
    }
    agent_chat::cached_chat_provider_capabilities(app, provider, account_id).await.map_err(ControlError::from)
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Launch {
    target: Option<super::selection::ProviderSelection>,
    #[serde(skip)]
    resolved: Option<super::selection::ResolvedSelection>,
    #[serde(skip)]
    selected_capabilities: Option<ProviderChatCapabilities>,
    workspace_id: String,
    client_request_id: String,
    provider: ProviderKind,
    permission_mode: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    context_window: Option<String>,
    service_tier: Option<String>,
    #[serde(default, deserialize_with = "null_as_default")] fast_mode: bool,
    title: Option<String>,
    message: Option<String>,
    #[serde(default, deserialize_with = "null_as_default")] select: bool,
    worktree: Option<Value>,
}

pub(super) async fn launch<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, args: Value) -> Result<Value, ControlError> {
    launch_placed(app, caller, args, false).await
}

/// With `child_workspace`, `workspace_id` names the caller's workspace and
/// the chat gets its own background workspace nested under it, in the same
/// folder, titled after the chat.
pub(crate) async fn launch_placed<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, args: Value, child_workspace: bool) -> Result<Value, ControlError> {
    if !args.as_object().is_some_and(|args| args.contains_key("permission_mode")) {
        return Err(ControlError::new(ErrorCode::PermissionModeRequired, "Select an advertised worker permission mode explicitly; null is allowed only when the provider has no modes"));
    }
    let mut input: Launch = parse(args.clone())?;
    identifier(&input.client_request_id, "client_request_id", 128)?;
    identifier(&input.workspace_id, "workspace_id", 256)?;
    // The placement is part of the request: a retry must not turn a tab
    // launch into a child workspace or back.
    let mut keyed = args;
    if child_workspace { keyed["placement"] = json!("child_workspace"); }
    let args = keyed;
    if let Some(receipt) = retry_receipt(app, caller, "thread_launch", &args)? { return Ok(receipt); }
    let workspace = workspace(app, &input.workspace_id, true)?;
    permission_ceiling(caller, input.provider, input.permission_mode.as_deref())?;
    if let Some(message) = input.message.as_deref() {
        if message.trim().is_empty() || message.len() > 65536 { return Err(ControlError::new(ErrorCode::InvalidArguments, "message must contain text and fit in 65536 bytes")); }
    }
    if let Some(title) = input.title.as_deref() { identifier(title, "title", 100)?; }
    let worktree = match input.worktree.take() {
        Some(raw) => Some(plan_worktree(caller, child_workspace, &input, &workspace, raw).await?),
        None => None,
    };
    prepare_selection(app, &mut input, &workspace.cwd).await?;
    validate_worker(app, caller, &input).await?;
    authorize(app, caller)?;
    let epoch = app.state::<NativeControlState>().epoch.clone();
    let fingerprint = fingerprint(&args)?;
    let thread = uuid::Uuid::new_v4().to_string();
    // Named before admission so the receipt and any retry point at it.
    let target = if child_workspace || worktree.is_some() { AppStateStore::reserve_workspace_id().0 } else { workspace.workspace_id.0.clone() };
    let admitted = caller.admit(||app.state::<DatabaseStore>().admit_control_operation_for_thread(
        &caller.principal, &input.client_request_id, "thread_launch", &fingerprint, &epoch,
        Some(&target), Some(&thread), caller.thread_id().zip(caller.admission_generation()),
    )).map_err(|e| ControlError::from_text(e, ErrorCode::OperationAdmissionFailed))?;
    let receipt = serde_json::to_value(&admitted.receipt).map_err(|e|ControlError::new(ErrorCode::SerializationFailed,e.to_string()))?;
    if !admitted.newly_admitted { return Ok(receipt); }
    if child_workspace {
        let title = input.title.clone().unwrap_or_else(|| chat_title(input.provider, input.message.as_deref()));
        let created = app.state::<AppStateStore>().create_chat_child_workspace(
            &crate::state::WorkspaceId(target.clone()), &workspace.workspace_id.0, &workspace.cwd, &title,
        );
        if let Err(error) = created {
            let error = ControlError::new(ErrorCode::WorkspaceNotFound, error);
            let db = app.state::<DatabaseStore>();
            let _ = db.orchestration_launch_finished(&thread);
            let _ = db.update_control_operation(&caller.principal, &admitted.receipt.operation_id, &epoch, "failed", None, None, None, serde_json::to_value(&error).ok().as_ref());
            return Err(error);
        }
        input.title = Some(title);
        input.workspace_id = target.clone();
        crate::state::emit_app_state(app);
    }
    let app = app.clone();
    let mut caller = caller.clone();
    let operation = admitted.receipt.operation_id;
    if caller.thread_id().is_some(){caller.admitted=Some(super::AdmittedAuthority::Operation{operation_id:operation.clone()});}
    tauri::async_runtime::spawn(async move {
        let db = app.state::<DatabaseStore>();
        if db.update_control_operation(&caller.principal, &operation, &epoch, "running", None, None, None, None).is_err() { return; }
        let selection = input.resolved.as_ref().map(|s| json!({"selection":s}));
        if let Some(selection) = &selection {
            let _ = db.update_control_operation(
                &caller.principal,
                &operation,
                &epoch,
                "running",
                None,
                None,
                Some(selection),
                None,
            ); }
        let idle_launch = input.message.is_none();
        let workspace_id = input.workspace_id.clone();
        let outcome = match &worktree {
            None => materialize(&app, &caller, input, &thread).await,
            Some(plan) => launch_in_worktree(&app, &caller, input, &thread, plan, &target).await,
        };
        if idle_launch || outcome.is_err() { let _ = db.orchestration_launch_finished(&thread); }
        if child_workspace && outcome.is_err() { close_empty_child_workspace(&app, &workspace_id); }
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

/// A title for a launched chat that named none: its message's first line.
fn chat_title(provider: ProviderKind, message: Option<&str>) -> String {
    let line = message.and_then(|m| m.lines().map(str::trim).find(|l| !l.is_empty()));
    match line {
        Some(line) if line.chars().count() > 60 => format!("{}…", line.chars().take(59).collect::<String>().trim_end()),
        Some(line) => line.to_string(),
        None => format!("{} chat", crate::commands::delegation::provider_label(provider)),
    }
}

/// `thread_launch.worktree`: where the new checkout goes, all of it checked
/// before admission with local git only. The origin fetch, if any, runs after.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorktreeLaunch {
    branch: Option<String>,
    base: Option<String>,
    start_from_origin: Option<bool>,
}

struct WorktreePlan {
    repo: PathBuf,
    branch: String,
    base: String,
    origin: OriginPolicy,
    /// The source workspace's project, so the new worktree nests under it
    /// even when the identity recomputed from today's origin would differ.
    project_uid: Option<String>,
}

async fn plan_worktree(
    caller: &ControlCaller,
    child_workspace: bool,
    input: &Launch,
    workspace: &WorkspaceSnapshot,
    raw: Value,
) -> Result<WorktreePlan, ControlError> {
    if child_workspace || caller.thread_id().is_some() {
        return Err(ControlError::new(ErrorCode::UnsupportedOperation, "In-app chats start worktrees with workspace_launch"));
    }
    // The new worktree runs the project's setup scripts.
    super::registry::require(caller, Access::Machine)?;
    let launch: WorktreeLaunch = parse(raw).map_err(|error| ControlError::new(error.code, format!("worktree: {}", error.message)))?;
    for (field, value) in [("worktree.branch", &launch.branch), ("worktree.base", &launch.base)] {
        if let Some(value) = value {
            workspaces::check_ref_text(field, value).map_err(|error| super::workspace_tools::git_error(error, ErrorCode::InvalidArguments))?;
        }
    }
    let folder = PathBuf::from(workspace.project_root.as_deref().unwrap_or(&workspace.cwd));
    let title = input.title.clone().unwrap_or_else(|| chat_title(input.provider, input.message.as_deref()));
    let (repo, branch, base) = super::workspace_tools::blocking(move || {
        let repo = crate::config::workspace_config::find_git_root(&folder).ok_or_else(|| {
            ControlError::new(ErrorCode::NotAGitRepository, "The workspace's folder is not in a git repository")
        })?;
        let branch = launch.branch.unwrap_or_else(|| branch_for_title(&title, &repo));
        let base = launch.base.unwrap_or_else(|| workspaces::default_base_name(&repo));
        Ok((repo, branch, base))
    }).await?;
    let git_error = |error| super::workspace_tools::git_error(error, ErrorCode::WorktreeCreateFailed);
    workspaces::check_new_worktree(&repo, &branch).await.map_err(git_error)?;
    let origin = match launch.start_from_origin {
        Some(true) => OriginPolicy::Required,
        Some(false) => OriginPolicy::Local,
        None => OriginPolicy::PreferFresh,
    };
    // A local base fails now rather than after the receipt is admitted.
    if origin == OriginPolicy::Local {
        workspaces::resolve_base(&repo, &base, origin).await.map_err(base_error)?;
    }
    Ok(WorktreePlan { repo, branch, base, origin, project_uid: workspace.project_uid.clone() })
}

/// A new branch named after the chat, unused in the repository; a random
/// name when the title leaves nothing usable. Runs git synchronously.
fn branch_for_title(title: &str, repo: &Path) -> String {
    let slug = crate::branch_name::sanitize_branch_name(&title.to_lowercase());
    if slug.is_empty() {
        crate::branch_name::generate_random_name(repo)
    } else {
        crate::branch_name::deconflict_branch_name(&slug, repo)
    }
}

/// Resolving a base fails only for the base: a bad name, or an origin or ref
/// that cannot be read.
fn base_error(error: String) -> ControlError {
    super::workspace_tools::git_error(error, ErrorCode::WorktreeBaseUnavailable)
}

/// The admitted half of a worktree launch: create the checkout under the
/// reserved id `target`, then start the thread in it. A checkout that exists
/// when the start fails is kept, since setup scripts may already have written
/// to it.
async fn launch_in_worktree<R: Runtime>(
    app: &AppHandle<R>,
    caller: &ControlCaller,
    mut input: Launch,
    thread: &str,
    plan: &WorktreePlan,
    target: &str,
) -> Result<Value, ControlError> {
    let base = workspaces::resolve_base(&plan.repo, &plan.base, plan.origin).await.map_err(base_error)?;
    // Another creator may have taken the branch while the base was fetched.
    workspaces::check_new_worktree(&plan.repo, &plan.branch)
        .await
        .map_err(|error| super::workspace_tools::git_error(error, ErrorCode::WorktreeCreateFailed))?;
    // A grant revoked during the fetch gets no checkout and no setup scripts.
    authorize(app, caller)?;
    let created = WorkspaceService::new(app)
        .create_worktree(WorktreeRequest {
            repo_path: plan.repo.display().to_string(),
            branch: plan.branch.clone(),
            new_branch: true,
            // A detached default records the commit, not "HEAD".
            base: WorktreeBase::Commit { sha: base.commit.clone(), label: (base.label != "HEAD").then(|| base.label.clone()) },
            layout: crate::state::WorkspacePresetLayout::Empty,
            preset: None,
            pr_number: None,
            select: false,
            initial_chat: None,
            reserved_id: Some(WorkspaceId(target.to_string())),
        })
        .await
        .map_err(|error| super::workspace_tools::git_error(error, ErrorCode::WorktreeCreateFailed))?;
    if created.adopted {
        return Err(ControlError::new(ErrorCode::WorktreeConflict, "The checkout was claimed concurrently; no prompt was sent"));
    }
    let state = app.state::<AppStateStore>();
    if plan.project_uid.is_some() {
        state.set_workspace_project_identity(target, plan.project_uid.clone(), None);
    }
    if let Some(title) = &input.title {
        state.rename_workspace(target, title.clone());
    }
    crate::state::emit_app_state(app);
    input.workspace_id = target.to_string();
    let worktree = json!({"branch": plan.branch, "base": base.label, "base_commit": base.commit, "path": created.cwd});
    match materialize(app, caller, input, thread).await {
        Ok(mut result) => {
            result["worktree"] = worktree;
            Ok(result)
        }
        Err(error) => Err(ControlError::new(error.code, format!("{}. The created workspace {target} was kept for inspection.", error.message.trim_end_matches('.')))),
    }
}

/// A failed launch leaves no empty row behind in the sidebar.
fn close_empty_child_workspace<R: Runtime>(app: &AppHandle<R>, workspace_id: &str) {
    let state = app.state::<AppStateStore>();
    if state.find_workspace(workspace_id).is_some_and(|w| w.surfaces.is_empty()) && state.close_workspace(workspace_id).is_ok() {
        crate::state::emit_app_state(app);
    }
}

async fn prepare_selection<R: Runtime>(
    app: &AppHandle<R>,
    input: &mut Launch,
    cwd: &str,
) -> Result<(), ControlError> {
    let Some(target) = input.target.clone() else {
        return Ok(());
    };
    let (resolved, caps, _) = super::selection::resolve(app, target, cwd, super::selection::Discovery::Launch).await?;
    input.selected_capabilities = Some(caps);
    if resolved.provider != input.provider {
        return Err(ControlError::new(
            ErrorCode::InvalidArguments,
            "Target instance and provider disagree",
        ));
    }
    if input.model.is_some() && input.model != resolved.model {
        return Err(ControlError::new(
            ErrorCode::InvalidArguments,
            "Conflicting model selections",
        ));
    }
    input.model = resolved.model.clone();
    if let Some(effort) = resolved.options.get("effort").and_then(Value::as_str) {
        input.effort = Some(effort.into());
    }
    if let Some(window) = resolved
        .options
        .get("context_window")
        .and_then(Value::as_str)
    {
        input.context_window = Some(window.into());
    }
    if let Some(tier) = resolved.options.get("service_tier").and_then(Value::as_str) {
        input.service_tier = Some(tier.into());
    }
    if let Some(fast) = resolved.options.get("fast_mode").and_then(Value::as_bool) {
        input.fast_mode = fast;
    }
    input.resolved = Some(resolved);
    Ok(())
}

pub(crate) fn validate_permission_mode(provider: ProviderKind, mode: Option<&str>, caps: &ProviderChatCapabilities) -> Result<(), ControlError> {
    // Plan is an app-owned collaboration mode, with an explicit read-only sandbox.
    let plan = matches!(provider, ProviderKind::Claude | ProviderKind::Codex) && mode == Some("plan");
    if plan || (caps.permission_modes.is_empty() && mode.is_none()) || caps.permission_modes.iter().any(|option| Some(option.value.as_str()) == mode) {
        Ok(())
    } else {
        Err(ControlError::new(ErrorCode::UnsupportedPermissionMode, "Select an exact mode from orchestrator_capabilities"))
    }
}

async fn validate_worker<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, input: &Launch) -> Result<(), ControlError> {
    permission_ceiling(caller, input.provider, input.permission_mode.as_deref())?;
    agent_chat::feature_flag_on(&app.state::<crate::observability::ObservabilityStore>())?;
    if app.state::<ProviderRegistry>().get(input.provider).await.is_none() {
        return Err(ControlError::new(ErrorCode::ProviderNotConfigured, "The selected provider is not configured in CodeMux"));
    }
    let caps = if let Some(caps) = &input.selected_capabilities {
        caps.clone()
    } else {
        capabilities(app, input.provider).await?
    };
    validate_permission_mode(input.provider, input.permission_mode.as_deref(), &caps)?;
    let model = match &input.model {
        Some(id) => Some(caps.models.iter().find(|model| &model.id == id)
            .ok_or_else(|| ControlError::new(ErrorCode::UnsupportedModel, "Select an exact model from agent_capabilities"))?),
        None => None,
    };
    if let Some(effort) = &input.effort {
        if !model.is_some_and(|model| model.effort_levels.contains(effort)) {
            return Err(ControlError::new(ErrorCode::UnsupportedEffort, "Explicit effort requires a selected model advertising that exact value"));
        }
    }
    if let Some(context) = &input.context_window {
        if !model.is_some_and(|model| model.context_window_options.iter().any(|option| &option.value == context)) {
            return Err(ControlError::new(ErrorCode::UnsupportedContextWindow, "The selected model does not advertise that context window"));
        }
    }
    if input.service_tier.as_ref().is_some_and(|tier| {
        !model.is_some_and(|m| m.service_tiers.iter().any(|option| &option.value == tier))
    }) {
        return Err(ControlError::new(
            ErrorCode::UnsupportedServiceTier,
            "The selected model does not advertise that service tier",
        ));
    }
    if input.fast_mode && !model.is_some_and(|model| model.supports_fast_mode) {
        return Err(ControlError::new(ErrorCode::UnsupportedFastMode, "The selected model does not advertise fast mode"));
    }
    Ok(())
}

/// An in-app launch validates the same worker options before creating a
/// worktree, then uses the native owner to publish and start its chat.
pub(crate) struct PreparedWorkspaceChat(Launch);

impl PreparedWorkspaceChat {
    /// `target` selects a configured instance (a Hermes profile, an
    /// account); `cwd` is where a cold instance catalog is discovered.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn prepare_selected<R: Runtime>(
        app: &AppHandle<R>, caller: &ControlCaller, provider: ProviderKind,
        target: Option<super::selection::ProviderSelection>, cwd: &str,
        title: String, message: String, model: Option<String>, effort: Option<String>,
    ) -> Result<Self, ControlError> {
        let mut input = Launch {
            target,
            resolved: None,
            selected_capabilities: None,
            workspace_id: String::new(), client_request_id: uuid::Uuid::new_v4().to_string(),
            provider, permission_mode: if let Some(ceiling)=caller.ceiling() {ceiling.inherited_workspace(provider)?} else if caller.access==ControlAccess::FullAccess {crate::commands::agent_chat::fallback_permission_mode(provider).map(str::to_string)} else {return Err(ControlError::new(ErrorCode::UnsupportedPermissionCeiling,"Explicit reviewed worker policy required"));},
            title: Some(title), message: Some(message), model, effort,
            context_window: None,
            service_tier: None, fast_mode: false, select: false, worktree: None,
        };
        prepare_selection(app, &mut input, cwd).await?;
        validate_worker(app, caller, &input).await?;
        authorize_native(app, caller, false)?;
        Ok(Self(input))
    }

    pub(crate) async fn materialize<R: Runtime>(
        mut self, app: &AppHandle<R>, caller: &ControlCaller, workspace_id: String, thread: &str,
    ) -> Result<Value, ControlError> {
        self.0.workspace_id = workspace_id;
        materialize(app, caller, self.0, thread).await
    }
}

async fn materialize<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, input: Launch, thread: &str) -> Result<Value, ControlError> {
    authorize(app, caller)?;
    let workspace = workspace(app, &input.workspace_id, true)?;
    permission_ceiling(caller, input.provider, input.permission_mode.as_deref())?;
    if let Some(selection) = &input.resolved {
        super::selection::verify(app, selection)?;
    }
    let provider_name = serde_json::to_value(input.provider).map_err(|e|ControlError::new(ErrorCode::SerializationFailed,e.to_string()))?;
    let db = app.state::<DatabaseStore>();
    // Save configuration before publishing a bound pane. A mounting renderer
    // cannot see an unbound draft and start a competing provider session.
    db.upsert_agent_chat_session(thread, &input.workspace_id, Some(&workspace.cwd), provider_name.as_str().unwrap_or_default())?;
    db.update_agent_chat_session_config(thread, &AgentChatSessionConfig {
        model: Some(input.model.clone()), effort: Some(input.effort.clone()),
        context_window: Some(input.context_window.clone()), permission_mode: Some(input.permission_mode.clone()),
        fast_mode: Some(input.fast_mode), service_tier: input.service_tier.clone(),
    })?;
    if let Some(title) = &input.title { db.set_agent_chat_title(thread, title)?; }
    if let Some(selection) = &input.resolved {
        super::selection::bind_new_thread(app, selection, thread)?; }
    agent_chat::feature_flag_on(&app.state::<crate::observability::ObservabilityStore>())?;
    let state = app.state::<AppStateStore>();
    let (pane, created) = state.create_or_reuse_agent_chat_pane_with_selection(
        &input.workspace_id, Some(input.provider), Some(workspace.cwd.clone()),
        Some(crate::presets::LaunchMode::NewTab), Some(thread.into()), input.select && caller.thread_id().is_none(),
    )?;
    if let Some(title) = input.title.as_deref().filter(|_| created) { state.title_chat_tab(&input.workspace_id, &pane, title); }
    crate::state::emit_app_state(app);
    let pane = pane.0;
    let plan_mode = input.provider == ProviderKind::Codex && (input.permission_mode.as_deref() == Some("plan") || (input.permission_mode.as_deref() == Some("read-only") && caller.ceiling().is_some_and(|c| c.plan)));
    let start = StartSessionInput {
        thread_id: ThreadId(thread.into()), cwd: workspace.cwd.into(), model: input.model,
        resume_cursor: None, fresh_session: true, permission_mode: input.permission_mode,
        effort: input.effort, context_window: input.context_window, fast_mode: input.fast_mode, service_tier: input.service_tier,
        additional_directories: vec![], env: None, workspace_id: None, remote: None, extra: input
            .resolved
            .as_ref()
            .map(|r| r.extra.clone())
            .unwrap_or(Value::Null), recorded_usage_baseline: None,
    };
    let control = NativeControlGuard { caller:caller.clone(),workspace_id:input.workspace_id.clone(),thread_id:thread.into(),provider:input.provider,admission:None }.retain_admission(app);
    let actual = agent_chat::agent_chat_start_session_guarded(app.clone(), pane.clone(), input.provider, start, Some(thread.into()), Some(control.clone())).await?;
    if actual.0 != thread { return Err(ControlError::new(ErrorCode::UnexpectedThreadBinding, "Provider changed the native thread ID; inspect the pane before retrying")); }
    authorize(app, caller)?;
    let turn = if let Some(message) = input.message {
        let command: SendTurnCommandInput = serde_json::from_value(json!({
            "thread_id": thread, "text": message, "delivery":"queue", "model_override":null,
            "permission_mode_override": if plan_mode {Some("plan")} else {None},
            "client_nonce": input.client_request_id,
        })).map_err(|e| ControlError::new(ErrorCode::InvalidArguments, e.to_string()))?;
        let mut context = super::delivery::SendContext::controller(caller);
        context.control = Some(control);
        Some(agent_chat::send_turn_with_context(app.clone(), input.provider, command, context).await?)
    } else { None };
    Ok(json!({"workspace_id":input.workspace_id,"thread_id":thread,"pane_id":pane,"turn":turn,"selection":input.resolved}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OperationStatusArgs { operation_id: String }

pub(super) fn operation_status<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, args: Value) -> Result<Value, ControlError> {
    let args: OperationStatusArgs = parse(args)?;
    identifier(&args.operation_id, "operation_id", 256)?;
    let epoch = app.state::<NativeControlState>().epoch.clone();
    let receipt = app.state::<DatabaseStore>().control_operation(&caller.principal, &args.operation_id, &epoch)?
        .ok_or_else(|| ControlError::new(ErrorCode::OperationNotFound, "No operation belongs to this client and ID"))?;
    serde_json::to_value(receipt).map_err(|e| ControlError::new(ErrorCode::SerializationFailed,e.to_string()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ThreadListArgs { workspace_id: String, cursor: Option<String>, limit: Option<u32> }

pub(super) fn thread_list<R: Runtime>(app: &AppHandle<R>, args: Value) -> Result<Value, ControlError> {
    let args: ThreadListArgs = parse(args)?;
    workspace(app, &args.workspace_id, false)?;
    let limit = args.limit.unwrap_or(20);
    if !(1..=100).contains(&limit) { return Err(ControlError::new(ErrorCode::InvalidArguments, "limit must be between 1 and 100")); }
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ThreadReadArgs { workspace_id: String, thread_id: String, cursor: Option<i64>, limit: Option<u32> }

pub(super) fn thread_read<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, args: Value) -> Result<Value, ControlError> {
    let args: ThreadReadArgs = parse(args)?;
    if caller.thread_id().is_none() { workspace(app, &args.workspace_id, false)?; }
    identifier(&args.thread_id, "thread_id", 256)?;
    let limit = args.limit.unwrap_or(20);
    if !(1..=100).contains(&limit) || args.cursor.is_some_and(|cursor| cursor < 0) {
        return Err(ControlError::new(ErrorCode::InvalidArguments, "limit must be 1–100 and cursor must be nonnegative"));
    }
    let db = app.state::<DatabaseStore>();
    let record = db.get_agent_chat_session(&args.thread_id)
        .ok_or_else(|| ControlError::new(ErrorCode::ThreadNotFound, "No native thread has this ID"))?;
    if record.workspace_id != args.workspace_id {
        return Err(ControlError::new(ErrorCode::ThreadOutsideWorkspace, "This thread does not belong to the selected workspace"));
    }
    let page = db.read_agent_chat_history_page(&args.workspace_id, &args.thread_id, args.cursor, limit)?;
    serde_json::to_value(page).map_err(|e| ControlError::new(ErrorCode::SerializationFailed, e.to_string()))
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Target { pub workspace_id: String, pub thread_id: String }

pub(crate) fn resolve_thread<R: Runtime>(app: &AppHandle<R>, target: &Target, mutation: bool) -> Result<(crate::database::AgentChatSessionRecord, ProviderKind), ControlError> {
    workspace(app, &target.workspace_id, mutation)?;
    identifier(&target.thread_id, "thread_id", 256)?;
    let record = app.state::<DatabaseStore>().get_agent_chat_session(&target.thread_id)
        .ok_or_else(|| ControlError::new(ErrorCode::ThreadNotFound, "No native thread has this ID"))?;
    if record.workspace_id != target.workspace_id { return Err(ControlError::new(ErrorCode::ThreadOutsideWorkspace, "This thread does not belong to the selected workspace")); }
    let provider: ProviderKind = serde_json::from_value(json!(record.provider))
        .map_err(|_| ControlError::new(ErrorCode::UnsupportedProvider, "This persisted thread has an unsupported provider"))?;
    if mutation {
        if record.imported_from.is_some() { return Err(ControlError::new(ErrorCode::ImportedSnapshotReadOnly, "Imported threads are read-only snapshots")); }
        let state = app.state::<AppStateStore>();
        let pane = state.agent_chat_pane_id_for_thread(&record.thread_id)
            .ok_or_else(|| ControlError::new(ErrorCode::ThreadNotBound, "Reopen this thread in CodeMux before controlling it"))?;
        if state.workspace_id_for_pane(&pane).as_deref() != Some(target.workspace_id.as_str())
            || state.agent_chat_pane_thread(&pane) != Some((provider,target.thread_id.clone())) {
            return Err(ControlError::new(ErrorCode::StaleThreadBinding, "The pane no longer belongs to this workspace/provider/thread"));
        }
    }
    Ok((record, provider))
}

pub(super) async fn thread_status<R: Runtime>(app: &AppHandle<R>, args: Value) -> Result<Value, ControlError> {
    let target: Target = parse(args)?;
    let (record, provider) = resolve_thread(app, &target, false)?;
    let implementation = app.state::<ProviderRegistry>().get(provider).await;
    let runtime_live = match &implementation { Some(provider) => provider.has_session(&ThreadId(target.thread_id.clone())).await, None => false };
    let active = if runtime_live {
        agent_chat::agent_chat_turn_active(app.clone(), provider, ThreadId(target.thread_id.clone())).await?
    } else { false };
    let latest = app.state::<DatabaseStore>().control_last_thread_run_event(&target.thread_id)?;
    // The database resolves matching completion within the latest logical run.
    let terminal = latest.as_ref().filter(|event| event["type"] == "turn_completed");
    let observed = app.state::<NativeControlState>().thread_runtime(&target.thread_id);
    let pending: Vec<_> = observed.as_ref().map(|view|view.pending_approvals.iter().map(|request|
        json!({"request_id":request.outside_handle,"turn_id":request.turn_id,"request_kind":request.request_kind})
    ).collect()).unwrap_or_default();
    let inactive = app.state::<DatabaseStore>().inactive_delivery_queue_ids(&target.thread_id)?;
    let queued: Vec<_> = observed.as_ref().map(|view|view.queued_ids.clone()).unwrap_or_default().into_iter().filter(|id| !inactive.contains(id)).collect();
    let uncertain_queued = app.state::<DatabaseStore>().control_unresolved_queued_ids(&target.thread_id)?
        .into_iter().filter(|id| !inactive.contains(id) && (!runtime_live || !queued.contains(id))).collect::<Vec<_>>();
    let deliveries: Vec<_> = app.state::<DatabaseStore>().thread_message_delivery_attention(&target.thread_id)?.into_iter().map(|row|json!({
        "delivery_id":row.id,"message_id":row.message_id,"state":row.state,"reason":row.error,
        "display_text":row.payload["input"]["display_text"].as_str().or_else(||row.payload["input"]["text"].as_str()),
    })).collect();
    let inflight = app.state::<DatabaseStore>().has_inflight_message_deliveries(&target.thread_id)?;
    let busy = inflight || active || !pending.is_empty() || !queued.is_empty() || !uncertain_queued.is_empty()
        || observed.as_ref().is_some_and(|view|matches!(view.phase.as_str(),"starting"|"running"|"waiting_approval"));
    let historical_error = !active && observed.as_ref().is_none_or(|view| view.phase == "unknown")
        && terminal.is_some_and(|event| super::state::turn_outcome(event["status"]["kind"].as_str(), event["status"]["subtype"].as_str()).0 == "error");
    let phase = if !pending.is_empty() { "waiting_approval" }
    else if let Some(view) = observed.as_ref().filter(|view| matches!(view.phase.as_str(), "closed" | "error")) { view.phase.as_str() }
    else if historical_error { "error" }
    else if !deliveries.is_empty() { "needs_attention" }
    else if !uncertain_queued.is_empty() && !active && queued.is_empty() { "needs_attention" }
    else if let Some(view) = observed.as_ref().filter(|view|view.phase != "unknown") { view.phase.as_str() }
    else if active { "running" } else if let Some(event) = terminal {
        super::state::turn_outcome(event["status"]["kind"].as_str(), event["status"]["subtype"].as_str()).0
    } else if runtime_live { "ready" } else { "unknown" };
    let last_turn = observed.as_ref().and_then(|view|view.last_turn.clone()).or_else(||terminal.map(|event|
        json!({"type":"turn_completed","turn_id":event["turn_id"],"status":{"kind":super::state::turn_outcome(event["status"]["kind"].as_str(), event["status"]["subtype"].as_str()).1}})));
    let settled = !busy && matches!(phase,"completed"|"interrupted"|"error"|"closed"|"ready"|"needs_attention");
    // Historical identity is correlation only, never a restored live owner.
    let turn_id = observed.as_ref().and_then(|view|view.turn_id.clone()).or_else(||latest.as_ref().and_then(|event|event["turn_id"].as_str().filter(|turn| !turn.is_empty()).map(str::to_owned)));
    Ok(json!({
        "workspace_id":target.workspace_id,"thread_id":target.thread_id,"provider":provider,
        "pane_id":app.state::<AppStateStore>().agent_chat_pane_id_for_thread(&record.thread_id),
        "runtime_live":runtime_live,"phase":phase,"settled":settled,
        "turn_id":turn_id,"pending_approvals":pending,"queued_ids":queued,"uncertain_queued_ids":uncertain_queued,"last_turn":last_turn,
        "imported_snapshot":record.imported_from.is_some(),"deliveries_needing_attention":deliveries,
    }))
}

#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ThreadSendArgs {
    workspace_id: String,
    thread_id: String,
    client_request_id: String,
    message: String,
    /// Stays a plain value, so the re-serialized fingerprint says "auto"
    /// whether delivery was omitted or null.
    #[serde(default = "auto_delivery", deserialize_with = "null_as_auto")]
    delivery: crate::agent_provider::types::MessageDelivery,
}

fn auto_delivery() -> crate::agent_provider::types::MessageDelivery {
    crate::agent_provider::types::MessageDelivery::Auto
}

/// `MessageDelivery`'s own default is queue, so a null delivery needs the
/// argument default instead of [`null_as_default`].
fn null_as_auto<'de, D: serde::Deserializer<'de>>(value: D) -> Result<crate::agent_provider::types::MessageDelivery, D::Error> {
    Option::deserialize(value).map(|delivery| delivery.unwrap_or_else(auto_delivery))
}

pub(super) async fn thread_send<R: Runtime>(
    app: &AppHandle<R>,
    caller: &ControlCaller,
    args: Value,
) -> Result<Value, ControlError> {
    let input: ThreadSendArgs = parse(args)?;
    let args = serde_json::to_value(&input)
        .map_err(|e| ControlError::new(ErrorCode::InvalidArguments, e.to_string()))?;
    identifier(&input.client_request_id, "client_request_id", 128)?;
    if input.message.trim().is_empty() || input.message.len() > 65536 {
        return Err(ControlError::new(
            ErrorCode::InvalidArguments,
            "message must contain text and fit in 65536 bytes",
        ));
    }
    if let Some(receipt) = retry_receipt(app, caller, "thread_send", &args)? {
        return Ok(receipt);
    }
    let target = Target {
        workspace_id: input.workspace_id.clone(),
        thread_id: input.thread_id.clone(),
    };
    authorize(app, caller)?;
    let (record, provider) = resolve_thread(app, &target, true)?;
    if record.imported_from.is_some() {
        return Err(ControlError::new(
            ErrorCode::ImportedSnapshotReadOnly,
            "Imported threads are read-only snapshots",
        ));
    }
    permission_ceiling(caller, provider, record.permission_mode.as_deref())?;
    let id = uuid::Uuid::new_v4().to_string();
    let mut context = super::delivery::SendContext::controller(caller);
    // A sending chat is titled after itself; outside clients keep their label.
    if let Some(title) = caller.thread_id().and_then(|thread| app.state::<DatabaseStore>().get_agent_chat_title(thread)) {
        context.provenance.sender_title = Some(title);
    }
    context.provenance.message_id = Some(id.clone());
    let control = NativeControlGuard { caller:caller.clone(),workspace_id:input.workspace_id.clone(),thread_id:input.thread_id.clone(),provider,admission:None }.retain_admission(app);
    let command: SendTurnCommandInput = serde_json::from_value(json!({
        "thread_id":input.thread_id,"text":input.message,"delivery":input.delivery,
        "model_override":null,"client_nonce":input.client_request_id,
    }))
    .map_err(|e| ControlError::new(ErrorCode::InvalidArguments, e.to_string()))?;
    let payload = serde_json::to_value(super::delivery::DeliveryPayload {
        provider,
        input: command,
        origin: context.origin,
        provenance: context.provenance,
        caller: Some(caller.clone()),
        pr_watch: None,
    })
    .map_err(|e| ControlError::new(ErrorCode::InvalidArguments, e.to_string()))?;
    let hash = fingerprint(&args)?;
    let epoch = app.state::<NativeControlState>().epoch.clone();
    let db = app.state::<DatabaseStore>();
    let generation = db.delivery_generation(&input.thread_id)?.0;
    let (id, new) = caller.admit(|| db.admit_message_delivery(
            &id,
            &input.thread_id,
            &id,
            &payload,
            &epoch,
            generation,
            Some(crate::database::message_delivery::DeliveryOperation {
                principal: &caller.principal,
                key: &input.client_request_id,
                hash: &hash,
                workspace: &input.workspace_id,
            }),
            true,
        ))
        .map_err(|e| ControlError::from_text(e, ErrorCode::OperationAdmissionFailed))?;
    let receipt = db
        .control_operation(&caller.principal, &id, &epoch)?
        .ok_or_else(|| ControlError::new(ErrorCode::OperationNotFound, "Missing admitted receipt"))?;
    if new {
        let owned = app.clone();
        let row = db
            .message_delivery(&id)?
            .ok_or_else(|| ControlError::new(ErrorCode::DeliveryNotFound, "Missing admitted delivery"))?;
        #[cfg(test)]
        let barrier = app
            .state::<NativeControlState>()
            .fixture_operation_barrier
            .lock()
            .unwrap()
            .take();
        tauri::async_runtime::spawn(async move {
            #[cfg(test)]
            if let Some(barrier) = barrier {
                barrier.acquire().await.unwrap().forget();
            }
            super::delivery::dispatch_admitted(&owned, row, Some(control)).await;
        });
        super::delivery::notify();
    }
    let mut result=serde_json::to_value(receipt).map_err(|e|ControlError::new(ErrorCode::SerializationFailed,e.to_string()))?;
    if let Some(task)=db.delegation_task(&format!("local:followup:{id}"))? {result["task_id"]=json!(task.task_id);}
    Ok(result)
}

fn retry_receipt<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, tool: &str, args: &Value) -> Result<Option<Value>, ControlError> {
    authorize(app, caller)?;
    for field in ["workspace_id", "thread_id"] {
        if let Some(value) = args.get(field) {
            identifier(value.as_str().ok_or_else(|| ControlError::new(ErrorCode::InvalidArguments, field))?, field, 256)?;
        }
    }
    let key = args["client_request_id"].as_str().ok_or_else(|| ControlError::new(ErrorCode::InvalidArguments, "client_request_id is required"))?;
    identifier(key, "client_request_id", 128)?;
    let hash = fingerprint(args)?;
    app.state::<DatabaseStore>().control_operation_by_key(&caller.principal, key, tool, &hash, &app.state::<NativeControlState>().epoch)
        .map_err(|e| ControlError::from_text(e, ErrorCode::OperationLookupFailed))?
        .map(serde_json::to_value).transpose().map_err(|e| ControlError::new(ErrorCode::SerializationFailed, e.to_string()))
}

/// A keyed call's arguments, hashed: a retry must send the same ones.
fn fingerprint(args: &Value) -> Result<String, ControlError> {
    let bytes = serde_json::to_vec(args).map_err(|e| ControlError::new(ErrorCode::InvalidArguments, e.to_string()))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn keyed(args: &Value) -> bool {
    args.get("client_request_id").is_some_and(|key| !key.is_null())
}

/// What a retry of a keyed call that was already admitted returns instead of
/// running again; `None` for an unkeyed or new call. Handlers whose checks
/// read state the first call changed (a closed workspace, an answered
/// question) ask this before checking.
pub(crate) fn recorded<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, tool: &str, args: &Value) -> Result<Option<Value>, ControlError> {
    if !keyed(args) {
        return Ok(None);
    }
    retry_receipt(app, caller, tool, args)?.map(replay).transpose()
}

/// A mutation that finishes within the call, optionally under a durable
/// receipt. Without client_request_id the work runs with no receipt. With one, a
/// retry returns the recorded outcome instead of repeating side effects, and
/// the receipt names its workspace from admission on: `target_workspace`, or
/// with `reserve_workspace` a new id, reserved before the workspace exists and
/// handed to `work`. A successful result's `workspace_id` replaces that name
/// (an open folder reused, a checkout adopted).
///
/// The work runs in its own task, which also records a keyed call's outcome.
/// A caller that goes away (an HTTP client disconnecting, a proxy timing out)
/// only stops waiting: the work is never cut off halfway, and its receipt
/// still settles.
pub(crate) async fn run_receipted<R, F, Fut>(
    app: &AppHandle<R>,
    caller: &ControlCaller,
    tool: &'static str,
    args: &Value,
    reserve_workspace: bool,
    target_workspace: Option<&str>,
    work: F,
) -> Result<Value, ControlError>
where
    R: Runtime,
    F: FnOnce(Option<WorkspaceId>) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<Value, ControlError>> + Send + 'static,
{
    if !keyed(args) {
        return detached(work(None)).await;
    }
    if let Some(receipt) = retry_receipt(app, caller, tool, args)? {
        return replay(receipt);
    }
    let key = args["client_request_id"].as_str().unwrap_or_default();
    let reserved = reserve_workspace.then(AppStateStore::reserve_workspace_id);
    let hash = fingerprint(args)?;
    let epoch = app.state::<NativeControlState>().epoch.clone();
    let db = app.state::<DatabaseStore>();
    let admitted = caller
        .admit(|| db.admit_control_operation_for_thread(
            &caller.principal, key, tool, &hash, &epoch,
            reserved.as_ref().map(|id| id.0.as_str()).or(target_workspace),
            args["thread_id"].as_str(), caller.thread_id().zip(caller.admission_generation()),
        ))
        .map_err(|e| ControlError::from_text(e, ErrorCode::OperationAdmissionFailed))?;
    let operation = admitted.receipt.operation_id.clone();
    if !admitted.newly_admitted {
        // A concurrent duplicate won the admission.
        return replay(serde_json::to_value(&admitted.receipt).map_err(|e| ControlError::new(ErrorCode::SerializationFailed, e.to_string()))?);
    }
    db.update_control_operation(&caller.principal, &operation, &epoch, "running", None, None, None, None)
        .map_err(|e| ControlError::new(ErrorCode::OperationAdmissionFailed, e))?;
    let (owned, principal, id) = (app.clone(), caller.principal.clone(), operation.clone());
    #[cfg(test)]
    let barrier = app.state::<NativeControlState>().fixture_operation_barrier.lock().unwrap().take();
    let mut value = detached(async move {
        #[cfg(test)]
        if let Some(barrier) = barrier {
            barrier.acquire().await.unwrap().forget();
        }
        let outcome = work(reserved).await;
        let db = owned.state::<DatabaseStore>();
        let saved = match &outcome {
            Ok(value) => db.update_control_operation(
                &principal, &id, &epoch, "succeeded",
                value["workspace_id"].as_str(), value["thread_id"].as_str(), Some(value), None,
            ),
            Err(error) => db.update_control_operation(
                &principal, &id, &epoch, "failed", None, None, None, serde_json::to_value(error).ok().as_ref(),
            ),
        };
        if let Err(error) = saved {
            eprintln!("[codemux::agent_control] could not save operation outcome: {error}");
        }
        outcome
    })
    .await?;
    if !value.is_object() {
        value = json!({"result": value});
    }
    value["operation_id"] = json!(operation);
    value["state"] = json!("succeeded");
    Ok(value)
}

/// Runs `work` in a task of its own: dropping the caller's future leaves it
/// running to the end.
async fn detached<Fut>(work: Fut) -> Result<Value, ControlError>
where
    Fut: std::future::Future<Output = Result<Value, ControlError>> + Send + 'static,
{
    tauri::async_runtime::spawn(work)
        .await
        .map_err(|error| ControlError::new(ErrorCode::NativeControlFailed, format!("Worker failed: {error}")))?
}

/// What a retry of a synchronous keyed call returns: the recorded result once
/// it succeeded, else the receipt itself, as every keyed tool's retry does. A
/// failed one carries the first call's `error {code, message, retryable}`;
/// an unfinished one says where it stands, so the client polls
/// operation_status.
fn replay(receipt: Value) -> Result<Value, ControlError> {
    if receipt["state"] != "succeeded" {
        return Ok(receipt);
    }
    let mut value = match receipt["result"].clone() {
        value @ Value::Object(_) => value,
        value => json!({"result": value}),
    };
    value["operation_id"] = receipt["operation_id"].clone();
    value["state"] = json!("succeeded");
    value["replayed"] = json!(true);
    Ok(value)
}

fn admit_mutation<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, tool: &str, args: &Value, target: &Target) -> Result<crate::database::agent_control::Admission, ControlError> {
    authorize(app,caller)?;
    let key = args["client_request_id"].as_str().ok_or_else(||ControlError::new(ErrorCode::InvalidArguments,"client_request_id is required"))?;
    identifier(key,"client_request_id",128)?;
    let hash = fingerprint(args)?;
    let epoch = app.state::<NativeControlState>().epoch.clone();
    caller.admit(||app.state::<DatabaseStore>().admit_control_operation_for_thread(&caller.principal,key,tool,&hash,&epoch,Some(&target.workspace_id),Some(&target.thread_id),caller.thread_id().zip(caller.admission_generation())))
        .map_err(|e|ControlError::from_text(e, ErrorCode::OperationAdmissionFailed))
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

/// Why chats of `kind` cannot use Codemux's collaboration tools, when known
/// without starting anything: OpenCode 1 never names the calling session.
fn collaboration_unavailable<R: Runtime>(app: &AppHandle<R>, kind: ProviderKind) -> Option<&'static str> {
    let manager = app.try_state::<std::sync::Arc<crate::agent_provider::opencode::OpenCodeServerManager>>()?;
    (kind == ProviderKind::OpenCode
        && manager.cached_catalog()?.0 == crate::agent_provider::opencode::api::OpenCodeApi::Legacy)
        .then_some(crate::agent_provider::opencode::session::LEGACY_COLLABORATION)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CapabilitiesArgs { workspace_id: String, provider: Option<ProviderKind> }

pub(super) async fn agent_capabilities<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, args: Value) -> Result<Value, ControlError> {
    let args: CapabilitiesArgs = parse(args)?;
    workspace(app,&args.workspace_id,false)?;
    let registry = app.state::<ProviderRegistry>();
    let registered = match args.provider {
        Some(kind) => vec![(kind,registry.get(kind).await.ok_or_else(||ControlError::new(ErrorCode::ProviderNotConfigured,"The selected provider is not configured in CodeMux"))?)],
        None => registry.all().await,
    };
    let mut providers = Vec::new();
    let mut pending = Vec::new();
    let db = app.state::<DatabaseStore>();
    let selected_workspace = workspace(app, &args.workspace_id, false)?;
    for (kind,implementation) in registered {
        let operations = implementation.capabilities();
        let instances: Vec<String> = match kind {
            ProviderKind::Acp => db
                .acp_agents()?
                .into_iter()
                .filter(|a| a.enabled)
                .map(|a| format!("acp:{}", a.id))
                .collect(),
            ProviderKind::Hermes => match super::selection::profiles(&db) {
                Ok(profiles) => profiles
                    .into_iter()
                    .map(|p| format!("hermes:{}", p.id))
                    .collect(),
                Err(error) => {
                    providers.push(json!({"provider":kind,"configured":false,"discovery_error":error,"allowed_permission_modes":[]}));
                    continue;
                }
            },
            provider if crate::provider_accounts::supports(provider) => {
                std::iter::once(crate::commands::delegation::provider_id(provider).to_string())
                    .chain(
                        db.provider_accounts()?
                            .into_iter()
                            .filter(|a| {
                                a.provider == crate::commands::delegation::provider_id(provider)
                            })
                            .map(|a| format!("{}:{}", a.provider, a.id)),
                    )
                    .collect()
            }
            provider => vec![crate::commands::delegation::provider_id(provider).into()],
        };
        pending.extend(instances.into_iter().map(|instance| (kind, operations.clone(), instance)));
    }
    // Listing every provider reads cached composer catalogs only, so it
    // never spawns an agent or opens a provider session, and instances
    // resolve concurrently. Naming one provider also reads a catalog nothing
    // has cached yet (`Discovery::Catalog`): an outside agent has no other way
    // to learn an exact model id before its first launch.
    let discovery = if args.provider.is_some() { super::selection::Discovery::Catalog } else { super::selection::Discovery::Cached };
    let resolved = futures_util::future::join_all(pending.iter().map(|(_, _, instance)| {
        let selection = super::selection::ProviderSelection { instance_id: instance.clone(), ..Default::default() };
        super::selection::resolve(app, selection, &selected_workspace.cwd, discovery)
    }))
    .await;
    for ((kind, operations, instance), outcome) in pending.into_iter().zip(resolved) {
        match outcome {
            Ok((resolved,capabilities,options)) => {
                let allowed_modes: Vec<_> = capabilities.permission_modes.iter()
                    .filter(|mode|permission_ceiling(caller,kind,Some(&mode.value)).is_ok())
                    .map(|m|m.value.clone()).collect();
                providers.push(json!({"provider":kind,"provider_instance_id":instance,"selection":resolved,"configured":true,"capabilities":capabilities,"options":options,"operations":operations,"allowed_permission_modes":allowed_modes,"discovery_error":null,"collaboration_unavailable":collaboration_unavailable(app,kind)}));
            }
            Err(error) => providers.push(json!({"provider":kind,"provider_instance_id":instance,"configured":true,"capabilities":null,"operations":operations,"allowed_permission_modes":[],"discovery_error":error})),
        }
    }
    authorize(app,caller)?;
    Ok(json!({"workspace_id":args.workspace_id,"access":caller.access,"providers":providers}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct InterruptArgs { workspace_id: String, thread_id: String, client_request_id: String, turn_id: Option<String> }

pub(super) async fn thread_interrupt<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, args: Value) -> Result<Value, ControlError> {
    let input: InterruptArgs = parse(args.clone())?;
    identifier(&input.client_request_id,"client_request_id",128)?;
    if let Some(turn) = &input.turn_id { identifier(turn,"turn_id",256)?; }
    if let Some(receipt) = retry_receipt(app, caller, "thread_interrupt", &args)? { return Ok(receipt); }
    let target = Target { workspace_id:input.workspace_id,thread_id:input.thread_id };
    let (record, provider) = resolve_thread(app,&target,true)?;
    permission_ceiling(caller,provider,record.permission_mode.as_deref())?;
    let implementation = app.state::<ProviderRegistry>().get(provider).await
        .ok_or_else(||ControlError::new(ErrorCode::ProviderNotConfigured,"The selected provider is not configured in CodeMux"))?;
    if !implementation.capabilities().supports_interrupt { return Err(ControlError::new(ErrorCode::UnsupportedOperation,"This provider does not support interruption")); }
    // Capture the selected run at admission. A delayed operation must not
    // interrupt a newer run that started while its receipt was pending.
    let mut selected = input.turn_id.clone().or_else(|| app.state::<NativeControlState>().thread_runtime(&target.thread_id).and_then(|view| view.turn_id));
    if selected.is_none() {
        selected = implementation.list_sessions().await.map_err(|e|ControlError::from(e.to_string()))?.into_iter()
            .find(|session|session.thread_id.0 == target.thread_id)
            .and_then(|session| match session.status {crate::agent_provider::SessionStatus::Running {active_turn} => Some(active_turn.0), _ => None});
    }
    let admission = admit_mutation(app,caller,"thread_interrupt",&args,&target)?;
    let receipt = serde_json::to_value(&admission.receipt).map_err(|e|ControlError::new(ErrorCode::SerializationFailed,e.to_string()))?;
    if !admission.newly_admitted { return Ok(receipt); }
    let owned_app = app.clone();
    let owned_caller = caller.clone();
    spawn_operation(app,caller,admission.receipt,async move {
        authorize(&owned_app,&owned_caller)?;
        let (record,current_provider) = resolve_thread(&owned_app,&target,true)?;
        if provider != current_provider { return Err(ControlError::new(ErrorCode::StaleThreadBinding,"The thread changed provider after admission")); }
        permission_ceiling(&owned_caller,provider,record.permission_mode.as_deref())?;
        let control = NativeControlGuard { caller:owned_caller.clone(),workspace_id:target.workspace_id.clone(),thread_id:target.thread_id.clone(),provider,admission:None };
        let reached = if let Some(selected) = selected { agent_chat::agent_chat_interrupt_turn_guarded(owned_app.clone(),provider,ThreadId(target.thread_id.clone()),Some(crate::agent_provider::TurnId(selected)),Some(control)).await? } else { false };
        Ok(json!({"workspace_id":target.workspace_id,"thread_id":target.thread_id,"reached_provider":reached,"requested_turn_id":input.turn_id}))
    });
    Ok(receipt)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StopArgs { workspace_id: String, thread_id: String, client_request_id: String }

pub(super) async fn thread_stop<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, args: Value) -> Result<Value, ControlError> {
    let input: StopArgs = parse(args.clone())?;
    identifier(&input.client_request_id,"client_request_id",128)?;
    if let Some(receipt) = retry_receipt(app, caller, "thread_stop", &args)? { return Ok(receipt); }
    let target = Target { workspace_id:input.workspace_id,thread_id:input.thread_id };
    let (record, provider) = resolve_thread(app,&target,true)?;
    permission_ceiling(caller,provider,record.permission_mode.as_deref())?;
    let admission = admit_mutation(app,caller,"thread_stop",&args,&target)?;
    let receipt = serde_json::to_value(&admission.receipt).map_err(|e|ControlError::new(ErrorCode::SerializationFailed,e.to_string()))?;
    if !admission.newly_admitted { return Ok(receipt); }
    let owned_app = app.clone();
    let owned_caller = caller.clone();
    spawn_operation(app,caller,admission.receipt,async move {
        authorize(&owned_app,&owned_caller)?;
        let (record,current_provider) = resolve_thread(&owned_app,&target,true)?;
        if provider != current_provider { return Err(ControlError::new(ErrorCode::StaleThreadBinding,"The thread changed provider after admission")); }
        permission_ceiling(&owned_caller,provider,record.permission_mode.as_deref())?;
        let control = NativeControlGuard { caller:owned_caller.clone(),workspace_id:target.workspace_id.clone(),thread_id:target.thread_id.clone(),provider,admission:None };
        // The controller's explicit Stop is durable. Session restart and
        // provider replacement use the session owner without this Stop fence.
        owned_app.state::<DatabaseStore>().stop_message_deliveries(&target.thread_id)?;
        agent_chat::agent_chat_stop_session_guarded(owned_app.clone(),provider,ThreadId(target.thread_id.clone()),Some(control)).await?;
        Ok(json!({"workspace_id":target.workspace_id,"thread_id":target.thread_id,"stopped":true}))
    });
    Ok(receipt)
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Decision { AllowOnce, AllowAlways, Deny }

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RespondArgs { workspace_id: String, thread_id: String, client_request_id: String, request_id: String, decision: Decision }

pub(super) async fn thread_respond<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, args: Value) -> Result<Value, ControlError> {
    if caller.thread_id().is_some() { return Err(ControlError::new(ErrorCode::PermissionDenied,"Agents cannot grant permission approvals")); }
    let input: RespondArgs = parse(args.clone())?;
    // "Always" outlives the run a connector client is watching, so only the
    // person on this computer grants it.
    if caller.kind() == CallerKind::Client && matches!(input.decision, Decision::AllowAlways) {
        return Err(ControlError::new(ErrorCode::PermissionDenied, "Connector clients can allow a request once or deny it; allow_always is only available on this computer"));
    }
    identifier(&input.client_request_id,"client_request_id",128)?;
    identifier(&input.request_id,"request_id",256)?;
    if let Some(receipt) = retry_receipt(app, caller, "thread_respond", &args)? { return Ok(receipt); }
    let target = Target { workspace_id:input.workspace_id,thread_id:input.thread_id };
    let (record,provider) = resolve_thread(app,&target,true)?;
    permission_ceiling(caller,provider,record.permission_mode.as_deref())?;
    if caller.access != ControlAccess::FullAccess && !matches!(input.decision,Decision::Deny) {
        return Err(ControlError::new(ErrorCode::PermissionCeiling,"Supervised clients cannot approve worker requests"));
    }
    let decision = match input.decision {
        Decision::AllowOnce => crate::agent_provider::ApprovalDecision::Allow { updated_input:None,updated_permissions:None },
        Decision::AllowAlways => crate::agent_provider::ApprovalDecision::AllowForSession,
        Decision::Deny => crate::agent_provider::ApprovalDecision::Deny { message:"Denied by the outside assistant".into() },
    };
    let admission = admit_mutation(app,caller,"thread_respond",&args,&target)?;
    let receipt = serde_json::to_value(&admission.receipt).map_err(|e|ControlError::new(ErrorCode::SerializationFailed,e.to_string()))?;
    if !admission.newly_admitted { return Ok(receipt); }
    let owned_app = app.clone();
    let control = NativeControlGuard { caller:caller.clone(),workspace_id:target.workspace_id.clone(),thread_id:target.thread_id.clone(),provider,admission:None };
    spawn_operation(app,caller,admission.receipt,async move {
        let provider_request = owned_app.state::<NativeControlState>()
            .provider_request_for_handle(&target.thread_id, &input.request_id)
            .ok_or_else(|| ControlError::new(ErrorCode::StaleProviderCallback, "No current-process callback matches this outside handle"))?;
        agent_chat::agent_chat_respond_to_request_guarded(owned_app,provider,ThreadId(target.thread_id.clone()),
            crate::agent_provider::RequestId(provider_request),decision,Some(control)).await?;
        Ok(json!({"workspace_id":target.workspace_id,"thread_id":target.thread_id,"request_id":input.request_id,"delivered":true}))
    });
    Ok(receipt)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AnswerArgs { workspace_id: String, thread_id: String, client_request_id: String, question_id: String, answers: Vec<String> }

/// Answers a worker's open question for the person, through the same path as
/// the question card. The answer never carries a permission-mode change: on
/// the card the person can approve a plan and leave plan mode in one go, but
/// an outside answer leaves the thread's mode as it is.
pub(super) async fn thread_answer<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, args: Value) -> Result<Value, ControlError> {
    use crate::agent_provider::QuestionResolution;
    use crate::commands::async_questions::{agent_chat_answer_question, QuestionAction};
    if caller.thread_id().is_some() { return Err(ControlError::new(ErrorCode::PermissionDenied, "Agents cannot answer questions meant for the person")); }
    let input: AnswerArgs = parse(args.clone())?;
    identifier(&input.client_request_id, "client_request_id", 128)?;
    identifier(&input.question_id, "question_id", 256)?;
    if let Some(done) = recorded(app, caller, "thread_answer", &args)? { return Ok(done); }
    authorize(app, caller)?;
    let target = Target { workspace_id: input.workspace_id.clone(), thread_id: input.thread_id.clone() };
    let (record, provider) = resolve_thread(app, &target, true)?;
    permission_ceiling(caller, provider, record.permission_mode.as_deref())?;
    let (question, resolution) = app.state::<DatabaseStore>().async_question(&target.thread_id, &input.question_id)
        .map_err(|_| ControlError::new(ErrorCode::QuestionNotFound, "No question has this ID in the thread"))?;
    match resolution {
        QuestionResolution::Pending | QuestionResolution::Failed { .. } => {}
        // Still listed in pending.questions, so not "closed".
        QuestionResolution::Submitting { .. } | QuestionResolution::Unknown { .. } => return Err(answer_unsettled()),
        QuestionResolution::Answered { .. } | QuestionResolution::Dismissed => {
            return Err(ControlError::new(ErrorCode::QuestionClosed, "This question was already answered or dismissed"));
        }
    }
    question.answer_text(&input.answers).map_err(|e| ControlError::new(ErrorCode::InvalidArguments, e))?;
    // The provider dedupes on the submission id, so a key keeps one.
    let submission = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, json!(["codemux:thread_answer", caller.principal, input.client_request_id]).to_string().as_bytes()).to_string();
    let AnswerArgs { workspace_id, thread_id, question_id, answers, .. } = input;
    let receipt_workspace = workspace_id.clone();
    let owned = app.clone();
    run_receipted(app, caller, "thread_answer", &args, false, Some(&receipt_workspace), move |_| async move {
        let action = QuestionAction::Answer { answers, submission_id: submission.clone(), retry_unknown: false, permission_mode_override: None };
        let resolution = agent_chat_answer_question(owned, ThreadId(thread_id.clone()), question_id.clone(), action)
            .await
            .map_err(|e| ControlError::new(ErrorCode::QuestionAnswerFailed, e))?;
        match &resolution {
            QuestionResolution::Answered { submission_id, delivery, .. } if *submission_id == submission => {
                let turn = match delivery {
                    crate::agent_provider::QuestionDelivery::Inflight { turn_id } | crate::agent_provider::QuestionDelivery::NewTurn { turn_id } => turn_id.0.clone(),
                };
                Ok(json!({"workspace_id": workspace_id, "thread_id": thread_id, "question_id": question_id, "turn_id": turn, "resolution": resolution}))
            }
            QuestionResolution::Failed { submission_id, message, .. } if *submission_id == submission => {
                Err(ControlError::new(ErrorCode::QuestionAnswerFailed, format!("The worker did not take the answer: {message}")))
            }
            // Sent, but the provider never confirmed it: answering again
            // could deliver it twice, so the person settles it on the card.
            QuestionResolution::Unknown { submission_id, .. } | QuestionResolution::Submitting { submission_id, .. } if *submission_id == submission => {
                Err(ControlError::new(ErrorCode::QuestionAnswerFailed, "The answer may have reached the worker, but its delivery was not confirmed. Do not answer again; the person can check the question in CodeMux"))
            }
            // Someone else's answer claimed it first and has not settled.
            QuestionResolution::Submitting { .. } | QuestionResolution::Unknown { .. } => Err(answer_unsettled()),
            _ => Err(ControlError::new(ErrorCode::QuestionClosed, "The question was answered or dismissed meanwhile")),
        }
    }).await
}

/// A question whose answer is being delivered, or was sent without the
/// provider confirming it, stays open, but a second answer could reach the
/// worker twice.
fn answer_unsettled() -> ControlError {
    ControlError::new(ErrorCode::QuestionAnswerFailed, "An answer to this question is being delivered or awaits confirmation; the person settles it in CodeMux")
}

#[cfg(test)]
mod replay_tests {
    use super::replay;
    use serde_json::json;

    /// A retry of a failed workspace or worktree call reads like a retry of
    /// any other keyed tool: the receipt, carrying the first call's own code.
    #[test]
    fn a_retry_returns_the_result_once_it_succeeded_and_the_receipt_otherwise() {
        let failed = json!({
            "operation_id": "op-1", "tool": "workspace_close", "state": "failed", "workspace_id": "ws-1",
            "thread_id": null, "result": null,
            "error": {"code": "workspace_close_refused", "message": "The worktree has uncommitted work", "retryable": false},
            "delivery_id": null, "dispatch_disposition": null, "created_at": "t0", "updated_at": "t1",
        });
        assert_eq!(replay(failed.clone()).unwrap(), failed);
        let running = json!({"operation_id": "op-2", "tool": "worktree_create", "state": "running", "workspace_id": "ws-2"});
        assert_eq!(replay(running.clone()).unwrap(), running);
        let closed = json!({"operation_id": "op-3", "state": "succeeded", "result": {"workspace_id": "ws-3", "closed": true}});
        assert_eq!(
            replay(closed).unwrap(),
            json!({"workspace_id": "ws-3", "closed": true, "operation_id": "op-3", "state": "succeeded", "replayed": true})
        );
        let scalar = json!({"operation_id": "op-4", "state": "succeeded", "result": 5});
        assert_eq!(replay(scalar).unwrap()["result"], 5);
    }
}
