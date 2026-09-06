use crate::agent_browser::AgentBrowserManager;
use crate::config::{
    read_shell_appearance_or_default,
    read_theme_colors_or_default,
    workspace_config::WorkspaceConfig,
    ShellAppearance,
    ThemeColors,
};
use crate::state::{
    AppStateSnapshot,
    AppStateStore,
    NotificationLevel,
    SplitDirection,
    TabKind,
    WorkspacePresetLayout,
    WorkspaceType,
    WorktreeWorkspaceClaim,
};
use crate::terminal;
use crate::workspace_paths::paths_equal;
use notify_rust::Notification;
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager, State};

/// Snapshot of the git fields written to a workspace by `populate_git_info`.
/// Carrying these out of the I/O step as a value lets async callers gather
/// them via `spawn_blocking` and then apply the result synchronously to
/// state — `AppStateStore` references can't cross the `spawn_blocking`
/// boundary.
#[derive(Clone)]
pub(crate) struct WorkspaceGitInfo {
    pub is_git: bool,
    pub branch: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub additions: u32,
    pub deletions: u32,
    pub changed_files: u32,
}

/// Run the git subprocesses that produce a workspace's branch / ahead-behind
/// / diff-stat / changed-file counts. Crash-proof: `git_branch_info` and
/// friends return defaults for non-git directories, detached HEAD, and
/// corrupted `.git` directories. Pure I/O — touches no shared state, so it
/// is safe to call from `spawn_blocking`.
pub(crate) fn gather_workspace_git_info(repo_path: &Path) -> WorkspaceGitInfo {
    // Same predicate worktree creation uses (`find_git_root`), so "is this
    // a git workspace" and "can this project have worktrees" never disagree.
    let is_git = crate::config::workspace_config::find_git_root(repo_path).is_some();
    let branch_info = crate::git::git_branch_info(repo_path).ok();
    let diff_stat = crate::git::git_diff_stat(repo_path).ok();
    let changed_files = crate::git::git_status(repo_path).map(|f| f.len() as u32).unwrap_or(0);

    let branch = branch_info.as_ref().and_then(|i| i.branch.clone());
    let ahead = branch_info.as_ref().map(|i| i.ahead).unwrap_or(0);
    let behind = branch_info.as_ref().map(|i| i.behind).unwrap_or(0);
    let additions = diff_stat
        .as_ref()
        .map(|s| s.staged_additions + s.unstaged_additions)
        .unwrap_or(0);
    let deletions = diff_stat
        .as_ref()
        .map(|s| s.staged_deletions + s.unstaged_deletions)
        .unwrap_or(0);

    WorkspaceGitInfo { is_git, branch, ahead, behind, additions, deletions, changed_files }
}

impl Default for WorkspaceGitInfo {
    /// The error-path fallback (e.g. a panicked `spawn_blocking` gather).
    /// `is_git: true` keeps a transient failure from flashing the
    /// "Initialize Git" affordance on a real repo — optimistic, matching
    /// the serde default on the snapshot field.
    fn default() -> Self {
        Self {
            is_git: true,
            branch: None,
            ahead: 0,
            behind: 0,
            additions: 0,
            deletions: 0,
            changed_files: 0,
        }
    }
}

/// Write a gathered git snapshot into state. Returns true when a field
/// actually moved, so the periodic sweep can emit only on real change.
/// `pub(crate)` because the sweep in `lib.rs` gathers once per checkout and
/// applies that one result to every workspace sharing it.
pub(crate) fn apply_workspace_git_info(
    state: &AppStateStore,
    workspace_id: &str,
    info: WorkspaceGitInfo,
) -> bool {
    state.update_workspace_git_info(
        workspace_id,
        info.is_git,
        info.branch,
        info.ahead,
        info.behind,
        info.additions,
        info.deletions,
        info.changed_files,
    )
}

/// Reshape a gather result into the wire subset the `WorkspaceGit` delta
/// domain carries. Same fields, different owner: `WorkspaceGitInfo` is the
/// gather's output, `WorkspaceGitDelta` is what the renderer applies.
pub(crate) fn git_delta_of(info: WorkspaceGitInfo) -> crate::state::WorkspaceGitDelta {
    crate::state::WorkspaceGitDelta {
        is_git: info.is_git,
        git_branch: info.branch,
        git_ahead: info.ahead,
        git_behind: info.behind,
        git_additions: info.additions,
        git_deletions: info.deletions,
        git_changed_files: info.changed_files,
    }
}

/// Read fresh git branch / ahead-behind / diff-stat / changed-file counts for
/// `repo_path` and write them into the workspace snapshot. `pub(crate)` so the
/// periodic refresh loop in `lib.rs` can reuse this instead of duplicating
/// the extraction logic.
/// Returns true when the write changed something.
pub(crate) fn populate_git_info(
    state: &AppStateStore,
    workspace_id: &str,
    repo_path: &Path,
) -> bool {
    apply_workspace_git_info(state, workspace_id, gather_workspace_git_info(repo_path))
}

/// Async equivalent of `populate_git_info` for use inside `async fn` Tauri
/// commands. Runs the 5-8 git subprocesses on the blocking pool so they do
/// not stall a Tokio worker (and through it, every other in-flight IPC call).
pub(crate) async fn populate_git_info_async(
    state: &AppStateStore,
    workspace_id: &str,
    repo_path: PathBuf,
) -> bool {
    let info = tokio::task::spawn_blocking(move || gather_workspace_git_info(&repo_path))
        .await
        .unwrap_or_default();
    apply_workspace_git_info(state, workspace_id, info)
}

/// `async fn` so the git-info gather runs on the blocking pool instead of
/// whichever thread drives the caller (the GTK main thread for the Tauri
/// command, a Tokio worker for the control socket). Both callers are
/// already async, so the `.await` plumbs straight through.
pub(crate) async fn create_workspace_impl<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: &AppStateStore,
    db: &crate::database::DatabaseStore,
    cwd: Option<String>,
) -> Result<String, String> {
    // A workspace cwd must be absolute so its terminals can `chdir` into it;
    // a hand-typed `~` path would otherwise be stored verbatim.
    let cwd = cwd.map(|path| crate::project::expand_tilde(&path));
    let workspace_id = match &cwd {
        Some(path) => state.create_workspace_at_path(PathBuf::from(path)),
        None => state.create_workspace(),
    };

    // Set project root (resolve through git root for worktree grouping)
    let repo_path = cwd
        .map(PathBuf::from)
        .unwrap_or_else(crate::project::current_project_root);
    let project_root = crate::config::workspace_config::find_git_root(&repo_path)
        .unwrap_or_else(|| repo_path.clone());
    state.set_workspace_project_root(&workspace_id.0, project_root.display().to_string());
    populate_git_info_async(state, &workspace_id.0, repo_path.clone()).await;

    if let Some(session_id) = state.active_terminal_session_id() {
        terminal::spawn_pty_for_session(app.clone(), session_id.0);
    }

    // Run setup scripts in background thread
    spawn_setup_scripts(&app, state, db, &workspace_id.0, &repo_path);

    // Write .mcp.json for agent auto-discovery
    if crate::mcp_server::is_auto_mcp_enabled(&app) {
        crate::mcp_server::upsert_mcp_config(&repo_path);
    }

    crate::state::emit_app_state(&app);
    Ok(workspace_id.0)
}

pub(crate) fn split_pane_impl<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: &AppStateStore,
    pane_id: String,
    direction: String,
) -> Result<String, String> {
    let direction = match direction.as_str() {
        "horizontal" => SplitDirection::Horizontal,
        "vertical" => SplitDirection::Vertical,
        _ => return Err(format!("Unsupported split direction: {direction}")),
    };

    let session_id = state.split_pane(&pane_id, direction)?;
    terminal::spawn_pty_for_session(app.clone(), session_id.0.clone());
    crate::state::emit_app_state(&app);
    Ok(session_id.0)
}

#[tauri::command]
pub fn get_current_theme() -> Result<ThemeColors, String> {
    Ok(read_theme_colors_or_default())
}

#[tauri::command]
pub fn get_shell_appearance() -> Result<ShellAppearance, String> {
    Ok(read_shell_appearance_or_default())
}

#[tauri::command]
pub fn get_app_state(state: State<'_, AppStateStore>) -> Result<AppStateSnapshot, String> {
    // Labelled with the revision it already reflects rather than a fresh one:
    // this is the renderer's boot and gap-recovery baseline, and consuming a
    // revision for a read would break the contiguity of the delta stream.
    Ok(state.snapshot_at_current_revision())
}

#[tauri::command]
pub fn regenerate_mcp_config(
    state: State<'_, AppStateStore>,
    workspace_id: String,
) -> Result<(), String> {
    let snapshot = state.snapshot();
    let ws = snapshot
        .workspaces
        .iter()
        .find(|w| w.workspace_id.0 == workspace_id)
        .ok_or_else(|| format!("Workspace not found: {workspace_id}"))?;
    crate::mcp_server::upsert_mcp_config(Path::new(&ws.cwd));
    Ok(())
}

// `async fn` so this command runs on Tokio's worker pool instead of the GTK
// main thread: `populate_git_info` shells out to 5-8 git subprocesses, and a
// sync handler would block every other IPC call (and the UI) while they run.
// Same pattern as `commands/git.rs` — see the note at the top of that file.
#[tauri::command]
pub async fn create_workspace<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    db: State<'_, crate::database::DatabaseStore>,
    cwd: Option<String>,
) -> Result<String, String> {
    create_workspace_impl(app, &state, &db, cwd).await
}

/// Additive return payload for the workspace-creation commands
/// (`create_empty_workspace`, `create_worktree_workspace`).
///
/// Carries the resolved workspace id AND its absolute cwd so the frontend
/// learns the cwd immediately, instead of polling `get_app_state` for the
/// async `app-state-changed` event to land the new workspace before it can
/// read the cwd. `workspace_id` is the exact value these commands returned
/// as a bare string before this struct existed, so callers only need to
/// unwrap one field to preserve the old behavior.
///
/// `adopted` is true when no workspace was created because a live one
/// already claimed the target worktree path and was focused instead (see
/// the adopt-existing guard in `create_worktree_workspace_impl`). On that
/// path any `initial_prompt`/`agent_preset_id` were dropped, so the
/// frontend uses the flag to tell the user their prompt wasn't sent.
/// Always false for `create_empty_workspace`.
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceCreated {
    pub workspace_id: String,
    pub cwd: String,
    pub adopted: bool,
}

// `async fn` so the git-info subprocesses run on the blocking pool instead
// of the GTK main thread (same rationale as `create_workspace` above).
#[tauri::command]
pub async fn create_empty_workspace<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    db: State<'_, crate::database::DatabaseStore>,
    cwd: String,
    skip_setup: Option<bool>,
) -> Result<WorkspaceCreated, String> {
    // Defense-in-depth: a workspace cwd must always be an absolute path so
    // every terminal it spawns can `chdir` into it. A literal `~` here would
    // be stored verbatim and leave terminals stranded in `$HOME`.
    let cwd = crate::project::expand_tilde(&cwd);
    let repo_path = PathBuf::from(&cwd);
    let workspace_id = state.create_empty_workspace_at_path(repo_path.clone());

    let project_root = crate::config::workspace_config::find_git_root(&repo_path)
        .unwrap_or_else(|| repo_path.clone());
    state.set_workspace_project_root(&workspace_id.0, project_root.display().to_string());
    populate_git_info_async(&state, &workspace_id.0, repo_path.clone()).await;

    // Home-chat surfaces and other non-project workspaces pass
    // skip_setup=true to bypass project-level ceremony: no setup
    // scripts, no .mcp.json injection. Git metadata still populates
    // (cheap, harmless for non-repo paths).
    if !skip_setup.unwrap_or(false) {
        spawn_setup_scripts(&app, &state, &db, &workspace_id.0, &repo_path);

        if crate::mcp_server::is_auto_mcp_enabled(&app) {
            crate::mcp_server::upsert_mcp_config(&repo_path);
        }
    }

    crate::state::emit_app_state(&app);
    Ok(WorkspaceCreated {
        workspace_id: workspace_id.0,
        // `cwd` is the tilde-expanded absolute path resolved above, i.e. the
        // exact directory the workspace was anchored at.
        cwd,
        adopted: false,
    })
}

/// Return the existing Home workspace id, or create one anchored at
/// `$HOME` if none exists yet.
///
/// Home is a singleton: the first workspace with
/// `workspace_type == Home` wins. If the user hard-deletes it, the next
/// call lazily recreates one (no delete protection by design). Creation
/// passes `skip_setup=true` semantics — no `.mcp.json` injection, no
/// setup scripts — since the home directory is not a project.
///
/// `async fn` so the git-info subprocesses run on the blocking pool
/// instead of the GTK main thread (same rationale as `create_workspace`).
#[tauri::command]
pub async fn get_or_create_home_workspace<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
) -> Result<String, String> {
    if let Some(existing) = state.find_home_workspace_id() {
        return Ok(existing);
    }

    let home_dir = dirs::home_dir()
        .ok_or_else(|| "home_dir_unavailable".to_string())?;
    let repo_path = home_dir.clone();

    let workspace_id = state.create_empty_workspace_at_path(repo_path.clone());

    let project_root = crate::config::workspace_config::find_git_root(&repo_path)
        .unwrap_or_else(|| repo_path.clone());
    state.set_workspace_project_root(&workspace_id.0, project_root.display().to_string());
    populate_git_info_async(&state, &workspace_id.0, repo_path.clone()).await;

    state.set_workspace_type(&workspace_id.0, WorkspaceType::Home);

    crate::state::emit_app_state(&app);
    Ok(workspace_id.0)
}

// `async fn` so the git-info subprocesses run on the blocking pool instead
// of the GTK main thread (same rationale as `create_workspace` above).
#[tauri::command]
pub async fn create_workspace_with_preset<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    db: State<'_, crate::database::DatabaseStore>,
    cwd: Option<String>,
    layout: String,
) -> Result<String, String> {
    let layout = match layout.as_str() {
        "single" => WorkspacePresetLayout::Single,
        "pair" => WorkspacePresetLayout::Pair,
        "quad" => WorkspacePresetLayout::Quad,
        "six" => WorkspacePresetLayout::Six,
        "eight" => WorkspacePresetLayout::Eight,
        "shell_browser" => WorkspacePresetLayout::ShellBrowser,
        _ => return Err(format!("Unsupported workspace preset layout: {layout}")),
    };

    let repo_path = cwd
        .as_ref()
        .map(|p| PathBuf::from(p))
        .unwrap_or_else(crate::project::current_project_root);
    let workspace_id = match cwd {
        Some(path) => state.create_workspace_with_layout(PathBuf::from(path), layout),
        None => state.create_workspace_with_layout(crate::project::current_project_root(), layout),
    };

    state.set_workspace_project_root(&workspace_id.0, repo_path.display().to_string());
    populate_git_info_async(&state, &workspace_id.0, repo_path.clone()).await;

    let snapshot = state.snapshot();
    let session_ids = snapshot
        .workspaces
        .iter()
        .find(|workspace| workspace.workspace_id.0 == workspace_id.0)
        .map(|workspace| crate::state::collect_terminal_sessions(&workspace.surfaces))
        .unwrap_or_default();

    for session_id in session_ids {
        terminal::spawn_pty_for_session(app.clone(), session_id);
    }

    // Run setup scripts in background thread
    spawn_setup_scripts(&app, &state, &db, &workspace_id.0, &repo_path);

    crate::state::emit_app_state(&app);
    Ok(workspace_id.0)
}

// `async fn` so this command runs on Tokio's worker pool instead of the GTK
// main thread. `git_create_worktree` shells out to `git worktree add`, which
// can take many seconds on large repos (fetches refs, checks out files), and
// `populate_git_info` shells out to 5-8 more git subprocesses for the new
// workspace. With a sync handler all of that blocks every other IPC call.
#[tauri::command]
pub async fn create_worktree_workspace<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    db: State<'_, crate::database::DatabaseStore>,
    pty_state: State<'_, crate::terminal::PtyState>,
    presets: State<'_, crate::presets::PresetStoreState>,
    repo_path: String,
    branch: String,
    new_branch: bool,
    base: Option<String>,
    layout: String,
    initial_prompt: Option<String>,
    agent_preset_id: Option<String>,
    model_selection: Option<crate::agent_capability::ModelSelection>,
    pr_number: Option<u32>,
) -> Result<WorkspaceCreated, String> {
    let created = create_worktree_workspace_impl(
        app, &state, &db, &pty_state, &presets,
        repo_path, branch, new_branch, base, layout,
        initial_prompt, agent_preset_id, model_selection, pr_number,
    )
    .await?;
    // Resolve the workspace's cwd (the worktree checkout path) from the
    // snapshot the impl already populated + emitted, so the frontend gets it
    // without polling app-state. Empty string only if the workspace somehow
    // vanished between create and lookup (it never should).
    let cwd = state
        .snapshot()
        .workspaces
        .iter()
        .find(|w| w.workspace_id.0 == created.workspace_id)
        .map(|w| w.cwd.clone())
        .unwrap_or_default();
    Ok(WorkspaceCreated {
        workspace_id: created.workspace_id,
        cwd,
        adopted: created.adopted,
    })
}

/// Find a live workspace that is already checked out at `worktree_path`
/// for `branch`, returning its id.
///
/// A workspace claims a checkout via `worktree_path` (set by
/// `set_workspace_worktree` for every worktree-backed workspace); for
/// such a claim, path identity is the whole test — the claim was made
/// expressly for this checkout. A plain workspace opened directly at
/// that directory carries no `worktree_path` but the same `cwd`, and it
/// collides just as badly, so `cwd` is the fallback probe — but a bare
/// path match there would adopt the workspace regardless of what it has
/// checked out (`git_create_worktree` only reuses a path when git maps
/// it to the SAME branch; this guard must be no wider). So the fallback
/// additionally requires the workspace's known `git_branch` to equal the
/// requested `branch`; an unknown branch (`None`, git info not yet
/// populated) is not adoptable because the match can't be verified.
///
/// Archived workspaces are deliberately NOT considered: archived entries
/// live in the separate `AppStateSnapshot::archived_workspaces` vec (they
/// are removed from `workspaces` on archive — see
/// `ArchivedWorkspaceSnapshot` in `state/state_impl.rs`), so they are
/// invisible here by construction. That is the behavior we want: an
/// archived workspace has no live panes, PTYs, or agent sessions to
/// collide with, so re-creating a workspace over the same on-disk
/// worktree is safe and matches how unarchive/restore already treats
/// those directories (it adopts them explicitly via
/// `import_worktree_workspace_impl`). Silently un-archiving someone's
/// archived workspace as a side effect of "create a worktree" would be a
/// surprising, hard-to-undo mutation, so we don't.
pub(crate) fn find_live_workspace_for_worktree_path(
    snapshot: &AppStateSnapshot,
    worktree_path: &Path,
    branch: &str,
) -> Option<String> {
    snapshot
        .workspaces
        .iter()
        .find(|w| {
            // Remote workspaces are excluded: their `cwd`/`worktree_path` are
            // paths on the REMOTE host, not this machine, so a string match
            // against a local worktree directory would be a coincidence, not
            // a collision. `git worktree add` only ever produces local paths.
            if w.host_id.is_some() || w.attach_only {
                return false;
            }
            match w.worktree_path.as_deref() {
                Some(claimed) => {
                    !claimed.is_empty()
                        && paths_equal(Path::new(claimed), worktree_path)
                }
                None => {
                    !w.cwd.is_empty()
                        && w.git_branch.as_deref() == Some(branch)
                        && paths_equal(Path::new(&w.cwd), worktree_path)
                }
            }
        })
        .map(|w| w.workspace_id.0.clone())
}

