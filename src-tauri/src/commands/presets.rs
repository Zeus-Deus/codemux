use std::io::Write;
use std::sync::Arc;

use tauri::State;

use crate::database::DatabaseStore;
use crate::presets::{
    emit_presets_changed, save_presets, snapshot_from_store, LaunchMode, PresetStoreSnapshot,
    PresetStoreState, TerminalPreset,
};
use crate::state::AppStateStore;
use crate::terminal;
use crate::terminal::PtyState;

#[tauri::command]
pub fn get_presets(
    presets: State<'_, PresetStoreState>,
) -> Result<PresetStoreSnapshot, String> {
    let store = presets.inner.lock().unwrap_or_else(|e| e.into_inner());
    Ok(snapshot_from_store(&store))
}

#[tauri::command]
pub fn create_preset(
    app: tauri::AppHandle,
    db: State<'_, DatabaseStore>,
    presets: State<'_, PresetStoreState>,
    name: String,
    description: Option<String>,
    commands: Vec<String>,
    working_directory: Option<String>,
    launch_mode: LaunchMode,
    pinned: bool,
) -> Result<String, String> {
    let id = uuid::Uuid::new_v4().to_string();
    let preset = TerminalPreset {
        id: id.clone(),
        name,
        description,
        commands,
        working_directory,
        launch_mode,
        icon: None,
        pinned,
        is_builtin: false,
        auto_run_on_workspace: false,
        auto_run_on_new_tab: false,
    };

    let mut store = presets.inner.lock().unwrap_or_else(|e| e.into_inner());
    store.presets.push(preset);
    save_presets(&db, &store)?;
    drop(store);

    emit_presets_changed(&app);
    Ok(id)
}

#[tauri::command]
pub fn update_preset(
    app: tauri::AppHandle,
    db: State<'_, DatabaseStore>,
    presets: State<'_, PresetStoreState>,
    id: String,
    name: Option<String>,
    description: Option<String>,
    commands: Option<Vec<String>>,
    working_directory: Option<String>,
    launch_mode: Option<LaunchMode>,
    pinned: Option<bool>,
    icon: Option<String>,
    auto_run_on_workspace: Option<bool>,
    auto_run_on_new_tab: Option<bool>,
) -> Result<(), String> {
    let mut store = presets.inner.lock().unwrap_or_else(|e| e.into_inner());
    let preset = store
        .presets
        .iter_mut()
        .find(|p| p.id == id)
        .ok_or_else(|| format!("Preset not found: {id}"))?;

    // All presets are fully editable (only delete is protected for builtins)
    if let Some(name) = name {
        preset.name = name;
    }
    if let Some(desc) = description {
        preset.description = Some(desc);
    }
    if let Some(cmds) = commands {
        preset.commands = cmds;
    }
    if let Some(wd) = working_directory {
        preset.working_directory = if wd.is_empty() { None } else { Some(wd) };
    }
    if let Some(mode) = launch_mode {
        preset.launch_mode = mode;
    }
    if let Some(pinned) = pinned {
        preset.pinned = pinned;
    }
    if let Some(icon) = icon {
        preset.icon = if icon.is_empty() { None } else { Some(icon) };
    }
    if let Some(v) = auto_run_on_workspace {
        preset.auto_run_on_workspace = v;
    }
    if let Some(v) = auto_run_on_new_tab {
        preset.auto_run_on_new_tab = v;
    }

    save_presets(&db, &store)?;
    drop(store);

    emit_presets_changed(&app);
    Ok(())
}

#[tauri::command]
pub fn delete_preset(
    app: tauri::AppHandle,
    db: State<'_, DatabaseStore>,
    presets: State<'_, PresetStoreState>,
    id: String,
) -> Result<(), String> {
    let mut store = presets.inner.lock().unwrap_or_else(|e| e.into_inner());

    let preset = store
        .presets
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(|| format!("Preset not found: {id}"))?;

    if preset.is_builtin {
        return Err("Cannot delete built-in presets".into());
    }

    store.presets.retain(|p| p.id != id);

    if store.default_preset_id.as_deref() == Some(id.as_str()) {
        store.default_preset_id = None;
    }

    save_presets(&db, &store)?;
    drop(store);

    emit_presets_changed(&app);
    Ok(())
}

#[tauri::command]
pub fn set_preset_pinned(
    app: tauri::AppHandle,
    db: State<'_, DatabaseStore>,
    presets: State<'_, PresetStoreState>,
    id: String,
    pinned: bool,
) -> Result<(), String> {
    let mut store = presets.inner.lock().unwrap_or_else(|e| e.into_inner());
    let preset = store
        .presets
        .iter_mut()
        .find(|p| p.id == id)
        .ok_or_else(|| format!("Preset not found: {id}"))?;

    preset.pinned = pinned;
    save_presets(&db, &store)?;
    drop(store);

    emit_presets_changed(&app);
    Ok(())
}

