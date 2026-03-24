use crate::browser::BrowserManager;
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
};
use crate::terminal;
use notify_rust::Notification;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use tauri::{Manager, State};

fn populate_git_info(state: &AppStateStore, workspace_id: &str, repo_path: &Path) {
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
    cwd: Option<String>,
) -> Result<String, String> {
    let workspace_id = match &cwd {
        Some(path) => state.create_workspace_at_path(PathBuf::from(path)),
        None => state.create_workspace(),
    };

    // Populate git info
    let repo_path = cwd
        .map(PathBuf::from)
        .unwrap_or_else(crate::project::current_project_root);
    populate_git_info(state, &workspace_id.0, &repo_path);

    if let Some(session_id) = state.active_terminal_session_id() {
        terminal::spawn_pty_for_session(app.clone(), session_id.0);
    }

    // Run setup scripts in background thread
    spawn_setup_scripts(&app, state, &workspace_id.0, &repo_path);

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
pub fn create_workspace(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    cwd: Option<String>,
) -> Result<String, String> {
    create_workspace_impl(app, &state, cwd)
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
    spawn_setup_scripts(&app, &state, &workspace_id.0, &repo_path);

    crate::state::emit_app_state(&app);
    Ok(workspace_id.0)
}

#[tauri::command]
pub fn create_worktree_workspace(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    repo_path: String,
    branch: String,
    new_branch: bool,
    base: Option<String>,
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

    let worktree_path =
        crate::git::git_create_worktree(Path::new(&repo_path), &branch, new_branch, base.as_deref())?;
    let wt_path_buf = PathBuf::from(&worktree_path);
    let workspace_id = state.create_workspace_with_layout(wt_path_buf.clone(), layout);

    state.set_workspace_worktree(&workspace_id.0, worktree_path.clone(), branch.clone());

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

    // Run setup scripts in background thread
    spawn_setup_scripts(&app, &state, &workspace_id.0, &wt_path_buf);

    crate::state::emit_app_state(&app);
    Ok(workspace_id.0)
}

#[tauri::command]
pub fn import_worktree_workspace(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
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

    spawn_setup_scripts(&app, &state, &workspace_id.0, &wt_path_buf);

    crate::state::emit_app_state(&app);
    Ok(workspace_id.0)
}

#[tauri::command]
pub fn close_workspace_with_worktree(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    workspace_id: String,
    remove_worktree: bool,
    force_delete: Option<bool>,
) -> Result<(), String> {
    let force = force_delete.unwrap_or(false);

    // Get worktree path, branch, and title before closing
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
            ) {
                return Err(format!("Teardown failed: {e}\nUse force delete to skip teardown."));
            }
        }
    }

    state
        .close_workspace(&workspace_id)
        .map_err(|e| format!("Failed to close workspace: {e}"))?;

    if remove_worktree {
        if let Some(wt_path) = worktree_path {
            crate::git::git_remove_worktree(Path::new(&wt_path), branch.as_deref())?;
        }
    }

    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn activate_workspace(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    workspace_id: String,
) -> Result<(), String> {
    if state.activate_workspace(&workspace_id) {
        crate::state::emit_app_state(&app);
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
    workspace_id: String,
    force_delete: Option<bool>,
) -> Result<String, String> {
    let force = force_delete.unwrap_or(false);

    // Run teardown scripts before closing
    if !force {
        let cwd = {
            let snapshot = state.snapshot();
            let ws = snapshot
                .workspaces
                .iter()
                .find(|w| w.workspace_id.0 == workspace_id);
            ws.map(|w| (w.cwd.clone(), w.title.clone()))
        };
        if let Some((cwd, title)) = cwd {
            if let Err(e) = crate::scripts::run_teardown_scripts(
                Path::new(&cwd),
                &title,
                &workspace_id,
            ) {
                return Err(format!("Teardown failed: {e}\nUse force delete to skip teardown."));
            }
        }
    }

    let fallback = state.close_workspace(&workspace_id)?;
    crate::state::emit_app_state(&app);
    Ok(fallback.0)
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
        let terminal_state: State<'_, crate::terminal::PtyState> = app.state();
        crate::terminal::close_terminal_session(
            app.clone(),
            terminal_state,
            state.clone(),
            session_id.0.clone(),
        )
        .ok();
    }

    if let Some(browser_id) = removed_browser_id {
        let app_handle = app.clone();
        tauri::async_runtime::spawn(async move {
            let manager: State<'_, BrowserManager> = app_handle.state();
            if let Err(error) = manager.close_browser(&browser_id).await {
                eprintln!("[BROWSER] Failed to close browser {browser_id}: {error}");
            }
        });
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
        let _ = Notification::new()
            .summary("Codemux")
            .body(&body)
            .hint(notify_rust::Hint::DesktopEntry("com.codemux.app".to_string()))
            .hint(notify_rust::Hint::Transient(true))
            .urgency(notify_rust::Urgency::Critical)
            .show();

        if let Some(window) = app.get_webview_window("main") {
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
            let _ = window.request_user_attention(Some(tauri::UserAttentionType::Critical));
        }

        let _ = std::process::Command::new("hyprctl")
            .args(["dispatch", "focuswindow", "class:com.codemux.app"])
            .output();
    }

    crate::state::emit_app_state(&app);
    Ok(notification_id)
}