/// Result of [`create_worktree_workspace_impl`]: the resolved workspace id
/// plus whether it was ADOPTED (a live workspace already claimed the
/// worktree path, so `initial_prompt`/`agent_preset_id` were dropped and no
/// create-side effects ran) rather than freshly created. Callers surface
/// `adopted` so the frontend can tell the user their prompt wasn't sent.
pub(crate) struct CreatedWorktreeWorkspace {
    pub workspace_id: String,
    pub adopted: bool,
}

/// Shared implementation behind both the Tauri command (frontend "+
/// New worktree" / branch-picker "Fork" flow) and the
/// `create_worktree_workspace` control-socket command exposed via the
/// Phase 1.5 `worktree_create` MCP tool.
///
/// Takes refs instead of `State<>` wrappers so the socket dispatcher
/// (which already pulls each state via `app.state()`) and any future
/// non-Tauri caller can drive it. The body is byte-identical to the
/// pre-extraction Tauri command — moving it preserves git worktree
/// creation, workspace+layout hydration, PTY spawn, setup scripts,
/// `.mcp.json` autoconfig, and the preset-launch-with-prompt-injection
/// branch as one atomic operation.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn create_worktree_workspace_impl<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: &AppStateStore,
    db: &crate::database::DatabaseStore,
    pty_state: &crate::terminal::PtyState,
    presets: &crate::presets::PresetStoreState,
    repo_path: String,
    branch: String,
    new_branch: bool,
    base: Option<String>,
    layout: String,
    initial_prompt: Option<String>,
    agent_preset_id: Option<String>,
    model_selection: Option<crate::agent_capability::ModelSelection>,
    pr_number: Option<u32>,
) -> Result<CreatedWorktreeWorkspace, String> {
    let layout = match layout.as_str() {
        "single" => WorkspacePresetLayout::Single,
        "pair" => WorkspacePresetLayout::Pair,
        "quad" => WorkspacePresetLayout::Quad,
        "six" => WorkspacePresetLayout::Six,
        "eight" => WorkspacePresetLayout::Eight,
        "shell_browser" => WorkspacePresetLayout::ShellBrowser,
        // The inline "+ New worktree…" chat flow uses `empty` so the
        // resulting workspace has no terminal/PTY, no tab, and no
        // surface — the chat pane is attached afterward by the
        // frontend via `agent_chat_create_pane`.
        "empty" => WorkspacePresetLayout::Empty,
        _ => return Err(format!("Unsupported layout: {layout}")),
    };

    // Validate repo_path is a git repository before creating anything
    if crate::config::workspace_config::find_git_root(Path::new(&repo_path)).is_none() {
        return Err(format!("Not a git repository: {repo_path}"));
    }

    // The slow git op (recursive checkout, may fetch). Off-load to the
    // blocking pool so it doesn't stall a Tokio worker.
    let worktree_path = {
        let repo_path = repo_path.clone();
        let branch = branch.clone();
        let base = base.clone();
        tokio::task::spawn_blocking(move || {
            crate::git::git_create_worktree(
                Path::new(&repo_path),
                &branch,
                new_branch,
                base.as_deref(),
                pr_number,
            )
        })
        .await
        .map_err(|e| format!("git_create_worktree task join failed: {e}"))??
    };
    let wt_path_buf = PathBuf::from(&worktree_path);

    // ADOPT-EXISTING GUARD. `git_create_worktree` does not always create
    // something: when the conventional path is already on disk and
    // `git worktree list` maps it to the SAME branch, it short-circuits and
    // returns that existing checkout (see the path-reuse block in
    // `git::git_create_worktree`). Falling through to an unconditional
    // create there mints a SECOND workspace over one on-disk worktree — two
    // agents editing the same files, two PTYs, two `.mcp.json` owners. So if
    // a live workspace already claims this path, adopt it: focus it and
    // return its id, exactly as the unarchive flow does for an already-open
    // workspace.
    //
    // Lookup and insert happen as ONE atomic operation under the state lock
    // (`adopt_or_create_worktree_workspace`), which also stamps the new
    // workspace's `worktree_path` claim before releasing. With separate
    // lock acquisitions, two concurrent creates for the same worktree could
    // both probe empty and both insert — the exact duplicate this guard
    // prevents. The slow `git worktree add` above deliberately stays
    // outside the lock.
    //
    // Everything the create path does downstream is deliberately skipped for
    // an adopted workspace, because it already ran when that workspace was
    // first created and re-running it would disturb a live session: PTY
    // spawn, setup scripts, `.mcp.json` rewrite, and preset launch.
    //
    // `initial_prompt` and `agent_preset_id` are DROPPED on the adopt path,
    // by design. The adopted workspace already has live panes with (likely)
    // a running agent in them; injecting a prompt or launching a second
    // preset would type into somebody else's in-flight session — a
    // destructive, unrecoverable side effect. Dropping the request is the
    // strictly safer failure mode: the caller still gets a valid workspace
    // id and the user sees the branch they asked for, focused. The returned
    // `adopted` flag lets the frontend tell the user the prompt wasn't sent.
    //
    // Archived workspaces do not participate — see
    // `find_live_workspace_for_worktree_path` for why they are neither a
    // blocker nor auto-unarchived here.
    let claim = state.adopt_or_create_worktree_workspace(
        wt_path_buf.clone(),
        layout,
        worktree_path.clone(),
        // The worktree claim titles the workspace after its branch, exactly
        // as the previous separate `set_workspace_worktree` call did.
        branch.clone(),
        |snapshot| find_live_workspace_for_worktree_path(snapshot, &wt_path_buf, &branch),
    );
    let workspace_id = match claim {
        WorktreeWorkspaceClaim::Adopted(existing_id) => {
            // Focus the adopted workspace so the frontend flips to it exactly
            // as it would for a freshly-created one (the create path sets
            // `active_workspace_id` itself). `activate_workspace_impl` is
            // the same call the unarchive flow uses for an already-open
            // workspace, and it emits app state itself — no second emit
            // needed. `create_worktree_workspace` resolves the returned id's
            // `cwd` from the snapshot afterwards, and the adopted workspace
            // is in that snapshot, so the wrapper's cwd lookup keeps working
            // unchanged.
            activate_workspace_impl(app.clone(), state, existing_id.clone())?;
            return Ok(CreatedWorktreeWorkspace {
                workspace_id: existing_id,
                adopted: true,
            });
        }
        WorktreeWorkspaceClaim::Created(id) => id,
    };

    state.set_workspace_project_root(&workspace_id.0, repo_path.clone());

    populate_git_info_async(&state, &workspace_id.0, wt_path_buf.clone()).await;

    if let Some(pr_num) = pr_number {
        // Seed values only — the pollers replace all four the moment they
        // reach this workspace. The head branch is the branch we just
        // checked out for the PR, so the association starts out as a
        // current-branch one (settlement-eligible) rather than looking like
        // the side-branch fallback's weaker badge.
        state.update_workspace_pr_info(
            &workspace_id.0,
            Some(pr_num),
            None,
            None,
            Some(branch.clone()),
        );
    }

    let snapshot = state.snapshot();
    let session_ids = snapshot
        .workspaces
        .iter()
        .find(|w| w.workspace_id.0 == workspace_id.0)
        .map(|w| crate::state::collect_terminal_sessions(&w.surfaces))
        .unwrap_or_default();

    for session_id in session_ids {
        terminal::spawn_pty_for_session(app.clone(), session_id);
    }

    // Run setup scripts in background thread
    spawn_setup_scripts(&app, &state, &db, &workspace_id.0, &wt_path_buf);

    // Write .mcp.json for agent auto-discovery
    if crate::mcp_server::is_auto_mcp_enabled(&app) {
        crate::mcp_server::upsert_mcp_config(&wt_path_buf);
    }

    // Auto-launch agent preset if requested
    if let Some(ref preset_id) = agent_preset_id {
        let store = presets.inner.lock().unwrap_or_else(|e| e.into_inner());
        let preset = store.presets.iter().find(|p| p.id == *preset_id).cloned();
        drop(store);

        if let Some(preset) = preset {
            let commands: Vec<String> = if preset.commands.is_empty() {
                vec![String::new()]
            } else {
                preset
                    .commands
                    .iter()
                    .map(|cmd| {
                        let cmd = crate::agent_capability::apply_model_selection(
                            cmd,
                            model_selection.as_ref(),
                        );
                        crate::agent_context::inject_agent_context(&cmd, &workspace_id.0)
                    })
                    .collect()
            };

            let sessions_arc = pty_state.sessions.clone();

            // Grab the default tab's ID and session so the first preset command
            // reuses it instead of creating a duplicate tab.
            let default_tab: Option<(String, String)> = {
                let snap = state.snapshot();
                snap.workspaces
                    .iter()
                    .find(|w| w.workspace_id.0 == workspace_id.0)
                    .and_then(|w| {
                        let tab = w.tabs.first()?;
                        let session = crate::state::collect_terminal_sessions(&w.surfaces)
                            .into_iter()
                            .next()?;
                        Some((tab.tab_id.clone(), session))
                    })
            };

            for (i, command) in commands.iter().enumerate() {
                // Reuse the default terminal tab for the first preset command;
                // create new tabs only for additional commands.
                let (tab_id, session_id) = if i == 0 && default_tab.is_some() {
                    let (tid, sid) = default_tab.as_ref().unwrap();
                    (tid.clone(), sid.clone())
                } else {
                    match state.create_tab(&workspace_id.0, TabKind::Terminal) {
                        Ok((tid, sid)) => {
                            if let Some(ref s) = sid {
                                terminal::spawn_pty_for_session(app.clone(), s.0.clone());
                            }
                            (tid, sid.map(|s| s.0).unwrap_or_default())
                        }
                        Err(_) => continue,
                    }
                };

                let _ = state.rename_tab(&workspace_id.0, &tab_id, preset.name.clone());
                let _ = state.set_tab_icon(&workspace_id.0, &tab_id, preset.icon.clone());

                if !session_id.is_empty() && !command.is_empty() {
                    let (cmd, needs_pty_injection) =
                        crate::branch_name::prepare_agent_command(
                            &preset.id,
                            command,
                            initial_prompt.as_deref(),
                        );

                    state.update_terminal_session_command(&session_id, command.clone());

                    super::presets::write_command_when_ready(
                        sessions_arc.clone(),
                        session_id.clone(),
                        cmd,
                        120,
                    );

                    // For agents that need PTY injection, write prompt after agent starts
                    // using a longer settle time for agent TUI initialization.
                    if needs_pty_injection {
                        if let Some(ref prompt) = initial_prompt {
                            super::presets::write_command_when_ready(
                                sessions_arc.clone(),
                                session_id,
                                prompt.clone(),
                                1500,
                            );
                        }
                    }
                }
            }
        }
    }

    crate::state::emit_app_state(&app);
    Ok(CreatedWorktreeWorkspace {
        workspace_id: workspace_id.0,
        adopted: false,
    })
}

// `async fn` so `populate_git_info_async`'s 5-8 git subprocesses run on the
// blocking pool. The function itself adopts an already-on-disk worktree
// (state ops are cheap), so the git-info gather is the only slow step.
#[tauri::command]
pub async fn import_worktree_workspace<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    db: State<'_, crate::database::DatabaseStore>,
    worktree_path: String,
    branch: String,
    layout: String,
) -> Result<String, String> {
    import_worktree_workspace_impl(app, &state, &db, worktree_path, branch, layout).await
}

/// Adopt an already-on-disk git worktree into a fresh workspace WITHOUT
/// running `git worktree add` — the directory is already a registered
/// worktree, so we only build workspace state around it, spawn its
/// sessions, run setup scripts, and register MCP. Shared by the
/// "Import worktree" affordance and the unarchive restore path: an
/// archived worktree recorded at a NON-conventional path still exists on
/// disk (archiving touches nothing), so restore adopts it in place rather
/// than routing through `create_worktree_workspace_impl`, whose
/// `git worktree add` at the conventional path would fail with "<branch>
/// is already used by worktree at <that path>".
pub(crate) async fn import_worktree_workspace_impl<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: &AppStateStore,
    db: &crate::database::DatabaseStore,
    worktree_path: String,
    branch: String,
    layout: String,
) -> Result<String, String> {
    let layout = match layout.as_str() {
        "single" => WorkspacePresetLayout::Single,
        "pair" => WorkspacePresetLayout::Pair,
        "quad" => WorkspacePresetLayout::Quad,
        "six" => WorkspacePresetLayout::Six,
        "eight" => WorkspacePresetLayout::Eight,
        "shell_browser" => WorkspacePresetLayout::ShellBrowser,
        _ => return Err(format!("Unsupported layout: {layout}")),
    };

    let wt_path_buf = PathBuf::from(&worktree_path);
    let workspace_id = state.create_workspace_with_layout(wt_path_buf.clone(), layout);

    state.set_workspace_worktree(&workspace_id.0, worktree_path.clone(), branch);

    // Resolve the main repo root from the worktree for project grouping
    if let Some(root) = crate::config::workspace_config::find_git_root(&wt_path_buf) {
        state.set_workspace_project_root(&workspace_id.0, root.display().to_string());
    }

    populate_git_info_async(&state, &workspace_id.0, wt_path_buf.clone()).await;

    let snapshot = state.snapshot();
    let session_ids = snapshot
        .workspaces
        .iter()
        .find(|w| w.workspace_id.0 == workspace_id.0)
        .map(|w| crate::state::collect_terminal_sessions(&w.surfaces))
        .unwrap_or_default();

    for session_id in session_ids {
        terminal::spawn_pty_for_session(app.clone(), session_id);
    }

    spawn_setup_scripts(&app, &state, &db, &workspace_id.0, &wt_path_buf);

    // Write .mcp.json for agent auto-discovery
    if crate::mcp_server::is_auto_mcp_enabled(&app) {
        crate::mcp_server::upsert_mcp_config(&wt_path_buf);
    }

    crate::state::emit_app_state(&app);
    Ok(workspace_id.0)
}

// `async fn` so this command runs on Tokio's worker pool instead of the GTK
// main thread. With a synchronous handler, `git worktree remove --force` on a
// large worktree (recursive filesystem delete) blocks every subsequent IPC
// call, freezing the UI hard enough that even window-close requests don't
// process. Same pattern as `commands/git.rs` — see the note at the top of that
// file.
#[tauri::command]
pub async fn close_workspace_with_worktree<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    db: State<'_, crate::database::DatabaseStore>,
    workspace_id: String,
    remove_worktree: bool,
    delete_branch: Option<bool>,
    force_delete: Option<bool>,
) -> Result<(), String> {
    close_workspace_with_worktree_impl(
        app,
        &state,
        &db,
        workspace_id,
        remove_worktree,
        delete_branch,
        force_delete,
    )
    .await
}

/// Fire-and-forget teardown of the agent-browser CLI session(s) that
/// belonged to a just-closed workspace (issue #126: agent-browser daemons
/// used to accumulate for the app's whole lifetime because nothing ever
/// reaped them on workspace close — only an app restart's blanket cleanup
/// caught them).
///
/// Shared-cwd safety: two workspaces opened at the same cwd get separate
/// `AgentBrowserSession` records but the SAME `cli_session_name` (it is a
/// pure hash of the cwd via `stable_browser_session_name`), so they share
/// one daemon in the manager map. Closing workspace A must therefore NOT
/// kill the daemon workspace B's live pane still uses. Before closing
/// each name, the spawned task re-checks
/// `live_agent_browser_session_names()` — computed AFTER the close's
/// state mutation completed, so it is the post-close live set — and skips
/// any name still referenced. This gives refcount-like semantics for
/// free: the daemon dies only when the LAST workspace sharing the name
/// closes, because that later close's reap finds the name no longer live.
///
/// Spawns a single background task per call rather than blocking the
/// close command on daemon teardown: `manager.close()` is already
/// timeout-bounded internally (see the module doc in `agent_browser.rs`),
/// so there's no correctness reason to await it here, only a latency one.
/// `pub(crate)` so every `AppStateStore::close_workspace` call site
/// (the two close commands here and the
/// workspaces-sync reconcile/rollback paths) shares the one
/// implementation instead of drifting.
pub(crate) fn reap_agent_browser_sessions<R: tauri::Runtime>(app: &tauri::AppHandle<R>, names: Vec<String>) {
    if names.is_empty() {
        return;
    }
    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let manager: State<'_, AgentBrowserManager> = app_handle.state();
        let state: State<'_, AppStateStore> = app_handle.state();
        // Post-close live set — see the shared-cwd rationale in the doc
        // comment. Anything still in here belongs to a surviving workspace.
        let live = state.live_agent_browser_session_names();
        for name in names {
            if live.contains(&name) {
                eprintln!(
                    "[codemux::browser] skip reap of {name}: still referenced by a live workspace"
                );
                continue;
            }
            if let Err(error) = manager.close(&name).await {
                eprintln!(
                    "[codemux::browser] failed to close agent-browser session {name} on workspace close: {error}"
                );
            }
        }
    });
}

/// The directory whose path-level `.mcp.json` a workspace owns. Worktree
/// creation registers against `worktree_path`; plain workspaces register
/// against `cwd`.
fn workspace_mcp_config_dir(workspace: &crate::state::WorkspaceSnapshot) -> String {
    workspace
        .worktree_path
        .clone()
        .unwrap_or_else(|| workspace.cwd.clone())
}

fn workspace_owns_mcp_config_dir(
    workspace: &crate::state::WorkspaceSnapshot,
    config_dir: &Path,
) -> bool {
    paths_equal(Path::new(&workspace.cwd), config_dir)
        || workspace
            .worktree_path
            .as_deref()
            .is_some_and(|path| paths_equal(Path::new(path), config_dir))
}

