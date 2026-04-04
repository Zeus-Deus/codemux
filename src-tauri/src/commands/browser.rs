use crate::agent_browser::{AgentBrowserManager, BrowserAutomationResult};
use crate::state::AppStateStore;
use std::path::PathBuf;
use tauri::State;

pub(crate) fn create_browser_pane_impl(
    app: tauri::AppHandle,
    state: &AppStateStore,
    pane_id: String,
    url: Option<String>,
) -> Result<String, String> {
    // Check if this workspace has a detached agent browser session to reconnect to.
    let workspace_id = state.workspace_id_for_pane(&pane_id);
    let agent_session = workspace_id
        .as_ref()
        .and_then(|wid| state.find_detached_agent_browser(wid));

    // Use the agent session's URL if reconnecting and no explicit URL was given.
    let effective_url = if url.is_some() {
        url
    } else {
        agent_session.as_ref().and_then(|s| s.current_url.clone())
    };

    let (new_pane_id, browser_id) = state.create_browser_pane(&pane_id, effective_url.as_deref())?;

    // Attach the agent session to the new pane for reconnection.
    if let (Some(wid), Some(_)) = (&workspace_id, &agent_session) {
        let _ = state.attach_agent_browser_to_pane(wid, &new_pane_id, &browser_id);
        eprintln!("[BROWSER] Reconnected agent browser session to new pane in workspace {wid}");
    }

    crate::state::emit_app_state(&app);
    Ok(new_pane_id.0)
}

pub(crate) fn browser_open_url_impl(
    app: tauri::AppHandle,
    state: &AppStateStore,
    browser_id: String,
    url: String,
) -> Result<(), String> {
    state.update_browser_url(&browser_id, url)?;
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn create_browser_pane(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    pane_id: String,
    url: Option<String>,
) -> Result<String, String> {
    create_browser_pane_impl(app, &state, pane_id, url)
}

#[tauri::command]
pub fn browser_open_url(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    browser_id: String,
    url: String,
) -> Result<(), String> {
    browser_open_url_impl(app, &state, browser_id, url)
}

#[tauri::command]
pub fn browser_history_back(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    browser_id: String,
) -> Result<(), String> {
    state.browser_history_step(&browser_id, -1)?;
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn browser_history_forward(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    browser_id: String,
) -> Result<(), String> {
    state.browser_history_step(&browser_id, 1)?;
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn browser_reload(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    browser_id: String,
) -> Result<(), String> {
    state.reload_browser(&browser_id)?;
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub fn browser_set_loading_state(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    browser_id: String,
    is_loading: bool,
    error: Option<String>,
) -> Result<(), String> {
    state.set_browser_loading_state(&browser_id, is_loading, error)?;
    crate::state::emit_app_state(&app);
    Ok(())
}

#[tauri::command]
pub async fn agent_browser_spawn(
    manager: State<'_, AgentBrowserManager>,
    browser_id: String,
) -> Result<(), String> {
    manager.spawn(&browser_id).await
}

#[tauri::command]
pub async fn agent_browser_run(
    manager: State<'_, AgentBrowserManager>,
    browser_id: String,
    action: String,
    params: serde_json::Value,
) -> Result<BrowserAutomationResult, String> {
    manager.run_command(&browser_id, &action, params).await
}

#[tauri::command]
pub async fn agent_browser_close(
    manager: State<'_, AgentBrowserManager>,
    browser_id: String,
) -> Result<(), String> {
    manager.close(&browser_id).await
}

#[tauri::command]
pub async fn start_browser_stream(
    manager: State<'_, AgentBrowserManager>,
    browser_id: String,
) -> Result<String, String> {
    manager.start_stream(&browser_id).await
}

#[tauri::command]
pub async fn agent_browser_screenshot(
    manager: State<'_, AgentBrowserManager>,
    browser_id: String,
) -> Result<String, String> {
    manager.get_screenshot(&browser_id).await
}

// ── Browser Data Management ──

fn agent_browser_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".agent-browser")
}

fn dir_size(path: &std::path::Path) -> u64 {
    if !path.is_dir() {
        return 0;
    }
    let mut total = 0u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                total += meta.len();
            }
        }
    }
    total
}

#[tauri::command]
pub fn get_browser_data_size() -> Result<u64, String> {
    Ok(dir_size(&agent_browser_dir()))
}

#[tauri::command]
pub fn clear_browser_cookies() -> Result<(), String> {
    let sessions_dir = agent_browser_dir().join("sessions");
    if sessions_dir.exists() {
        std::fs::remove_dir_all(&sessions_dir)
            .map_err(|e| format!("Failed to clear browser cookies: {e}"))?;
    }
    Ok(())
}

#[tauri::command]
pub fn clear_all_browser_data() -> Result<(), String> {
    let dir = agent_browser_dir();
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .map_err(|e| format!("Failed to clear browser data: {e}"))?;
    }
    Ok(())
}
