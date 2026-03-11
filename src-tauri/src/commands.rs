use crate::browser::BrowserManager;
use crate::config::{read_shell_appearance_or_default, read_theme_colors_or_default, ShellAppearance, ThemeColors};
use crate::indexing::{
    rebuild_index, search_index, IndexSearchResult, ProjectIndexSnapshot, ProjectIndexStatus,
    ProjectIndexStore,
};
use crate::memory::{
    add_memory_entry, generate_handoff_packet, get_project_memory, update_project_memory,
    HandoffPacket, MemoryEntryKind, MemorySource, ProjectMemorySnapshot, ProjectMemoryUpdate,
};
use crate::openflow::{
    OpenFlowCreateRunRequest, OpenFlowDesignSpec, OpenFlowRunRecord, OpenFlowRunStatus,
    OpenFlowRuntimeSnapshot, OpenFlowRuntimeStore,
};
use crate::observability::{
    FeatureFlags, LogLevel, ObservabilitySnapshot, ObservabilityStore, PermissionPolicy,
    SafetyConfig,
};
use crate::state::{AppStateSnapshot, AppStateStore, NotificationLevel};
use crate::state::WorkspacePresetLayout;
use crate::terminal;
use notify_rust::Notification;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use tauri::{Emitter, Manager, Runtime, State};
use tokio::sync::oneshot;
use tokio::time::{timeout, Duration};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

static BROWSER_AUTOMATION_REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserProxyFetchResult {
    pub html: String,
    pub final_url: String,
    pub status: u16,
    pub content_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserEvalResult {
    pub result: String,
    pub error: Option<String>,
}

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
    let workspace_id = match cwd {
        Some(path) => state.create_workspace_at_path(PathBuf::from(path)),
        None => state.create_workspace(),
    };
    if let Some(session_id) = state.active_terminal_session_id() {
        terminal::spawn_pty_for_session(app.clone(), session_id.0);
    }
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

    let workspace_id = match cwd {
        Some(path) => state.create_workspace_with_layout(PathBuf::from(path), layout),
        None => state.create_workspace_with_layout(crate::project::current_project_root(), layout),
    };

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
    terminal::spawn_pty_for_session(app.clone(), session_id.0.clone());
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
    let removed_browser_id = state.pane_browser_id(&pane_id);
    let removed = state.close_pane(&pane_id)?;
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
pub fn browser_proxy_fetch(url: String) -> Result<BrowserProxyFetchResult, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let response = client
        .get(&url)
        .send()
        .map_err(|e| format!("Failed to fetch URL: {}", e))?;

    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let final_url = response.url().to_string();
    let html = response.text().map_err(|e| format!("Failed to read response: {}", e))?;

    Ok(BrowserProxyFetchResult {
        html,
        final_url,
        status,
        content_type,
    })
}

#[tauri::command]
pub fn browser_proxy_screenshot(url: String) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let response = client
        .get(&url)
        .send()
        .map_err(|e| format!("Failed to fetch URL: {}", e))?;

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "image/png".to_string());

    let bytes = response.bytes().map_err(|e| format!("Failed to read response: {}", e))?;

    let base64_data = BASE64.encode(&bytes);

    let mime_type = if content_type.contains("jpeg") || content_type.contains("jpg") {
        "image/jpeg"
    } else if content_type.contains("png") {
        "image/png"
    } else if content_type.contains("gif") {
        "image/gif"
    } else if content_type.contains("webp") {
        "image/webp"
    } else {
        "image/png"
    };

    Ok(format!("data:{};base64,{}", mime_type, base64_data))
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

#[tauri::command]
pub fn run_openflow_autonomous_loop(
    store: State<'_, OpenFlowRuntimeStore>,
    run_id: String,
) -> Result<OpenFlowRunRecord, String> {
    store.run_autonomous_loop(&run_id)
}

#[tauri::command]
pub fn apply_openflow_review_result(
    store: State<'_, OpenFlowRuntimeStore>,
    run_id: String,
    reviewer_score: u8,
    accepted: bool,
    issue: Option<String>,
) -> Result<OpenFlowRunRecord, String> {
    store.apply_review_result(&run_id, reviewer_score, accepted, issue)
}

#[tauri::command]
pub fn stop_openflow_run(
    store: State<'_, OpenFlowRuntimeStore>,
    run_id: String,
    status: String,
    reason: String,
) -> Result<OpenFlowRunRecord, String> {
    let status = match status.as_str() {
        "failed" => OpenFlowRunStatus::Failed,
        "cancelled" => OpenFlowRunStatus::Cancelled,
        "awaiting_approval" => OpenFlowRunStatus::AwaitingApproval,
        _ => OpenFlowRunStatus::Failed,
    };
    store.stop_run(&run_id, status, reason)
}

#[tauri::command]
pub fn get_observability_snapshot(
    store: State<'_, ObservabilityStore>,
) -> Result<ObservabilitySnapshot, String> {
    Ok(store.snapshot())
}

#[tauri::command]
pub fn add_structured_log(
    store: State<'_, ObservabilityStore>,
    source: String,
    level: String,
    message: String,
    metadata: Vec<(String, String)>,
) -> Result<(), String> {
    let level = match level.as_str() {
        "warning" => LogLevel::Warning,
        "error" => LogLevel::Error,
        _ => LogLevel::Info,
    };
    store.log(&source, level, message, metadata);
    Ok(())
}

#[tauri::command]
pub fn update_feature_flags(
    store: State<'_, ObservabilityStore>,
    flags: FeatureFlags,
) -> Result<(), String> {
    store.set_feature_flags(flags);
    Ok(())
}

#[tauri::command]
pub fn update_permission_policy(
    store: State<'_, ObservabilityStore>,
    policy: PermissionPolicy,
) -> Result<(), String> {
    store.set_permission_policy(policy);
    Ok(())
}

#[tauri::command]
pub fn update_safety_config(
    store: State<'_, ObservabilityStore>,
    config: SafetyConfig,
) -> Result<(), String> {
    store.set_safety_config(config);
    Ok(())
}

#[tauri::command]
pub fn add_replay_record(
    store: State<'_, ObservabilityStore>,
    title: String,
    summary: String,
) -> Result<(), String> {
    store.add_replay_record(title, summary);
    Ok(())
}

/// Open a native folder-picker dialog, properly parented to the calling window on all
/// desktop platforms (including Linux/Wayland). The built-in JS `open()` from
/// `tauri-plugin-dialog` skips `set_parent` on Linux due to an upstream bug
/// (https://github.com/tauri-apps/plugins-workspace/issues — `commands.rs` uses
/// `#[cfg(any(windows, target_os = "macos"))]` instead of `#[cfg(desktop)]`), which
/// means the portal-gtk dialog opens with no transient-for relationship and tiles
/// instead of floating. This command fixes that by calling `set_parent` unconditionally
/// on all desktop platforms.
#[tauri::command]
pub async fn pick_folder_dialog<R: Runtime>(
    window: tauri::Window<R>,
    app: tauri::AppHandle<R>,
    title: Option<String>,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    use tokio::sync::oneshot;

    let (tx, rx) = oneshot::channel();

    let mut builder = app
        .dialog()
        .file()
        .set_title(title.as_deref().unwrap_or("Choose folder"));

    #[cfg(desktop)]
    {
        builder = builder.set_parent(&window);
    }

    builder.pick_folder(move |path| {
        let _ = tx.send(path.map(|p| p.to_string()));
    });

    rx.await.map_err(|e| e.to_string())
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