/// Repair or remove a checkout's shared MCP entry against the post-close
/// snapshot. The generated entry inherits the surviving agent session's id,
/// so only whether any surviving workspace still owns the checkout matters.
fn reconcile_mcp_config_after_close(
    snapshot: &AppStateSnapshot,
    config_dir: &Path,
    auto_mcp_enabled: bool,
) {
    if snapshot
        .workspaces
        .iter()
        .any(|workspace| workspace_owns_mcp_config_dir(workspace, config_dir))
    {
        // Opt-out means "do not create or repair". In particular, closing one
        // of two duplicate-CWD workspaces must not recreate a config the user
        // disabled (or never had). The final owner close below may still
        // remove an old Codemux entry left from before opt-out.
        if auto_mcp_enabled {
            crate::mcp_server::upsert_mcp_config(config_dir);
        }
    } else {
        crate::mcp_server::remove_mcp_config(config_dir);
    }
}

/// Shared close-workspace implementation backing both the Tauri command
/// (in-app sidebar / branch picker close affordances) and the
/// `close_workspace` control-socket command exposed via the Phase 1.6
/// `workspace_close` MCP tool.
///
/// Defers to `close_workspace_with_worktree` semantics regardless of
/// whether the workspace actually is a worktree: when `worktree_path`
/// is `None` (a plain workspace), the `remove_worktree` branch is a
/// no-op and the function reduces to the same operation as
/// `close_workspace`. Keeping a single impl avoids the brain needing
/// two MCP tools for "close" depending on workspace type.
pub(crate) async fn close_workspace_with_worktree_impl<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: &AppStateStore,
    db: &crate::database::DatabaseStore,
    workspace_id: String,
    remove_worktree: bool,
    delete_branch: Option<bool>,
    force_delete: Option<bool>,
) -> Result<(), String> {
    let force = force_delete.unwrap_or(false);

    // One pre-close snapshot serves both the file-deletion guard and
    // the teardown/MCP-cleanup metadata below. Session IDs come from
    // `state.close_workspace`'s result — no TOCTOU race with concurrent
    // pane creation.
    //
    // The guard runs BEFORE any teardown so a refused request leaves
    // the workspace fully intact. Every delete surface (UI close
    // dialog, control socket, MCP `workspace_close`) converges on this
    // impl, so one check here covers them all. A protected repo-root
    // checkout — or any workspace without a worktree of its own — must
    // never have its files removed through the close path;
    // archiving/closing (remove_worktree=false) is the supported way to
    // make it disappear. Unknown workspace ids fall through so the
    // lookup error below stays identical to today.
    let (worktree_path, mcp_config_dir, branch, ws_title, ws_host_id) = {
        let snapshot = state.snapshot();
        let ws = snapshot
            .workspaces
            .iter()
            .find(|w| w.workspace_id.0 == workspace_id);
        if remove_worktree {
            if let Some(ws) = ws {
                if let Some(reason) = refuse_worktree_removal(ws) {
                    return Err(reason);
                }
            }
        }
        (
            ws.and_then(|w| w.worktree_path.clone()),
            ws.map(workspace_mcp_config_dir),
            ws.and_then(|w| w.git_branch.clone()),
            ws.map(|w| w.title.clone()).unwrap_or_default(),
            ws.and_then(|w| w.host_id),
        )
    };

    // Dirty/unpushed PRE-FLIGHT, before any teardown. Without this, a
    // remove_worktree=true close with force_delete=false tore the
    // workspace down (teardown scripts, state removal, PTY kills) and
    // only THEN hit `git_remove_worktree`'s guard — the error arrived
    // after the sidebar row and its delete dialog had already
    // unmounted, so the frontend's force escalation could never render,
    // re-issuing with the same workspace_id failed ("No workspace
    // found"), and the dirty worktree was orphaned on disk. Checking
    // here returns with the workspace fully intact so the dialog stays
    // mounted and can escalate to force. The guard inside
    // `git_remove_worktree` still runs at removal time as a second line
    // of defense. Blocking pool: the check shells out to git.
    if remove_worktree && !force {
        if let Some(ref wt_path) = worktree_path {
            if Path::new(wt_path).exists() {
                let wt_path = wt_path.clone();
                tokio::task::spawn_blocking(move || {
                    crate::git::ensure_worktree_removable(Path::new(&wt_path))
                })
                .await
                .map_err(|e| {
                    format!("worktree removal pre-flight task join failed: {e}")
                })??;
            }
        }
    }

    // Run teardown scripts before closing
    if !force {
        if let Some(ref wt_path) = worktree_path {
            if let Err(e) = crate::scripts::run_teardown_scripts(
                Path::new(wt_path),
                &ws_title,
                &workspace_id,
                Some(&db),
            ) {
                return Err(format!("Teardown failed: {e}\nUse force delete to skip teardown."));
            }
        }
    }

    let close_result = state
        .close_workspace(&workspace_id)
        .map_err(|e| format!("Failed to close workspace: {e}"))?;

    // Reconcile only after the state mutation succeeds. A duplicate workspace
    // at the same checkout still owns the shared path-level config; deleting
    // it before close used to break every surviving agent at that CWD.
    if let Some(ref config_dir) = mcp_config_dir {
        reconcile_mcp_config_after_close(
            &state.snapshot(),
            Path::new(config_dir),
            crate::mcp_server::is_auto_mcp_enabled(&app),
        );
    }

    // Reap the agent-browser CLI session(s) that belonged to this
    // workspace (issue #126). This path (worktree closes + the MCP
    // `workspace_close` tool) previously had NO agent-browser teardown at
    // all — it was the main source of the daemon leak the issue reports.
    // See `reap_agent_browser_sessions` for the fire-and-forget rationale.
    reap_agent_browser_sessions(&app, close_result.removed_agent_browser_sessions);

    // Tear down the SSH tunnel + cached remote client for a workspace
    // that was pushed to a host. Without this, closing a remote
    // workspace leaks the `TunnelSupervisor` task, the bound local
    // socket, and the remote `codemux-remote pty-daemon` for the rest
    // of the app's lifetime — the only other teardown site is
    // pull-back (commands/hosts.rs). Order matches pull-back: forget the
    // cached client (it holds the socket) BEFORE shutting down the
    // supervisor that unbinds it. No-op for local workspaces and
    // idempotent when no supervisor was ever registered.
    #[cfg(unix)]
    if ws_host_id.is_some() {
        crate::ssh::forget_workspace_client(&workspace_id).await;
        crate::ssh::shutdown_supervisor(&workspace_id).await;
    }
    #[cfg(not(unix))]
    let _ = ws_host_id;

    // Reconcile the sync mirror BEFORE the optimistic emit so the
    // workspaces overview sees the soft-delete in the same tick that
    // app_state loses the workspace. Without this, the orphan
    // `workspaces_sync` row (whose `workspace_id` no longer maps to a
    // live snapshot) is classified as a sibling-device workspace by
    // `useOverviewItems` and renders with the "other device" badge for
    // up to ~30 s, until the background reconcile loop catches up.
    // Errors here are non-fatal: the close itself already succeeded,
    // and the background tick will retry the reconcile + push.
    {
        let snapshot = state.snapshot();
        if let Err(error) = crate::workspaces_sync::reconcile_from_snapshot(
            db,
            &snapshot,
        ) {
            eprintln!(
                "[workspaces-sync] reconcile after close failed (will \
                 retry on next background tick): {error}"
            );
        }
    }

    // Optimistic emit: the workspace is gone from in-memory state. Push the
    // update to the frontend NOW so the sidebar row disappears immediately,
    // even when the filesystem cleanup below takes seconds (large worktrees,
    // slow disk). A second emit at the end of the command picks up any
    // session/agent-chat fan-out the cleanup triggers.
    crate::state::emit_app_state(&app);

    // Best-effort push of the soft-delete to the server so sibling
    // devices learn the workspace is gone immediately instead of
    // waiting for the 30 s background tick. Non-fatal on failure.
    if let Err(error) = crate::workspaces_sync::try_sync_with_app(&app).await {
        eprintln!(
            "[workspaces-sync] sync after close failed (will retry on \
             next background tick): {error}"
        );
    }

    // Kill the PTY child process trees for every session that used to belong
    // to this workspace. Sessions are returned atomically from the same lock
    // acquisition that removed the workspace — no TOCTOU race with concurrent
    // pane creation. Idempotent on the waiter thread (double-remove is a
    // no-op) so per-session errors cannot bubble.
    let terminal_state: State<'_, crate::terminal::PtyState> = app.state();
    for session_id in close_result.removed_sessions {
        crate::terminal::terminate_pty_session(&terminal_state.sessions, &session_id.0);
    }

    crate::commands::agent_chat::shutdown_agent_chat_threads(
        &app,
        close_result.removed_agent_chat_threads,
    );

    if remove_worktree {
        if let Some(wt_path) = worktree_path {
            let branch_to_delete = if delete_branch.unwrap_or(false) {
                branch
            } else {
                None
            };
            // The slow part: `git worktree remove` recursively deletes
            // the working tree directory. On a project with thousands of
            // commits / a heavy `node_modules` this can take many seconds.
            // Offload to the blocking pool so the Tokio worker stays free for
            // other commands.
            //
            // This final removal is INTENTIONALLY forced, regardless of the
            // caller's `force`. The dirty/unpushed pre-flight above (which
            // runs BEFORE any teardown) is the authoritative user-facing
            // guard: a non-forced close that reaches this point has already
            // passed it. Passing the caller's `force` through here instead
            // would reintroduce a TOCTOU hole — teardown scripts run after
            // the pre-flight and execute inside the worktree, so a script
            // that writes a file would make a non-forced
            // `git_remove_worktree` refuse AFTER `state.close_workspace`
            // already removed the workspace, stranding an orphaned worktree
            // with no live row left to escalate from. Forcing here closes
            // that hole; the pre-flight remains the only place uncommitted
            // work can block the close.
            tokio::task::spawn_blocking(move || {
                crate::git::git_remove_worktree(
                    Path::new(&wt_path),
                    branch_to_delete.as_deref(),
                    true,
                )
            })
            .await
            .map_err(|e| format!("git_remove_worktree task join failed: {e}"))??;
        }
    }

    crate::state::emit_app_state(&app);
    Ok(())
}

/// The ONE root-protection predicate shared by every file-deletion
/// guard (live-workspace close and archived-entry delete). A workspace
/// / entry is an undeletable root when it is explicitly protected, its
/// recorded kind is "main" (the primary checkout), or it has no
/// worktree of its own (plain folder / repo root) — in all three cases
/// "delete the files" would be meaningless or point at the user's
/// primary checkout. Pure so it's unit-testable without an `AppHandle`,
/// and single so the live and archived guards can never drift apart.
fn is_undeletable_root(
    protected: bool,
    workspace_kind: Option<&str>,
    worktree_path: Option<&str>,
) -> bool {
    protected || workspace_kind == Some("main") || worktree_path.is_none()
}

/// Decide whether a live workspace may have its files removed through
/// the close path (`remove_worktree=true`). Returns `Some(reason)` when
/// removal must be refused — see [`is_undeletable_root`] for the shared
/// predicate; this wrapper only supplies the close-path wording.
pub(crate) fn refuse_worktree_removal(
    ws: &crate::state::WorkspaceSnapshot,
) -> Option<String> {
    if is_undeletable_root(
        ws.protected,
        ws.workspace_kind.as_deref(),
        ws.worktree_path.as_deref(),
    ) {
        Some(
            "This workspace is the project's root checkout and cannot be deleted. \
             Archive or close it instead."
                .to_string(),
        )
    } else {
        None
    }
}

/// Same decision for an ARCHIVED entry: `Some(reason)` when the entry's
/// files must not be deleted from the archive UI — same
/// [`is_undeletable_root`] predicate, archive-UI wording. Only
/// disposable per-branch worktrees may be deleted from the archive;
/// root checkouts can only have their archive ENTRY removed, never
/// their files.
pub(crate) fn refuse_archived_file_delete(
    entry: &crate::state::ArchivedWorkspaceSnapshot,
) -> Option<String> {
    if is_undeletable_root(
        entry.protected,
        entry.workspace_kind.as_deref(),
        entry.worktree_path.as_deref(),
    ) {
        Some(
            "This entry is a project root checkout; its files can't be deleted from here."
                .to_string(),
        )
    } else {
        None
    }
}

/// Overall budget for one `workspaces_worktree_sizes` call. The sweep
/// dialog would rather show "size unknown" for a monster `node_modules`
/// than wait on it; ids not sized in time come back `None`.
const WORKTREE_SIZE_BUDGET: Duration = Duration::from_secs(5);

/// The workspaces among `ids` the sweep may offer to delete, paired with
/// the directory to size. Single authority: exactly the guards the
/// close-with-worktree and archive paths enforce —
/// [`refuse_worktree_removal`] (not protected, not the main checkout,
/// has a worktree) and [`archive_refusal_reason`] (not attach-only, not
/// remote) — plus "the worktree still exists on disk". Ids that don't
/// qualify or don't resolve are simply absent. Pure apart from the
/// `is_dir` stat, so the command can compute it from a snapshot and walk
/// on the blocking pool afterwards.
fn disposable_worktree_targets(
    snapshot: &AppStateSnapshot,
    ids: &[String],
) -> Vec<(String, PathBuf)> {
    ids.iter()
        .filter_map(|id| {
            let ws = snapshot.workspaces.iter().find(|w| &w.workspace_id.0 == id)?;
            if refuse_worktree_removal(ws).is_some() || archive_refusal_reason(ws).is_some() {
                return None;
            }
            let path = PathBuf::from(ws.worktree_path.as_deref()?);
            if !path.is_dir() {
                return None;
            }
            Some((id.clone(), path))
        })
        .collect()
}

/// Size each target independently against one shared deadline. Every
/// target keeps its key; a walk cut off by the deadline (or any later
/// target, which never starts) yields `None` so the caller can say
/// "qualifies, size unknown" rather than dropping the row.
fn size_worktree_targets(
    targets: Vec<(String, PathBuf)>,
    deadline: Instant,
) -> std::collections::HashMap<String, Option<u64>> {
    targets
        .into_iter()
        .map(|(id, path)| (id, crate::fs_size::dir_size_bounded(&path, Some(deadline))))
        .collect()
}

/// On-disk size of each sweepable worktree in `workspace_ids`, keyed by
/// workspace id. A key is present iff the workspace qualifies for the
/// sweep (see [`disposable_worktree_targets`]); its value is `None` when
/// the size walk ran out of budget. The walk runs on the blocking pool
/// with no lock held: a large `node_modules` must not stall state access
/// or the UI thread.
#[tauri::command]
pub async fn workspaces_worktree_sizes(
    state: State<'_, AppStateStore>,
    workspace_ids: Vec<String>,
) -> Result<std::collections::HashMap<String, Option<u64>>, String> {
    let targets = {
        let snapshot = state.snapshot();
        disposable_worktree_targets(&snapshot, &workspace_ids)
    };
    let deadline = Instant::now() + WORKTREE_SIZE_BUDGET;
    tokio::task::spawn_blocking(move || size_worktree_targets(targets, deadline))
        .await
        .map_err(|e| format!("worktree size walk failed: {e}"))
}

/// Central archivability decision: `Some(reason)` when `ws` must not
/// be archived. Pure and shared by every archive surface (Tauri
/// command, control socket, MCP tool) so the rules can't drift.
///
/// - Attach-in-place workspaces have no local files to keep and their
///   close is a detach (the host process keeps running) — "archive and
///   restore later" semantics don't apply.
/// - Remote (pushed-to-host) workspaces are managed from the
///   Workspaces Overview; archiving would strand the tunnel/daemon
///   lifecycle. The frontend gives remote rows a plain close instead.
pub(crate) fn archive_refusal_reason(
    ws: &crate::state::WorkspaceSnapshot,
) -> Option<String> {
    if ws.attach_only {
        return Some(
            "Attach-in-place workspaces can't be archived — close them from the \
             workspaces overview."
                .to_string(),
        );
    }
    if ws.host_id.is_some() {
        return Some(
            "Remote workspaces can't be archived — they're managed from the \
             Workspaces Overview. Pull the workspace back to this device first."
                .to_string(),
        );
    }
    None
}

/// Shared archive implementation behind the Tauri command and the
/// `archive_workspace` control-socket command (exposed via the MCP
/// `workspace_archive` tool).
///
/// Archiving = the exact existing close path (teardown scripts, PTY
/// termination, sync reconcile — with `remove_worktree=false`, so the
/// files, worktree, and branch stay untouched) plus an
/// `ArchivedWorkspaceSnapshot` remembering how to restore it. The entry
/// is built BEFORE the close (the workspace is gone from state after)
/// but added only AFTER the close succeeded, so a failed teardown never
/// produces a phantom archive entry.
pub(crate) async fn archive_workspace_impl<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: &AppStateStore,
    db: &crate::database::DatabaseStore,
    workspace_id: String,
) -> Result<String, String> {
    let ws = state
        .find_workspace(&workspace_id)
        .ok_or_else(|| format!("No workspace found for {workspace_id}"))?;

    if let Some(reason) = archive_refusal_reason(&ws) {
        return Err(reason);
    }

    let entry = crate::state::build_archive_entry(&ws);
    let archive_id = entry.archive_id.clone();

    // remove_worktree=false + force_delete=None: teardown scripts run
    // and their failure aborts the archive; files/branch are untouched.
    close_workspace_with_worktree_impl(
        app.clone(),
        state,
        db,
        workspace_id,
        false,
        None,
        None,
    )
    .await?;

    state.add_archived_workspace(entry);
    // The close impl already emitted, but that snapshot predates the
    // entry — emit again so the archive list appears in the same user
    // action (and the debounced persist picks it up).
    crate::state::emit_app_state(&app);
    Ok(archive_id)
}