#[tauri::command]
pub fn set_preset_bar_visible(
    app: tauri::AppHandle,
    db: State<'_, DatabaseStore>,
    presets: State<'_, PresetStoreState>,
    visible: bool,
) -> Result<(), String> {
    let mut store = presets.inner.lock().unwrap_or_else(|e| e.into_inner());
    store.bar_visible = visible;
    save_presets(&db, &store)?;
    drop(store);

    emit_presets_changed(&app);
    Ok(())
}

#[tauri::command]
pub fn apply_preset(
    app: tauri::AppHandle,
    state: State<'_, AppStateStore>,
    pty_state: State<'_, PtyState>,
    presets: State<'_, PresetStoreState>,
    workspace_id: String,
    preset_id: String,
    override_mode: Option<String>,
) -> Result<(), String> {
    // Look up the preset
    let store = presets.inner.lock().unwrap_or_else(|e| e.into_inner());
    let preset = store
        .presets
        .iter()
        .find(|p| p.id == preset_id)
        .ok_or_else(|| format!("Preset not found: {preset_id}"))?
        .clone();
    drop(store);

    // Check that all command binaries exist before creating any tabs/splits.
    for cmd in &preset.commands {
        if !command_binary_exists(cmd) {
            let binary = cmd.split_whitespace().next().unwrap_or(cmd);
            return Err(format!("{} is not installed", binary));
        }
    }

    // Determine effective launch mode
    let effective_mode = match override_mode.as_deref() {
        Some("new_tab") => "new_tab",
        Some("split_pane") => "split_pane",
        Some("current_terminal") => "current_terminal",
        Some("existing_panes") => "existing_panes",
        _ => match preset.launch_mode {
            LaunchMode::NewTab => "new_tab",
            LaunchMode::SplitPane => "split_pane",
        },
    };

    // If preset has no commands (e.g. Shell preset), just create tab/split with no command
    let commands = if preset.commands.is_empty() {
        vec![String::new()]
    } else {
        preset.commands.iter()
            .map(|cmd| crate::agent_context::inject_agent_context(cmd))
            .collect()
    };

    let sessions_arc = pty_state.sessions.clone();

    match effective_mode {
        "current_terminal" => {
            // Write commands to the active terminal session
            let session_id = active_session_for_workspace(&state, &workspace_id)
                .ok_or_else(|| "No active terminal session in workspace".to_string())?;

            let combined = commands
                .iter()
                .filter(|c| !c.is_empty())
                .cloned()
                .collect::<Vec<_>>()
                .join(" && ");

            if !combined.is_empty() {
                write_command_to_pty(&sessions_arc, &session_id, &combined);
            }
        }
        "split_pane" => {
            // Create one split pane per command
            let active_pane = active_pane_for_workspace(&state, &workspace_id)
                .ok_or_else(|| "No active pane in workspace".to_string())?;

            for (i, command) in commands.iter().enumerate() {
                // For the first command, split the active pane; for subsequent ones,
                // use the most recently created pane
                let target_pane = if i == 0 {
                    active_pane.clone()
                } else {
                    // Get the current active pane (which is the last split we created)
                    active_pane_for_workspace(&state, &workspace_id)
                        .unwrap_or_else(|| active_pane.clone())
                };

                let session_id = state.split_pane(
                    &target_pane,
                    crate::state::SplitDirection::Horizontal,
                )?;

                terminal::spawn_pty_for_session(app.clone(), session_id.0.clone());

                if !command.is_empty() {
                    let sessions = sessions_arc.clone();
                    let sid = session_id.0.clone();
                    let cmd = command.clone();
                    write_command_when_ready(sessions, sid, cmd);
                }
            }
        }
        "existing_panes" => {
            // Write commands to all existing terminal sessions without creating new panes
            let snapshot = state.snapshot();
            let ws = snapshot
                .workspaces
                .iter()
                .find(|w| w.workspace_id.0 == workspace_id)
                .ok_or_else(|| format!("Workspace not found: {workspace_id}"))?;
            let session_ids = crate::state::collect_terminal_sessions(&ws.surfaces);

            let combined = commands
                .iter()
                .filter(|c| !c.is_empty())
                .cloned()
                .collect::<Vec<_>>()
                .join(" && ");

            if !combined.is_empty() {
                for sid in session_ids {
                    let sessions = sessions_arc.clone();
                    let cmd = combined.clone();
                    write_command_when_ready(sessions, sid, cmd);
                }
            }
        }
        _ => {
            // "new_tab" — create one tab per command
            for command in &commands {
                let (tab_id, session_id) = state.create_tab(
                    &workspace_id,
                    crate::state::TabKind::Terminal,
                )?;

                // Name the tab after the preset
                let _ = state.rename_tab(&workspace_id, &tab_id, preset.name.clone());

                if let Some(session_id) = session_id {
                    terminal::spawn_pty_for_session(app.clone(), session_id.0.clone());

                    if !command.is_empty() {
                        let sessions = sessions_arc.clone();
                        let sid = session_id.0.clone();
                        let cmd = command.clone();
                        write_command_when_ready(sessions, sid, cmd);
                    }
                }
            }
        }
    }

    crate::state::emit_app_state(&app);
    Ok(())
}

