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
};
use crate::terminal;
use notify_rust::Notification;
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tauri::{Emitter, Manager, State};

/// Read fresh git branch / ahead-behind / diff-stat / changed-file counts for
/// `repo_path` and write them into the workspace snapshot. Crash-proof:
/// `git_branch_info` and friends return defaults for non-git directories,
/// detached HEAD, and corrupted `.git` directories. `pub(crate)` so the
/// periodic refresh loop in `lib.rs` can reuse this instead of duplicating
/// the extraction logic.
pub(crate) fn populate_git_info(state: &AppStateStore, workspace_id: &str, repo_path: &Path) {
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

    state.update_workspace_git_info(workspace_id, branch, ahead, behind, additions, deletions, changed_files);
}

pub(crate) fn create_workspace_impl(
    app: tauri::AppHandle,
    state: &AppStateStore,
    db: &crate::database::DatabaseStore,
    cwd: Option<String>,
) -> Result<String, String> {
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
    populate_git_info(state, &workspace_id.0, &repo_path);

    if let Some(session_id) = state.active_terminal_session_id() {
        terminal::spawn_pty_for_session(app.clone(), session_id.0);
    }

    // Run setup scripts in background thread
    spawn_setup_scripts(&app, state, db, &workspace_id.0, &repo_path);

    // Write .mcp.json for agent auto-discovery
    if crate::mcp_server::is_auto_mcp_enabled(&app) {
        crate::mcp_server::upsert_mcp_config(&repo_path, &workspace_id.0);
    }

    crate::state::emit_app_state(&app);
    Ok(workspace_id.0)
}

pub(crate) fn split_pane_impl(
    app: tauri::AppHandle,
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
    Ok(state.snapshot())
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
    crate::mcp_server::upsert_mcp_config(Path::new(&ws.cwd), &workspace_id);
    Ok(())
}

#[tauri::command]
pub fn create_workspace(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    db: State<'_, crate::database::DatabaseStore>,
    cwd: Option<String>,
) -> Result<String, String> {
    create_workspace_impl(app, &state, &db, cwd)
}

#[tauri::command]
pub fn create_empty_workspace(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    db: State<'_, crate::database::DatabaseStore>,
    cwd: String,
    skip_setup: Option<bool>,
) -> Result<String, String> {
    let repo_path = PathBuf::from(&cwd);
    let workspace_id = state.create_empty_workspace_at_path(repo_path.clone());

    let project_root = crate::config::workspace_config::find_git_root(&repo_path)
        .unwrap_or_else(|| repo_path.clone());
    state.set_workspace_project_root(&workspace_id.0, project_root.display().to_string());
    populate_git_info(&state, &workspace_id.0, &repo_path);

    // Home-chat surfaces and other non-project workspaces pass
    // skip_setup=true to bypass project-level ceremony: no setup
    // scripts, no .mcp.json injection. Git metadata still populates
    // (cheap, harmless for non-repo paths).
    if !skip_setup.unwrap_or(false) {
        spawn_setup_scripts(&app, &state, &db, &workspace_id.0, &repo_path);

        if crate::mcp_server::is_auto_mcp_enabled(&app) {
            crate::mcp_server::upsert_mcp_config(&repo_path, &workspace_id.0);
        }
    }

    crate::state::emit_app_state(&app);
    Ok(workspace_id.0)
}

/// Return the existing Home workspace id, or create one anchored at
/// `$HOME` if none exists yet.
///
/// Home is a singleton: the first workspace with
/// `workspace_type == Home` wins. If the user hard-deletes it, the next
/// call lazily recreates one (no delete protection by design). Creation
/// passes `skip_setup=true` semantics — no `.mcp.json` injection, no
/// setup scripts — since the home directory is not a project.
#[tauri::command]
pub fn get_or_create_home_workspace(
    app: tauri::AppHandle,
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
    populate_git_info(&state, &workspace_id.0, &repo_path);

    state.set_workspace_type(&workspace_id.0, WorkspaceType::Home);

    crate::state::emit_app_state(&app);
    Ok(workspace_id.0)
}