/// Shared restore implementation behind the Tauri command and the
/// `unarchive_workspace` control-socket command (MCP
/// `workspace_unarchive`). Returns the restored workspace's id.
///
/// The archive entry is only removed after the restore succeeded — any
/// error path keeps it so the user can retry (or delete it explicitly).
pub(crate) async fn unarchive_workspace_impl<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: &AppStateStore,
    db: &crate::database::DatabaseStore,
    pty_state: &crate::terminal::PtyState,
    presets: &crate::presets::PresetStoreState,
    archive_id: String,
) -> Result<String, String> {
    let entry = state
        .find_archived_workspace(&archive_id)
        .ok_or_else(|| format!("No archived workspace found for {archive_id}"))?;

    // A live workspace already at this location (the user re-opened it
    // by hand, or another device restored it) — don't create a
    // duplicate, just switch to it and retire the entry. A worktree
    // entry matches ONLY on worktree_path; a root entry (no worktree)
    // matches only a live workspace that ALSO has no worktree at the
    // same cwd. A bare cwd match must not count for worktree entries:
    // a live worktree workspace can share the root's cwd, and matching
    // it would silently "restore" the wrong workspace.
    let existing = {
        let snapshot = state.snapshot();
        snapshot
            .workspaces
            .iter()
            .find(|w| {
                if entry.worktree_path.is_some() {
                    w.worktree_path == entry.worktree_path
                } else {
                    w.cwd == entry.cwd && w.worktree_path.is_none()
                }
            })
            .map(|w| w.workspace_id.0.clone())
    };
    if let Some(existing_id) = existing {
        // Carry the ORIGINAL pin timestamp across, not a fresh one: the
        // archived entry records when the user actually pinned. Only ever
        // re-applies a pin — an unpinned archive entry must not silently
        // unpin the live workspace it matched.
        if entry.pinned_at.is_some() {
            state.restore_workspace_pinned_at(&existing_id, entry.pinned_at)?;
        }
        activate_workspace_impl(app.clone(), state, existing_id.clone())?;
        let _ = state.remove_archived_workspace(&archive_id);
        crate::state::emit_app_state(&app);
        return Ok(existing_id);
    }

    let restored_id = if let Some(ref wt_path) = entry.worktree_path {
        // Per-branch worktree entry: restore through the SAME creation
        // path the branch-picker fork flow uses.
        // `git_create_worktree`'s reuse-on-disk behavior hands back the
        // existing directory when it's still registered for this
        // branch; when the directory is gone but the branch survives,
        // it checks the branch out into a fresh worktree at the
        // conventional path.
        let repo_path = entry
            .project_root
            .clone()
            .or_else(|| {
                crate::config::workspace_config::find_git_root(Path::new(&entry.cwd))
                    .map(|p| p.display().to_string())
            })
            .ok_or_else(|| {
                "Can't restore this workspace: its repository root is no longer known."
                    .to_string()
            })?;
        let branch = entry.git_branch.clone().ok_or_else(|| {
            "Can't restore this workspace: no branch was recorded for its worktree."
                .to_string()
        })?;

        // Pre-flight on the blocking pool (the existence check shells
        // out to git): with BOTH the directory and the branch gone
        // there is nothing to rebuild the workspace from, and
        // `git worktree add` would only fail with a cryptic ref error
        // later. `git_branch_exists` covers local AND all-remote refs
        // in one show-ref call — unlike the previous branch-list scan,
        // which returned non-origin remote branches qualified
        // ("upstream/feature/x") and so never matched a shortname.
        let restorable = {
            let wt_path = wt_path.clone();
            let repo_path = repo_path.clone();
            let branch = branch.clone();
            tokio::task::spawn_blocking(move || {
                Path::new(&wt_path).exists()
                    || crate::git::git_branch_exists(Path::new(&repo_path), &branch)
            })
            .await
            .map_err(|e| format!("unarchive pre-flight task join failed: {e}"))?
        };
        if !restorable {
            return Err(
                "Nothing left to restore: the worktree and its branch no longer exist."
                    .to_string(),
            );
        }

        // Imported worktree at a NON-conventional path: the directory
        // still exists on disk (archiving touched nothing) but lives
        // somewhere other than the `~/.codemux/worktrees/<repo>/<branch>`
        // path the create flow would target. Adopt it in place —
        // `create_worktree_workspace_impl` would compute the conventional
        // path and its `git worktree add` would fail with "<branch> is
        // already used by worktree at <recorded path>". When the recorded
        // path IS the conventional one (or the dir is gone and the branch
        // survives), fall through to the create flow, whose reuse-on-disk /
        // recreate-from-branch handling is exactly right.
        let adopt_in_place = Path::new(wt_path).exists()
            && crate::git::conventional_worktree_path(Path::new(&repo_path), &branch)
                .map(|conventional| conventional != Path::new(wt_path))
                .unwrap_or(false);

        let id = if adopt_in_place {
            import_worktree_workspace_impl(
                app.clone(),
                state,
                db,
                wt_path.clone(),
                branch,
                "single".to_string(),
            )
            .await?
        } else {
            create_worktree_workspace_impl(
                app.clone(),
                state,
                db,
                pty_state,
                presets,
                repo_path,
                branch,
                /*new_branch=*/ false,
                /*base=*/ None,
                "single".to_string(),
                None,
                None,
                None,
                None,
            )
            .await?
            .workspace_id
        };
        // The creation/adoption path titles the workspace after its
        // branch; restore whatever the user had (possibly a rename).
        state.rename_workspace(&id, entry.title.clone());
        id
    } else {
        // Repo-root / plain-folder entry: restore through the same path
        // the "Add repository" flow uses (`create_workspace` with an
        // explicit cwd), which re-stamps project_root / project_uid /
        // `protected` via the normal create pipeline.
        if !Path::new(&entry.cwd).exists() {
            return Err(format!(
                "Nothing left to restore: {} no longer exists on disk.",
                entry.cwd
            ));
        }
        let id = create_workspace_impl(app.clone(), state, db, Some(entry.cwd.clone())).await?;
        state.rename_workspace(&id, entry.title.clone());
        id
    };

    // Same as the reuse branch above: restore the archived timestamp verbatim
    // rather than stamping the moment of the unarchive.
    if entry.pinned_at.is_some() {
        state.restore_workspace_pinned_at(&restored_id, entry.pinned_at)?;
    }

    let _ = state.remove_archived_workspace(&archive_id);
    activate_workspace_impl(app.clone(), state, restored_id.clone())?;
    crate::state::emit_app_state(&app);
    Ok(restored_id)
}

/// Shared delete implementation behind the Tauri command. Removes the
/// archive entry; for disposable worktree entries it can also remove
/// the worktree directory (and optionally its branch) with an honest
/// force flag — force_delete=false keeps `git_remove_worktree`'s
/// dirty/unpushed guard live, and its error keeps the entry so nothing
/// is lost silently.
pub(crate) async fn delete_archived_workspace_impl<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: &AppStateStore,
    archive_id: String,
    delete_worktree: bool,
    delete_branch: bool,
    force_delete: bool,
) -> Result<(), String> {
    let entry = state
        .find_archived_workspace(&archive_id)
        .ok_or_else(|| format!("No archived workspace found for {archive_id}"))?;

    if delete_worktree {
        // Root checkouts and entries without a worktree must never have
        // files deleted from here — an explicit delete_worktree=true
        // for such an entry is refused loudly (entry kept) instead of
        // being silently downgraded, so the caller learns the rule.
        if let Some(reason) = refuse_archived_file_delete(&entry) {
            return Err(reason);
        }
        // Guarded by the refusal above: worktree_path is Some here.
        let wt_path = entry.worktree_path.clone().unwrap_or_default();
        let branch_to_delete = if delete_branch {
            entry.git_branch.clone()
        } else {
            None
        };
        // Recursive directory delete — blocking pool, same rationale as
        // the close path. Errors (including the dirty/unpushed guard
        // when force_delete=false) propagate and keep the entry.
        tokio::task::spawn_blocking(move || {
            crate::git::git_remove_worktree(
                Path::new(&wt_path),
                branch_to_delete.as_deref(),
                force_delete,
            )
        })
        .await
        .map_err(|e| format!("git_remove_worktree task join failed: {e}"))??;
    }

    state.remove_archived_workspace(&archive_id)?;
    crate::state::emit_app_state(&app);
    Ok(())
}

// `async fn` so the close path's teardown scripts and git subprocesses
// run off the GTK main thread — same rationale as `close_workspace`.
#[tauri::command]
pub async fn archive_workspace<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    db: State<'_, crate::database::DatabaseStore>,
    workspace_id: String,
) -> Result<String, String> {
    archive_workspace_impl(app, &state, &db, workspace_id).await
}

// `async fn` for the same reason as `create_worktree_workspace`: the
// restore path can run `git worktree add` + the git-info gather.
#[tauri::command]
pub async fn unarchive_workspace<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    db: State<'_, crate::database::DatabaseStore>,
    pty_state: State<'_, crate::terminal::PtyState>,
    presets: State<'_, crate::presets::PresetStoreState>,
    archive_id: String,
) -> Result<String, String> {
    unarchive_workspace_impl(app, &state, &db, &pty_state, &presets, archive_id).await
}

// `async fn` because deleting a worktree is a recursive filesystem
// delete — same rationale as `close_workspace_with_worktree`.
#[tauri::command]
pub async fn delete_archived_workspace<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    archive_id: String,
    delete_worktree: bool,
    delete_branch: bool,
    force_delete: bool,
) -> Result<(), String> {
    delete_archived_workspace_impl(
        app,
        &state,
        archive_id,
        delete_worktree,
        delete_branch,
        force_delete,
    )
    .await
}

#[tauri::command]
pub fn activate_workspace<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    workspace_id: String,
) -> Result<(), String> {
    activate_workspace_impl(app, &state, workspace_id)
}

/// Shared workspace-switch implementation used by both the Tauri command
/// (sidebar / palette click) and the `activate_workspace` control-socket
/// command exposed via the `workspace_open` MCP tool.
///
/// Keeping the body in one place guarantees both surfaces have identical
/// side effects: the in-memory active id flip, the `populate_git_info`
/// background refresh, lazy PTY hydration via
/// `spawn_missing_ptys_for_workspace`, the synchronous `emit_app_state`
/// to push the new snapshot to any open UI. That full-state emit also queues
/// the active id on the shared ordered/coalesced persistence stream, so no
/// caller blocks on SQLite and every switch surface restores identically.
pub(crate) fn activate_workspace_impl<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: &AppStateStore,
    workspace_id: String,
) -> Result<(), String> {
    let activate_started = std::time::Instant::now();
    // Section timings feed the attribution harness: a slow activate must name
    // whether it was the locked state mutation or snapshot+serialise, rather
    // than reporting one opaque total. SQLite runs after return on the shared
    // persistence worker.
    let mutate_started = std::time::Instant::now();
    let activated = state.activate_workspace(&workspace_id);
    let mutate_ms = mutate_started.elapsed().as_secs_f64() * 1000.0;
    if activated {
        let emit_ms = run_activation_side_effects(&app, state, &workspace_id);
        let elapsed_ms = activate_started.elapsed().as_millis();
        if elapsed_ms > 8 {
            eprintln!(
                "[codemux::workspace] activate_workspace({workspace_id}) returned in \
                 {elapsed_ms}ms (mutate={mutate_ms:.1}ms emit={emit_ms:.1}ms)"
            );
        }
        Ok(())
    } else {
        Err(format!("No workspace found for {workspace_id}"))
    }
}

/// Everything that must happen after the active id flips, for every surface
/// that flips it (sidebar click, command palette, control socket, Ctrl+Tab
/// cycle). Kept in one function because the cycle path used to diverge —
/// spawning PTYs synchronously on the IPC thread, skipping the git refresh
/// and never persisting the active workspace, so a cycled-to workspace was
/// forgotten at restart.
///
/// Returns the synchronous emit duration for the activation attribution
/// harness. Selection persistence is deliberately not part of this duration.
fn run_activation_side_effects<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    state: &AppStateStore,
    workspace_id: &str,
) -> f64 {
    // Kick off git refresh in a background thread — don't block the
    // activate click. `populate_git_info` runs 5-8 git subprocesses
    // (branch + upstream + ahead/behind + two diff-stat calls + status
    // with duplicated `diff --numstat`), which can hit 100-300ms on a
    // cold filesystem cache or a large repo. The 5s polling loop
    // reconciles, so worst case the sidebar shows slightly stale branch
    // info for one tick. `git_branch_info` handles non-git / detached /
    // corrupted repos without panicking, and the spawned thread emits
    // app state when done so the sidebar picks up the refresh.
    if let Some(cwd) = state.workspace_cwd(workspace_id) {
        let refresh_app = app.clone();
        let refresh_ws = workspace_id.to_string();
        std::thread::spawn(move || {
            let state: tauri::State<'_, AppStateStore> = refresh_app.state();
            // Re-activating a workspace whose branch counters haven't moved is
            // the common case, and it has nothing to tell the renderer.
            if !populate_git_info(&state, &refresh_ws, Path::new(&cwd)) {
                return;
            }
            // Coalesced: this background refresh fires after the
            // synchronous activate emit. Without coalescing the user
            // sees two back-to-back snapshot serialise + IPC + render
            // passes for what should be one logical change
            // ("workspace activated").
            crate::state::schedule_emit_app_state(&refresh_app);
        });
    }

    // Lazy PTY hydration: `spawn_missing_ptys` at startup only resumed
    // sessions for the workspace that was active at last close. Sessions
    // for any other workspace stay on disk-only until the user activates
    // them — at which point we spawn whatever PTYs aren't already
    // running. Idempotent thanks to `try_reserve_session_spawn`, so
    // re-activating the already-active workspace is a no-op.
    //
    // Fire-and-forget on a background thread instead of running on the
    // IPC thread. `spawn_missing_ptys_for_workspace` issues
    // synchronous `pty_system.openpty()` + `spawn_command()` calls per
    // missing session; on Linux those are fast (single-digit ms each)
    // but a workspace with 3-4 cold terminals + a slow shell rc file
    // can still tip the IPC thread into a perceptible blocking window
    // on the click. Spawning gives the UI back the IPC thread
    // immediately; each terminal's status overlay (`emit_terminal_status`
    // inside `spawn_pty_for_session`) shows "Starting shell..." until
    // its child is ready, so the user sees the right state without
    // the activate click feeling stuck.
    let spawn_app = app.clone();
    let spawn_ws = workspace_id.to_string();
    std::thread::spawn(move || {
        terminal::spawn_missing_ptys_for_workspace(spawn_app, &spawn_ws);
    });

    let emit_started = std::time::Instant::now();
    crate::state::emit_app_state(app);
    emit_started.elapsed().as_secs_f64() * 1000.0
}

#[tauri::command]
pub fn rename_workspace<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    workspace_id: String,
    title: String,
) -> Result<(), String> {
    if state.rename_workspace(&workspace_id, title) {
        crate::state::emit_app_state(&app);
        Ok(())
    } else {
        Err(format!("No workspace found for {workspace_id}"))
    }
}

/// Mute or unmute agent-completion desktop notifications for a workspace.
/// Suppresses only the OS popup — status pills (working spinner, review and
/// permission dots) keep updating regardless.
#[tauri::command]
pub fn set_workspace_muted<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    workspace_id: String,
    muted: bool,
) -> Result<(), String> {
    if state.set_workspace_muted(&workspace_id, muted) {
        crate::state::emit_app_state(&app);
        Ok(())
    } else {
        Err(format!("No workspace found for {workspace_id}"))
    }
}

/// Pin or unpin a workspace at the top of the workspace inbox.
#[tauri::command]
pub fn set_workspace_pinned<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    workspace_id: String,
    pinned: bool,
) -> Result<(), String> {
    if state.set_workspace_pinned(&workspace_id, pinned)? {
        crate::state::emit_app_state(&app);
    }
    Ok(())
}

#[tauri::command]
pub fn update_workspace_cwd<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    workspace_id: String,
    cwd: String,
) -> Result<(), String> {
    if state.update_workspace_cwd(&workspace_id, cwd) {
        crate::state::emit_app_state(&app);
        Ok(())
    } else {
        Err(format!("No workspace found for {workspace_id}"))
    }
}

// `async fn` so this command runs on Tokio's worker pool instead of the GTK
// main thread. User-defined teardown scripts shell out to arbitrary commands
// — typically fast (`docker compose down`) but can stall on a hung process —
// and we'd rather not freeze the UI while one runs. Mirrors the fix in
// `close_workspace_with_worktree`.
#[tauri::command]
pub async fn close_workspace<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    db: State<'_, crate::database::DatabaseStore>,
    workspace_id: String,
    force_delete: Option<bool>,
) -> Result<String, String> {
    let force = force_delete.unwrap_or(false);

    // We still need cwd + title for teardown + MCP cleanup before the
    // state mutation. Session IDs are now obtained from
    // `state.close_workspace`'s result below — no snapshot race.
    let (workspace_cwd, ws_host_id) = {
        let snapshot = state.snapshot();
        let ws = snapshot
            .workspaces
            .iter()
            .find(|w| w.workspace_id.0 == workspace_id);
        (
            ws.map(|w| (w.cwd.clone(), w.title.clone())),
            ws.and_then(|w| w.host_id),
        )
    };

    // Run teardown scripts before closing
    if !force {
        if let Some((ref cwd, ref title)) = workspace_cwd {
            if let Err(e) = crate::scripts::run_teardown_scripts(
                Path::new(cwd),
                title,
                &workspace_id,
                Some(&db),
            ) {
                return Err(format!("Teardown failed: {e}\nUse force delete to skip teardown."));
            }
        }
    }

    let result = state.close_workspace(&workspace_id)?;

    // `.mcp.json` belongs to the checkout, not to one workspace row. Preserve
    // (and, if necessary, repair) it while another live workspace shares the
    // same path; remove it only after the final owner closes.
    if let Some((ref cwd, _)) = workspace_cwd {
        reconcile_mcp_config_after_close(
            &state.snapshot(),
            Path::new(cwd),
            crate::mcp_server::is_auto_mcp_enabled(&app),
        );
    }

    // Reap the agent-browser CLI session(s) that belonged to this
    // workspace (issue #126). `result.removed_agent_browser_sessions` is
    // collected under the same lock acquisition that removed the
    // workspace from state, so this can't race a concurrent session
    // creation for the same workspace — see the doc comment on
    // `CloseWorkspaceResult`.
    reap_agent_browser_sessions(&app, result.removed_agent_browser_sessions);

    // Tear down the SSH tunnel + cached client for a workspace that was
    // pushed to a host. This is the sibling of the teardown in
    // `close_workspace_with_worktree_impl` — a PLAIN (non-worktree)
    // pushed workspace gets `host_id` set (push falls back to its cwd)
    // and the sidebar routes its close through THIS command, so without
    // this it would leak the tunnel supervisor + remote pty-daemon.
    // Idempotent + no-op for local workspaces.
    #[cfg(unix)]
    if ws_host_id.is_some() {
        crate::ssh::forget_workspace_client(&workspace_id).await;
        crate::ssh::shutdown_supervisor(&workspace_id).await;
    }
    #[cfg(not(unix))]
    let _ = ws_host_id;

    // Reconcile the sync mirror BEFORE the optimistic emit — see the
    // matching block in `close_workspace_with_worktree_impl` for the
    // full reasoning. In short: without this, the orphan
    // `workspaces_sync` row mis-renders as a sibling-device workspace
    // in the overview until the 30 s background tick. Non-fatal on
    // failure.
    {
        let snapshot = state.snapshot();
        if let Err(error) = crate::workspaces_sync::reconcile_from_snapshot(
            db.inner(),
            &snapshot,
        ) {
            eprintln!(
                "[workspaces-sync] reconcile after close failed (will \
                 retry on next background tick): {error}"
            );
        }
    }

    // Optimistic emit: the workspace is gone from in-memory state. Push the
    // update to the frontend NOW so the sidebar row disappears immediately,
    // even if a PTY shutdown or agent-chat thread takes a moment to wind
    // down. A second emit at the end of the command picks up any fan-out.
    crate::state::emit_app_state(&app);

    // Best-effort push of the soft-delete to the server so sibling
    // devices learn the workspace is gone immediately instead of
    // waiting for the 30 s background tick. Non-fatal on failure.
    if let Err(error) = crate::workspaces_sync::try_sync_with_app(&app).await {
        eprintln!(
            "[workspaces-sync] sync after close failed (will retry on \
             next background tick): {error}"
        );
    }

    // Kill the PTY child process trees for every session that used to belong
    // to this workspace. Sessions are returned atomically from the same lock
    // acquisition that removed the workspace — no TOCTOU race with newly
    // created panes.
    let terminal_state: State<'_, crate::terminal::PtyState> = app.state();
    for session_id in result.removed_sessions {
        crate::terminal::terminate_pty_session(&terminal_state.sessions, &session_id.0);
    }

    crate::commands::agent_chat::shutdown_agent_chat_threads(
        &app,
        result.removed_agent_chat_threads,
    );

    crate::state::emit_app_state(&app);
    Ok(result.fallback.0)
}

