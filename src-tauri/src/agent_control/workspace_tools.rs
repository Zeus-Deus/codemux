//! Typed workspace, worktree and project tools for outside callers: list,
//! info, open-or-create, worktree create, close, archive and restore, and the
//! projects this computer knows, listed or cloned.
//!
//! The registry's gate has already run (Read for list and info, Machine for the
//! rest). These handlers add the rules that depend on the arguments and on
//! where the caller is: connector clients reach this computer over the network,
//! so they only open or branch folders of projects already opened here, never
//! force a deletion and never check out the branch the project root has
//! checked out. A terminal preset needs Admin. Keyed calls run under
//! a durable receipt (`service::run_receipted`), and the lifecycle asks for
//! the caller's authority again wherever it waited ([`checkpoint`]).
use super::{registry, service, Access, ControlCaller, ControlError, ErrorCode};
use crate::workspace_service::{
    self as workspaces, Checkpoint, CloseWorkspace, CommitLoss, InfoError, OpenWorkspace,
    WorkspaceService, WorkspaceTarget, WorktreeArgs,
};
use crate::state::AppStateStore;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager, Runtime};

#[derive(Deserialize, Default, Clone, Copy, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Detail {
    #[default]
    Summary,
    Full,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ListArgs {
    #[serde(default, deserialize_with = "service::null_as_default")]
    detail: Detail,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct InfoArgs {
    workspace_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CreateArgs {
    path: Option<String>,
    #[serde(default, deserialize_with = "service::null_as_default")]
    select: bool,
    client_request_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CloseArgs {
    workspace_id: String,
    #[serde(default, deserialize_with = "service::null_as_default")]
    delete_worktree: bool,
    #[serde(default, deserialize_with = "service::null_as_default")]
    delete_branch: bool,
    #[serde(default, deserialize_with = "service::null_as_default")]
    force_delete: bool,
    client_request_id: Option<String>,
}

/// Every workspace open in Codemux: summaries, or with `detail: "full"` the
/// complete snapshots.
pub(super) fn list<R: Runtime>(app: &AppHandle<R>, args: Value) -> Result<Value, ControlError> {
    let ListArgs { detail } = service::parse(args)?;
    let workspaces = WorkspaceService::new(app);
    let rows = match detail {
        Detail::Summary => serde_json::to_value(workspaces.summaries()),
        Detail::Full => serde_json::to_value(workspaces.snapshots()),
    }
    .map_err(serialization)?;
    Ok(json!({"workspaces": rows}))
}

/// One workspace's full snapshot. Local callers may mean the focused one;
/// a connector client on another computer has no focus of its own.
pub(super) fn info<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, args: Value) -> Result<Value, ControlError> {
    let InfoArgs { workspace_id } = service::parse(args)?;
    let target = match workspace_id.as_deref() {
        Some(id) => {
            service::identifier(id, "workspace_id", 256)?;
            WorkspaceTarget::Id(id)
        }
        None if caller.is_network() => {
            return Err(ControlError::new(ErrorCode::InvalidArguments, "workspace_id is required: connector clients name the workspace they mean"));
        }
        None => WorkspaceTarget::Active,
    };
    let workspace = WorkspaceService::new(app).info(target).map_err(|error| match error {
        InfoError::NotFound => ControlError::new(ErrorCode::WorkspaceNotFound, "Select an existing workspace explicitly"),
        InfoError::NoActive => ControlError::new(ErrorCode::NoActiveWorkspace, "No workspace is focused in CodeMux; pass workspace_id"),
    })?;
    let mut value = serde_json::to_value(&workspace).map_err(serialization)?;
    value["native_control_available"] = json!(workspaces::native_control(&workspace).is_ok());
    Ok(value)
}

/// Opens a folder as a workspace, or returns the one already open there.
pub(super) async fn create<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, args: Value) -> Result<Value, ControlError> {
    let args = without_nulls(args);
    let CreateArgs { path, select, client_request_id } = service::parse(args.clone())?;
    if let Some(key) = &client_request_id {
        service::identifier(key, "client_request_id", 128)?;
    }
    // A retry replays even if the folder has since moved or gone.
    if let Some(done) = service::recorded(app, caller, "workspace_create", &args)? {
        return Ok(done);
    }
    let path = match path {
        Some(path) if caller.is_network() => Some(known_folder(app, path).await?),
        Some(path) => Some(local_folder(&path)?),
        None if caller.is_network() => {
            return Err(ControlError::new(ErrorCode::InvalidArguments, "path is required: connector clients name the folder to open"));
        }
        // The legacy default: the folder Codemux itself was started in.
        None => None,
    };
    let workspaces = WorkspaceService::new(app).checked(checkpoint(app, caller));
    service::run_receipted(app, caller, "workspace_create", &args, true, None, move |reserved| async move {
        let opened = workspaces
            .open(OpenWorkspace { path, select, reuse_open: true, reserved_id: reserved })
            .await
            .map_err(|error| ControlError::from_text(error, ErrorCode::WorkspaceCreateFailed))?;
        serde_json::to_value(opened).map_err(serialization)
    })
    .await
}

/// Creates a git worktree and its workspace, or adopts the open one that
/// already holds that checkout.
pub(super) async fn create_worktree<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, args: Value) -> Result<Value, ControlError> {
    let args = without_nulls(args);
    let mut input: WorktreeArgs = service::parse(args.clone())?;
    // A preset types its prompt into a terminal CLI that runs as the person,
    // outside any chat's permission mode. Without a preset the prompt is
    // dropped (`WorktreeArgs::into_request`) and nothing launches.
    if input.agent_preset_id.is_some() {
        registry::require(caller, Access::Admin)?;
    }
    if let Some(key) = &input.client_request_id {
        service::identifier(key, "client_request_id", 128)?;
    }
    workspaces::check_ref_text("branch", &input.branch).map_err(|error| git_error(error, ErrorCode::InvalidArguments))?;
    if let Some(base) = &input.base {
        workspaces::check_ref_text("base", base).map_err(|error| git_error(error, ErrorCode::InvalidArguments))?;
    }
    // A retry replays even if the root has since switched branches.
    if let Some(done) = service::recorded(app, caller, "worktree_create", &args)? {
        return Ok(done);
    }
    input.repo_path = if caller.is_network() {
        let folder = known_folder(app, input.repo_path.clone()).await?;
        let branch = input.branch.clone();
        let adopting = input.new_branch == Some(false);
        blocking(move || {
            let root = crate::config::workspace_config::find_git_root(Path::new(&folder))
                .ok_or_else(|| ControlError::new(ErrorCode::NotAGitRepository, format!("{folder} is not in a git repository")))?;
            // Checking it out elsewhere would switch the person's own
            // checkout away from it first.
            if adopting && crate::git::current_branch(&root).as_deref() == Some(branch.as_str()) {
                return Err(ControlError::new(ErrorCode::WorktreeConflict, "That branch is checked out in the project root; switch it there first"));
            }
            Ok(root.display().to_string())
        })
        .await?
    } else {
        local_folder(&input.repo_path)?
    };
    let workspaces = WorkspaceService::new(app).checked(checkpoint(app, caller));
    service::run_receipted(app, caller, "worktree_create", &args, true, None, move |reserved| async move {
        // Resolving the base may fetch from origin. A grant revoked meanwhile
        // fails the checkpoint the service asks before the checkout.
        let request = input.into_request(None, reserved).await.map_err(request_error)?;
        let created = workspaces
            .create_worktree(request)
            .await
            .map_err(|error| git_error(error, ErrorCode::WorktreeCreateFailed))?;
        Ok(json!({
            "workspace_id": created.workspace_id,
            "cwd": created.cwd,
            "adopted": created.adopted,
            "base_branch": created.base_branch,
        }))
    })
    .await
}