#[tauri::command]
pub fn mark_workspace_notifications_read(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    workspace_id: String,
) -> Result<(), String> {
    if state.mark_workspace_notifications_read(&workspace_id) {
        crate::state::emit_app_state(&app);
    }
    Ok(())
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

    for session_id in result.removed_sessions {
        let terminal_state: State<'_, crate::terminal::PtyState> = app.state();
        crate::terminal::close_terminal_session(
            app.clone(),
            terminal_state.clone(),
            state.clone(),
            session_id.0,
        )
        .ok();
    }

    if let Some(browser_id) = result.removed_browser_id {
        let app_handle = app.clone();
        tauri::async_runtime::spawn(async move {
            let manager: State<'_, BrowserManager> = app_handle.state();
            if let Err(error) = manager.close_browser(&browser_id.0).await {
                eprintln!("[BROWSER] Failed to close browser {}: {error}", browser_id.0);
            }
        });
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

fn find_editors() -> Vec<EditorInfo> {
    EDITOR_CANDIDATES
        .iter()
        .filter(|(cmd, _)| {
            std::process::Command::new("which")
                .arg(cmd)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        })
        .map(|(cmd, name)| EditorInfo {
            id: cmd.to_string(),
            name: name.to_string(),
            command: cmd.to_string(),
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
    std::process::Command::new(&editor.command)
        .arg(&path)
        .spawn()
        .map_err(|e| format!("Failed to open editor: {e}"))?;
    Ok(())
}

// ---- Setup/teardown scripts ----

/// Spawn setup scripts in a background thread so workspace creation isn't blocked.
fn spawn_setup_scripts(
    app: &tauri::AppHandle,
    state: &AppStateStore,
    workspace_id: &str,
    workspace_path: &Path,
) {
    let ws_title = {
        let snapshot = state.snapshot();
        snapshot
            .workspaces
            .iter()
            .find(|w| w.workspace_id.0 == workspace_id)
            .map(|w| w.title.clone())
            .unwrap_or_default()
    };
    let ws_path = workspace_path.to_path_buf();
    let ws_id = workspace_id.to_string();
    let app2 = app.clone();

    std::thread::spawn(move || {
        // Wait for frontend to mount the overlay and register event listeners
        std::thread::sleep(std::time::Duration::from_millis(500));
        if let Err(e) = crate::scripts::run_setup_scripts(&ws_path, &ws_title, &ws_id, &app2) {
            eprintln!("[codemux::scripts] Setup failed for workspace {ws_id}: {e}");
        }
    });
}

#[tauri::command]
pub fn get_workspace_config(path: String) -> Option<WorkspaceConfig> {
    crate::config::workspace_config::read_workspace_config(Path::new(&path))
}

#[tauri::command]
pub fn run_workspace_setup(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    workspace_id: String,
) -> Result<(), String> {
    let (cwd, title) = {
        let snapshot = state.snapshot();
        let ws = snapshot
            .workspaces
            .iter()
            .find(|w| w.workspace_id.0 == workspace_id)
            .ok_or_else(|| format!("No workspace found for {workspace_id}"))?;
        (ws.cwd.clone(), ws.title.clone())
    };
    crate::scripts::run_setup_scripts(Path::new(&cwd), &title, &workspace_id, &app)
}

// ---- Workspace sections ----

#[tauri::command]
pub fn create_section(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    name: String,
    color: String,
) -> Result<String, String> {
    let section_id = state.create_section(name, color);
    crate::state::emit_app_state(&app);
    Ok(section_id)
}

#[tauri::command]
pub fn rename_section(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    section_id: String,
    name: String,
) -> Result<(), String> {
    if state.rename_section(&section_id, name) {
        crate::state::emit_app_state(&app);
        Ok(())
    } else {
        Err(format!("No section found for {section_id}"))
    }
}

#[tauri::command]
pub fn delete_section(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    section_id: String,
) -> Result<(), String> {
    if state.delete_section(&section_id) {
        crate::state::emit_app_state(&app);
        Ok(())
    } else {
        Err(format!("No section found for {section_id}"))
    }
}

#[tauri::command]
pub fn set_section_color(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    section_id: String,
    color: String,
) -> Result<(), String> {
    if state.set_section_color(&section_id, color) {
        crate::state::emit_app_state(&app);
        Ok(())
    } else {
        Err(format!("No section found for {section_id}"))
    }
}

#[tauri::command]
pub fn toggle_section_collapsed(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    section_id: String,
) -> Result<(), String> {
    if state.toggle_section_collapsed(&section_id) {
        crate::state::emit_app_state(&app);
        Ok(())
    } else {
        Err(format!("No section found for {section_id}"))
    }
}

#[tauri::command]
pub fn move_workspace_to_section(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    workspace_id: String,
    section_id: Option<String>,
    position: Option<usize>,
) -> Result<(), String> {
    if state.move_workspace_to_section(&workspace_id, section_id.as_deref(), position) {
        crate::state::emit_app_state(&app);
        Ok(())
    } else {
        Err("Failed to move workspace to section".to_string())
    }
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

#[tauri::command]
pub fn reorder_sections(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    section_ids: Vec<String>,
) -> Result<(), String> {
    if state.reorder_sections(section_ids) {
        crate::state::emit_app_state(&app);
        Ok(())
    } else {
        Err("Failed to reorder sections".to_string())
    }
}