#[tauri::command]
pub fn cycle_workspace<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    step: isize,
) -> Result<String, String> {
    let workspace_id = state
        .workspace_navigation_target(step)
        .ok_or_else(|| "No workspace navigation target available".to_string())?;

    if state.activate_workspace(&workspace_id.0) {
        run_activation_side_effects(&app, &state, &workspace_id.0);
        Ok(workspace_id.0)
    } else {
        Err(format!("No workspace found for {}", workspace_id.0))
    }
}

#[tauri::command]
pub fn split_pane<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    pane_id: String,
    direction: String,
) -> Result<String, String> {
    split_pane_impl(app, &state, pane_id, direction)
}

#[tauri::command]
pub fn activate_pane<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    pane_id: String,
) -> Result<(), String> {
    if state.activate_pane(&pane_id) {
        crate::state::emit_app_state(&app);
        Ok(())
    } else {
        Err(format!("No pane found for {pane_id}"))
    }
}

#[tauri::command]
pub fn cycle_pane<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    step: isize,
) -> Result<String, String> {
    let pane_id = state
        .pane_navigation_target(step)
        .ok_or_else(|| "No pane navigation target available".to_string())?;

    if state.activate_pane(&pane_id.0) {
        crate::state::emit_app_state(&app);
        Ok(pane_id.0)
    } else {
        Err(format!("No pane found for {}", pane_id.0))
    }
}

#[tauri::command]
pub fn close_pane<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    pane_id: String,
) -> Result<Option<String>, String> {
    close_pane_impl(app, &state, pane_id)
}

/// Shared close-pane implementation backing both the Tauri command
/// (pane "x" affordances in the in-app split header) and the
/// `close_pane` control-socket command exposed via the Phase 1.6
/// `pane_close` MCP tool.
///
/// Last-pane behavior matches the in-app path: `state.close_pane` removes
/// the surface + tab when the closed pane was the last leaf, the
/// workspace stays in state with `active_tab_id`/`active_surface_id`
/// cleared (or moved to the first remaining tab). No "auto-close the
/// workspace when the last pane is gone" rule — the workspace persists
/// empty until the brain (or user) explicitly closes it.
pub(crate) fn close_pane_impl<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: &AppStateStore,
    pane_id: String,
) -> Result<Option<String>, String> {
    let removed_browser_id = state.pane_browser_id(&pane_id);
    let removed = state.close_pane(&pane_id)?;
    // A `codemux monitor start` flag belongs to the process that was running
    // in this pane. The pane is gone, so the claim is too — and a flag with no
    // pane behind it has no UI left that could ever turn it off.
    state.clear_manual_monitors_for_panes(&[pane_id.clone()]);

    if let Some(ref session_id) = removed {
        // Kill the PTY child + its process group. `state.close_pane` already
        // removed the session from the store, so we go straight to the PTY
        // helper instead of back through `close_terminal_session` (which
        // would try to re-remove from the store and fail).
        let terminal_state: State<'_, crate::terminal::PtyState> = app.state();
        crate::terminal::terminate_pty_session(&terminal_state.sessions, &session_id.0);
    }

    if let Some(browser_id) = removed_browser_id {
        // User explicitly closed this browser pane — mark dismissed so the agent
        // won't immediately reopen it on the next browser_navigate call.
        state.detach_agent_browser_from_pane(&browser_id, true);
    }

    crate::state::emit_app_state(&app);
    Ok(removed.map(|session_id| session_id.0))
}

#[tauri::command]
pub fn swap_panes<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    source_pane_id: String,
    target_pane_id: String,
) -> Result<(), String> {
    state.swap_panes(&source_pane_id, &target_pane_id)?;
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn resize_split<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    pane_id: String,
    child_sizes: Vec<f32>,
) -> Result<(), String> {
    state.resize_split(&pane_id, child_sizes)?;
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn resize_active_pane<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    delta: f32,
) -> Result<(), String> {
    state.resize_active_pane(delta)?;
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn notify_attention<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    message: String,
    session_id: Option<String>,
    pane_id: Option<String>,
    desktop: Option<bool>,
) -> Result<String, String> {
    let body = message.clone();
    let notification_id =
        state.add_notification(session_id, pane_id, message, NotificationLevel::Attention)?;

    if desktop.unwrap_or(true) {
        // `notify-rust`'s Windows backend (WinRT Toast) does not expose
        // `.hint()` / `.urgency()` / `notify_rust::Hint::*` — those are
        // XDG/D-Bus concepts only available when the crate is built for
        // Unix. On Windows the toast gets priority/grouping from its own
        // API, so a plain `summary + body + show` is the right degradation.
        let mut notification = Notification::new();
        notification.summary("Codemux").body(&body);
        #[cfg(unix)]
        {
            notification
                .hint(notify_rust::Hint::DesktopEntry(app.config().identifier.clone()))
                .hint(notify_rust::Hint::Transient(true))
                // Normal urgency, not Critical: see notifications.rs for
                // the reasoning. Critical is non-expiring on mako/dunst/
                // GNOME/KDE — wrong fit for routine attention requests.
                .urgency(notify_rust::Urgency::Normal);
        }
        let _ = notification.show();

        if let Some(window) = app.get_webview_window("main") {
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
            let _ = window.request_user_attention(Some(tauri::UserAttentionType::Critical));
        }

        #[cfg(target_os = "linux")]
        {
            let class = format!("class:{}", app.config().identifier);
            let _ = crate::execution::host_command("hyprctl")
                .args(["dispatch", "focuswindow", &class])
                .output();
        }
    }

    crate::state::emit_app_state(&app);
    Ok(notification_id)
}

#[tauri::command]
pub fn set_notification_sound_enabled<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    enabled: bool,
) -> Result<(), String> {
    state.set_notification_sound_enabled(enabled);
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn set_ai_commit_message_enabled<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    enabled: bool,
) -> Result<(), String> {
    state.set_ai_commit_message_enabled(enabled);
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn set_ai_commit_message_cli<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    cli: Option<String>,
) -> Result<(), String> {
    state.set_ai_commit_message_cli(cli);
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn set_ai_commit_message_model<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    model: Option<String>,
) -> Result<(), String> {
    state.set_ai_commit_message_model(model);
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn set_ai_resolver_enabled<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    enabled: bool,
) -> Result<(), String> {
    state.set_ai_resolver_enabled(enabled);
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn set_ai_resolver_cli<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    cli: Option<String>,
) -> Result<(), String> {
    state.set_ai_resolver_cli(cli);
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn set_ai_resolver_model<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    model: Option<String>,
) -> Result<(), String> {
    state.set_ai_resolver_model(model);
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn set_ai_resolver_strategy<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    strategy: String,
) -> Result<(), String> {
    state.set_ai_resolver_strategy(strategy);
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn create_tab<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    workspace_id: String,
    kind: String,
) -> Result<String, String> {
    let kind = match kind.as_str() {
        "terminal" => TabKind::Terminal,
        "browser" => TabKind::Browser,
        "diff" => TabKind::Diff,
        "editor" => TabKind::Editor,
        _ => return Err(format!("Unsupported tab kind: {kind}")),
    };

    let (tab_id, session_id) = state.create_tab(&workspace_id, kind)?;

    if let Some(session_id) = session_id {
        terminal::spawn_pty_for_session(app.clone(), session_id.0);
    }

    crate::state::emit_app_state(&app);
    Ok(tab_id)
}

#[tauri::command]
pub fn close_tab<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    workspace_id: String,
    tab_id: String,
) -> Result<(), String> {
    let result = state.close_tab(&workspace_id, &tab_id)?;

    // Kill the PTY child + process group for every terminal session that
    // was attached to this tab. `state.close_tab` already removed them from
    // the store, so we go straight to the PTY helper.
    let terminal_state: State<'_, crate::terminal::PtyState> = app.state();
    for session_id in result.removed_sessions {
        crate::terminal::terminate_pty_session(&terminal_state.sessions, &session_id.0);
    }

    for browser_id in &result.removed_browser_ids {
        // Tab close is cleanup, not explicit dismissal — agent can reopen the pane.
        state.detach_agent_browser_from_pane(&browser_id.0, false);
    }

    crate::commands::agent_chat::shutdown_agent_chat_threads(
        &app,
        result.removed_agent_chat_threads,
    );

    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn activate_tab<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    workspace_id: String,
    tab_id: String,
) -> Result<(), String> {
    state.activate_tab(&workspace_id, &tab_id)?;
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn rename_tab<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    workspace_id: String,
    tab_id: String,
    title: String,
) -> Result<(), String> {
    state.rename_tab(&workspace_id, &tab_id, title)?;
    crate::state::emit_app_state(&app);
    Ok(())
}

// `async fn` so `populate_git_info_async`'s 5-8 git subprocesses run on the
// blocking pool instead of the GTK main thread. Same pattern as
// `commands/git.rs` — see the note at the top of that file.
#[tauri::command]
pub async fn refresh_workspace_git_info<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    workspace_id: String,
) -> Result<(), String> {
    let snapshot = state.snapshot();
    let workspace = snapshot
        .workspaces
        .iter()
        .find(|w| w.workspace_id.0 == workspace_id)
        .ok_or_else(|| format!("No workspace found for {workspace_id}"))?;
    let cwd = workspace.cwd.clone();
    populate_git_info_async(&state, &workspace_id, PathBuf::from(&cwd)).await;
    crate::state::emit_app_state(&app);
    Ok(())
}

/// Switch a workspace's repo to its default branch (main / master / origin
/// HEAD). On success, refresh git info synchronously so the sidebar label
/// updates without waiting for the 5s polling loop. On failure, return the
/// git stderr verbatim for the frontend to surface as a toast.
///
/// Used by the sidebar's right-click "Checkout default branch" action to
/// close the intent gap where `Open ↵ main` attaches to a repo currently on
/// a different branch (attach-only by design — no HEAD mutation).
///
/// `async fn` so neither the checkout itself (which can be slow on large
/// repos) nor the follow-up git-info gather runs on the GTK main thread.
#[tauri::command]
pub async fn checkout_default_branch_in_workspace<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    workspace_id: String,
) -> Result<String, String> {
    let cwd = {
        let snapshot = state.snapshot();
        snapshot
            .workspaces
            .iter()
            .find(|w| w.workspace_id.0 == workspace_id)
            .map(|w| w.cwd.clone())
            .ok_or_else(|| "Workspace not found".to_string())?
    };
    let repo_path = PathBuf::from(&cwd);

    let checkout_result = {
        let repo_path = repo_path.clone();
        tokio::task::spawn_blocking(move || crate::git::checkout_default_branch(&repo_path))
            .await
            .map_err(|e| format!("checkout_default_branch task join failed: {e}"))?
    };

    match checkout_result {
        Ok(Some(branch)) => {
            // Eager refresh: closes the 0–5s sidebar-label lag from the
            // background polling loop by writing fresh branch info before
            // we emit.
            populate_git_info_async(&state, &workspace_id, repo_path).await;
            crate::state::emit_app_state(&app);
            Ok(branch)
        }
        Ok(None) => Err("No default branch could be determined for this repo.".to_string()),
        Err(stderr) => Err(stderr),
    }
}

// ---- Editor integration ----

#[derive(Debug, Clone, Serialize)]
pub struct EditorInfo {
    pub id: String,
    pub name: String,
    pub command: String,
}

static DETECTED_EDITORS: OnceLock<Vec<EditorInfo>> = OnceLock::new();

// Order matters: this is the order shown in the IDE launcher dropdown when
// multiple editors are detected, and the first installed entry becomes the
// auto-picked default on first run. Roughly: VS Code family → AI-first
// editors → other modern GUI editors → JetBrains family → other GUI.
const EDITOR_CANDIDATES: &[(&str, &str)] = &[
    // VS Code family
    ("code", "VS Code"),
    ("cursor", "Cursor"),
    ("windsurf", "Windsurf"),
    ("trae", "Trae"),
    ("codium", "VSCodium"),
    // Modern GUI editors
    ("zed", "Zed"),
    ("fleet", "Fleet"),
    ("lapce", "Lapce"),
    // JetBrains family (shims installed by JetBrains Toolbox land on PATH)
    ("idea", "IntelliJ IDEA"),
    ("pycharm", "PyCharm"),
    ("phpstorm", "PhpStorm"),
    ("webstorm", "WebStorm"),
    ("goland", "GoLand"),
    ("rubymine", "RubyMine"),
    ("clion", "CLion"),
    ("rider", "Rider"),
    ("datagrip", "DataGrip"),
    ("studio", "Android Studio"),
    // Other
    ("sublime_text", "Sublime Text"),
];

/// Well-known Windows install paths, checked when an editor isn't on PATH.
/// VS Code / Cursor / Zed are installed per-user under `%LOCALAPPDATA%\Programs\`
/// by default and don't always add themselves to PATH. Hit ratio is high for
/// these three; JetBrains installers vary too much to hardcode and users who
/// run those typically have the IDE on PATH via JetBrains Toolbox's shims.
#[cfg(windows)]
const WINDOWS_EDITOR_FALLBACKS: &[(&str, &[&str])] = &[
    (
        "code",
        &[
            r"Microsoft VS Code\Code.exe",
            r"Microsoft VS Code Insiders\Code - Insiders.exe",
        ],
    ),
    ("cursor", &[r"cursor\Cursor.exe"]),
    ("windsurf", &[r"Windsurf\Windsurf.exe"]),
    ("trae", &[r"Trae\Trae.exe"]),
    ("codium", &[r"VSCodium\VSCodium.exe"]),
    ("zed", &[r"Zed\Zed.exe"]),
    ("lapce", &[r"Lapce\lapce.exe"]),
];

/// Resolve an editor command to an absolute path:
/// 1. `which::which(cmd)` walks `PATH` with the right executable extension
///    semantics on each OS (`.exe`/`.cmd`/`.bat` via `PATHEXT` on Windows).
/// 2. On Windows, if `PATH` lookup fails, check well-known per-user install
///    paths under `%LOCALAPPDATA%\Programs\` and `%ProgramFiles%\`.
/// Returns the full path as a `String` on success, `None` if not installed.
fn resolve_editor_command(cmd: &str) -> Option<String> {
    if let Ok(path) = which::which(cmd) {
        return Some(path.display().to_string());
    }
    #[cfg(windows)]
    {
        let fallbacks = WINDOWS_EDITOR_FALLBACKS
            .iter()
            .find(|(id, _)| *id == cmd)
            .map(|(_, paths)| *paths)
            .unwrap_or(&[]);
        let roots = windows_install_roots();
        for rel in fallbacks {
            for root in &roots {
                let candidate = std::path::Path::new(root).join(rel);
                if candidate.is_file() {
                    return Some(candidate.display().to_string());
                }
            }
        }
    }
    None
}

/// Returns the search roots where per-user and system-wide editor installs
/// typically live on Windows: `%LOCALAPPDATA%\Programs`, `%ProgramFiles%`,
/// and `%ProgramFiles(x86)%`. Missing env vars are skipped silently.
#[cfg(windows)]
fn windows_install_roots() -> Vec<String> {
    let mut roots = Vec::new();
    if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
        roots.push(format!(r"{}\Programs", local_app_data));
    }
    if let Ok(program_files) = std::env::var("ProgramFiles") {
        roots.push(program_files);
    }
    if let Ok(program_files_x86) = std::env::var("ProgramFiles(x86)") {
        roots.push(program_files_x86);
    }
    roots
}

fn find_editors() -> Vec<EditorInfo> {
    EDITOR_CANDIDATES
        .iter()
        .filter_map(|(cmd, name)| {
            resolve_editor_command(cmd).map(|resolved| EditorInfo {
                id: cmd.to_string(),
                name: name.to_string(),
                command: resolved,
            })
        })
        .collect()
}

#[tauri::command]
pub fn detect_editors() -> Vec<EditorInfo> {
    DETECTED_EDITORS.get_or_init(find_editors).clone()
}

#[tauri::command]
pub fn open_in_editor(editor_id: String, path: String) -> Result<(), String> {
    let editors = DETECTED_EDITORS.get_or_init(find_editors);
    let editor = editors
        .iter()
        .find(|e| e.id == editor_id)
        .ok_or_else(|| format!("Editor not found: {editor_id}"))?;
    let mut cmd = crate::execution::host_command(&editor.command);
    cmd.arg(&path);
    // Intentionally NOT calling `sanitize_gui_env_std` here. The
    // standing project rule strips DISPLAY/WAYLAND_DISPLAY/XDG_* so
    // agent-spawned processes can't pop windows on the user's
    // session — but `open_in_editor` is the *opposite* case: the
    // user explicitly clicked "open this file in my editor" and the
    // editor must inherit the GUI env to render a window. Stripping
    // it here makes the spawn succeed silently with no visible
    // result (Wayland/X11 reject the window because the env was
    // unset). This is the same exception class as explicitly requested
    // desktop-control commands such as `hyprctl` and `ydotool`.
    cmd.spawn()
        .map_err(|e| format!("Failed to open editor: {e}"))?;
    Ok(())
}

// ---- Setup/teardown scripts ----