/// Closes a workspace through the desktop close dialog's guards.
pub(super) async fn close<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, args: Value) -> Result<Value, ControlError> {
    let args = without_nulls(args);
    let CloseArgs { workspace_id, delete_worktree, delete_branch, force_delete, client_request_id } = service::parse(args.clone())?;
    service::identifier(&workspace_id, "workspace_id", 256)?;
    if let Some(key) = &client_request_id {
        service::identifier(key, "client_request_id", 128)?;
    }
    // Forcing discards uncommitted or unpushed work; that stays with the
    // person at this computer.
    if force_delete && caller.is_network() {
        return Err(ControlError::new(ErrorCode::PermissionDenied, "force_delete is only available on this computer"));
    }
    // Unforced, a close loses no commit that nothing else keeps: not one a
    // detached HEAD made, nor one only the branch it deletes holds. A
    // worker's new branch has no upstream, so the unpushed-work check alone
    // would let them go.
    let commits = if force_delete { CommitLoss::Allow } else { CommitLoss::Refuse };
    // A retry after the close replays it instead of finding no workspace.
    if let Some(done) = service::recorded(app, caller, "workspace_close", &args)? {
        return Ok(done);
    }
    service::workspace(app, &workspace_id, false)?;
    let receipt_workspace = workspace_id.clone();
    let workspaces = WorkspaceService::new(app).checked(checkpoint(app, caller));
    service::run_receipted(app, caller, "workspace_close", &args, false, Some(&receipt_workspace), move |_| async move {
        let closed = workspaces
            .close(CloseWorkspace { workspace_id, remove_worktree: delete_worktree, delete_branch, force: force_delete, commits })
            .await
            // Verbatim: the text says why (uncommitted work, a shared
            // checkout). A withdrawn grant keeps its own code.
            .map_err(|error| ControlError::from_text(error, ErrorCode::WorkspaceCloseRefused))?;
        serde_json::to_value(closed).map_err(serialization)
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ArchiveArgs {
    workspace_id: String,
    client_request_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct UnarchiveArgs {
    archive_id: String,
    #[serde(default, deserialize_with = "service::null_as_default")]
    select: bool,
    client_request_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct NoArgs {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CloneArgs {
    url: String,
    parent: String,
    name: Option<String>,
    #[serde(default, deserialize_with = "service::null_as_default")]
    select: bool,
    client_request_id: Option<String>,
}

/// Archives a workspace: the close path, keeping every file, plus an entry
/// `workspace_unarchive` restores.
pub(super) async fn archive<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, args: Value) -> Result<Value, ControlError> {
    let args = without_nulls(args);
    let ArchiveArgs { workspace_id, client_request_id } = service::parse(args.clone())?;
    service::identifier(&workspace_id, "workspace_id", 256)?;
    if let Some(key) = &client_request_id {
        service::identifier(key, "client_request_id", 128)?;
    }
    // A retry after the archive replays it instead of finding no workspace.
    if let Some(done) = service::recorded(app, caller, "workspace_archive", &args)? {
        return Ok(done);
    }
    service::workspace(app, &workspace_id, false)?;
    let receipt_workspace = workspace_id.clone();
    let workspaces = WorkspaceService::new(app).checked(checkpoint(app, caller));
    service::run_receipted(app, caller, "workspace_archive", &args, false, Some(&receipt_workspace), move |_| async move {
        let archive_id = workspaces
            .archive(workspace_id.clone())
            .await
            .map_err(|error| ControlError::from_text(error, ErrorCode::WorkspaceArchiveFailed))?;
        Ok(json!({"workspace_id": workspace_id, "archive_id": archive_id, "archived": true}))
    })
    .await
}

/// Restores an archive entry, or returns the live workspace already in its
/// folder.
pub(super) async fn unarchive<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, args: Value) -> Result<Value, ControlError> {
    let args = without_nulls(args);
    let UnarchiveArgs { archive_id, select, client_request_id } = service::parse(args.clone())?;
    service::identifier(&archive_id, "archive_id", 256)?;
    if let Some(key) = &client_request_id {
        service::identifier(key, "client_request_id", 128)?;
    }
    // A restore retires its entry, so a retry replays before looking.
    if let Some(done) = service::recorded(app, caller, "workspace_unarchive", &args)? {
        return Ok(done);
    }
    if app.state::<AppStateStore>().find_archived_workspace(&archive_id).is_none() {
        return Err(ControlError::new(ErrorCode::ArchiveNotFound, "No archived workspace has this archive_id; see workspace_archive_list"));
    }
    let workspaces = WorkspaceService::new(app).checked(checkpoint(app, caller));
    service::run_receipted(app, caller, "workspace_unarchive", &args, false, None, move |_| async move {
        let workspace_id = workspaces
            .unarchive(archive_id.clone(), select)
            .await
            .map_err(|error| ControlError::from_text(error, ErrorCode::WorkspaceUnarchiveFailed))?;
        Ok(json!({"archive_id": archive_id, "workspace_id": workspace_id, "restored": true}))
    })
    .await
}

pub(super) fn archive_list<R: Runtime>(app: &AppHandle<R>, args: Value) -> Result<Value, ControlError> {
    let NoArgs {} = service::parse(args)?;
    let entries = serde_json::to_value(app.state::<AppStateStore>().archived_workspaces_list()).map_err(serialization)?;
    Ok(json!({"archived_workspaces": entries}))
}

/// The project roots connector clients may open: what `known_folder` checks.
pub(super) async fn project_list<R: Runtime>(app: &AppHandle<R>, args: Value) -> Result<Value, ControlError> {
    let NoArgs {} = service::parse(args)?;
    let app = app.clone();
    blocking(move || {
        let recent: Vec<(PathBuf, crate::database::RecentProject)> = app
            .state::<crate::database::DatabaseStore>()
            .get_recent_projects(workspaces::RECENT_PROJECT_LIMIT)
            .into_iter()
            .filter_map(|project| Some((Path::new(&project.path).canonicalize().ok()?, project)))
            .collect();
        let projects: Vec<Value> = workspaces::known_project_roots(&app)
            .into_iter()
            .map(|root| {
                let recent = recent.iter().find(|(path, _)| *path == root).map(|(_, project)| project);
                let folder = root.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
                json!({
                    "path": root.display().to_string(),
                    "name": recent.map_or(folder, |project| project.name.clone()),
                    "last_opened_at": recent.map(|project| project.last_opened_at.clone()),
                })
            })
            .collect();
        Ok(json!({"projects": projects}))
    })
    .await
}

/// Clones a repository into a new folder, opens it as a workspace and
/// records it as a Recent project, so it stays a known project.
pub(super) async fn project_clone<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller, args: Value) -> Result<Value, ControlError> {
    let args = without_nulls(args);
    let CloneArgs { url, parent, name, select, client_request_id } = service::parse(args.clone())?;
    if let Some(key) = &client_request_id {
        service::identifier(key, "client_request_id", 128)?;
    }
    clone_url(&url, !caller.is_network())?;
    let name = match name {
        Some(name) => name,
        None => repository_name(&url)?,
    };
    folder_name(&name)?;
    // A retry replays even though the folder now exists.
    if let Some(done) = service::recorded(app, caller, "project_clone", &args)? {
        return Ok(done);
    }
    let parent = if caller.is_network() { known_parent(app, parent).await? } else { local_folder(&parent)? };
    let target = Path::new(&parent).join(&name);
    if target.exists() {
        return Err(ControlError::new(ErrorCode::InvalidArguments, format!("{} already exists; pick another name", target.display())));
    }
    let owned = app.clone();
    let workspaces = WorkspaceService::new(app).checked(checkpoint(app, caller));
    let check = checkpoint(app, caller);
    service::run_receipted(app, caller, "project_clone", &args, true, None, move |reserved| async move {
        check.check().map_err(|error| ControlError::from_text(error, ErrorCode::ProjectCloneFailed))?;
        let path = crate::commands::git::git_clone_repo(owned.clone(), url, target.display().to_string())
            .await
            .map_err(|error| ControlError::new(ErrorCode::ProjectCloneFailed, error))?;
        let opened = workspaces
            .open(OpenWorkspace { path: Some(path.clone()), select, reuse_open: true, reserved_id: reserved })
            .await
            .map_err(|error| ControlError::from_text(error, ErrorCode::WorkspaceCreateFailed))?;
        owned.state::<crate::database::DatabaseStore>().add_recent_project(&path, &name)?;
        Ok(json!({"workspace_id": opened.workspace_id, "cwd": opened.cwd, "path": path, "created": opened.created}))
    })
    .await
}

/// A repository URL git cannot read as an option or a command: https, http,
/// ssh and git URLs, `user@host:path`, and for callers on this computer
/// `file://` URLs. Transports such as `ext::` run commands.
fn clone_url(url: &str, local: bool) -> Result<(), ControlError> {
    let invalid = || ControlError::new(ErrorCode::InvalidArguments, "url must be an https, http, ssh or git URL, or user@host:path");
    if url.starts_with('-') || url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(invalid());
    }
    let schemes: &[&str] = if local { &["https://", "http://", "ssh://", "git://", "file://"] } else { &["https://", "http://", "ssh://", "git://"] };
    let scheme = schemes.iter().any(|scheme| url.len() > scheme.len() && url[..scheme.len()].eq_ignore_ascii_case(scheme));
    let scp_like = !url.contains("://")
        && url.split_once(':').is_some_and(|(host, path)| host.contains('@') && !host.contains('/') && !path.is_empty());
    if scheme || scp_like { Ok(()) } else { Err(invalid()) }
}

/// The folder name git itself would pick: the URL's last path segment
/// without `.git`.
fn repository_name(url: &str) -> Result<String, ControlError> {
    let path = url.trim_end_matches('/');
    let last = path.rsplit(['/', ':']).next().unwrap_or_default();
    let name = last.strip_suffix(".git").unwrap_or(last);
    if name.is_empty() {
        return Err(ControlError::new(ErrorCode::InvalidArguments, "The URL names no repository; pass name"));
    }
    Ok(name.to_string())
}

/// One plain, visible folder name directly under the parent.
fn folder_name(name: &str) -> Result<(), ControlError> {
    if name.is_empty() || name.len() > 255 || name.starts_with('.') || name.contains(['/', '\\']) || name.chars().any(char::is_control) {
        return Err(ControlError::new(ErrorCode::InvalidArguments, format!("\"{name}\" is not a usable folder name; pass name")));
    }
    Ok(())
}

/// A folder a connector client may clone into: one that already holds a
/// project this computer works in. Returns the canonical path.
async fn known_parent<R: Runtime>(app: &AppHandle<R>, path: String) -> Result<String, ControlError> {
    let app = app.clone();
    blocking(move || {
        let folder = PathBuf::from(local_folder(&path)?);
        let folder = folder.canonicalize().map_err(|error| ControlError::new(ErrorCode::InvalidArguments, format!("{path}: {error}")))?;
        if workspaces::known_project_roots(&app).iter().any(|root| root.parent() == Some(folder.as_path())) {
            Ok(folder.display().to_string())
        } else {
            Err(ControlError::new(ErrorCode::UnknownProjectRoot, format!("{path} holds no project already opened in Codemux")))
        }
    })
    .await
}

/// The caller's authority, as the workspace lifecycle asks for it again after
/// each wait: on the lifecycle lock, a fetch, a large checkout, a teardown
/// command, stopping a closing workspace's agents. A grant revoked or a
/// connector disabled meanwhile stops the operation there, and its receipt
/// records why.
///
/// A new workspace's setup runs on after the call returned, past an in-app
/// chat's turn and a web-remote session's connection, so it asks only what a
/// person withdraws ([`service::authorize_background`]).
pub(super) fn checkpoint<R: Runtime>(app: &AppHandle<R>, caller: &ControlCaller) -> Checkpoint {
    let (app, caller) = (app.clone(), caller.clone());
    let (setup_app, setup_caller) = (app.clone(), caller.clone());
    Checkpoint::new(move || service::authorize(&app, &caller).map_err(|error| error.to_string()))
        .with_background(move || {
            service::authorize_background(&setup_app, &setup_caller).map_err(|error| error.to_string())
        })
}

/// Maps the workspace and git layers' `prefix: message` errors to codes. Text
/// that names a known code keeps it; anything else becomes `fallback`.
pub(super) fn git_error(text: String, fallback: ErrorCode) -> ControlError {
    const PREFIXES: [(&str, ErrorCode); 3] = [
        ("invalid_input: ", ErrorCode::InvalidArguments),
        ("conflict: ", ErrorCode::WorktreeConflict),
        ("operation_failed: ", ErrorCode::WorktreeBaseUnavailable),
    ];
    for (prefix, code) in PREFIXES {
        if let Some(message) = text.strip_prefix(prefix) {
            return ControlError::new(code, message);
        }
    }
    if text.starts_with("Not a git repository: ") {
        return ControlError::new(ErrorCode::NotAGitRepository, text);
    }
    ControlError::from_text(text, fallback)
}

/// `WorktreeArgs::into_request` fails on its arguments or, with
/// `start_from_origin`, on resolving the base.
fn request_error(text: String) -> ControlError {
    if text.starts_with("Unsupported layout: ") {
        return ControlError::new(ErrorCode::InvalidArguments, text);
    }
    git_error(text, ErrorCode::WorktreeBaseUnavailable)
}

/// The schema reads a `null` optional argument as absent. Dropping them first
/// makes the typed arguments and a receipt's fingerprint agree with it.
fn without_nulls(mut args: Value) -> Value {
    if let Some(object) = args.as_object_mut() {
        object.retain(|_, value| !value.is_null());
    }
    args
}

/// A folder a connector client may open or branch: an existing folder in a
/// project this computer already works in. Returns the canonical path.
async fn known_folder<R: Runtime>(app: &AppHandle<R>, path: String) -> Result<String, ControlError> {
    let app = app.clone();
    blocking(move || {
        let known = workspaces::known_project_roots(&app);
        workspaces::resolve_under_known_root(&known, Path::new(&path))
            .map(|folder| folder.display().to_string())
            .map_err(|error| ControlError::from_text(error, ErrorCode::InvalidArguments))
    })
    .await
}

/// A folder a local caller names: absolute once `~` is expanded, and
/// existing. A relative path would resolve against the app's own folder.
fn local_folder(path: &str) -> Result<String, ControlError> {
    let folder = PathBuf::from(crate::project::expand_tilde(path));
    if !folder.is_absolute() {
        return Err(ControlError::new(ErrorCode::InvalidArguments, format!("path must be absolute: {path}")));
    }
    if !folder.is_dir() {
        return Err(ControlError::new(ErrorCode::InvalidArguments, format!("{path} is not an existing folder")));
    }
    Ok(folder.display().to_string())
}

/// Filesystem and git reads off the async runtime.
pub(super) async fn blocking<T: Send + 'static>(work: impl FnOnce() -> Result<T, ControlError> + Send + 'static) -> Result<T, ControlError> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|error| ControlError::new(ErrorCode::NativeControlFailed, format!("Worker failed: {error}")))?
}

fn serialization(error: serde_json::Error) -> ControlError {
    ControlError::new(ErrorCode::SerializationFailed, error.to_string())
}
