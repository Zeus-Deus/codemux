use crate::agent_browser::{AgentBrowserManager, BrowserAutomationResult};
use crate::browser::BrowserManager;
use crate::state::AppStateStore;
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
pub async fn browser_spawn(
    manager: State<'_, BrowserManager>,
    browser_id: String,
) -> Result<String, String> {
    manager.spawn_browser(browser_id).await?;
    Ok("Browser spawned".to_string())
}

#[tauri::command]
pub async fn browser_navigate(
    manager: State<'_, BrowserManager>,
    browser_id: String,
    url: String,
) -> Result<String, String> {
    manager.navigate(&browser_id, &url).await
}

#[tauri::command]
pub async fn browser_screenshot(
    manager: State<'_, BrowserManager>,
    browser_id: String,
) -> Result<String, String> {
    manager.screenshot(&browser_id).await
}

#[tauri::command]
pub async fn browser_click(
    manager: State<'_, BrowserManager>,
    browser_id: String,
    x: f64,
    y: f64,
) -> Result<String, String> {
    manager.click(&browser_id, x, y).await
}

#[tauri::command]
pub async fn browser_type(
    manager: State<'_, BrowserManager>,
    browser_id: String,
    text: String,
) -> Result<String, String> {
    manager.type_text(&browser_id, &text).await
}

#[tauri::command]
pub async fn browser_close(
    manager: State<'_, BrowserManager>,
    browser_id: String,
) -> Result<(), String> {
    manager.close_browser(&browser_id).await
}

#[tauri::command]
pub async fn browser_resize_viewport(
    manager: State<'_, BrowserManager>,
    browser_id: String,
    width: u32,
    height: u32,
) -> Result<(), String> {
    manager.resize_viewport(&browser_id, width, height).await
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
pub fn agent_browser_get_stream_url(
    manager: State<'_, AgentBrowserManager>,
) -> Result<String, String> {
    Ok(manager.get_stream_url())
}

#[tauri::command]
pub async fn start_browser_stream(
    manager: State<'_, AgentBrowserManager>,
    browser_id: String,
) -> Result<String, String> {
    eprintln!("[stream] start_browser_stream command called with browser_id={:?}", browser_id);
    let result = manager.start_stream(&browser_id).await;
    match &result {
        Ok(url) => eprintln!("[stream] returning stream_url={}", url),
        Err(e) => eprintln!("[stream] start_stream FAILED: {}", e),
    }
    result
}

#[tauri::command]
pub async fn agent_browser_screenshot(
    manager: State<'_, AgentBrowserManager>,
    browser_id: String,
) -> Result<String, String> {
    manager.get_screenshot(&browser_id).await
}