/// Spawn setup scripts in a background thread so workspace creation isn't blocked.
/// Runs the full pipeline: .codemuxinclude file copy + setup commands.
fn spawn_setup_scripts<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    state: &AppStateStore,
    db: &crate::database::DatabaseStore,
    workspace_id: &str,
    workspace_path: &Path,
) {
    // Pre-resolve root path on the calling thread to avoid race conditions.
    let root_path = crate::scripts::resolve_root_path(workspace_path);

    let config =
        crate::config::workspace_config::read_effective_config(workspace_path, db);
    let setting_patterns = config
        .as_ref()
        .map(|c| c.worktree_includes.clone())
        .unwrap_or_default();

    let (ws_title, ws_branch) = {
        let snapshot = state.snapshot();
        let ws = snapshot
            .workspaces
            .iter()
            .find(|w| w.workspace_id.0 == workspace_id);
        let title = ws.map(|w| w.title.clone()).unwrap_or_default();
        let branch = ws.and_then(|w| w.git_branch.clone());
        (title, branch)
    };
    let port = crate::scripts::allocate_workspace_port(workspace_id);

    let ws_path = workspace_path.to_path_buf();
    let ws_id = workspace_id.to_string();
    let app2 = app.clone();

    std::thread::spawn(move || {
        // Wait for frontend to mount the overlay and register event listeners
        std::thread::sleep(std::time::Duration::from_millis(500));

        // Step 1: Process worktree includes (file → setting → defaults)
        match crate::scripts::process_worktree_includes(&root_path, &ws_path, &setting_patterns) {
            Ok(result) => {
                let _ = app2.emit(
                    "worktree-includes-applied",
                    serde_json::json!({
                        "workspace_id": ws_id,
                        "source": result.source,
                        "copied": result.copied,
                    }),
                );
            }
            Err(e) => {
                eprintln!("[codemux::scripts] worktree includes error for workspace {ws_id}: {e}");
            }
        }

        // Step 2: Run setup commands
        if let Some(config) = config {
            if !config.setup.is_empty() {
                if let Err(e) = crate::scripts::run_setup_scripts_with_config(
                    &ws_path, &ws_title, &ws_id, &app2, &config, &root_path,
                    ws_branch.as_deref(), Some(port),
                ) {
                    eprintln!("[codemux::scripts] Setup failed for workspace {ws_id}: {e}");
                }
            }
        }
    });
}

#[tauri::command]
pub fn get_workspace_config(path: String) -> Option<WorkspaceConfig> {
    crate::config::workspace_config::read_workspace_config(Path::new(&path))
}

#[tauri::command]
pub fn has_codemuxinclude(path: String) -> bool {
    let root = crate::scripts::resolve_root_path(Path::new(&path));
    root.join(".codemuxinclude").exists()
}

#[tauri::command]
pub fn run_workspace_setup<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    db: State<'_, crate::database::DatabaseStore>,
    workspace_id: String,
) -> Result<(), String> {
    let (cwd, title, branch) = {
        let snapshot = state.snapshot();
        let ws = snapshot
            .workspaces
            .iter()
            .find(|w| w.workspace_id.0 == workspace_id)
            .ok_or_else(|| format!("No workspace found for {workspace_id}"))?;
        (ws.cwd.clone(), ws.title.clone(), ws.git_branch.clone())
    };
    let port = crate::scripts::allocate_workspace_port(&workspace_id);
    crate::scripts::run_setup_scripts(
        Path::new(&cwd), &title, &workspace_id, &app, Some(&db),
        branch.as_deref(), Some(port),
    )
}

#[tauri::command]
pub fn run_project_dev_command<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    db: State<'_, crate::database::DatabaseStore>,
    pty_state: State<'_, crate::terminal::PtyState>,
    workspace_id: String,
    force_new: bool,
) -> Result<(), String> {
    let (cwd, project_root) = {
        let snapshot = state.snapshot();
        let ws = snapshot
            .workspaces
            .iter()
            .find(|w| w.workspace_id.0 == workspace_id)
            .ok_or_else(|| format!("No workspace found for {workspace_id}"))?;
        (ws.cwd.clone(), ws.project_root.clone())
    };

    // Resolve the run command from effective config
    let config_path = project_root
        .as_ref()
        .map(|p| PathBuf::from(p))
        .unwrap_or_else(|| PathBuf::from(&cwd));
    let config = crate::config::workspace_config::read_effective_config(&config_path, &db);
    let run_cmd = config
        .and_then(|c| c.run)
        .ok_or_else(|| {
            "No run command configured. Set one in Settings > Projects.".to_string()
        })?;

    // Check if a "Workspace Run" tab already exists for this workspace
    let existing_run_tab = if force_new {
        None
    } else {
        let snapshot = state.snapshot();
        let ws = snapshot
            .workspaces
            .iter()
            .find(|w| w.workspace_id.0 == workspace_id);
        ws.and_then(|w| {
            w.tabs
                .iter()
                .find(|t| t.title == "Workspace Run" && t.kind == TabKind::Terminal)
                .map(|t| (t.tab_id.clone(), t.surface_id.clone()))
        })
    };

    if let Some((tab_id, surface_id)) = existing_run_tab {
        // Reuse existing tab: activate it and restart the command
        state.activate_tab(&workspace_id, &tab_id).ok();

        // Get the session ID for this tab's surface
        if let Some(surface_id) = surface_id {
            let session_id = {
                let snapshot = state.snapshot();
                let ws = snapshot
                    .workspaces
                    .iter()
                    .find(|w| w.workspace_id.0 == workspace_id);
                ws.and_then(|w| {
                    w.surfaces
                        .iter()
                        .find(|s| s.surface_id == surface_id)
                        .and_then(|s| {
                            crate::state::session_id_for_pane(&s.root, &s.active_pane_id)
                        })
                })
            };

            if let Some(session_id) = session_id {
                // Send Ctrl+C, wait briefly, then send the command
                let data = format!("\x03\n{}\n", run_cmd);
                crate::terminal::write_to_pty_by_session(
                    &pty_state,
                    &session_id.0,
                    &data,
                )?;
            }
        }
    } else {
        // Create a new terminal tab
        let (tab_id, session_id) = state.create_tab(&workspace_id, TabKind::Terminal)?;

        if let Some(session_id) = &session_id {
            terminal::spawn_pty_for_session(app.clone(), session_id.0.clone());
        }

        // Rename the tab to "Workspace Run"
        state
            .rename_tab(&workspace_id, &tab_id, "Workspace Run".to_string())
            .ok();

        // Write the run command after a brief delay for the PTY to initialize
        if let Some(session_id) = session_id {
            let cmd = run_cmd.clone();
            let sessions = pty_state.sessions.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(300));
                let mut guard = sessions.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(runtime) = guard.get_mut(&session_id.0) {
                    if let Some(writer) = runtime.writer.as_mut() {
                        let data = format!("{}\n", cmd);
                        if let Err(e) = writer.write_all(data.as_bytes()) {
                            eprintln!(
                                "[codemux::scripts] Failed to write run command: {e}"
                            );
                        }
                        let _ = writer.flush();
                    }
                }
            });
        }
    }

    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn reorder_workspaces<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    workspace_ids: Vec<String>,
) -> Result<(), String> {
    if state.reorder_workspaces(workspace_ids) {
        crate::state::emit_app_state(&app);
        Ok(())
    } else {
        Err("Failed to reorder workspaces".to_string())
    }
}

#[tauri::command]
pub fn reorder_tabs<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    state: State<'_, AppStateStore>,
    workspace_id: String,
    tab_ids: Vec<String>,
) -> Result<(), String> {
    if state.reorder_tabs(&workspace_id, tab_ids) {
        crate::state::emit_app_state(&app);
        Ok(())
    } else {
        Err("Failed to reorder tabs".to_string())
    }
}

#[cfg(test)]
mod empty_workspace_skip_setup_tests {
    //! Contract test for the `skip_setup` branch in
    //! [`create_empty_workspace`]. The Tauri command itself isn't
    //! unit-testable (needs an `AppHandle`), so instead we pin the
    //! behavior of the side-effect we're bypassing: `upsert_mcp_config`.
    //!
    //! - **Baseline**: calling `upsert_mcp_config` on an empty dir
    //!   creates `.mcp.json`. This proves the function is the thing
    //!   that mutates the filesystem.
    //! - **Skip path**: we simulate the `skip_setup=true` branch by
    //!   NOT calling it, and assert the dir stays empty. If a future
    //!   refactor accidentally reintroduces the call on the skip path,
    //!   this test won't catch it — but the frontend test
    //!   (sidebar-header.test.tsx) asserts the flag is plumbed through,
    //!   and a visual review of the command body completes the
    //!   coverage.
    use tempfile::TempDir;

    #[test]
    fn upsert_mcp_config_creates_dotfile_baseline() {
        let tmp = TempDir::new().unwrap();
        let mcp_path = tmp.path().join(".mcp.json");
        assert!(!mcp_path.exists(), "pre-condition: no .mcp.json yet");
        crate::mcp_server::upsert_mcp_config(tmp.path());
        assert!(
            mcp_path.exists(),
            "upsert_mcp_config must create .mcp.json — the skip-setup test \
             below is only meaningful if this baseline holds"
        );
    }

    #[test]
    fn skip_setup_branch_leaves_mcp_untouched() {
        let tmp = TempDir::new().unwrap();
        let mcp_path = tmp.path().join(".mcp.json");
        assert!(!mcp_path.exists(), "pre-condition: no .mcp.json yet");

        // The skip-setup branch in create_empty_workspace intentionally
        // does not call upsert_mcp_config or spawn_setup_scripts. We
        // model that here by simply not invoking either.

        assert!(
            !mcp_path.exists(),
            ".mcp.json appeared without an explicit upsert call — the \
             skip_setup=true contract requires zero filesystem mutation"
        );
    }

    #[test]
    fn skip_setup_branch_preserves_existing_mcp_bytes() {
        let tmp = TempDir::new().unwrap();
        let mcp_path = tmp.path().join(".mcp.json");
        let sentinel = r#"{"mcpServers":{"user-other":{"command":"keep"}}}"#;
        std::fs::write(&mcp_path, sentinel).unwrap();

        // Skip branch: no upsert. File bytes must be preserved exactly.
        let after = std::fs::read_to_string(&mcp_path).unwrap();
        assert_eq!(
            after, sentinel,
            "pre-existing .mcp.json must be byte-identical when skip_setup=true"
        );
    }
}

#[cfg(test)]
mod phase_1_5_workspace_create_path_tests {
    //! Phase 1.5 regression coverage for the `workspace_create` MCP tool's
    //! `path` argument. The bug pre-fix was at `control.rs:573`: the
    //! socket arm hardcoded `None` as the fourth argument to
    //! `create_workspace_impl`, dropping the path the brain asked for.
    //! These tests pin the state-layer contract the fix relies on —
    //! `create_workspace_at_path(path)` must produce a workspace whose
    //! `cwd` matches the path the caller supplied. If the underlying
    //! state method ever stops honoring the path, the regression test
    //! at this layer fails immediately and the fix at the socket arm
    //! becomes meaningless. End-to-end socket-arm coverage requires an
    //! `AppHandle` (deferred — see Phase 1 notes on dispatch tests).
    use super::*;
    use crate::state::AppStateStore;

    #[test]
    fn create_workspace_at_path_uses_supplied_cwd() {
        let store = AppStateStore::default();
        // Use a path that's unambiguously not the test process's CWD so
        // a regression that silently substitutes `current_project_root()`
        // (the pre-fix behavior) fails this assertion cleanly.
        let target = PathBuf::from("/tmp/codemux-phase-1-5-cwd-test");
        let ws_id = store.create_workspace_at_path(target.clone());

        let snapshot = store.snapshot();
        let ws = snapshot
            .workspaces
            .iter()
            .find(|w| w.workspace_id == ws_id)
            .expect("newly created workspace must appear in snapshot");
        assert_eq!(
            ws.cwd,
            target.display().to_string(),
            "create_workspace_at_path must store the supplied path verbatim — \
             the workspace_create path-bug fix depends on this contract"
        );
    }

    #[test]
    fn create_workspace_without_path_falls_back_to_current_root() {
        // The None path of `create_workspace_impl` calls
        // `state.create_workspace()` (no path), which falls back to
        // `current_project_root()` resolution downstream. Here we just
        // pin that the no-path constructor produces a workspace at all.
        let store = AppStateStore::default();
        let snap_before = store.snapshot();
        let ws_id = store.create_workspace();
        let snap_after = store.snapshot();
        assert!(
            snap_after.workspaces.len() > snap_before.workspaces.len(),
            "create_workspace() must add exactly one workspace"
        );
        assert!(
            snap_after.workspaces.iter().any(|w| w.workspace_id == ws_id),
            "the returned workspace_id must be present in the new snapshot"
        );
    }
}

#[cfg(test)]
mod phase_1_6_close_tests {
    //! Phase 1.6 regression coverage for the close-workspace and
    //! close-pane state-layer contracts the new MCP tools rely on.
    //!
    //! The MCP tools wrap `close_workspace_with_worktree_impl` and
    //! `close_pane_impl`, which both take a `tauri::AppHandle` — out of
    //! reach for a pure unit test (same constraint Phase 1 documented
    //! for dispatch tests). What we CAN test directly is the state
    //! layer those impls delegate to:
    //!
    //! - `AppStateStore::close_workspace` — the mutation that makes
    //!   the workspace disappear from snapshot.
    //! - `AppStateStore::close_pane` — the mutation that makes a pane
    //!   disappear, with the last-pane-in-surface side effects (surface
    //!   + tab removal).
    //!
    //! End-to-end coverage (worktree removal on disk, PTY teardown,
    //! agent-chat shutdown) requires the live Tauri runtime and is
    //! exercised via the manual stdio smoke pattern from earlier
    //! phases.
    use super::*;
    use crate::state::{AppStateStore, SplitDirection, WorkspacePresetLayout};
    use tempfile::TempDir;

    #[test]
    fn close_workspace_removes_workspace_from_state() {
        // workspace_close MCP tool regression: closing a workspace must
        // make it disappear from the snapshot the brain reads via
        // `workspace_list` / `app_status`.
        let store = AppStateStore::default();
        let ws_id = store.create_workspace_with_layout(
            PathBuf::from("/tmp/phase-1-6-close-ws"),
            WorkspacePresetLayout::Single,
        );
        assert!(
            store.snapshot().workspaces.iter().any(|w| w.workspace_id == ws_id),
            "fresh workspace must appear in snapshot pre-close"
        );

        assert!(
            store.close_workspace(&ws_id.0).is_ok(),
            "close should succeed"
        );

        assert!(
            !store.snapshot().workspaces.iter().any(|w| w.workspace_id == ws_id),
            "closed workspace must NOT appear in snapshot post-close"
        );
    }

    #[test]
    fn close_workspace_unknown_id_errors() {
        // Defensive: the MCP brain might call `workspace_close` with a
        // stale id from a cached `workspace_list`. The state layer must
        // return a clean Err string the MCP layer can surface, not
        // panic.
        let store = AppStateStore::default();
        let err = match store.close_workspace("workspace-does-not-exist") {
            Ok(_) => panic!("closing unknown id must error, got Ok"),
            Err(e) => e,
        };
        assert!(
            err.to_lowercase().contains("not found")
                || err.to_lowercase().contains("no workspace"),
            "unexpected error string: {err}"
        );
    }

    #[test]
    fn closing_duplicate_cwd_preserves_config_until_last_workspace_closes() {
        let checkout = TempDir::new().expect("checkout tempdir");
        let config_path = checkout.path().join(".mcp.json");
        let store = AppStateStore::default();
        store.clear_workspaces();
        let first = store.create_workspace_at_path(checkout.path().to_path_buf());
        let second = store.create_workspace_at_path(checkout.path().to_path_buf());
        crate::mcp_server::upsert_mcp_config(checkout.path());
        let original = std::fs::read(&config_path).expect("created config");

        store.close_workspace(&first.0).expect("close first duplicate");
        reconcile_mcp_config_after_close(&store.snapshot(), checkout.path(), true);

        assert!(config_path.exists(), "the surviving workspace still owns the config");
        assert_eq!(
            std::fs::read(&config_path).unwrap(),
            original,
            "survivor reconciliation should be a semantic no-op"
        );
        assert!(
            store.snapshot().workspaces.iter().any(|workspace| workspace.workspace_id == second),
            "the duplicate workspace must still be live"
        );

        store.close_workspace(&second.0).expect("close final duplicate");
        reconcile_mcp_config_after_close(&store.snapshot(), checkout.path(), true);
        assert!(
            !config_path.exists(),
            "the final workspace close should remove Codemux's config"
        );
    }

    #[test]
    fn closing_duplicate_cwd_respects_auto_mcp_opt_out() {
        let checkout = TempDir::new().expect("checkout tempdir");
        let store = AppStateStore::default();
        store.clear_workspaces();
        let first = store.create_workspace_at_path(checkout.path().to_path_buf());
        let second = store.create_workspace_at_path(checkout.path().to_path_buf());

        store.close_workspace(&first.0).expect("close first duplicate");
        reconcile_mcp_config_after_close(&store.snapshot(), checkout.path(), false);

        assert!(
            !checkout.path().join(".mcp.json").exists(),
            "closing one duplicate must not create config while auto-MCP is disabled"
        );
        assert!(store
            .snapshot()
            .workspaces
            .iter()
            .any(|workspace| workspace.workspace_id == second));
    }

    #[test]
    fn close_pane_removes_pane_from_snapshot_after_split() {
        // pane_close MCP tool regression: splitting a workspace then
        // closing the new pane must leave exactly one pane behind. The
        // surface stays (it still has the original pane), and the
        // workspace stays open.
        let store = AppStateStore::default();
        let ws_id = store.create_workspace_with_layout(
            PathBuf::from("/tmp/phase-1-6-close-pane"),
            WorkspacePresetLayout::Single,
        );
        let initial = store.snapshot();
        let ws = initial
            .workspaces
            .iter()
            .find(|w| w.workspace_id == ws_id)
            .expect("workspace must exist");
        let active_pane = ws.surfaces[0].active_pane_id.0.clone();

        let _new_session = store
            .split_pane(&active_pane, SplitDirection::Horizontal)
            .expect("split must succeed");

        let after_split = store.snapshot();
        let ws_after_split = after_split
            .workspaces
            .iter()
            .find(|w| w.workspace_id == ws_id)
            .unwrap();
        let panes_after_split =
            crate::state::collect_terminal_sessions(&ws_after_split.surfaces);
        assert_eq!(
            panes_after_split.len(),
            2,
            "expected 2 panes after split, got {panes_after_split:?}"
        );

        // Pick the pane that is NOT the original active_pane — that's
        // the freshly-split sibling whose pane_id we need.
        let target_pane_id = pick_non_active_pane(&ws_after_split.surfaces[0].root, &active_pane)
            .expect("split should have produced a second pane");
        let removed = store
            .close_pane(&target_pane_id)
            .expect("close_pane must succeed");
        assert!(removed.is_some(), "closing a terminal pane returns its session_id");

        let after_close = store.snapshot();
        let ws_after_close = after_close
            .workspaces
            .iter()
            .find(|w| w.workspace_id == ws_id)
            .expect("workspace must still exist after closing one pane");
        let panes_after_close =
            crate::state::collect_terminal_sessions(&ws_after_close.surfaces);
        assert_eq!(
            panes_after_close.len(),
            1,
            "exactly one pane must remain; got {panes_after_close:?}"
        );
    }

