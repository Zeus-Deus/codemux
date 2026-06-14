use std::io::Write;
use std::sync::Arc;

use tauri::State;

use crate::database::DatabaseStore;
use crate::presets::{
    emit_presets_changed, save_presets, snapshot_from_store, AgentConfig, LaunchMode, PresetKind,
    PresetStoreSnapshot, PresetStoreState, TerminalPreset,
};
use crate::state::AppStateStore;
use crate::terminal;
use crate::terminal::PtyState;

#[tauri::command]
pub fn get_presets(presets: State<'_, PresetStoreState>) -> Result<PresetStoreSnapshot, String> {
    let store = presets.inner.lock().unwrap_or_else(|e| e.into_inner());
    Ok(snapshot_from_store(&store))
}

/// Return the static agent catalog that drives the structured preset
/// editor (agent picker, model suggestions, reasoning options). See
/// [`crate::agent_catalog`] for the metadata contract.
#[tauri::command]
pub fn list_agent_catalog() -> Vec<crate::agent_catalog::AgentCatalogEntry> {
    crate::agent_catalog::agent_catalog()
}

/// Listing of presets with each entry's runtime-binary availability resolved
/// for the current PATH. Used by the Phase 1.5 `get_presets` socket command
/// and the `preset_list` MCP tool so a brain can pick a `preset_id` without
/// blindly hoping the agent CLI is installed.
///
/// `commands_available` is `true` when every command in `preset.commands`
/// (the `which`-lookup target, first whitespace-delimited token) resolves
/// via `command_binary_exists` — which also checks
/// `%USERPROFILE%\.local\bin` on Windows so AUR-installed agents are found.
/// `kind` maps `PresetKind::Cli` → `"terminal"` and `PresetKind::ChatAgent`
/// → `"chat"` to match the lowercase tag the brain's MCP schema describes.
pub(crate) fn list_presets_with_availability(
    presets: &PresetStoreState,
) -> Vec<serde_json::Value> {
    let store = presets.inner.lock().unwrap_or_else(|e| e.into_inner());
    let default_id = store.default_preset_id.clone();
    store
        .presets
        .iter()
        .map(|p| {
            let commands_available = p.commands.is_empty()
                || p.commands.iter().all(|c| command_binary_exists(c));
            let kind = match p.kind {
                PresetKind::Cli => "terminal",
                PresetKind::ChatAgent => "chat",
            };
            serde_json::json!({
                "preset_id": p.id,
                "name": p.name,
                "description": p.description,
                "kind": kind,
                "is_default": default_id.as_deref() == Some(p.id.as_str()),
                "commands_available": commands_available,
            })
        })
        .collect()
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
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
    icon: Option<String>,
    agent_config: Option<AgentConfig>,
) -> Result<String, String> {
    let id = uuid::Uuid::new_v4().to_string();
    let preset = TerminalPreset {
        id: id.clone(),
        name,
        description,
        commands,
        working_directory,
        launch_mode,
        icon,
        pinned,
        is_builtin: false,
        auto_run_on_workspace: false,
        auto_run_on_new_tab: false,
        // User-created presets default to CLI. If we later add UI
        // for creating ChatAgent presets, plumb a `kind` arg through.
        kind: PresetKind::Cli,
        // For structured "agent launcher" presets, the frontend passes
        // the assembled `commands` *and* the source `agent_config` so the
        // editor can round-trip the dropdowns. Raw presets pass `None`.
        agent_config,
    };

    let mut store = presets.inner.lock().unwrap_or_else(|e| e.into_inner());
    store.presets.push(preset);
    save_presets(&db, &store)?;
    drop(store);

    emit_presets_changed(&app);
    Ok(id)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
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
    agent_config: Option<AgentConfig>,
    clear_agent_config: Option<bool>,
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
    // Structured agent config follows a set/clear/leave-unchanged
    // convention: `agent_config: Some(..)` sets it (structured save),
    // `clear_agent_config: Some(true)` removes it (switching to a raw
    // command preset), and passing neither leaves it untouched (so
    // unrelated updates like the auto-run toggles don't wipe it).
    if let Some(cfg) = agent_config {
        preset.agent_config = Some(cfg);
    } else if clear_agent_config == Some(true) {
        preset.agent_config = None;
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

/// Reorder a preset to a new position in the global preset list.
///
/// `target_index` is in the global Vec, not the pinned-only view. The
/// frontend translates pinned-bar drag indices to global indices before
/// invoking this (see `getTargetIndexForPinnedReorder` in
/// `preset-bar.tsx`); the settings list passes the global index
/// directly. Either way the server's job is the same: splice the
/// preset to its new position and persist.
#[tauri::command]
pub fn reorder_presets(
    app: tauri::AppHandle,
    db: State<'_, DatabaseStore>,
    presets: State<'_, PresetStoreState>,
    preset_id: String,
    target_index: usize,
) -> Result<(), String> {
    let mut store = presets.inner.lock().unwrap_or_else(|e| e.into_inner());

    let current_index = store
        .presets
        .iter()
        .position(|p| p.id == preset_id)
        .ok_or_else(|| format!("Preset not found: {preset_id}"))?;

    if target_index >= store.presets.len() {
        return Err(format!(
            "target_index {target_index} out of bounds (len={})",
            store.presets.len()
        ));
    }

    if current_index == target_index {
        return Ok(());
    }

    let preset = store.presets.remove(current_index);
    store.presets.insert(target_index, preset);

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
    initial_prompt: Option<String>,
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

    // ChatAgent presets are dispatched by the frontend via
    // `agentChatCreatePane` + `agentChatStartSession` + `agentChatSendTurn`
    // (see `src/lib/agent-chat/materialize.ts::materializeWithPreset`).
    // Routing one through the terminal path would fall into the empty-
    // commands Shell branch below and spawn a blank shell, so short-
    // circuit here with an explicit error.
    if matches!(preset.kind, PresetKind::ChatAgent) {
        return Err(format!(
            "apply_preset cannot launch ChatAgent preset `{}`; dispatch via the agent-chat API instead.",
            preset.id
        ));
    }

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
        preset
            .commands
            .iter()
            .map(|cmd| crate::agent_context::inject_agent_context(cmd, &workspace_id))
            .collect()
    };

    let sessions_arc = pty_state.sessions.clone();

    match effective_mode {
        "current_terminal" => {
            // Write commands to the active terminal session
            let session_id = active_session_for_workspace(&state, &workspace_id)
                .ok_or_else(|| "No active terminal session in workspace".to_string())?;

            if initial_prompt.is_some() {
                // Agent launch with prompt: rename tab, embed prompt in command
                let snap = state.snapshot();
                if let Some(ws) = snap.workspaces.iter().find(|w| w.workspace_id.0 == workspace_id) {
                    let _ = state.rename_tab(&workspace_id, &ws.active_tab_id, preset.name.clone());
                    let _ = state.set_tab_icon(&workspace_id, &ws.active_tab_id, preset.icon.clone());
                }

                for command in &commands {
                    if command.is_empty() { continue; }
                    let (cmd, needs_pty_injection) =
                        crate::branch_name::prepare_agent_command(
                            &preset_id,
                            command,
                            initial_prompt.as_deref(),
                        );
                    state.update_terminal_session_command(&session_id, command.clone());
                    write_command_when_ready(
                        sessions_arc.clone(), session_id.clone(), cmd, 120,
                    );
                    if needs_pty_injection {
                        if let Some(ref prompt) = initial_prompt {
                            write_command_when_ready(
                                sessions_arc.clone(), session_id.clone(), prompt.clone(), 1500,
                            );
                        }
                    }
                }
            } else {
                let combined = commands
                    .iter()
                    .filter(|c| !c.is_empty())
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(" && ");

                if !combined.is_empty() {
                    state.update_terminal_session_command(&session_id, combined.clone());
                    write_command_to_pty(&sessions_arc, &session_id, &combined);
                }
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

                let session_id =
                    state.split_pane(&target_pane, crate::state::SplitDirection::Horizontal)?;

                terminal::spawn_pty_for_session(app.clone(), session_id.0.clone());

                if !command.is_empty() {
                    state.update_terminal_session_command(&session_id.0, command.clone());
                    let sessions = sessions_arc.clone();
                    let sid = session_id.0.clone();
                    let cmd = command.clone();
                    write_command_when_ready(sessions, sid, cmd, 120);
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
                    state.update_terminal_session_command(&sid, combined.clone());
                    let sessions = sessions_arc.clone();
                    let cmd = combined.clone();
                    write_command_when_ready(sessions, sid, cmd, 120);
                }
            }
        }
        _ => {
            // "new_tab" — create one tab per command
            for command in &commands {
                let (tab_id, session_id) =
                    state.create_tab(&workspace_id, crate::state::TabKind::Terminal)?;

                // Name the tab after the preset and set its icon
                let _ = state.rename_tab(&workspace_id, &tab_id, preset.name.clone());
                let _ = state.set_tab_icon(&workspace_id, &tab_id, preset.icon.clone());

                if let Some(session_id) = session_id {
                    terminal::spawn_pty_for_session(app.clone(), session_id.0.clone());

                    if !command.is_empty() {
                        state.update_terminal_session_command(&session_id.0, command.clone());
                        let sessions = sessions_arc.clone();
                        let sid = session_id.0.clone();
                        let cmd = command.clone();
                        write_command_when_ready(sessions, sid, cmd, 120);
                    }
                }
            }
        }
    }

    crate::state::emit_app_state(&app);
    Ok(())
}

/// Get the active terminal session ID for a specific workspace.
fn active_session_for_workspace(state: &AppStateStore, workspace_id: &str) -> Option<String> {
    let snapshot = state.snapshot();
    let workspace = snapshot
        .workspaces
        .iter()
        .find(|w| w.workspace_id.0 == workspace_id)?;
    let surface = workspace
        .surfaces
        .iter()
        .find(|s| s.surface_id == workspace.active_surface_id)?;
    crate::state::session_id_for_pane(&surface.root, &surface.active_pane_id).map(|sid| sid.0)
}

/// Get the active pane ID for a specific workspace.
fn active_pane_for_workspace(state: &AppStateStore, workspace_id: &str) -> Option<String> {
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

/// Line terminator written to a PTY after a programmatically-injected
/// command.
///
/// **Use CR (0x0D), not LF (0x0A).** This matches the byte that xterm.js
/// emits when the user physically presses Enter in the terminal — the
/// Codemux frontend's `term.onData` → `writeToPty` path already sends
/// `\r` on both Linux and Windows, so writing `\r` here means the
/// programmatic-injection path and the keystroke path are byte-for-byte
/// identical.
///
/// Why this must be `\r` on Windows:
///
/// - On Linux (bash/zsh/fish with readline) the default bindings map
///   BOTH `C-m` (`\r`) and `C-j` (`\n`) to `accept-line`, so either
///   byte submits a command. Codemux historically sent `\n` and it
///   happened to work.
///
/// - On Windows (PowerShell 5.1 / PowerShell 7 with PSReadLine), only
///   `\r` is recognized as the Enter keystroke — ConPTY dispatches
///   incoming `\r` bytes as `VK_RETURN` key-down events, which
///   PSReadLine's input handler binds to "submit current input". A
///   `\n` byte does NOT produce `VK_RETURN`; PSReadLine interprets it
///   as a soft line-break (equivalent to pressing Shift+Enter in many
///   shells) and inserts a literal newline into the pending input
///   buffer. The result is PowerShell displaying the preset command
///   text followed by a `>>` continuation prompt, waiting for the
///   user to hit Enter on their keyboard — the preset is typed into
///   the prompt but never executed. This was the reported Windows
///   bug that motivated switching from `\n` to `\r`.
///
/// Sending `\r` is correct on both platforms: readline's `C-m` binding
/// handles Linux, and ConPTY's `VK_RETURN` dispatch handles Windows.
const PTY_COMMAND_TERMINATOR: &[u8] = b"\r";

/// Write a command string to a PTY session's stdin immediately.
/// Only the raw command text + a carriage return are written — no
/// serialization. See `PTY_COMMAND_TERMINATOR` for the rationale on
/// why CR is used rather than LF.
///
/// IMPORTANT: command bytes and the terminator are combined into a
/// SINGLE `write_all` call. See `build_pty_command_payload` for the
/// "Linux phantom-prompt" regression this prevents.
fn write_command_to_pty(
    sessions: &Arc<std::sync::Mutex<std::collections::HashMap<String, terminal::SessionRuntime>>>,
    session_id: &str,
    command: &str,
) {
    let payload = build_pty_command_payload(command);
    let mut guard = sessions.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(runtime) = guard.get_mut(session_id) {
        if let Some(writer) = runtime.writer.as_mut() {
            let _ = writer.write_all(&payload);
            let _ = writer.flush();
        }
    }
}

/// Concatenate `command` + the PTY command terminator into a single
/// byte buffer suitable for one `write_all` call.
///
/// Why this exists as a discrete helper: the daemon-backed `Writer`
/// implementation (`DaemonWriter` in `terminal/daemon_backed.rs`) is
/// fire-and-forget. Each call to `Write::write` spawns its own Tokio
/// task that round-trips a separate `Write` RPC to the PTY daemon —
/// there is no FIFO between successive writes on the same handle.
///
/// The previous code wrote command bytes and the terminator in two
/// separate `write_all` calls. On the daemon path, those became two
/// independent async tasks that could land at the daemon's master fd
/// in either order:
///
///   1. Command first, CR second → shell echoes `claude ...`, then
///      receives CR → readline submits → preset launches. Correct.
///   2. CR first, command second → shell receives CR on an empty
///      line → submits nothing, redraws prompt → THEN the command
///      bytes arrive and sit at the new prompt with no terminator.
///      User sees the command text typed in the input but never
///      executed; pressing Enter manually launches the preset. This
///      was the "sometimes does nothing" bug after the writer-poll
///      fix (the writer-poll fix uncovered it by removing the
///      synthesis path's duplicate write that happened to fire later
///      in lock-step order).
///
/// Combining into one buffer means a single `write_all` → single
/// `Write::write` call → single spawn → single daemon RPC. The
/// command and its terminator are atomic from the daemon's
/// perspective; nothing can race them apart.
fn build_pty_command_payload(command: &str) -> Vec<u8> {
    let mut payload = Vec::with_capacity(command.len() + PTY_COMMAND_TERMINATOR.len());
    payload.extend_from_slice(command.as_bytes());
    payload.extend_from_slice(PTY_COMMAND_TERMINATOR);
    payload
}

/// Write a command to a newly-spawned PTY after the shell is ready.
///
/// Phase 1: Polls for `writer.is_some()` (every 50ms, up to 5s).
/// Phase 2: Detects shell readiness via quiet-after-output heuristic — waits
///          until PTY output has arrived and then gone quiet for `settle_ms`.
///
/// Callers are low-frequency (preset application, workspace creation),
/// so thread-per-command is acceptable.
pub(crate) fn write_command_when_ready(
    sessions: Arc<std::sync::Mutex<std::collections::HashMap<String, terminal::SessionRuntime>>>,
    session_id: String,
    command: String,
    settle_ms: u64,
) {
    std::thread::spawn(move || {
        wait_and_write_command(&sessions, &session_id, &command, settle_ms);
    });
}

/// Synchronous core of `write_command_when_ready`. Blocks the calling thread.
fn wait_and_write_command(
    sessions: &Arc<std::sync::Mutex<std::collections::HashMap<String, terminal::SessionRuntime>>>,
    session_id: &str,
    command: &str,
    settle_ms: u64,
) {
    // Phase 1: poll until the PTY writer is available (shell process spawned).
    //
    // This used to share a single 5-second budget with Phase 2's quiet-
    // detection. That was fine for the in-process Windows spawn path
    // (writer is set within milliseconds), but the Linux daemon-backed
    // spawn can take longer: cold pty_daemon start, the SSH-tunneled
    // round-trip for remote workspaces, the daemon's own list/spawn/
    // attach handshake. When apply_preset called us right after
    // `spawn_pty_for_session` returned (which only kicks off the
    // async spawn — the writer isn't set until the spawn task lands),
    // a slow spawn would starve Phase 1, the poll would hit the 5s
    // wall before the writer appeared, and we'd return without
    // writing anything. The user-visible symptom was "clicking a
    // preset on Linux sometimes does nothing." Pre-fix, the bug was
    // masked because the daemon-backed spawn ALSO synthesized a
    // duplicate write that fired after the writer was set — every
    // launch had a backup. With the Linux preset-leak fix gating
    // that synthesis off for fresh preset launches (see
    // daemon_backed.rs::should_synthesize_agent_relaunch), the
    // apply_preset write is the only one, so its budget has to
    // actually cover daemon spawn time.
    //
    // 15s comfortably covers a cold local daemon start (~1-3s
    // observed) and a first-time SSH-tunneled remote spawn (~5-10s
    // observed). Phase 2 keeps its own independent 5s budget below.
    let writer_ready_timeout = std::time::Duration::from_secs(15);
    let writer_poll_start = std::time::Instant::now();
    let mut writer_found = false;
    loop {
        std::thread::sleep(std::time::Duration::from_millis(50));
        if writer_poll_start.elapsed() >= writer_ready_timeout {
            break;
        }
        let ready = {
            let guard = sessions.lock().unwrap_or_else(|e| e.into_inner());
            guard
                .get(session_id)
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
            "[codemux::presets] Timeout waiting for PTY writer for session {session_id} \
             after {:?}",
            writer_poll_start.elapsed()
        );
        return;
    }

    let command_to_write = {
        let mut guard = sessions.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .get_mut(session_id)
            .map(|rt| {
                if let Some(resume_command) = rt.resume_command.take() {
                    if rt.skip_preset_launch {
                        eprintln!(
                            "[codemux::presets] Skipping auto-launch for {session_id}: resumable session"
                        );
                    }
                    return resume_command;
                }
                if rt.skip_preset_launch {
                    eprintln!(
                        "[codemux::presets] Resumable session for {session_id} had no resume command; falling back to normal launch"
                    );
                }
                command.to_string()
            })
            .unwrap_or_else(|| command.to_string())
    };

    if command_to_write.is_empty() {
        return;
    }

    // Phase 2: wait for shell output to arrive and then go quiet.
    //
    // Independent 5s budget — does NOT share with Phase 1's writer
    // timeout. A slow daemon spawn that ate most of a shared budget
    // used to leave this phase with no time to detect quiet,
    // forcing the writer to fire while the shell was still painting
    // its prompt. Splitting the budgets means each phase gets its
    // documented headroom regardless of the other's wall-clock.
    let quiet_phase_timeout = std::time::Duration::from_secs(5);
    let quiet_phase_start = std::time::Instant::now();
    let quiet_threshold = std::time::Duration::from_millis(settle_ms);
    let poll_interval = std::time::Duration::from_millis(30);

    let initial_len = {
        let guard = sessions.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .get(session_id)
            .map(|rt| rt.pending_output.len())
            .unwrap_or(0)
    };

    let mut snapshot_len = initial_len;
    let mut last_growth = std::time::Instant::now();
    // If output already arrived during Phase 1, start quiet timer immediately.
    let mut saw_output = initial_len > 0;
    let mut detection_method = "timeout_fallback";

    loop {
        std::thread::sleep(poll_interval);

        if quiet_phase_start.elapsed() >= quiet_phase_timeout {
            break;
        }

        let current_len = {
            let guard = sessions.lock().unwrap_or_else(|e| e.into_inner());
            guard
                .get(session_id)
                .map(|rt| rt.pending_output.len())
                .unwrap_or(0)
        };

        if current_len > snapshot_len {
            snapshot_len = current_len;
            last_growth = std::time::Instant::now();
            saw_output = true;
        }

        if saw_output && last_growth.elapsed() >= quiet_threshold {
            detection_method = "quiet_detected";
            break;
        }
    }

    eprintln!(
        "[codemux::presets] Shell readiness for {session_id}: {detection_method} \
         (output_chunks={snapshot_len}, quiet_phase_elapsed={:?}, \
          total_elapsed={:?})",
        quiet_phase_start.elapsed(),
        writer_poll_start.elapsed()
    );

    // Write the plain command text followed by the PTY command
    // terminator in a SINGLE `write_all` call. See
    // `build_pty_command_payload` for why this MUST be one call (and
    // not two): on the daemon-backed Linux path, separate `write_all`s
    // race each other and can land the terminator before the command
    // bytes, leaving the shell with the command typed at a fresh
    // prompt waiting for a manual Enter. See `PTY_COMMAND_TERMINATOR`
    // for why CR is the right terminator byte on both Linux and
    // Windows.
    let payload = build_pty_command_payload(&command_to_write);
    let mut guard = sessions.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(runtime) = guard.get_mut(session_id) {
        if let Some(writer) = runtime.writer.as_mut() {
            let write_result = writer.write_all(&payload);
            let flush_result = writer.flush();
            eprintln!(
                "[codemux::presets] wrote preset/resume command to {session_id} \
                 (write_ok={}, flush_ok={}, payload_len={}, cmd={command_to_write:?})",
                write_result.is_ok(),
                flush_result.is_ok(),
                payload.len(),
            );
        } else {
            eprintln!(
                "[codemux::presets] cannot write command to {session_id}: \
                 runtime.writer is None"
            );
        }
    } else {
        eprintln!(
            "[codemux::presets] cannot write command to {session_id}: \
             runtime missing from sessions map"
        );
    }
}

/// Check whether a command's binary exists on `PATH` for the current user.
/// Returns true for empty commands (e.g. the Shell preset).
///
/// Uses the `which` crate (cross-platform PATH walk) instead of shelling out
/// to the `which` Unix binary. The shellout version returned `false` for every
/// command on Windows because `which.exe` is not part of cmd.exe — Windows
/// users hitting "Apply preset" got an unhelpful "<binary> is not installed"
/// error for binaries that were actually installed and on PATH. The crate-based
/// implementation walks `PATH` directly with the right separator and executable
/// extension semantics on each platform.
///
/// **Windows fallback**: if `which::which` fails, we also check the known
/// per-user install directory `%USERPROFILE%\.local\bin`. This is the path
/// where the Claude Code native installer (`irm https://claude.ai/install.ps1 |
/// iex`) drops `claude.exe` and where other CLI tools that target Windows
/// with a POSIX-style install layout tend to land. The fallback is necessary
/// because PATH changes made by installers propagate to running processes
/// via `WM_SETTINGCHANGE`, which Codemux may miss if it was launched from a
/// parent process with a stale environment block. A fresh terminal sees the
/// updated PATH; Codemux's IPC process may not. Without this fallback, users
/// who installed Claude Code after launching Codemux (a very common timeline)
/// see an unhelpful "claude is not installed" error for a binary they just
/// ran successfully in a normal terminal.
fn command_binary_exists(command: &str) -> bool {
    let binary = command.split_whitespace().next().unwrap_or("");
    if binary.is_empty() {
        return true;
    }
    if which::which(binary).is_ok() {
        return true;
    }
    #[cfg(windows)]
    {
        if find_in_windows_user_local_bin(binary).is_some() {
            return true;
        }
    }
    false
}

/// Returns the absolute path to `binary` inside `%USERPROFILE%\.local\bin`
/// if any of the standard Windows executable extensions match an existing
/// file there. Returns `None` if `USERPROFILE` is unset, the directory
/// doesn't exist, or no matching file is found.
///
/// Tries `binary.exe`, `binary.cmd`, `binary.bat`, `binary.ps1`, and the
/// extensionless `binary` name (in that order). This list covers every
/// install shape we've seen for CLI agents on Windows: compiled Rust/Go
/// binaries (`.exe`), npm wrapper scripts (`.cmd`), legacy DOS batch
/// files (`.bat`), PowerShell shims (`.ps1`), and Git Bash-style
/// shebang scripts that Windows can't directly execute but which a
/// user-configured shell might resolve.
///
/// If the caller already supplied an extension (e.g. `"claude.exe"`),
/// we try the exact name first and skip the extension permutation.
#[cfg(windows)]
fn find_in_windows_user_local_bin(binary: &str) -> Option<std::path::PathBuf> {
    let user_profile = std::env::var_os("USERPROFILE")?;
    let local_bin = std::path::Path::new(&user_profile)
        .join(".local")
        .join("bin");
    if !local_bin.is_dir() {
        return None;
    }

    // If the binary already has an extension, honor it and bail on mismatch —
    // don't try to "improve" the user's explicit request.
    if std::path::Path::new(binary).extension().is_some() {
        let candidate = local_bin.join(binary);
        if candidate.is_file() {
            return Some(candidate);
        }
        return None;
    }

    // Extensionless name — try each known executable extension.
    for ext in &["exe", "cmd", "bat", "ps1", ""] {
        let candidate = if ext.is_empty() {
            local_bin.join(binary)
        } else {
            local_bin.join(format!("{binary}.{ext}"))
        };
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::SessionRuntime;
    use std::collections::HashMap;

    fn make_sessions() -> Arc<std::sync::Mutex<HashMap<String, SessionRuntime>>> {
        Arc::new(std::sync::Mutex::new(HashMap::new()))
    }

    /// Helper: create a session with a mock writer (a Vec<u8> sink).
    fn insert_session_with_writer(
        sessions: &Arc<std::sync::Mutex<HashMap<String, SessionRuntime>>>,
        session_id: &str,
    ) {
        let mut guard = sessions.lock().unwrap();
        let mut runtime = SessionRuntime::new(session_id);
        // Use a Vec<u8> as a mock writer (implements Write)
        let mock_writer: Box<dyn Write + Send> = Box::new(Vec::<u8>::new());
        runtime.writer = Some(mock_writer);
        guard.insert(session_id.to_string(), runtime);
    }

    #[test]
    fn test_write_command_to_pty_immediate() {
        let sessions = make_sessions();
        insert_session_with_writer(&sessions, "sess");

        write_command_to_pty(&sessions, "sess", "echo hello");

        // Verify command was written — text followed by CR (0x0D),
        // not LF (0x0A). See `PTY_COMMAND_TERMINATOR` for why CR is
        // used. Asserting the exact byte catches any regression that
        // accidentally switches the terminator back to `\n`, which
        // would re-break the Windows PowerShell preset-launch path.
        let guard = sessions.lock().unwrap();
        let runtime = guard.get("sess").unwrap();
        let writer = runtime.writer.as_ref().unwrap();
        // Downcast to check contents — the writer is a Vec<u8>
        let writer_ptr = writer.as_ref() as *const dyn Write as *const Vec<u8>;
        let written = unsafe { &*writer_ptr };
        assert_eq!(written, b"echo hello\r");
    }

    /// `Write` impl that records every individual `write` call as a
    /// separate byte vec. Lets tests assert how many syscalls happened
    /// and what bytes each one carried — important for the daemon-
    /// backed path where each `write` becomes its own async task and
    /// the payload from a single `write` is what travels as one
    /// network RPC.
    struct RecordingWriter {
        calls: std::sync::Arc<std::sync::Mutex<Vec<Vec<u8>>>>,
    }

    impl Write for RecordingWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.calls.lock().unwrap().push(buf.to_vec());
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn build_payload_concatenates_command_and_cr() {
        let payload = build_pty_command_payload("claude --dangerously-skip-permissions");
        assert_eq!(
            payload,
            b"claude --dangerously-skip-permissions\r",
            "payload must be `<command>\\r` so a single write_all \
             call carries both atomically — see helper doc comment \
             for why this prevents the Linux phantom-prompt race"
        );
    }

    #[test]
    fn write_command_to_pty_uses_single_write_call() {
        // Regression test for the "preset launches sometimes, sometimes
        // shows command typed but not executed" Linux bug. The fix
        // routes the command + terminator through one buffer so the
        // daemon-backed writer's per-call spawn dispatches a single
        // RPC, preserving byte order at the daemon's master fd. If
        // someone later "refactors" this back to two write_all calls,
        // this test breaks loudly.
        let sessions = make_sessions();
        let calls: std::sync::Arc<std::sync::Mutex<Vec<Vec<u8>>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        {
            let mut guard = sessions.lock().unwrap();
            let mut runtime = crate::terminal::SessionRuntime::new("sess");
            runtime.writer = Some(Box::new(RecordingWriter {
                calls: calls.clone(),
            }));
            guard.insert("sess".into(), runtime);
        }

        write_command_to_pty(&sessions, "sess", "claude --dangerously-skip-permissions");

        let recorded = calls.lock().unwrap();
        assert_eq!(
            recorded.len(),
            1,
            "command + terminator must reach the writer in exactly ONE \
             write() call. Two calls would let the daemon-backed path's \
             fire-and-forget Tokio spawn race them apart, producing the \
             'command typed but not submitted' Linux bug. Got {} calls: {:?}",
            recorded.len(),
            recorded
        );
        assert_eq!(
            &recorded[0][..],
            b"claude --dangerously-skip-permissions\r"
        );
    }

    #[test]
    fn test_pty_command_terminator_is_carriage_return() {
        // Regression guard: the programmatic preset-injection path
        // MUST use CR (0x0D) as the Enter keystroke, not LF (0x0A).
        // See the doc comment on `PTY_COMMAND_TERMINATOR` for the full
        // rationale. Short version: PSReadLine on Windows only
        // dispatches `VK_RETURN` for CR; LF is treated as a literal
        // soft-newline that leaves PowerShell at the `>>` continuation
        // prompt waiting for the user to hit Enter physically.
        //
        // This test exists specifically to break loudly in CI if
        // someone "fixes" the terminator back to `\n` based on the
        // intuition that `\n` is "the POSIX newline". Linux accepts
        // both via readline's C-m / C-j bindings; Windows only accepts
        // CR via ConPTY's VK_RETURN dispatch.
        assert_eq!(
            PTY_COMMAND_TERMINATOR, b"\r",
            "PTY command terminator must be CR (0x0D) — see doc comment for why"
        );
    }

    #[test]
    fn test_quiet_detection() {
        let sessions = make_sessions();
        insert_session_with_writer(&sessions, "sess");

        let sessions_clone = sessions.clone();
        // Simulate shell startup output from a background thread
        let producer = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            let mut guard = sessions_clone.lock().unwrap();
            let runtime = guard.get_mut("sess").unwrap();
            runtime.pending_output.push_back(vec![b'$', b' ']);
        });

        let start = std::time::Instant::now();
        // Call synchronous core directly with short settle time
        wait_and_write_command(&sessions, "sess", "test-cmd", 80);
        let elapsed = start.elapsed();

        producer.join().unwrap();

        // Should have detected quiet and fired, not hit the 5s timeout
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "should detect quiet quickly, took {elapsed:?}"
        );
    }

    #[test]
    fn test_hard_timeout() {
        // Writer is present, so Phase 1 passes immediately. No output
        // ever arrives, so Phase 2 must run its quiet-detection budget
        // to completion. That budget is independent of Phase 1's
        // writer-poll budget — Phase 2 alone should take ~5s here.
        let sessions = make_sessions();
        insert_session_with_writer(&sessions, "sess");

        let start = std::time::Instant::now();
        wait_and_write_command(&sessions, "sess", "timeout-cmd", 120);
        let elapsed = start.elapsed();

        assert!(
            elapsed >= std::time::Duration::from_secs(4),
            "Phase 2 quiet-detection should run its full 5s budget, took {elapsed:?}"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(7),
            "Phase 2 budget is 5s independent of Phase 1; should NOT \
             leak Phase 1's 15s window into total wall-clock here. \
             Took {elapsed:?}"
        );
    }

    #[test]
    fn test_writer_not_found_timeout() {
        // No writer ever appears — Phase 1 must wait its full
        // writer-ready budget (15s) before bailing. Phase 2 is never
        // reached. This test takes ~15s; the long wait is what gives
        // a real daemon-backed Linux spawn (cold pty_daemon, remote
        // SSH tunnel) enough room to land the writer before
        // apply_preset gives up. See `wait_and_write_command` Phase 1
        // comment for the regression history.
        let sessions = make_sessions();
        {
            let mut guard = sessions.lock().unwrap();
            guard.insert("no-writer".to_string(), SessionRuntime::new("no-writer"));
        }

        let start = std::time::Instant::now();
        wait_and_write_command(&sessions, "no-writer", "cmd", 120);
        let elapsed = start.elapsed();

        assert!(
            elapsed >= std::time::Duration::from_secs(14),
            "Phase 1 should wait its full 15s writer-ready budget so \
             slow daemon spawns aren't starved; took {elapsed:?}"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(20),
            "Phase 1 budget is 15s; should NOT exceed significantly. \
             Took {elapsed:?}"
        );
    }

    #[test]
    fn test_slow_writer_then_quiet_detected() {
        // Regression test for the Linux preset-launch intermittence
        // ("sometimes the preset launches, sometimes it doesn't").
        // Simulates the daemon-backed spawn race: apply_preset spawns
        // this poll thread immediately, but the writer doesn't land
        // until well after the OLD 5s shared budget would have
        // expired. With split per-phase budgets, Phase 1's 15s window
        // covers the slow spawn, then Phase 2 detects shell quiet and
        // the command actually gets written. Pre-fix this would hit
        // the shared 5s wall, return without writing, and the preset
        // would silently fail to launch.
        let sessions = make_sessions();
        // Insert the runtime stub the apply_preset path would see
        // *before* the daemon spawn lands the writer.
        {
            let mut guard = sessions.lock().unwrap();
            guard.insert(
                "slow-spawn".to_string(),
                SessionRuntime::new("slow-spawn"),
            );
        }

        let sessions_for_producer = sessions.clone();
        // Producer thread: after 7 seconds (well past the old 5s
        // shared budget) install the writer + a bit of pending output
        // (shell prompt) so Phase 2 can detect quiet and complete the
        // write.
        let producer = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(7));
            let mut guard = sessions_for_producer.lock().unwrap();
            let runtime = guard.get_mut("slow-spawn").unwrap();
            runtime.writer =
                Some(Box::new(Vec::<u8>::new()) as Box<dyn Write + Send>);
            runtime.pending_output.push_back(vec![b'$', b' ']);
        });

        let start = std::time::Instant::now();
        // settle_ms = 80 so Phase 2's quiet detection fires quickly.
        wait_and_write_command(&sessions, "slow-spawn", "echo hi", 80);
        let elapsed = start.elapsed();
        producer.join().unwrap();

        // Must have waited for the writer (≥7s) but not the full
        // 20s upper bound (Phase 1 15s + Phase 2 5s).
        assert!(
            elapsed >= std::time::Duration::from_secs(7),
            "should have waited for the slow writer to land, took {elapsed:?}"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(12),
            "Phase 2 should fire shortly after writer lands; total \
             elapsed should be writer-land-time + a brief quiet \
             window. Took {elapsed:?}"
        );

        // The write must have actually happened — that's the user-
        // visible thing the regression broke.
        let guard = sessions.lock().unwrap();
        let runtime = guard.get("slow-spawn").unwrap();
        let writer = runtime.writer.as_ref().unwrap();
        let writer_ptr = writer.as_ref() as *const dyn Write as *const Vec<u8>;
        let written = unsafe { &*writer_ptr };
        assert_eq!(
            written, b"echo hi\r",
            "command must have been written after the writer became \
             available — pre-fix the shared 5s budget timed out first \
             and nothing was written, manifesting as 'preset click \
             did nothing' on Linux"
        );
    }

    #[test]
    fn test_resume_command_overrides_original_command() {
        let sessions = make_sessions();
        insert_session_with_writer(&sessions, "sess");

        {
            let mut guard = sessions.lock().unwrap();
            let runtime = guard.get_mut("sess").unwrap();
            runtime.skip_preset_launch = true;
            runtime.resume_command = Some("claude --dangerously-skip-permissions --system-prompt \"$CODEMUX_AGENT_CONTEXT\" --resume abc-123".into());
        }

        let sessions_clone = sessions.clone();
        let producer = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            let mut guard = sessions_clone.lock().unwrap();
            let runtime = guard.get_mut("sess").unwrap();
            runtime.pending_output.push_back(vec![b'$', b' ']);
        });

        wait_and_write_command(
            &sessions,
            "sess",
            "claude --dangerously-skip-permissions --system-prompt \"$CODEMUX_AGENT_CONTEXT\"",
            80,
        );

        producer.join().unwrap();

        let guard = sessions.lock().unwrap();
        let runtime = guard.get("sess").unwrap();
        let writer = runtime.writer.as_ref().unwrap();
        let writer_ptr = writer.as_ref() as *const dyn Write as *const Vec<u8>;
        let written = unsafe { &*writer_ptr };
        // Trailing byte must be CR (0x0D), not LF (0x0A) — see
        // `PTY_COMMAND_TERMINATOR`. Asserting the exact byte catches
        // accidental regressions that would re-break the Windows
        // preset-launch path.
        assert_eq!(
            written,
            b"claude --dangerously-skip-permissions --system-prompt \"$CODEMUX_AGENT_CONTEXT\" --resume abc-123\r"
        );
    }

    // ── command_binary_exists Windows fallback tests ────────────────
    //
    // These tests drive the `find_in_windows_user_local_bin` helper
    // directly by temporarily pointing `USERPROFILE` at a tempdir we
    // control, then populating `{tempdir}\.local\bin` with fake
    // binaries. Setting an env var inside a test is normally a parallel-
    // execution hazard, but since these tests use `#[serial_test::serial]`-
    // style serialization via a shared `Mutex` we construct below,
    // they're safe to run alongside the rest of the suite.
    //
    // On non-Windows the fallback is cfg-gated out, so the tests live
    // inside `#[cfg(windows)]` blocks — they compile only on Windows.

    #[cfg(windows)]
    mod windows_fallback {
        use super::super::*;
        use std::sync::Mutex;

        /// Serialize env-var mutations across tests in this module.
        /// We're changing USERPROFILE, which is a global env var, so
        /// concurrent tests could clobber each other without this lock.
        static USERPROFILE_LOCK: Mutex<()> = Mutex::new(());

        /// RAII guard: sets USERPROFILE to `value` and restores the
        /// previous value on drop. Panics are caught by the Mutex
        /// poison handling in the next test — the drop still runs.
        struct UserProfileGuard {
            previous: Option<std::ffi::OsString>,
        }

        impl UserProfileGuard {
            fn set(value: &std::path::Path) -> Self {
                let previous = std::env::var_os("USERPROFILE");
                std::env::set_var("USERPROFILE", value);
                Self { previous }
            }
        }

        impl Drop for UserProfileGuard {
            fn drop(&mut self) {
                match self.previous.take() {
                    Some(prev) => std::env::set_var("USERPROFILE", prev),
                    None => std::env::remove_var("USERPROFILE"),
                }
            }
        }

        /// Build a tempdir that looks like `{tempdir}\.local\bin` with
        /// the given binary filenames created as empty files. Returns
        /// the tempdir path so the caller can pass it to
        /// `UserProfileGuard::set`.
        fn stage_local_bin(binaries: &[&str]) -> tempfile::TempDir {
            let tempdir = tempfile::tempdir().expect("failed to create tempdir");
            let local_bin = tempdir.path().join(".local").join("bin");
            std::fs::create_dir_all(&local_bin).expect("failed to create .local\\bin");
            for name in binaries {
                std::fs::write(local_bin.join(name), b"").expect("failed to write stub binary");
            }
            tempdir
        }

        #[test]
        fn finds_claude_exe_in_user_local_bin() {
            let _lock = USERPROFILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let tempdir = stage_local_bin(&["claude.exe"]);
            let _guard = UserProfileGuard::set(tempdir.path());

            let result = find_in_windows_user_local_bin("claude");
            assert!(
                result.is_some(),
                "expected claude.exe in staged .local\\bin to be found",
            );
            assert!(result.unwrap().to_string_lossy().ends_with("claude.exe"));
        }

        #[test]
        fn finds_claude_cmd_in_user_local_bin() {
            let _lock = USERPROFILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let tempdir = stage_local_bin(&["claude.cmd"]);
            let _guard = UserProfileGuard::set(tempdir.path());

            let result = find_in_windows_user_local_bin("claude");
            assert!(result.is_some());
            assert!(result.unwrap().to_string_lossy().ends_with("claude.cmd"));
        }

        #[test]
        fn prefers_exe_over_cmd_when_both_present() {
            let _lock = USERPROFILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let tempdir = stage_local_bin(&["claude.exe", "claude.cmd", "claude.bat"]);
            let _guard = UserProfileGuard::set(tempdir.path());

            let result = find_in_windows_user_local_bin("claude");
            assert!(result.is_some());
            // Extension priority: exe > cmd > bat > ps1 > (extensionless)
            assert!(
                result.unwrap().to_string_lossy().ends_with("claude.exe"),
                "exe should win when both exe and cmd are present",
            );
        }

        #[test]
        fn returns_none_when_binary_missing() {
            let _lock = USERPROFILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let tempdir = stage_local_bin(&["codex.exe"]);
            let _guard = UserProfileGuard::set(tempdir.path());

            let result = find_in_windows_user_local_bin("claude");
            assert!(
                result.is_none(),
                "looking up `claude` must not match `codex.exe`",
            );
        }

        #[test]
        fn returns_none_when_local_bin_missing() {
            let _lock = USERPROFILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let tempdir = tempfile::tempdir().expect("tempdir");
            // NO .local\bin directory created.
            let _guard = UserProfileGuard::set(tempdir.path());

            let result = find_in_windows_user_local_bin("claude");
            assert!(result.is_none());
        }

        #[test]
        fn honors_explicit_extension() {
            // If the caller asks for `claude.exe`, we try exactly that
            // and do NOT permute extensions — respecting the explicit
            // request.
            let _lock = USERPROFILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let tempdir = stage_local_bin(&["claude.cmd"]);
            let _guard = UserProfileGuard::set(tempdir.path());

            let result = find_in_windows_user_local_bin("claude.exe");
            assert!(
                result.is_none(),
                "explicit claude.exe must not fall through to claude.cmd",
            );
        }

        #[test]
        fn command_binary_exists_integrates_with_fallback() {
            // End-to-end: command_binary_exists must return true when
            // the binary is only reachable via the %USERPROFILE%\.local\bin
            // fallback (not on the process PATH).
            let _lock = USERPROFILE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let tempdir = stage_local_bin(&["codemux-test-stub.exe"]);
            let _guard = UserProfileGuard::set(tempdir.path());

            // The stub name is unlikely to exist on any real PATH, so
            // the only way command_binary_exists returns true is via
            // the fallback.
            assert!(
                command_binary_exists("codemux-test-stub --flag"),
                "command_binary_exists should find the stub via the .local\\bin fallback",
            );
        }
    }

    // ──────────────────────────────────────────────────────────────────
    // Phase 1.5: list_presets_with_availability
    //
    // Backs the `get_presets` socket command (and the `preset_list` MCP
    // tool). The brain relies on `commands_available` to filter out
    // presets whose CLI isn't installed on the host before calling
    // `preset_apply` or `worktree_create`. These tests pin the shape
    // and the PATH-resolution semantics.
    // ──────────────────────────────────────────────────────────────────

    fn make_preset(id: &str, command: &str, kind: PresetKind) -> TerminalPreset {
        TerminalPreset {
            id: id.into(),
            name: id.into(),
            description: Some(format!("test preset {id}")),
            commands: if command.is_empty() {
                vec![]
            } else {
                vec![command.into()]
            },
            working_directory: None,
            launch_mode: LaunchMode::NewTab,
            icon: None,
            pinned: false,
            is_builtin: false,
            auto_run_on_workspace: false,
            auto_run_on_new_tab: false,
            kind,
            agent_config: None,
        }
    }

    fn store_with(presets: Vec<TerminalPreset>, default_id: Option<&str>) -> PresetStoreState {
        PresetStoreState {
            inner: std::sync::Mutex::new(crate::presets::PresetStore {
                schema_version: 1,
                presets,
                default_preset_id: default_id.map(str::to_string),
                bar_visible: true,
            }),
        }
    }

    #[test]
    fn list_presets_returns_required_fields() {
        let store = store_with(
            vec![make_preset("p-shell", "sh -c 'echo hi'", PresetKind::Cli)],
            Some("p-shell"),
        );
        let entries = list_presets_with_availability(&store);
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        // The MCP schema for `preset_list` advertises these exact keys.
        // If any are renamed, the brain's filter logic breaks silently.
        for key in &[
            "preset_id",
            "name",
            "description",
            "kind",
            "is_default",
            "commands_available",
        ] {
            assert!(e.get(*key).is_some(), "missing key: {key} in {e:?}");
        }
    }

    #[test]
    fn list_presets_kind_maps_cli_to_terminal_and_chat_agent_to_chat() {
        // PresetKind::Cli must surface as "terminal" so the brain's
        // schema-described enum matches reality. PresetKind::ChatAgent
        // must surface as "chat" so the brain knows it routes through
        // the agent-chat pane (Phase 2 territory, but the tag must
        // still be honest today).
        let store = store_with(
            vec![
                make_preset("p-cli", "true", PresetKind::Cli),
                make_preset("p-chat", "true", PresetKind::ChatAgent),
            ],
            None,
        );
        let entries = list_presets_with_availability(&store);
        let cli = entries.iter().find(|e| e["preset_id"] == "p-cli").unwrap();
        let chat = entries.iter().find(|e| e["preset_id"] == "p-chat").unwrap();
        assert_eq!(cli["kind"], "terminal");
        assert_eq!(chat["kind"], "chat");
    }

    #[test]
    fn list_presets_is_default_reflects_default_preset_id() {
        let store = store_with(
            vec![
                make_preset("p-a", "true", PresetKind::Cli),
                make_preset("p-b", "true", PresetKind::Cli),
            ],
            Some("p-b"),
        );
        let entries = list_presets_with_availability(&store);
        let a = entries.iter().find(|e| e["preset_id"] == "p-a").unwrap();
        let b = entries.iter().find(|e| e["preset_id"] == "p-b").unwrap();
        assert_eq!(a["is_default"], false);
        assert_eq!(b["is_default"], true);
    }

    #[test]
    fn list_presets_commands_available_true_for_path_resolvable_binary() {
        // `sh` is on PATH on every Unix CI runner; `git` is on every
        // platform we target. Either is fine — pick one that's
        // guaranteed available. The point is that a preset whose
        // first whitespace token resolves via `which::which` must
        // come back `commands_available: true`.
        let store = store_with(
            vec![make_preset("p-resolvable", "git status", PresetKind::Cli)],
            None,
        );
        let entries = list_presets_with_availability(&store);
        assert_eq!(
            entries[0]["commands_available"], true,
            "preset launching `git` must report commands_available=true; \
             git is required on PATH for every supported platform"
        );
    }

    #[test]
    fn list_presets_commands_available_false_for_missing_binary() {
        // The brain MUST be able to detect uninstalled agent CLIs.
        // Pick a name that's almost certainly not on any PATH so the
        // assertion holds even on dev machines with lots of tooling.
        let bogus = "codemux-phase-1-5-binary-that-does-not-exist-anywhere";
        let store = store_with(
            vec![make_preset(
                "p-missing",
                &format!("{bogus} --flag"),
                PresetKind::Cli,
            )],
            None,
        );
        let entries = list_presets_with_availability(&store);
        assert_eq!(
            entries[0]["commands_available"], false,
            "preset whose CLI is not on PATH must report \
             commands_available=false so the brain can pre-filter"
        );
    }

    #[test]
    fn list_presets_empty_commands_treated_as_available() {
        // Shell presets have no commands (the user types into the
        // freshly-spawned terminal). With an empty commands list there
        // is no CLI to validate, so commands_available is vacuously
        // true. The shell itself is launched by the platform; if it's
        // missing, far bigger problems exist.
        let store = store_with(
            vec![make_preset("p-shell-empty", "", PresetKind::Cli)],
            None,
        );
        let entries = list_presets_with_availability(&store);
        assert_eq!(entries[0]["commands_available"], true);
    }
}
