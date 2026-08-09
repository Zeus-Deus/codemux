pub mod agent_chat;
pub mod ai;
pub mod auth;
pub mod automations;
pub mod branch_name;
pub mod browser;
pub mod database;
pub mod files;
pub mod gemini;
pub mod git;
pub mod github;
pub mod hosts;
pub mod mcp;
pub mod opencode;
pub mod package_detect;
pub mod permissions;
pub mod presets;
pub mod project_files;
pub mod settings_sync;
pub mod skills;
pub mod skills_sync;
pub mod source_control;
pub mod update;
pub mod usage;
pub mod usage_import;
pub mod workspace;
pub mod workspaces_sync;

pub use agent_chat::*;
pub use ai::*;
pub use auth::*;
pub use automations::*;
pub use branch_name::*;
pub use browser::*;
pub use database::*;
pub use files::*;
pub use gemini::*;
pub use git::*;
pub use github::*;
pub use hosts::*;
pub use mcp::*;
pub use opencode::*;
pub use package_detect::*;
pub use permissions::*;
pub use usage::*;
pub use usage_import::*;
pub use presets::*;
pub use project_files::*;
pub use settings_sync::*;
pub use skills::*;
pub use skills_sync::*;
pub use source_control::*;
pub use update::*;
pub use workspace::*;
pub use workspaces_sync::*;

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
pub fn clear_adapter_captures<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    app_state: State<'_, crate::state::AppStateStore>,
    session_id: String,
) -> Result<(), String> {
    // The captures drive the frontend's resume affordance, so clearing them
    // without an emit leaves it offering a session that is gone.
    if app_state.clear_terminal_adapter_captures(&session_id) {
        crate::state::schedule_emit_app_state(&app);
    }
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

/// Quit the Codemux app cleanly. Used by the Settings → Interface →
/// Agent Chat GUI toggle (the GUI is the default interface; the toggle
/// is the opt-out back to the classic CLI view): flipping the master
/// flag requires the process to come up fresh under the new state
/// (backend singletons — MCP runtime, OpenCode supervisor, capability
/// caches, ProviderRegistry — only initialise on app boot), and the simplest
/// way to guarantee that is to close the app and let the user reopen
/// it manually.
///
/// Auto-restart was attempted earlier (detached spawn + setsid +
/// /dev/null stdio + control-socket teardown) but the dev-server
/// WebView path can't survive the original cargo runner exiting,
/// and adding a "we're in dev mode, please rerun manually" branch
/// undermined the whole point. A plain quit is honest: the user
/// reopens the app, sees the new state, and there's no "half-broken"
/// surface area to worry about. See git history for the abandoned
/// auto-restart machinery.
///
/// Uses `app.exit(0)` (not `std::process::exit`) so Tauri runs its
/// graceful-shutdown hooks (window-close events, plugin teardown)
/// before the process actually dies.
#[tauri::command]
pub fn quit_app<R: Runtime>(app: tauri::AppHandle<R>) -> Result<(), String> {
    app.exit(0);
    Ok(())
}

/// Atomic flip of the Agent Chat GUI master toggle (default on;
/// turning it off returns to the classic terminal-first interface).
/// Sets `enable_agent_chat` and `enable_lazy_workspace_creation` to
/// the same value in one mutex-held write so the two flags can never
/// end up half-on (the chat surface assumes both are true to
/// function correctly).
///
/// The two flags exist as separate fields for wire-compat with
/// dogfooding observability.json files; every production read-site
/// pairs them with `&&`, so there's no real toggle matrix to expose.
/// `update_feature_flags` is the lower-level setter (writes whatever
/// `FeatureFlags` you hand it); use this command from the Settings
/// → Interface UI to keep the two paired without forcing the
/// frontend to read-modify-write the whole struct.
///
/// Other flags (`unstable_*`) are left untouched.
#[tauri::command]
pub fn set_agent_chat_enabled(
    store: State<'_, ObservabilityStore>,
    enabled: bool,
) -> Result<(), String> {
    let mut flags = store.feature_flags();
    flags.enable_agent_chat = enabled;
    flags.enable_lazy_workspace_creation = enabled;
    store.set_feature_flags(flags);
    Ok(())
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
pub async fn pick_folder_dialog<R: Runtime>(
    window: tauri::Window<R>,
    app: tauri::AppHandle<R>,
    title: Option<String>,
) -> Result<Option<String>, String> {
    crate::ensure_gui_mode(&app)?;
    use tauri_plugin_dialog::DialogExt;
    use tokio::sync::oneshot;

    // Decide the dialog backend up front (issue #95). On Linux, when
    // the portal is unusable but zenity exists, drive zenity ourselves
    // with a sanitized env + timeout instead of letting rfd inherit the
    // broken session environment (which hangs the picker).
    #[cfg(target_os = "linux")]
    {
        match crate::dialog_preflight::select_backend().await {
            crate::dialog_preflight::Backend::Portal => {}
            crate::dialog_preflight::Backend::Zenity(zenity) => {
                let dialog_title = title.clone().unwrap_or_else(|| "Choose folder".to_string());
                return Ok(crate::dialog_fallback::pick_folder(&zenity, &dialog_title)
                    .await?
                    .map(|path| path.to_string_lossy().into_owned()));
            }
            crate::dialog_preflight::Backend::None(message) => {
                return Err(format!(
                    "{}: {message}",
                    crate::dialog_preflight::NO_BACKEND_MARKER
                ));
            }
        }
    }

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
    crate::ensure_gui_mode(&app)?;
    use tauri_plugin_dialog::DialogExt;
    use tokio::sync::oneshot;

    // See pick_folder_dialog — same backend decision (issue #95).
    #[cfg(target_os = "linux")]
    {
        match crate::dialog_preflight::select_backend().await {
            crate::dialog_preflight::Backend::Portal => {}
            crate::dialog_preflight::Backend::Zenity(zenity) => {
                let dialog_title = title.clone().unwrap_or_else(|| "Attach files".to_string());
                return Ok(crate::dialog_fallback::pick_files(&zenity, &dialog_title)
                    .await?
                    .into_iter()
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect());
            }
            crate::dialog_preflight::Backend::None(message) => {
                return Err(format!(
                    "{}: {message}",
                    crate::dialog_preflight::NO_BACKEND_MARKER
                ));
            }
        }
    }

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

/// Save-as dialog. Used by the skills export flow (Stage 4).
/// Caller passes a default filename (e.g.
/// `codemux-skills-export-2026-04-29.json`) and an optional title.
/// Returns `Some(path)` when the user picks one, `None` if they
/// cancel.
#[tauri::command]
pub async fn pick_save_file_dialog<R: Runtime>(
    window: tauri::Window<R>,
    app: tauri::AppHandle<R>,
    title: Option<String>,
    default_filename: Option<String>,
    filter_name: Option<String>,
    filter_extensions: Option<Vec<String>>,
) -> Result<Option<String>, String> {
    crate::ensure_gui_mode(&app)?;
    use tauri_plugin_dialog::DialogExt;
    use tokio::sync::oneshot;

    // See pick_folder_dialog — same backend decision (issue #95).
    #[cfg(target_os = "linux")]
    {
        match crate::dialog_preflight::select_backend().await {
            crate::dialog_preflight::Backend::Portal => {}
            crate::dialog_preflight::Backend::Zenity(zenity) => {
                let dialog_title = title.clone().unwrap_or_else(|| "Save as".to_string());
                return Ok(crate::dialog_fallback::save_file(
                    &zenity,
                    &dialog_title,
                    default_filename.as_deref(),
                    filter_name.as_deref(),
                    filter_extensions.as_deref(),
                )
                .await?
                .map(|path| path.to_string_lossy().into_owned()));
            }
            crate::dialog_preflight::Backend::None(message) => {
                return Err(format!(
                    "{}: {message}",
                    crate::dialog_preflight::NO_BACKEND_MARKER
                ));
            }
        }
    }

    let (tx, rx) = oneshot::channel();

    let mut builder = app
        .dialog()
        .file()
        .set_title(title.as_deref().unwrap_or("Save as"));

    if let Some(name) = default_filename.as_deref() {
        builder = builder.set_file_name(name);
    }
    if let (Some(name), Some(exts)) = (filter_name.as_deref(), filter_extensions.as_ref()) {
        let ext_refs: Vec<&str> = exts.iter().map(String::as_str).collect();
        builder = builder.add_filter(name, &ext_refs);
    }

    #[cfg(desktop)]
    {
        builder = builder.set_parent(&window);
    }

    builder.save_file(move |path| {
        let _ = tx.send(path.map(|p| p.to_string()));
    });

    rx.await.map_err(|error| error.to_string())
}

/// Single-file open-dialog with filter. Used by the skills
/// import flow (Stage 4) where the user is picking a previously-
/// saved JSON backup. Returns `Some(path)` on selection, `None`
/// on cancel.
#[tauri::command]
pub async fn pick_open_file_dialog<R: Runtime>(
    window: tauri::Window<R>,
    app: tauri::AppHandle<R>,
    title: Option<String>,
    filter_name: Option<String>,
    filter_extensions: Option<Vec<String>>,
) -> Result<Option<String>, String> {
    crate::ensure_gui_mode(&app)?;
    use tauri_plugin_dialog::DialogExt;
    use tokio::sync::oneshot;

    // See pick_folder_dialog — same backend decision (issue #95).
    #[cfg(target_os = "linux")]
    {
        match crate::dialog_preflight::select_backend().await {
            crate::dialog_preflight::Backend::Portal => {}
            crate::dialog_preflight::Backend::Zenity(zenity) => {
                let dialog_title = title.clone().unwrap_or_else(|| "Open".to_string());
                return Ok(crate::dialog_fallback::pick_open_file(
                    &zenity,
                    &dialog_title,
                    filter_name.as_deref(),
                    filter_extensions.as_deref(),
                )
                .await?
                .map(|path| path.to_string_lossy().into_owned()));
            }
            crate::dialog_preflight::Backend::None(message) => {
                return Err(format!(
                    "{}: {message}",
                    crate::dialog_preflight::NO_BACKEND_MARKER
                ));
            }
        }
    }

    let (tx, rx) = oneshot::channel();

    let mut builder = app
        .dialog()
        .file()
        .set_title(title.as_deref().unwrap_or("Open"));

    if let (Some(name), Some(exts)) = (filter_name.as_deref(), filter_extensions.as_ref()) {
        let ext_refs: Vec<&str> = exts.iter().map(String::as_str).collect();
        builder = builder.add_filter(name, &ext_refs);
    }

    #[cfg(desktop)]
    {
        builder = builder.set_parent(&window);
    }

    builder.pick_file(move |path| {
        let _ = tx.send(path.map(|p| p.to_string()));
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