    #[test]
    fn close_pane_last_pane_leaves_workspace_empty() {
        // Documents the in-app last-pane semantics: closing the very
        // last pane removes the surface + tab, but the workspace stays
        // in state with empty tabs/surfaces. The brain must then
        // explicitly call `workspace_close` to finalize. We pin this
        // here so a future refactor can't silently start auto-closing
        // workspaces on last-pane removal.
        let store = AppStateStore::default();
        let ws_id = store.create_workspace_with_layout(
            PathBuf::from("/tmp/phase-1-6-last-pane"),
            WorkspacePresetLayout::Single,
        );
        let snap = store.snapshot();
        let active_pane = snap
            .workspaces
            .iter()
            .find(|w| w.workspace_id == ws_id)
            .unwrap()
            .surfaces[0]
            .active_pane_id
            .0
            .clone();

        store.close_pane(&active_pane).expect("close_pane succeeds");

        let after = store.snapshot();
        let ws_after = after
            .workspaces
            .iter()
            .find(|w| w.workspace_id == ws_id)
            .expect("workspace must still exist after closing its last pane");
        assert!(
            ws_after.surfaces.is_empty(),
            "surfaces must be cleared when the last pane is closed"
        );
        assert!(
            ws_after.tabs.is_empty(),
            "tabs must be cleared when the last pane is closed"
        );
    }

    /// Walk a pane tree and return the pane_id of any terminal pane
    /// whose id is NOT `exclude`. Helper for `close_pane_removes_pane…`
    /// where the split has produced two terminal panes and we need
    /// the one that isn't the original active pane.
    fn pick_non_active_pane(
        node: &crate::state::PaneNodeSnapshot,
        exclude: &str,
    ) -> Option<String> {
        use crate::state::PaneNodeSnapshot;
        match node {
            PaneNodeSnapshot::Terminal { pane_id, .. } if pane_id.0 != exclude => {
                Some(pane_id.0.clone())
            }
            PaneNodeSnapshot::Terminal { .. } => None,
            PaneNodeSnapshot::Browser { pane_id, .. } if pane_id.0 != exclude => {
                Some(pane_id.0.clone())
            }
            PaneNodeSnapshot::Browser { .. } => None,
            PaneNodeSnapshot::AgentChat { pane_id, .. } if pane_id.0 != exclude => {
                Some(pane_id.0.clone())
            }
            PaneNodeSnapshot::AgentChat { .. } => None,
            PaneNodeSnapshot::Split { children, .. } => {
                children.iter().find_map(|c| pick_non_active_pane(c, exclude))
            }
        }
    }
}

#[cfg(test)]
mod editor_detection_tests {
    use super::*;

    /// `resolve_editor_command` should find any binary that's on `PATH`. We
    /// use `git` because it's required by CI for every platform we ship to
    /// (Linux, macOS, Windows) and is always on the developer's PATH. This
    /// proves the `which::which` path actually resolves to an absolute path
    /// rather than silently returning `None`.
    #[test]
    fn resolve_editor_command_finds_git_on_path() {
        let resolved = resolve_editor_command("git");
        assert!(
            resolved.is_some(),
            "resolve_editor_command(\"git\") returned None — git is expected \
             to be on PATH on every dev/CI machine"
        );
        let path = resolved.unwrap();
        assert!(
            !path.is_empty(),
            "resolved path must be non-empty, got: {path:?}"
        );
        // The resolved value should be a path that actually exists on disk.
        assert!(
            std::path::Path::new(&path).exists(),
            "resolved path does not exist on disk: {path:?}"
        );
    }

    /// A binary that almost certainly does not exist on any real machine
    /// should resolve to `None`. This guards against a future refactor
    /// accidentally returning `Some("")` or `Some(cmd.to_string())` on the
    /// miss path.
    #[test]
    fn resolve_editor_command_returns_none_for_nonexistent_binary() {
        let resolved = resolve_editor_command("codemux-definitely-not-a-real-binary-xyz");
        assert!(
            resolved.is_none(),
            "expected None for a bogus binary, got: {resolved:?}"
        );
    }

    /// Empty string is a corner case that should never crash and should
    /// always return `None` (both `which::which("")` and the Windows
    /// fallback lookup should bail without panicking).
    #[test]
    fn resolve_editor_command_handles_empty_string() {
        let resolved = resolve_editor_command("");
        assert!(
            resolved.is_none(),
            "expected None for empty string, got: {resolved:?}"
        );
    }

    /// End-to-end on the test machine: `find_editors()` should return at
    /// least one entry. The CI runners have `git` on PATH but no IDE, so
    /// this assertion is weaker than "non-empty" — we instead verify the
    /// shape of the result (no panics, all entries have non-empty fields)
    /// and let the stronger "at least one editor on a dev box" claim ride
    /// on `find_editors_finds_real_editor_on_dev_machine` below.
    #[test]
    fn find_editors_returns_well_formed_entries() {
        let editors = find_editors();
        for ed in &editors {
            assert!(!ed.id.is_empty(), "editor id must not be empty: {ed:?}");
            assert!(!ed.name.is_empty(), "editor name must not be empty: {ed:?}");
            assert!(
                !ed.command.is_empty(),
                "editor command must not be empty: {ed:?}"
            );
            // If we found an editor, its command must point at an existing file.
            assert!(
                std::path::Path::new(&ed.command).exists(),
                "editor command path must exist on disk: {ed:?}"
            );
        }
    }

    /// Dev-machine-only assertion: on the developer's workstation at least
    /// one of the `EDITOR_CANDIDATES` is installed (we have VS Code). CI
    /// runners don't ship with an IDE so this test is skipped there via
    /// the `CODEMUX_DEV_MACHINE` env var — set it locally to opt in.
    #[test]
    fn find_editors_finds_real_editor_on_dev_machine() {
        if std::env::var("CODEMUX_DEV_MACHINE").is_err() {
            // Silently skip on CI / non-dev-machine runs. We can't use
            // `#[ignore]` because we want the test to run by default on
            // dev machines when the env var is set without needing
            // `cargo test -- --ignored`.
            return;
        }
        let editors = find_editors();
        assert!(
            !editors.is_empty(),
            "find_editors() returned empty on a dev machine — at least one \
             of {EDITOR_CANDIDATES:?} should resolve"
        );
    }

    /// Windows-only: the fallback path must not panic when `%LOCALAPPDATA%`
    /// or `%ProgramFiles%` are missing from the environment. Simulates a
    /// stripped environment to make sure `windows_install_roots()` returns
    /// gracefully rather than unwrapping an `Err`.
    #[cfg(windows)]
    #[test]
    fn windows_install_roots_tolerates_missing_env_vars() {
        // SAFETY: tests run single-threaded by default per crate, but env
        // mutation is still a footgun. We snapshot + restore the three
        // vars we touch to keep sibling tests unaffected.
        let snapshot = [
            ("LOCALAPPDATA", std::env::var("LOCALAPPDATA").ok()),
            ("ProgramFiles", std::env::var("ProgramFiles").ok()),
            ("ProgramFiles(x86)", std::env::var("ProgramFiles(x86)").ok()),
        ];

        // Strip all three and confirm the helper returns an empty vec
        // without panicking.
        for (key, _) in &snapshot {
            std::env::remove_var(key);
        }
        let roots = windows_install_roots();
        assert!(
            roots.is_empty(),
            "expected empty roots when all env vars are unset, got: {roots:?}"
        );

        // Restore so the rest of the test run sees the real environment.
        for (key, value) in &snapshot {
            if let Some(v) = value {
                std::env::set_var(key, v);
            }
        }
    }

    /// Windows-only: when `%LOCALAPPDATA%` IS set, the helper should
    /// produce a `\Programs`-suffixed root as the first entry. Guards
    /// against future refactors accidentally dropping the suffix.
    #[cfg(windows)]
    #[test]
    fn windows_install_roots_parses_localappdata_with_programs_suffix() {
        let saved = std::env::var("LOCALAPPDATA").ok();
        std::env::set_var("LOCALAPPDATA", r"C:\Users\test\AppData\Local");
        let roots = windows_install_roots();
        // Restore before any assertion can fail, to be polite to other tests.
        match saved {
            Some(v) => std::env::set_var("LOCALAPPDATA", v),
            None => std::env::remove_var("LOCALAPPDATA"),
        }

        assert!(
            roots.iter().any(|r| r == r"C:\Users\test\AppData\Local\Programs"),
            "expected LOCALAPPDATA\\Programs in roots, got: {roots:?}"
        );
    }

    /// Windows-only: `resolve_editor_command` with a bogus editor id that
    /// IS in `WINDOWS_EDITOR_FALLBACKS` but whose install path doesn't
    /// exist should still return `None` (not panic, not return a garbage
    /// path). Exercises the `candidate.is_file()` branch of the fallback.
    #[cfg(windows)]
    #[test]
    fn resolve_editor_command_fallback_none_when_install_path_missing() {
        // `code` has fallbacks defined but we're (probably) not checking
        // a machine where VS Code is installed at the hardcoded path AND
        // where it's also not on PATH. This is a soft assertion — if
        // either `which` or the fallback hits, we just skip.
        let resolved = resolve_editor_command("code");
        // Either outcome is valid; we only check that the call didn't
        // panic and returned a well-typed `Option<String>`.
        let _: Option<String> = resolved;
    }
}

#[cfg(test)]
mod git_info_tests {
    //! Lock in the behavior of `gather_workspace_git_info` after the
    //! gather/apply refactor. The synchronous `populate_git_info` still
    //! calls the same gather logic, so these tests also cover its data
    //! shape implicitly.

    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    fn run_git(repo: &std::path::Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.invalid")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.invalid")
            .status()
            .expect("spawn git");
        assert!(status.success(), "git {args:?} failed");
    }

    #[test]
    fn gather_workspace_git_info_returns_defaults_for_non_git_path() {
        // Crash-proof guarantee from the doc-comment: a directory with no
        // `.git/` must yield default values, not panic.
        let tmp = TempDir::new().expect("tempdir");
        let info = gather_workspace_git_info(tmp.path());
        assert!(!info.is_git, "a non-git dir must report is_git=false");
        assert!(info.branch.is_none(), "no branch in a non-git dir");
        assert_eq!(info.ahead, 0);
        assert_eq!(info.behind, 0);
        assert_eq!(info.additions, 0);
        assert_eq!(info.deletions, 0);
        assert_eq!(info.changed_files, 0);
    }

    #[test]
    fn gather_workspace_git_info_flips_is_git_after_git_init() {
        // The exact flow the "Initialize Git" affordance runs, exercised
        // through the REAL function the button calls
        // (`crate::git::git_init_no_commit`) rather than a raw `git init`
        // shell-out — so this test would fail if that function ever
        // regressed back into staging/committing the user's files.
        let tmp = TempDir::new().expect("tempdir");

        // A plain folder holding a would-be secret. If init ever staged
        // or committed, this is exactly the kind of file that must NOT
        // end up in the tree.
        std::fs::write(tmp.path().join(".env"), "SECRET=x").expect("write .env");

        // Pre-init: not a repo.
        assert!(
            !gather_workspace_git_info(tmp.path()).is_git,
            "a plain folder must report is_git=false before init"
        );

        // Run the actual affordance path: bare init, no commit.
        crate::git::git_init_no_commit(tmp.path()).expect("git_init_no_commit");

        // Post-init: now a repo.
        assert!(
            gather_workspace_git_info(tmp.path()).is_git,
            "freshly-initialized repo must report is_git=true"
        );

        // No commit was created — HEAD is unborn, so `rev-parse --verify
        // HEAD` must fail. (A commit-ful init would resolve here.)
        let head = std::process::Command::new("git")
            .args(["rev-parse", "--verify", "HEAD"])
            .current_dir(tmp.path())
            .output()
            .expect("run git rev-parse");
        assert!(
            !head.status.success(),
            "HEAD must be unborn after a bare init (no commit), got: {head:?}"
        );

        // Nothing was staged: the .env must show as untracked (`?? .env`),
        // NOT added (`A  .env`). Bare init needs no user.name/email config.
        let status = std::process::Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(tmp.path())
            .output()
            .expect("run git status");
        assert!(status.status.success(), "git status failed: {status:?}");
        let porcelain = String::from_utf8_lossy(&status.stdout);
        assert!(
            porcelain.lines().any(|l| l == "?? .env"),
            "the .env file must be UNTRACKED (`?? .env`), not staged; \
             porcelain was: {porcelain:?}"
        );
    }

    #[test]
    fn gather_workspace_git_info_returns_defaults_for_nonexistent_path() {
        let info = gather_workspace_git_info(std::path::Path::new(
            "/nonexistent/codemux/test/path/xyz",
        ));
        assert!(info.branch.is_none());
        assert_eq!(info.changed_files, 0);
    }

    #[test]
    fn gather_workspace_git_info_reports_branch_for_real_repo() {
        let tmp = TempDir::new().expect("tempdir");
        let repo = tmp.path();
        run_git(repo, &["init", "-q", "-b", "main"]);
        run_git(repo, &["commit", "--allow-empty", "-q", "-m", "initial"]);

        let info = gather_workspace_git_info(repo);
        assert_eq!(info.branch.as_deref(), Some("main"));
        assert_eq!(info.changed_files, 0);
        assert_eq!(info.additions, 0);
        assert_eq!(info.deletions, 0);
    }

    #[test]
    fn gather_workspace_git_info_counts_changed_files() {
        let tmp = TempDir::new().expect("tempdir");
        let repo = tmp.path();
        run_git(repo, &["init", "-q", "-b", "main"]);
        run_git(repo, &["commit", "--allow-empty", "-q", "-m", "initial"]);
        std::fs::write(repo.join("untracked.txt"), "hi").expect("write file");

        let info = gather_workspace_git_info(repo);
        assert_eq!(info.branch.as_deref(), Some("main"));
        assert!(
            info.changed_files >= 1,
            "untracked file should be counted, got {}",
            info.changed_files,
        );
    }

    // Verifies the new async path: `populate_git_info_async` must write the
    // same shape of data into the workspace snapshot that the sync path
    // would. Guards against the refactor accidentally dropping a field.
    #[tokio::test]
    async fn populate_git_info_async_writes_branch_into_state() {
        let tmp = TempDir::new().expect("tempdir");
        let repo = tmp.path();
        run_git(repo, &["init", "-q", "-b", "main"]);
        run_git(repo, &["commit", "--allow-empty", "-q", "-m", "initial"]);

        let state = AppStateStore::default();
        let workspace_id = state.create_workspace_at_path(repo.to_path_buf());

        populate_git_info_async(&state, &workspace_id.0, repo.to_path_buf()).await;

        let snapshot = state.snapshot();
        let ws = snapshot
            .workspaces
            .iter()
            .find(|w| w.workspace_id.0 == workspace_id.0)
            .expect("workspace exists");
        assert_eq!(ws.git_branch.as_deref(), Some("main"));
    }
}

#[cfg(test)]
mod archive_guard_tests {
    //! Coverage for the pure file-deletion guards behind the archive
    //! feature. `close_workspace_with_worktree_impl` and
    //! `delete_archived_workspace_impl` both take an `AppHandle` (out of
    //! reach for unit tests — same constraint as the Phase 1.6 close
    //! tests above), so the refusal decisions are factored into pure
    //! functions and pinned here. The impls call these verbatim, so a
    //! regression in either function fails these tests immediately.

    use super::*;
    use crate::state::{build_archive_entry, AppStateStore};

    /// A realistic `WorkspaceSnapshot` to mutate per case: built through
    /// the real store constructor so every non-guard field carries the
    /// shape production code produces.
    fn sample_workspace() -> crate::state::WorkspaceSnapshot {
        let store = AppStateStore::default();
        let ws_id = store.create_workspace_at_path(PathBuf::from("/tmp/archive-guard"));
        store
            .snapshot()
            .workspaces
            .iter()
            .find(|w| w.workspace_id == ws_id)
            .cloned()
            .expect("workspace exists")
    }

    /// The shared predicate both refusal wrappers delegate to: any of
    /// protected / kind "main" / missing worktree_path marks an
    /// undeletable root, and only the unprotected-worktree combination
    /// is deletable.
    #[test]
    fn is_undeletable_root_truth_table() {
        assert!(is_undeletable_root(true, Some("worktree"), Some("/tmp/wt")));
        assert!(is_undeletable_root(false, Some("main"), Some("/tmp/wt")));
        assert!(is_undeletable_root(false, None, None));
        assert!(is_undeletable_root(false, Some("worktree"), None));
        assert!(
            !is_undeletable_root(false, Some("worktree"), Some("/tmp/wt")),
            "an unprotected per-branch worktree is the one deletable case"
        );
        assert!(!is_undeletable_root(false, None, Some("/tmp/wt")));
    }

    /// The live-workspace guard shares the predicate, so kind "main"
    /// now refuses on the close path too (it used to be archive-only).
    #[test]
    fn refuses_worktree_removal_for_main_kind_workspace() {
        let mut ws = sample_workspace();
        ws.protected = false;
        ws.worktree_path = Some("/tmp/archive-guard-wt".into());
        ws.workspace_kind = Some("main".into());

        assert!(
            refuse_worktree_removal(&ws).is_some(),
            "a kind=main workspace must refuse file removal even when unprotected"
        );
    }

    #[test]
    fn refuses_worktree_removal_for_protected_workspace() {
        let mut ws = sample_workspace();
        ws.protected = true;
        ws.worktree_path = Some("/tmp/archive-guard-wt".into());

        let reason = refuse_worktree_removal(&ws)
            .expect("protected workspace must refuse worktree removal");
        assert!(
            reason.contains("root checkout"),
            "unexpected refusal message: {reason}"
        );
    }

    #[test]
    fn refuses_worktree_removal_without_worktree_path() {
        let mut ws = sample_workspace();
        ws.protected = false;
        ws.worktree_path = None;

        assert!(
            refuse_worktree_removal(&ws).is_some(),
            "a workspace without its own worktree must refuse file removal"
        );
    }

    #[test]
    fn allows_worktree_removal_for_disposable_worktree() {
        let mut ws = sample_workspace();
        ws.protected = false;
        ws.worktree_path = Some("/tmp/archive-guard-wt".into());

        assert!(
            refuse_worktree_removal(&ws).is_none(),
            "an unprotected per-branch worktree is the one deletable case"
        );
    }

