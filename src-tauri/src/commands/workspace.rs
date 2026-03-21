use crate::browser::BrowserManager;
use crate::config::{
    read_shell_appearance_or_default,
    read_theme_colors_or_default,
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
use std::path::PathBuf;
use tauri::{Manager, State};

pub(crate) fn create_workspace_impl(
    app: tauri::AppHandle,
    state: &AppStateStore,
    cwd: Option<String>,
) -> Result<String, String> {
    let workspace_id = match &cwd {
        Some(path) => state.create_workspace_at_path(PathBuf::from(path)),
        None => state.create_workspace(),
    };

    // Populate git branch info
    let repo_path = cwd
        .map(PathBuf::from)
        .unwrap_or_else(crate::project::current_project_root);
    if let Ok(info) = crate::git::git_branch_info(&repo_path) {
        state.update_workspace_git_branch(&workspace_id.0, info.branch);
    }

    if let Some(session_id) = state.active_terminal_session_id() {
        terminal::spawn_pty_for_session(app.clone(), session_id.0);
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

    if let Ok(info) = crate::git::git_branch_info(&repo_path) {
        state.update_workspace_git_branch(&workspace_id.0, info.branch);
    }

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

    crate::state::emit_app_state(&app);
    Ok(workspace_id.0)
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
) -> Result<String, String> {
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
