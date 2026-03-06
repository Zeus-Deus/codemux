use crate::config::{read_theme_colors_or_default, ThemeColors};
use crate::indexing::{
    rebuild_index, search_index, IndexSearchResult, ProjectIndexSnapshot, ProjectIndexStatus,
    ProjectIndexStore,
};
use crate::memory::{
    add_memory_entry, generate_handoff_packet, get_project_memory, update_project_memory,
    HandoffPacket, MemoryEntryKind, MemorySource, ProjectMemorySnapshot, ProjectMemoryUpdate,
};
use crate::openflow::{
    OpenFlowCreateRunRequest, OpenFlowDesignSpec, OpenFlowRunRecord, OpenFlowRuntimeSnapshot,
    OpenFlowRuntimeStore,
};
use crate::state::{AppStateSnapshot, AppStateStore, NotificationLevel};
use notify_rust::Notification;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use tauri::{Emitter, State};
use tokio::sync::oneshot;
use tokio::time::{timeout, Duration};

static BROWSER_AUTOMATION_REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Default)]
pub struct BrowserAutomationCoordinator {
    pending: Mutex<HashMap<String, oneshot::Sender<Result<BrowserAutomationResult, String>>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BrowserAutomationAction {
    OpenUrl { url: String },
    DomSnapshot,
    AccessibilitySnapshot,
    Click { selector: String },
    Fill { selector: String, value: String },
    TypeText { text: String },
    Scroll { x: f64, y: f64 },
    Evaluate { script: String },
    Screenshot,
    ConsoleLogs,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserAutomationRequest {
    pub request_id: String,
    pub browser_id: String,
    pub action: BrowserAutomationAction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserAutomationResult {
    pub request_id: String,
    pub browser_id: String,
    pub data: Value,
    pub message: Option<String>,
}

async fn dispatch_browser_automation(
    app: tauri::AppHandle,
    coordinator: &State<'_, BrowserAutomationCoordinator>,
    request: BrowserAutomationRequest,
) -> Result<BrowserAutomationResult, String> {
    let (tx, rx) = oneshot::channel();
    coordinator
        .pending
        .lock()
        .unwrap()
        .insert(request.request_id.clone(), tx);

    app.emit("browser-automation-request", &request)
        .map_err(|error| format!("Failed to emit browser automation request: {error}"))?;

    match timeout(Duration::from_secs(12), rx).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => Err("Browser automation channel closed unexpectedly".into()),
        Err(_) => {
            coordinator.pending.lock().unwrap().remove(&request.request_id);
            Err("Browser automation request timed out".into())
        }
    }
}

#[tauri::command]
pub fn get_current_theme() -> Result<ThemeColors, String> {
    Ok(read_theme_colors_or_default())
}

#[tauri::command]
pub fn get_app_state(state: State<'_, AppStateStore>) -> Result<AppStateSnapshot, String> {
    Ok(state.snapshot())
}

#[tauri::command]
pub fn create_workspace(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
) -> Result<String, String> {
    let workspace_id = state.create_workspace();
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
    let direction = match direction.as_str() {
        "horizontal" => crate::state::SplitDirection::Horizontal,
        "vertical" => crate::state::SplitDirection::Vertical,
        _ => return Err(format!("Unsupported split direction: {direction}")),
    };

    let session_id = state.split_pane(&pane_id, direction)?;
    crate::state::emit_app_state(&app);
    Ok(session_id.0)
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
    let removed = state.close_pane(&pane_id)?;
    crate::state::emit_app_state(&app);
    Ok(removed.map(|session_id| session_id.0))
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
        let _ = Notification::new().summary("Codemux").body(&body).show();
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
pub fn create_browser_pane(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    pane_id: String,
) -> Result<String, String> {
    let browser_id = state.create_browser_pane(&pane_id)?;
    crate::state::emit_app_state(&app);
    Ok(browser_id.0)
}

#[tauri::command]
pub fn browser_open_url(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    browser_id: String,
    url: String,
) -> Result<(), String> {
    state.update_browser_url(&browser_id, url)?;
    crate::state::emit_app_state(&app);
    Ok(())
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
pub fn browser_capture_screenshot(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    browser_id: String,
) -> Result<String, String> {
    let base = dirs::cache_dir()
        .ok_or_else(|| "Could not determine cache directory".to_string())?
        .join("codemux")
        .join("screenshots");
    std::fs::create_dir_all(&base)
        .map_err(|error| format!("Failed to create screenshot directory: {error}"))?;

    let output = base.join(format!("{browser_id}.png"));
    let status = Command::new("grim")
        .arg(output.as_os_str())
        .status()
        .map_err(|error| format!("Failed to run grim for screenshot capture: {error}"))?;

    if !status.success() {
        return Err("Screenshot capture command failed".into());
    }

    let path = output.display().to_string();
    state.set_browser_screenshot_path(&browser_id, path.clone())?;
    crate::state::emit_app_state(&app);
    Ok(path)
}

#[tauri::command]
pub async fn browser_automation_run(
    app: tauri::AppHandle,
    coordinator: State<'_, BrowserAutomationCoordinator>,
    browser_id: String,
    action: BrowserAutomationAction,
) -> Result<BrowserAutomationResult, String> {
    let request = BrowserAutomationRequest {
        request_id: format!(
            "browser-automation-{}",
            BROWSER_AUTOMATION_REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed)
        ),
        browser_id,
        action,
    };

    dispatch_browser_automation(app, &coordinator, request).await
}

#[tauri::command]
pub fn browser_automation_complete(
    coordinator: State<'_, BrowserAutomationCoordinator>,
    request_id: String,
    result: Result<BrowserAutomationResult, String>,
) -> Result<(), String> {
    let sender = coordinator
        .pending
        .lock()
        .unwrap()
        .remove(&request_id)
        .ok_or_else(|| format!("No pending browser automation request found for {request_id}"))?;

    sender
        .send(result)
        .map_err(|_| format!("Failed to deliver browser automation result for {request_id}"))?;

    Ok(())
}

#[tauri::command]
pub fn get_project_memory_snapshot(project_root: Option<String>) -> Result<ProjectMemorySnapshot, String> {
    get_project_memory(project_root)
}

#[tauri::command]
pub fn update_project_memory_snapshot(
    project_root: Option<String>,
    update: ProjectMemoryUpdate,
) -> Result<ProjectMemorySnapshot, String> {
    update_project_memory(project_root, update)
}

#[tauri::command]
pub fn add_project_memory_entry(
    project_root: Option<String>,
    kind: MemoryEntryKind,
    source: MemorySource,
    content: String,
    tags: Vec<String>,
    tool_name: Option<String>,
    session_label: Option<String>,
) -> Result<ProjectMemorySnapshot, String> {
    add_memory_entry(
        project_root,
        kind,
        source,
        content,
        tags,
        tool_name,
        session_label,
    )
}

#[tauri::command]
pub fn generate_project_handoff(project_root: Option<String>) -> Result<HandoffPacket, String> {
    generate_handoff_packet(project_root)
}

#[tauri::command]
pub fn rebuild_project_index(
    store: State<'_, ProjectIndexStore>,
    project_root: Option<String>,
) -> Result<ProjectIndexSnapshot, String> {
    let snapshot = rebuild_index(project_root)?;
    store.replace_snapshot(snapshot.clone());
    Ok(snapshot)
}

#[tauri::command]
pub fn get_project_index_status(store: State<'_, ProjectIndexStore>) -> Result<ProjectIndexStatus, String> {
    Ok(store.status())
}

#[tauri::command]
pub fn search_project_index(
    store: State<'_, ProjectIndexStore>,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<IndexSearchResult>, String> {
    Ok(search_index(&store, &query, limit))
}

#[tauri::command]
pub fn get_openflow_design_spec() -> Result<OpenFlowDesignSpec, String> {
    Ok(crate::openflow::default_openflow_spec())
}

#[tauri::command]
pub fn get_openflow_runtime_snapshot(
    store: State<'_, OpenFlowRuntimeStore>,
) -> Result<OpenFlowRuntimeSnapshot, String> {
    Ok(store.snapshot())
}

#[tauri::command]
pub fn create_openflow_run(
    store: State<'_, OpenFlowRuntimeStore>,
    request: OpenFlowCreateRunRequest,
) -> Result<OpenFlowRunRecord, String> {
    Ok(store.create_run(request))
}

#[tauri::command]
pub fn advance_openflow_run_phase(
    store: State<'_, OpenFlowRuntimeStore>,
    run_id: String,
) -> Result<OpenFlowRunRecord, String> {
    store.advance_run_phase(&run_id)
}

#[tauri::command]
pub fn retry_openflow_run(
    store: State<'_, OpenFlowRuntimeStore>,
    run_id: String,
) -> Result<OpenFlowRunRecord, String> {
    store.retry_run(&run_id)
}