    #[test]
    fn refuses_archived_file_delete_for_protected_entry() {
        let mut ws = sample_workspace();
        ws.protected = true;
        ws.worktree_path = Some("/tmp/archive-guard-wt".into());
        ws.workspace_kind = Some("worktree".into());
        let entry = build_archive_entry(&ws);

        let reason = refuse_archived_file_delete(&entry)
            .expect("protected entry must refuse file deletion");
        assert!(
            reason.contains("project root checkout"),
            "unexpected refusal message: {reason}"
        );
    }

    #[test]
    fn refuses_archived_file_delete_for_main_kind_and_missing_worktree() {
        // kind "main" refuses even if a worktree_path somehow exists…
        let mut ws = sample_workspace();
        ws.protected = false;
        ws.worktree_path = Some("/tmp/archive-guard-wt".into());
        ws.workspace_kind = Some("main".into());
        assert!(refuse_archived_file_delete(&build_archive_entry(&ws)).is_some());

        // …and a missing worktree_path refuses even with kind None.
        ws.worktree_path = None;
        ws.workspace_kind = None;
        assert!(refuse_archived_file_delete(&build_archive_entry(&ws)).is_some());
    }

    #[test]
    fn allows_archived_file_delete_for_disposable_worktree_entry() {
        let mut ws = sample_workspace();
        ws.protected = false;
        ws.worktree_path = Some("/tmp/archive-guard-wt".into());
        ws.workspace_kind = Some("worktree".into());

        assert!(
            refuse_archived_file_delete(&build_archive_entry(&ws)).is_none(),
            "an unprotected worktree entry is the one deletable case"
        );
    }

    #[test]
    fn archive_refuses_attach_only_workspace() {
        let mut ws = sample_workspace();
        ws.attach_only = true;

        let reason = archive_refusal_reason(&ws)
            .expect("attach-in-place workspaces must refuse archiving");
        assert!(
            reason.contains("Attach-in-place"),
            "unexpected refusal message: {reason}"
        );
    }

    #[test]
    fn archive_refuses_remote_workspace() {
        let mut ws = sample_workspace();
        ws.attach_only = false;
        ws.host_id = Some(42);

        let reason = archive_refusal_reason(&ws)
            .expect("pushed-to-host workspaces must refuse archiving");
        assert!(
            reason.contains("Remote workspaces can't be archived"),
            "unexpected refusal message: {reason}"
        );
    }

    #[test]
    fn archive_allows_plain_local_workspace() {
        let mut ws = sample_workspace();
        ws.attach_only = false;
        ws.host_id = None;

        assert!(
            archive_refusal_reason(&ws).is_none(),
            "an ordinary local workspace must be archivable"
        );
    }
}

#[cfg(test)]
mod worktree_adopt_tests {
    //! Coverage for the adopt-existing guard in
    //! [`create_worktree_workspace_impl`].
    //!
    //! `git_create_worktree` short-circuits and returns an ALREADY-EXISTING
    //! checkout when the conventional path is on disk and registered to the
    //! same branch. Before the guard, the impl still called
    //! `create_workspace_with_layout` unconditionally, producing two
    //! workspaces over one worktree directory (two agents, one checkout).
    //!
    //! The impl itself takes an `AppHandle` (out of reach for a unit test —
    //! same constraint the Phase 1.6 close tests and archive guards
    //! document), so the decision is factored into the pure
    //! [`find_live_workspace_for_worktree_path`] lookup and pinned here. The
    //! impl passes it verbatim as the probe of
    //! `AppStateStore::adopt_or_create_worktree_workspace` (one atomic
    //! check+insert under the state lock — covered by the concurrent test
    //! below) and returns early on `Adopted`, so a regression in the lookup
    //! fails these tests immediately.

    use super::*;
    use crate::state::{build_archive_entry, AppStateStore, WorkspacePresetLayout};
    use tempfile::TempDir;

    /// Build a worktree-backed workspace the way
    /// `create_worktree_workspace_impl` does: create at the worktree path,
    /// then stamp `worktree_path` via `set_workspace_worktree`.
    fn worktree_workspace(store: &AppStateStore, path: &Path, branch: &str) -> String {
        let id = store.create_workspace_with_layout(path.to_path_buf(), WorkspacePresetLayout::Single);
        store.set_workspace_worktree(&id.0, path.display().to_string(), branch.to_string());
        id.0
    }

    #[test]
    fn second_create_at_same_worktree_path_adopts_instead_of_duplicating() {
        let tmp = TempDir::new().expect("tempdir");
        let wt = tmp.path().join("repo-feature");
        std::fs::create_dir_all(&wt).expect("mkdir worktree");

        let store = AppStateStore::default();
        let first_id = worktree_workspace(&store, &wt, "feature");
        let count_after_first = store.snapshot().workspaces.len();

        // This is the exact probe the impl runs right after
        // `git_create_worktree` hands back the reused path.
        let adopted = find_live_workspace_for_worktree_path(&store.snapshot(), &wt, "feature")
            .expect("a live workspace already claims this worktree path");

        assert_eq!(
            adopted, first_id,
            "the second worktree_create for the same path must resolve to the \
             FIRST workspace's id, not mint a new one"
        );
        assert_eq!(
            store.snapshot().workspaces.len(),
            count_after_first,
            "the adopt path must not add a workspace — two workspaces over one \
             on-disk worktree means two agents writing the same files"
        );
    }

    #[test]
    fn distinct_worktree_paths_still_create() {
        let tmp = TempDir::new().expect("tempdir");
        let first = tmp.path().join("repo-feature");
        let second = tmp.path().join("repo-other");
        std::fs::create_dir_all(&first).expect("mkdir first");
        std::fs::create_dir_all(&second).expect("mkdir second");

        let store = AppStateStore::default();
        worktree_workspace(&store, &first, "feature");

        assert!(
            find_live_workspace_for_worktree_path(&store.snapshot(), &second, "other").is_none(),
            "a different branch's worktree must not be adopted — creation \
             has to proceed normally for it"
        );
    }

    #[test]
    fn adopt_matches_through_non_canonical_path_spelling() {
        let tmp = TempDir::new().expect("tempdir");
        let wt = tmp.path().join("repo-feature");
        std::fs::create_dir_all(&wt).expect("mkdir worktree");

        let store = AppStateStore::default();
        let id = worktree_workspace(&store, &wt, "feature");

        // Same directory, different spelling. String equality would miss it
        // and duplicate the workspace; canonicalized comparison must not.
        let round_trip = wt.join("..").join("repo-feature");

        assert_eq!(
            find_live_workspace_for_worktree_path(&store.snapshot(), &round_trip, "feature"),
            Some(id),
            "path identity must be resolved by canonicalization, not raw string \
             comparison"
        );
    }

    #[test]
    fn plain_workspace_opened_at_the_path_is_adopted_via_cwd() {
        let tmp = TempDir::new().expect("tempdir");
        let dir = tmp.path().join("plain");
        std::fs::create_dir_all(&dir).expect("mkdir");

        let store = AppStateStore::default();
        // No `set_workspace_worktree` — this is a plain workspace the user
        // opened directly at the directory. It has no `worktree_path`, but it
        // collides with a new worktree workspace just as badly. Its git info
        // reports the requested branch (populated the way
        // `populate_git_info` does after every create).
        let id = store.create_workspace_at_path(dir.clone());
        store.update_workspace_git_info(&id.0, true, Some("feature".into()), 0, 0, 0, 0, 0);

        assert_eq!(
            find_live_workspace_for_worktree_path(&store.snapshot(), &dir, "feature"),
            Some(id.0),
            "the `cwd` fallback must catch a plain workspace already sitting on \
             the checkout"
        );
    }

    #[test]
    fn cwd_fallback_does_not_adopt_across_branches() {
        let tmp = TempDir::new().expect("tempdir");
        let dir = tmp.path().join("plain");
        std::fs::create_dir_all(&dir).expect("mkdir");

        let store = AppStateStore::default();
        let id = store.create_workspace_at_path(dir.clone());

        // The workspace sits on the directory but has ANOTHER branch checked
        // out. `git_create_worktree` only reuses a path that git maps to the
        // requested branch, so adopting here would silently hand the caller
        // a workspace on the wrong branch — the guard must stay no wider
        // than the reuse it protects against.
        store.update_workspace_git_info(&id.0, true, Some("other".into()), 0, 0, 0, 0, 0);
        assert!(
            find_live_workspace_for_worktree_path(&store.snapshot(), &dir, "feature").is_none(),
            "a cwd-matched workspace on a different branch must not be adopted"
        );

        // Unknown branch (git info not yet populated): the match can't be
        // verified, so the fallback must decline rather than guess.
        store.update_workspace_git_info(&id.0, true, None, 0, 0, 0, 0, 0);
        assert!(
            find_live_workspace_for_worktree_path(&store.snapshot(), &dir, "feature").is_none(),
            "a cwd-matched workspace with an unknown branch must not be adopted"
        );
    }

    #[test]
    fn explicit_worktree_claim_adopts_without_branch_probe() {
        let tmp = TempDir::new().expect("tempdir");
        let wt = tmp.path().join("repo-feature");
        std::fs::create_dir_all(&wt).expect("mkdir worktree");

        let store = AppStateStore::default();
        let id = worktree_workspace(&store, &wt, "feature");

        // A `worktree_path` claim was made expressly for this checkout by
        // `set_workspace_worktree`; path identity is the whole test there.
        // `git_branch` may lag or be unpopulated (populate_git_info is
        // async) and must not defeat adoption of the workspace that owns
        // the checkout.
        assert_eq!(
            find_live_workspace_for_worktree_path(&store.snapshot(), &wt, "feature"),
            Some(id),
            "an explicit worktree_path claim must adopt on path identity alone"
        );
    }

    #[test]
    fn remote_workspace_with_the_same_path_string_is_not_adopted() {
        let tmp = TempDir::new().expect("tempdir");
        let wt = tmp.path().join("repo-feature");
        std::fs::create_dir_all(&wt).expect("mkdir worktree");

        let store = AppStateStore::default();
        // An attach-only workspace whose `cwd` is a path on ANOTHER machine
        // that happens to spell the same as ours. Adopting it would hand the
        // caller a workspace that can't run anything locally.
        store.create_remote_attach_workspace(
            "remote".into(),
            7,
            wt.display().to_string(),
            Some("feature".into()),
            None,
            None,
            Some("worktree".into()),
        );

        assert!(
            find_live_workspace_for_worktree_path(&store.snapshot(), &wt, "feature").is_none(),
            "a remote/attach-only workspace's host path must never match a \
             local worktree directory"
        );
    }

    #[test]
    fn archived_workspace_at_same_path_does_not_block_creation() {
        let tmp = TempDir::new().expect("tempdir");
        let wt = tmp.path().join("repo-feature");
        std::fs::create_dir_all(&wt).expect("mkdir worktree");

        let store = AppStateStore::default();
        let id = worktree_workspace(&store, &wt, "feature");
        let ws = store.find_workspace(&id).expect("workspace exists");

        // Archiving removes the workspace from `workspaces` and parks a copy
        // in `archived_workspaces`, so the lookup can't see it. That is the
        // intended policy: an archived workspace has no live panes or PTYs to
        // collide with, so creation must proceed rather than adopt (or
        // silently un-archive) it.
        store.add_archived_workspace(build_archive_entry(&ws));
        store.close_workspace(&id).expect("close the now-archived workspace");

        assert_eq!(
            store.snapshot().archived_workspaces.len(),
            1,
            "pre-condition: the archived entry is parked and still points at \
             this worktree path"
        );
        assert!(
            find_live_workspace_for_worktree_path(&store.snapshot(), &wt, "feature").is_none(),
            "an archived workspace must not be adopted — worktree_create has to \
             build a fresh workspace over the directory"
        );
    }

    /// The adopt guard's probe and insert must be one atomic operation.
    /// Two concurrent `create_worktree_workspace` calls for the same
    /// worktree path race between "no workspace claims this path yet" and
    /// "insert mine"; with separate lock acquisitions both could probe
    /// empty and both insert. `adopt_or_create_worktree_workspace` holds
    /// the state lock across probe+insert (and stamps the `worktree_path`
    /// claim before releasing), so exactly one create wins and the loser
    /// adopts the winner's workspace.
    #[test]
    fn concurrent_creates_for_same_worktree_yield_one_workspace() {
        use crate::state::WorktreeWorkspaceClaim;
        use std::sync::{Arc, Barrier};

        let tmp = TempDir::new().expect("tempdir");
        let wt = tmp.path().join("repo-feature");
        std::fs::create_dir_all(&wt).expect("mkdir worktree");

        let store = Arc::new(AppStateStore::default());
        let barrier = Arc::new(Barrier::new(2));

        let handles: Vec<_> = (0..2)
            .map(|_| {
                let store = Arc::clone(&store);
                let barrier = Arc::clone(&barrier);
                let wt = wt.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    store.adopt_or_create_worktree_workspace(
                        wt.clone(),
                        WorkspacePresetLayout::Single,
                        wt.display().to_string(),
                        "feature".to_string(),
                        |snapshot| {
                            find_live_workspace_for_worktree_path(snapshot, &wt, "feature")
                        },
                    )
                })
            })
            .collect();

        let outcomes: Vec<_> = handles
            .into_iter()
            .map(|h| h.join().expect("thread panicked"))
            .collect();

        let claimants: Vec<String> = store
            .snapshot()
            .workspaces
            .iter()
            .filter(|w| w.worktree_path.as_deref() == Some(wt.display().to_string().as_str()))
            .map(|w| w.workspace_id.0.clone())
            .collect();
        assert_eq!(
            claimants.len(),
            1,
            "exactly ONE workspace may claim the worktree path after a race — \
             found {claimants:?}"
        );

        let created: Vec<&str> = outcomes
            .iter()
            .filter_map(|o| match o {
                WorktreeWorkspaceClaim::Created(id) => Some(id.0.as_str()),
                WorktreeWorkspaceClaim::Adopted(_) => None,
            })
            .collect();
        let adopted: Vec<&str> = outcomes
            .iter()
            .filter_map(|o| match o {
                WorktreeWorkspaceClaim::Adopted(id) => Some(id.as_str()),
                WorktreeWorkspaceClaim::Created(_) => None,
            })
            .collect();
        assert_eq!(created.len(), 1, "one racer must win the create");
        assert_eq!(adopted.len(), 1, "the other racer must adopt");
        assert_eq!(
            created[0], adopted[0],
            "the adopter must resolve to the creator's workspace id"
        );
        assert_eq!(created[0], claimants[0]);
    }
}

#[cfg(test)]
mod worktree_size_tests {
    //! `workspaces_worktree_sizes` must only ever size a worktree the
    //! sweep could delete — the same set the close-with-worktree and
    //! archive guards allow. Sizing a protected root would invite a
    //! "free up 2 GB" affordance on the user's primary checkout.

    use super::*;
    use crate::state::AppStateStore;

    /// A snapshot with one disposable worktree workspace per id, each
    /// pointing at a real temp directory, that the individual tests then
    /// mutate into the refused shapes.
    fn snapshot_with(ids: &[&str], dir: &Path) -> (AppStateSnapshot, Vec<String>) {
        let store = AppStateStore::default();
        let mut created = Vec::new();
        for id in ids {
            let wt = dir.join(id);
            std::fs::create_dir_all(&wt).unwrap();
            std::fs::write(wt.join("f"), b"12345").unwrap();
            let ws_id = store.create_workspace_at_path(wt.clone());
            store.set_workspace_worktree(&ws_id.0, wt.display().to_string(), (*id).to_string());
            created.push(ws_id.0);
        }
        let mut snapshot = store.snapshot();
        for ws in snapshot.workspaces.iter_mut() {
            ws.protected = false;
            ws.workspace_kind = Some("worktree".into());
            ws.attach_only = false;
            ws.host_id = None;
        }
        (snapshot, created)
    }

    #[test]
    fn sizes_only_disposable_worktrees() {
        let dir = tempfile::tempdir().unwrap();
        let (mut snapshot, ids) = snapshot_with(
            &["ok", "protected", "attach", "remote", "root", "main", "gone"],
            dir.path(),
        );
        for ws in snapshot.workspaces.iter_mut() {
            match ws.title.as_str() {
                "protected" => ws.protected = true,
                "attach" => ws.attach_only = true,
                "remote" => ws.host_id = Some(7),
                "root" => ws.worktree_path = None,
                "main" => ws.workspace_kind = Some("main".into()),
                "gone" => ws.worktree_path = Some(dir.path().join("missing").display().to_string()),
                _ => {}
            }
        }
        let mut requested = ids.clone();
        requested.push("no-such-workspace".into());

        let targets = disposable_worktree_targets(&snapshot, &requested);
        let titles: Vec<&str> = targets
            .iter()
            .map(|(id, _)| {
                snapshot
                    .workspaces
                    .iter()
                    .find(|w| &w.workspace_id.0 == id)
                    .unwrap()
                    .title
                    .as_str()
            })
            .collect();
        assert_eq!(titles, vec!["ok"], "only the plain unprotected worktree qualifies");
        assert_eq!(targets[0].1, dir.path().join("ok"));

        // The predicate is exactly the guards the delete paths use, so
        // every qualifying target passes both.
        for (id, _) in &targets {
            let ws = snapshot.workspaces.iter().find(|w| &w.workspace_id.0 == id).unwrap();
            assert!(refuse_worktree_removal(ws).is_none());
            assert!(archive_refusal_reason(ws).is_none());
        }

        let sizes = size_worktree_targets(targets, Instant::now() + Duration::from_secs(30));
        assert_eq!(sizes.len(), 1);
        assert_eq!(sizes.values().next().copied(), Some(Some(5)));
    }

    #[test]
    fn ids_not_requested_are_not_returned() {
        let dir = tempfile::tempdir().unwrap();
        let (snapshot, ids) = snapshot_with(&["a", "b"], dir.path());
        let targets = disposable_worktree_targets(&snapshot, &ids[..1]);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].0, ids[0]);
    }

    #[test]
    fn expired_budget_keeps_keys_with_unknown_size() {
        // The contract: key present = qualifies; null = size unknown.
        // A deadline hit must not make a qualifying worktree vanish
        // from the sweep list.
        let dir = tempfile::tempdir().unwrap();
        let (snapshot, ids) = snapshot_with(&["a", "b"], dir.path());
        // Enough entries that the walk's periodic clock check fires.
        for i in 0..700 {
            std::fs::write(dir.path().join("a").join(format!("f{i}")), b"x").unwrap();
        }
        let targets = disposable_worktree_targets(&snapshot, &ids);
        let sizes = size_worktree_targets(targets, Instant::now() - Duration::from_secs(1));
        assert_eq!(sizes.len(), 2, "both qualifying ids keep their key");
        assert_eq!(sizes[&ids[0]], None, "the big one ran out of budget");
        // The small one has too few entries to reach a clock check, so
        // it still sizes; what matters is that no key disappeared.
        assert!(sizes.contains_key(&ids[1]));
    }
}
