pub mod browser;
pub mod files;
pub mod git;
pub mod github;
pub mod openflow;
pub mod presets;
pub mod workspace;

pub use browser::*;
pub use files::*;
pub use git::*;
pub use github::*;
pub use openflow::*;
pub use presets::*;
pub use workspace::*;

use crate::indexing::{
    rebuild_index,
    search_index,
    IndexSearchResult,
    ProjectIndexSnapshot,
    ProjectIndexStatus,
    ProjectIndexStore,
};
use crate::memory::{
    add_memory_entry,
    generate_handoff_packet,
    get_project_memory,
    update_project_memory,
    HandoffPacket,
    MemoryEntryKind,
    MemorySource,
    ProjectMemorySnapshot,
    ProjectMemoryUpdate,
};
use crate::observability::{
    FeatureFlags,
    LogLevel,
    ObservabilitySnapshot,
    ObservabilityStore,
    PermissionPolicy,
    SafetyConfig,
};
use tauri::{Runtime, State};

#[tauri::command]
pub fn get_project_memory_snapshot(
    project_root: Option<String>,
) -> Result<ProjectMemorySnapshot, String> {
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
pub fn get_project_index_status(
    store: State<'_, ProjectIndexStore>,
) -> Result<ProjectIndexStatus, String> {
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
        let _ = tx.send(path.map(|path| path.to_string()));
    });

    rx.await.map_err(|error| error.to_string())
}

// ---- Port management ----

#[tauri::command]
pub fn get_detected_ports(
    state: State<'_, crate::state::AppStateStore>,
) -> Vec<crate::state::PortInfoSnapshot> {
    state.snapshot().detected_ports
}

#[tauri::command]
pub fn kill_port(port: u16) -> Result<(), String> {
    let ports = crate::ports::detect_listening_ports();
    let target = ports
        .iter()
        .find(|p| p.port == port)
        .ok_or_else(|| format!("No process found listening on port {port}"))?;

    let pid = target.pid;
    let output = std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .output()
        .map_err(|e| format!("Failed to kill PID {pid}: {e}"))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("Failed to kill PID {pid}: {stderr}"))
    }
}