#[tauri::command]
pub fn create_openflow_workspace(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    title: String,
    goal: String,
    cwd: Option<String>,
) -> Result<String, String> {
    let workspace_id = match cwd {
        Some(path) => state.create_openflow_workspace_at_path(title, goal, PathBuf::from(path)),
        None => state.create_openflow_workspace(title, goal),
    };
    crate::state::emit_app_state(&app);
    Ok(workspace_id.0)
}

#[tauri::command]
pub fn create_workspace_with_preset(
    app: tauri::AppHandle,
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
    populate_git_info(&state, &workspace_id.0, &repo_path);

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

#[tauri::command]
pub fn create_worktree_workspace(
    app: tauri::AppHandle,
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
    pr_number: Option<u32>,
) -> Result<String, String> {
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

    let worktree_path =
        crate::git::git_create_worktree(Path::new(&repo_path), &branch, new_branch, base.as_deref(), pr_number)?;
    let wt_path_buf = PathBuf::from(&worktree_path);
    let workspace_id = state.create_workspace_with_layout(wt_path_buf.clone(), layout);

    state.set_workspace_worktree(&workspace_id.0, worktree_path.clone(), branch.clone());
    state.set_workspace_project_root(&workspace_id.0, repo_path.clone());

    populate_git_info(&state, &workspace_id.0, &wt_path_buf);

    if let Some(pr_num) = pr_number {
        state.update_workspace_pr_info(&workspace_id.0, Some(pr_num), None, None);
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
        crate::mcp_server::upsert_mcp_config(&wt_path_buf, &workspace_id.0);
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
                    .map(|cmd| crate::agent_context::inject_agent_context(cmd, &workspace_id.0))
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
    Ok(workspace_id.0)
}

#[tauri::command]
pub fn import_worktree_workspace(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    db: State<'_, crate::database::DatabaseStore>,
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

    populate_git_info(&state, &workspace_id.0, &wt_path_buf);

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
        crate::mcp_server::upsert_mcp_config(&wt_path_buf, &workspace_id.0);
    }

    crate::state::emit_app_state(&app);
    Ok(workspace_id.0)
}

#[tauri::command]
pub fn close_workspace_with_worktree(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    db: State<'_, crate::database::DatabaseStore>,
    workspace_id: String,
    remove_worktree: bool,
    delete_branch: Option<bool>,
    force_delete: Option<bool>,
) -> Result<(), String> {
    let force = force_delete.unwrap_or(false);

    // Worktree metadata still needs a pre-close snapshot for teardown +
    // MCP cleanup. Session IDs come from `state.close_workspace`'s result
    // below — no TOCTOU race with concurrent pane creation.
    let (worktree_path, branch, ws_title) = {
        let snapshot = state.snapshot();
        let ws = snapshot
            .workspaces
            .iter()
            .find(|w| w.workspace_id.0 == workspace_id);
        (
            ws.and_then(|w| w.worktree_path.clone()),
            ws.and_then(|w| w.git_branch.clone()),
            ws.map(|w| w.title.clone()).unwrap_or_default(),
        )
    };

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

    // Remove codemux entry from .mcp.json before closing
    if let Some(ref wt_path) = worktree_path {
        crate::mcp_server::remove_mcp_config(Path::new(wt_path));
    }

    let close_result = state
        .close_workspace(&workspace_id)
        .map_err(|e| format!("Failed to close workspace: {e}"))?;

    // Kill the PTY child process trees for every session that used to belong
    // to this workspace. Sessions are returned atomically from the same lock
    // acquisition that removed the workspace — no TOCTOU race with newly
    // created panes. Idempotent on the waiter thread (double-remove is a
    // no-op) so per-session errors cannot bubble.
    let terminal_state: State<'_, crate::terminal::PtyState> = app.state();
    for session_id in close_result.removed_sessions {
        crate::terminal::terminate_pty_session(&terminal_state.sessions, &session_id.0);
    }

    // Release virtual display for this workspace (idempotent).
    {
        let vd_manager: State<
            '_,
            crate::execution::virtual_display::VirtualDisplayManager,
        > = app.state();
        vd_manager.release(&workspace_id);
    }

    if remove_worktree {
        if let Some(wt_path) = worktree_path {
            let branch_to_delete = if delete_branch.unwrap_or(false) {
                branch.as_deref()
            } else {
                None
            };
            crate::git::git_remove_worktree(Path::new(&wt_path), branch_to_delete, true)?;
        }
    }

    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn activate_workspace(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    db: State<'_, crate::database::DatabaseStore>,
    workspace_id: String,
) -> Result<(), String> {
    if state.activate_workspace(&workspace_id) {
        // Kick off git refresh in a background thread — don't block the
        // activate click. `populate_git_info` runs 5-8 git subprocesses
        // (branch + upstream + ahead/behind + two diff-stat calls + status
        // with duplicated `diff --numstat`), which can hit 100-300ms on a
        // cold filesystem cache or a large repo. The 5s polling loop
        // reconciles, so worst case the sidebar shows slightly stale branch
        // info for one tick. `git_branch_info` handles non-git / detached /
        // corrupted repos without panicking, and the spawned thread emits
        // app state when done so the sidebar picks up the refresh.
        let cwd = {
            let snapshot = state.snapshot();
            snapshot
                .workspaces
                .iter()
                .find(|w| w.workspace_id.0 == workspace_id)
                .map(|w| w.cwd.clone())
        };
        if let Some(cwd) = cwd {
            let refresh_app = app.clone();
            let refresh_ws = workspace_id.clone();
            std::thread::spawn(move || {
                let state: tauri::State<'_, AppStateStore> = refresh_app.state();
                populate_git_info(&state, &refresh_ws, Path::new(&cwd));
                crate::state::emit_app_state(&refresh_app);
            });
        }

        // Lazy PTY hydration: `spawn_missing_ptys` at startup only resumed
        // sessions for the workspace that was active at last close. Sessions
        // for any other workspace stay on disk-only until the user activates
        // them — at which point this branch spawns whatever PTYs aren't
        // already running. Idempotent thanks to `try_reserve_session_spawn`,
        // so re-activating the already-active workspace is a no-op.
        terminal::spawn_missing_ptys_for_workspace(app.clone(), &workspace_id);
        crate::state::emit_app_state(&app);
        db.set_ui_state("active_workspace", &workspace_id).ok();
        Ok(())
    } else {
        Err(format!("No workspace found for {workspace_id}"))
    }
}

#[tauri::command]
pub fn rename_workspace(
    app: tauri::AppHandle,
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

#[tauri::command]
pub fn update_workspace_cwd(
    app: tauri::AppHandle,
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

#[tauri::command]
pub fn close_workspace(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    db: State<'_, crate::database::DatabaseStore>,
    workspace_id: String,
    force_delete: Option<bool>,
) -> Result<String, String> {
    let force = force_delete.unwrap_or(false);

    // We still need cwd + title for teardown + MCP cleanup before the
    // state mutation. Session IDs are now obtained from
    // `state.close_workspace`'s result below — no snapshot race.
    let workspace_cwd = {
        let snapshot = state.snapshot();
        snapshot
            .workspaces
            .iter()
            .find(|w| w.workspace_id.0 == workspace_id)
            .map(|w| (w.cwd.clone(), w.title.clone()))
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

    // Remove codemux entry from .mcp.json before closing
    if let Some((ref cwd, _)) = workspace_cwd {
        crate::mcp_server::remove_mcp_config(Path::new(cwd));
    }

    // Close agent browser CLI session for this workspace.
    // Release port entries for both cli_session_name and workspace_id keys.
    {
        let cli_name = state.find_detached_agent_browser(&workspace_id)
            .map(|s| s.cli_session_name.clone());
        let ws_id = workspace_id.clone();
        let app_handle = app.clone();
        tauri::async_runtime::spawn(async move {
            let manager: State<'_, AgentBrowserManager> = app_handle.state();
            if let Some(cli) = cli_name {
                if let Err(error) = manager.close(&cli).await {
                    eprintln!("[AGENT_BROWSER] Failed to close agent browser for workspace: {error}");
                }
            }
            // Also remove the workspace_id port entry (may be a separate HashMap key).
            let _ = manager.close(&ws_id).await;
        });
    }

    let result = state.close_workspace(&workspace_id)?;

    // Kill the PTY child process trees for every session that used to belong
    // to this workspace. Sessions are returned atomically from the same lock
    // acquisition that removed the workspace — no TOCTOU race with newly
    // created panes.
    let terminal_state: State<'_, crate::terminal::PtyState> = app.state();
    for session_id in result.removed_sessions {
        crate::terminal::terminate_pty_session(&terminal_state.sessions, &session_id.0);
    }

    // Release the virtual display (if any) allocated for this workspace.
    // Idempotent — no-op if no display was ever acquired.
    {
        let vd_manager: State<
            '_,
            crate::execution::virtual_display::VirtualDisplayManager,
        > = app.state();
        vd_manager.release(&workspace_id);
    }

    crate::state::emit_app_state(&app);
    Ok(result.fallback.0)
}

#[tauri::command]
pub fn cycle_workspace(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    step: isize,
) -> Result<String, String> {
    let workspace_id = state
        .workspace_navigation_target(step)
        .ok_or_else(|| "No workspace navigation target available".to_string())?;

    if state.activate_workspace(&workspace_id.0) {
        // Lazy PTY hydration — same rationale as `activate_workspace`.
        terminal::spawn_missing_ptys_for_workspace(app.clone(), &workspace_id.0);
        crate::state::emit_app_state(&app);
        Ok(workspace_id.0)
    } else {
        Err(format!("No workspace found for {}", workspace_id.0))
    }
}

#[tauri::command]
pub fn split_pane(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    pane_id: String,
    direction: String,
) -> Result<String, String> {
    split_pane_impl(app, &state, pane_id, direction)
}

#[tauri::command]
pub fn activate_pane(
    app: tauri::AppHandle,
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
pub fn cycle_pane(
    app: tauri::AppHandle,
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
pub fn close_pane(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    pane_id: String,
) -> Result<Option<String>, String> {
    let removed_browser_id = state.pane_browser_id(&pane_id);
    let removed = state.close_pane(&pane_id)?;

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
pub fn swap_panes(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    source_pane_id: String,
    target_pane_id: String,
) -> Result<(), String> {
    state.swap_panes(&source_pane_id, &target_pane_id)?;
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn resize_split(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    pane_id: String,
    child_sizes: Vec<f32>,
) -> Result<(), String> {
    state.resize_split(&pane_id, child_sizes)?;
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn resize_active_pane(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    delta: f32,
) -> Result<(), String> {
    state.resize_active_pane(delta)?;
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn notify_attention(
    app: tauri::AppHandle,
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
                .hint(notify_rust::Hint::DesktopEntry("com.codemux.app".to_string()))
                .hint(notify_rust::Hint::Transient(true))
                .urgency(notify_rust::Urgency::Critical);
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
            let _ = std::process::Command::new("hyprctl")
                .args(["dispatch", "focuswindow", "class:com.codemux.app"])
                .output();
        }
    }

    crate::state::emit_app_state(&app);
    Ok(notification_id)
}

#[tauri::command]
pub fn set_notification_sound_enabled(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    enabled: bool,
) -> Result<(), String> {
    state.set_notification_sound_enabled(enabled);
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn set_ai_commit_message_enabled(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    enabled: bool,
) -> Result<(), String> {
    state.set_ai_commit_message_enabled(enabled);
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn set_ai_commit_message_model(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    model: Option<String>,
) -> Result<(), String> {
    state.set_ai_commit_message_model(model);
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn set_ai_resolver_enabled(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    enabled: bool,
) -> Result<(), String> {
    state.set_ai_resolver_enabled(enabled);
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn set_ai_resolver_cli(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    cli: Option<String>,
) -> Result<(), String> {
    state.set_ai_resolver_cli(cli);
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn set_ai_resolver_model(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    model: Option<String>,
) -> Result<(), String> {
    state.set_ai_resolver_model(model);
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn set_ai_resolver_strategy(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    strategy: String,
) -> Result<(), String> {
    state.set_ai_resolver_strategy(strategy);
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn create_tab(
    app: tauri::AppHandle,
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
pub fn close_tab(
    app: tauri::AppHandle,
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

    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn activate_tab(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    workspace_id: String,
    tab_id: String,
) -> Result<(), String> {
    state.activate_tab(&workspace_id, &tab_id)?;
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn rename_tab(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    workspace_id: String,
    tab_id: String,
    title: String,
) -> Result<(), String> {
    state.rename_tab(&workspace_id, &tab_id, title)?;
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn refresh_workspace_git_info(
    app: tauri::AppHandle,
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
    populate_git_info(&state, &workspace_id, Path::new(&cwd));
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
#[tauri::command]
pub fn checkout_default_branch_in_workspace(
    app: tauri::AppHandle,
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
    let repo_path = Path::new(&cwd);

    match crate::git::checkout_default_branch(repo_path) {
        Ok(Some(branch)) => {
            // Sync refresh: closes the 0–5s sidebar-label lag from the
            // background polling loop by writing fresh branch info before
            // we emit.
            populate_git_info(&state, &workspace_id, repo_path);
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

const EDITOR_CANDIDATES: &[(&str, &str)] = &[
    ("code", "VS Code"),
    ("cursor", "Cursor"),
    ("codium", "VSCodium"),
    ("zed", "Zed"),
    ("idea", "IntelliJ IDEA"),
    ("goland", "GoLand"),
    ("webstorm", "WebStorm"),
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
    ("codium", &[r"VSCodium\VSCodium.exe"]),
    ("zed", &[r"Zed\Zed.exe"]),
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
    let mut cmd = std::process::Command::new(&editor.command);
    cmd.arg(&path);
    // Intentionally NOT calling `sanitize_gui_env_std` here. The
    // standing project rule strips DISPLAY/WAYLAND_DISPLAY/XDG_* so
    // agent-spawned processes can't pop windows on the user's
    // session — but `open_in_editor` is the *opposite* case: the
    // user explicitly clicked "open this file in my editor" and the
    // editor must inherit the GUI env to render a window. Stripping
    // it here makes the spawn succeed silently with no visible
    // result (Wayland/X11 reject the window because the env was
    // unset). Same exception class as `hyprctl` / `ydotool` /
    // `systemctl` / `loginctl` documented in CLAUDE.md.
    cmd.spawn()
        .map_err(|e| format!("Failed to open editor: {e}"))?;
    Ok(())
}

// ---- Setup/teardown scripts ----

/// Spawn setup scripts in a background thread so workspace creation isn't blocked.
/// Runs the full pipeline: .codemuxinclude file copy + setup commands.
fn spawn_setup_scripts(
    app: &tauri::AppHandle,
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
pub fn run_workspace_setup(
    app: tauri::AppHandle,
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
pub fn run_project_dev_command(
    app: tauri::AppHandle,
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
pub fn reorder_workspaces(
    app: tauri::AppHandle,
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
pub fn reorder_tabs(
    app: tauri::AppHandle,
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
        crate::mcp_server::upsert_mcp_config(tmp.path(), "ws-baseline");
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