/// Get the active terminal session ID for a specific workspace.
fn active_session_for_workspace(
    state: &AppStateStore,
    workspace_id: &str,
) -> Option<String> {
    let snapshot = state.snapshot();
    let workspace = snapshot
        .workspaces
        .iter()
        .find(|w| w.workspace_id.0 == workspace_id)?;
    let surface = workspace
        .surfaces
        .iter()
        .find(|s| s.surface_id == workspace.active_surface_id)?;
    crate::state::session_id_for_pane(&surface.root, &surface.active_pane_id)
        .map(|sid| sid.0)
}

/// Get the active pane ID for a specific workspace.
fn active_pane_for_workspace(
    state: &AppStateStore,
    workspace_id: &str,
) -> Option<String> {
    let snapshot = state.snapshot();
    let workspace = snapshot
        .workspaces
        .iter()
        .find(|w| w.workspace_id.0 == workspace_id)?;
    let surface = workspace
        .surfaces
        .iter()
        .find(|s| s.surface_id == workspace.active_surface_id)?;
    Some(surface.active_pane_id.0.clone())
}

/// Write a command string to a PTY session's stdin immediately.
/// Only the raw command text + a newline are written — no serialization.
fn write_command_to_pty(
    sessions: &Arc<std::sync::Mutex<std::collections::HashMap<String, terminal::SessionRuntime>>>,
    session_id: &str,
    command: &str,
) {
    let mut guard = sessions.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(runtime) = guard.get_mut(session_id) {
        if let Some(writer) = runtime.writer.as_mut() {
            let _ = writer.write_all(command.as_bytes());
            let _ = writer.write_all(b"\n");
            let _ = writer.flush();
        }
    }
}

/// Write a command string to a newly-spawned PTY after the shell is ready.
/// Polls for the PTY writer to become available (every 50ms, up to 5s timeout),
/// then waits an additional 150ms for the shell prompt to finish rendering
/// before writing the plain command text + newline.
fn write_command_when_ready(
    sessions: Arc<std::sync::Mutex<std::collections::HashMap<String, terminal::SessionRuntime>>>,
    session_id: String,
    command: String,
) {
    std::thread::spawn(move || {
        // Poll until the PTY writer is available (shell process spawned).
        let max_attempts = 100; // 100 × 50ms = 5s timeout
        let mut writer_found = false;
        for _ in 0..max_attempts {
            std::thread::sleep(std::time::Duration::from_millis(50));
            let ready = {
                let guard = sessions.lock().unwrap_or_else(|e| e.into_inner());
                guard
                    .get(&session_id)
                    .map(|rt| rt.writer.is_some())
                    .unwrap_or(false)
            };
            if ready {
                writer_found = true;
                break;
            }
        }

        if !writer_found {
            eprintln!(
                "[codemux::presets] Timeout waiting for PTY writer for session {session_id}"
            );
            return;
        }

        // Let the shell prompt finish rendering before sending the command.
        std::thread::sleep(std::time::Duration::from_millis(150));

        // Write only the plain command text followed by a newline.
        let mut guard = sessions.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(runtime) = guard.get_mut(&session_id) {
            if let Some(writer) = runtime.writer.as_mut() {
                let _ = writer.write_all(command.as_bytes());
                let _ = writer.write_all(b"\n");
                let _ = writer.flush();
            }
        }
    });
}

/// Check whether a command's binary exists on the system via `which`.
/// Returns true for empty commands (e.g. the Shell preset).
fn command_binary_exists(command: &str) -> bool {
    let binary = command.split_whitespace().next().unwrap_or("");
    if binary.is_empty() {
        return true;
    }
    std::process::Command::new("which")
        .arg(binary)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
