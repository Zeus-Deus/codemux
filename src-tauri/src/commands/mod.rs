pub mod agent_chat;
pub mod ai;
pub mod auth;
pub mod branch_name;
pub mod browser;
pub mod database;
pub mod files;
pub mod git;
pub mod github;
pub mod openflow;
pub mod package_detect;
pub mod permissions;
pub mod presets;
pub mod settings_sync;
pub mod update;
pub mod virtual_display;
pub mod workspace;

pub use agent_chat::*;
pub use ai::*;
pub use auth::*;
pub use branch_name::*;
pub use browser::*;
pub use database::*;
pub use files::*;
pub use git::*;
pub use github::*;
pub use openflow::*;
pub use package_detect::*;
pub use permissions::*;
pub use presets::*;
pub use settings_sync::*;
pub use update::*;
pub use virtual_display::*;
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
    app_state: State<'_, crate::state::AppStateStore>,
    project_root: Option<String>,
) -> Result<ProjectIndexSnapshot, String> {
    let root = project_root.or_else(|| app_state.active_workspace_cwd().map(|(_, cwd)| cwd));
    let snapshot = rebuild_index(root)?;
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
pub fn debug_log(message: String) {
    eprintln!("{message}");
}

#[tauri::command]
pub fn clear_adapter_captures(
    app_state: State<'_, crate::state::AppStateStore>,
    session_id: String,
) -> Result<(), String> {
    app_state.clear_terminal_adapter_captures(&session_id);
    Ok(())
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

/// Read the current feature-flag snapshot.
///
/// Thin wrapper around
/// [`ObservabilityStore::feature_flags`](crate::observability::ObservabilityStore::feature_flags)
/// so the frontend can check a single gate without pulling the whole
/// observability snapshot. Used by the agent-chat pane shell to decide
/// whether to offer the `agent_chat` pane kind.
#[tauri::command]
pub fn get_feature_flags(
    store: State<'_, ObservabilityStore>,
) -> Result<FeatureFlags, String> {
    Ok(store.feature_flags())
}

/// Return the user's home directory as a string.
///
/// Used by the sidebar-header "+" chat flow to create a workspace
/// anchored at `~` when the user wants an ambient chat not tied to
/// a specific project. Errors if `dirs::home_dir()` returns `None`
/// (no `HOME` env on Unix, no `USERPROFILE` on Windows).
#[tauri::command]
pub fn get_home_dir() -> Result<String, String> {
    dirs::home_dir()
        .map(|p| p.display().to_string())
        .ok_or_else(|| "home_dir_unavailable".to_string())
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

#[tauri::command]
pub async fn pick_files_dialog<R: Runtime>(
    window: tauri::Window<R>,
    app: tauri::AppHandle<R>,
    title: Option<String>,
) -> Result<Vec<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    use tokio::sync::oneshot;

    let (tx, rx) = oneshot::channel();

    let mut builder = app
        .dialog()
        .file()
        .set_title(title.as_deref().unwrap_or("Attach files"));

    #[cfg(desktop)]
    {
        builder = builder.set_parent(&window);
    }

    builder.pick_files(move |paths| {
        let _ = tx.send(
            paths
                .map(|ps| ps.into_iter().map(|p| p.to_string()).collect())
                .unwrap_or_default(),
        );
    });

    rx.await.map_err(|error| error.to_string())
}

// ---- Platform info ----

/// Returns the current OS as reported by `std::env::consts::OS`.
///
/// Values are the standard Rust target strings: `"linux"`, `"macos"`,
/// `"windows"`, `"freebsd"`, `"android"`, `"ios"`, etc. The frontend uses this
/// to gate Windows-incompatible features (e.g., OpenFlow — the bash wrapper
/// scripts in `openflow::prompts` do not have Windows equivalents yet).
#[tauri::command]
pub fn get_platform() -> String {
    std::env::consts::OS.to_string()
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
    let output = if cfg!(windows) {
        let mut cmd = std::process::Command::new("taskkill");
        cmd.args(["/PID", &pid.to_string(), "/F"]);
        crate::execution::sanitize_gui_env_std(&mut cmd);
        cmd.output()
            .map_err(|e| format!("Failed to kill PID {pid}: {e}"))?
    } else {
        let mut cmd = std::process::Command::new("kill");
        cmd.args(["-9", &pid.to_string()]);
        crate::execution::sanitize_gui_env_std(&mut cmd);
        cmd.output()
            .map_err(|e| format!("Failed to kill PID {pid}: {e}"))?
    };

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("Failed to kill PID {pid}: {stderr}"))
    }
}
