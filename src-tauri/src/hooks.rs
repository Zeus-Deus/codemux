use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tauri::{AppHandle, Manager};

use crate::state::{self, AppStateStore, PaneStatus};

static HOOK_PORT: OnceLock<u16> = OnceLock::new();
static MONITOR_SESSIONS: OnceLock<Mutex<HashMap<String, Arc<AtomicBool>>>> = OnceLock::new();

pub fn hook_port() -> Option<u16> {
    HOOK_PORT.get().copied()
}

/// Start the agent hook notification server on a random localhost port.
/// Returns the allocated port number.
pub fn start_hook_server(app: AppHandle) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("Failed to bind hook server");
    let port = listener.local_addr().unwrap().port();
    HOOK_PORT.set(port).ok();

    std::thread::spawn(move || {
        // Accept connections until the app exits
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(2)));

            let app = app.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 2048];
                let n = match stream.read(&mut buf) {
                    Ok(n) if n > 0 => n,
                    _ => return,
                };
                let request = String::from_utf8_lossy(&buf[..n]);

                // Parse the GET request line for query parameters
                let first_line = request.lines().next().unwrap_or("");
                if !first_line.starts_with("GET /hook") {
                    let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
                    return;
                }

                let query = first_line
                    .split_once('?')
                    .and_then(|(_, rest)| rest.split_once(' ').map(|(q, _)| q))
                    .unwrap_or("");

                let params: std::collections::HashMap<&str, &str> = query
                    .split('&')
                    .filter_map(|pair| pair.split_once('='))
                    .collect();

                let event_type = params.get("eventType").copied().unwrap_or("");
                let session_id = params.get("sessionId").copied().unwrap_or("");

                if event_type.is_empty() || session_id.is_empty() {
                    let _ = stream.write_all(
                        b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n",
                    );
                    return;
                }

                let status = match map_event_type(event_type) {
                    Some(s) => s,
                    None => {
                        // Unknown event type — acknowledge but ignore
                        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
                        return;
                    }
                };

                handle_lifecycle_event(&app, session_id, status);

                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
            });
        }
    });

    port
}

/// Map agent-specific event names to canonical PaneStatus.
fn map_event_type(event_type: &str) -> Option<PaneStatus> {
    match event_type {
        // Start events → Working
        "Start" | "UserPromptSubmit" | "PostToolUse" | "PostToolUseFailure" | "BeforeAgent"
        | "AfterTool" | "sessionStart" | "userPromptSubmitted" | "postToolUse" => {
            Some(PaneStatus::Working)
        }
        // Stop events → Review (caller decides idle vs review)
        "Stop" | "agent-turn-complete" | "AfterAgent" => Some(PaneStatus::Review),
        // Session end → Idle (agent is exiting, always clear)
        "sessionEnd" | "SessionEnd" => Some(PaneStatus::Idle),
        // Permission events
        "PermissionRequest" | "Notification" | "preToolUse" | "permission.ask"
        | "beforeShellExecution" | "beforeMCPExecution" => Some(PaneStatus::Permission),
        _ => None,
    }
}

fn handle_lifecycle_event(app: &AppHandle, session_id: &str, status: PaneStatus) {
    let state: tauri::State<'_, AppStateStore> = app.state();

    // For Stop events, check if the pane is in the active workspace+tab — if so, go idle
    let resolved_status = if status == PaneStatus::Review {
        let snapshot = state.snapshot();
        let is_active = is_pane_active_for_session(&snapshot, session_id);
        if is_active {
            PaneStatus::Idle
        } else {
            PaneStatus::Review
        }
    } else {
        status
    };

    let is_active_status =
        matches!(resolved_status, PaneStatus::Working | PaneStatus::Permission);
    state.set_pane_status_by_session(session_id, resolved_status.clone());
    state::emit_app_state(app);

    // When status becomes Working/Permission, start monitoring for agent exit.
    // This catches cases where the agent exits without sending a Stop hook
    // (e.g., user presses Ctrl+C or Escape to kill/exit the agent CLI).
    if is_active_status {
        let pty_state: tauri::State<'_, crate::terminal::PtyState> = app.state();
        if let Some(shell_pid) = pty_state.get_session_pids().get(session_id).copied() {
            start_agent_exit_monitor(app.clone(), session_id.to_string(), shell_pid);
        }
    }
}

/// Check if the pane for a session is in the currently active workspace.
fn is_pane_active_for_session(
    snapshot: &state::AppStateSnapshot,
    session_id: &str,
) -> bool {
    for ws in &snapshot.workspaces {
        if ws.workspace_id != snapshot.active_workspace_id {
            continue;
        }
        for surface in &ws.surfaces {
            if find_session_in_node(&surface.root, session_id) {
                return true;
            }
        }
    }
    false
}

fn find_session_in_node(
    node: &state::PaneNodeSnapshot,
    target_session_id: &str,
) -> bool {
    match node {
        state::PaneNodeSnapshot::Terminal { session_id, .. } => {
            session_id.0 == target_session_id
        }
        state::PaneNodeSnapshot::Split { children, .. } => {
            children.iter().any(|c| find_session_in_node(c, target_session_id))
        }
        state::PaneNodeSnapshot::Browser { .. } => false,
    }
}

// ── Agent exit monitor ──
// Detects when an agent process exits without sending a Stop hook by polling
// the shell's foreground process group via /proc. When the shell becomes the
// foreground process (no child command running), any stuck Working/Permission
// status is cleared.

fn monitor_sessions() -> &'static Mutex<HashMap<String, Arc<AtomicBool>>> {
    MONITOR_SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Check if the shell process is the foreground process group (no child command
/// running). Returns `true` when the shell's pgrp equals the terminal's
/// foreground pgrp, or when the process no longer exists.
#[cfg(target_os = "linux")]
fn shell_is_foreground(shell_pid: u32) -> bool {
    let stat_path = format!("/proc/{shell_pid}/stat");
    let stat = match std::fs::read_to_string(&stat_path) {
        Ok(s) => s,
        Err(_) => return true, // Process gone — treat as exited
    };
    // /proc/PID/stat: PID (comm) state ppid pgrp session tty_nr tpgid ...
    let after_comm = match stat.rfind(')') {
        Some(idx) if idx + 2 < stat.len() => &stat[idx + 2..],
        _ => return false,
    };
    let fields: Vec<&str> = after_comm.split_whitespace().collect();
    // [0]=state [1]=ppid [2]=pgrp [3]=session [4]=tty_nr [5]=tpgid
    if fields.len() < 6 {
        return false;
    }
    let pgrp: i32 = fields[2].parse().unwrap_or(0);
    let tpgid: i32 = fields[5].parse().unwrap_or(-1);
    tpgid == pgrp
}

#[cfg(not(target_os = "linux"))]
fn shell_is_foreground(_shell_pid: u32) -> bool {
    false
}

/// Start a background thread that monitors when an agent exits so that stuck
/// Working/Permission status indicators can be cleared.
fn start_agent_exit_monitor(app: AppHandle, session_id: String, shell_pid: u32) {
    let monitors = monitor_sessions();
    let mut guard = monitors.lock().unwrap_or_else(|e| e.into_inner());

    // Don't spawn duplicate monitors for the same session
    if let Some(active) = guard.get(&session_id) {
        if active.load(Ordering::Relaxed) {
            return;
        }
    }

    let active = Arc::new(AtomicBool::new(true));
    guard.insert(session_id.clone(), active.clone());
    drop(guard);

    std::thread::spawn(move || {
        // Give the agent time to start before polling
        std::thread::sleep(Duration::from_secs(2));

        while active.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(1000));

            // Check if the pane status is still active (Working/Permission)
            let state: tauri::State<'_, AppStateStore> = app.state();
            let snapshot = state.snapshot();
            let still_active = snapshot
                .workspaces
                .iter()
                .find_map(|ws| {
                    ws.surfaces
                        .iter()
                        .find_map(|s| state::find_terminal_pane_id(&s.root, &session_id))
                })
                .and_then(|pane_id| snapshot.pane_statuses.get(&pane_id.0))
                .map(|s| matches!(s, PaneStatus::Working | PaneStatus::Permission))
                .unwrap_or(false);

            if !still_active {
                break; // Status already cleared by a hook or terminal exit
            }

            // Check if the shell is the foreground process (agent has exited)
            if shell_is_foreground(shell_pid) {
                state.clear_transient_pane_status_by_session(&session_id);
                state::emit_app_state(&app);
                break;
            }
        }

        // Cleanup
        if let Ok(mut guard) = monitor_sessions().lock() {
            guard.remove(&session_id);
        }
    });
}

// ── Hook script and agent registration ──

const HOOK_SCRIPT: &str = r#"#!/bin/sh
# Codemux agent lifecycle hook — notifies the hook server of agent status changes.
# Injected env: CODEMUX_HOOK_PORT, CODEMUX_SESSION_ID
[ -z "$CODEMUX_HOOK_PORT" ] && exit 0
[ -z "$CODEMUX_SESSION_ID" ] && exit 0

EVENT_TYPE="${1:-}"

# Claude Code passes hook event name as $1
[ -z "$EVENT_TYPE" ] && exit 0

curl -s --connect-timeout 1 --max-time 2 \
  "http://127.0.0.1:${CODEMUX_HOOK_PORT}/hook?sessionId=${CODEMUX_SESSION_ID}&eventType=${EVENT_TYPE}" \
  >/dev/null 2>&1 || true &
exit 0
"#;

/// Write the hook notification script to ~/.codemux/hooks/notify.sh
pub fn ensure_hook_script() -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    let hooks_dir = std::path::PathBuf::from(&home).join(".codemux/hooks");
    std::fs::create_dir_all(&hooks_dir).ok()?;

    let script_path = hooks_dir.join("notify.sh");
    std::fs::write(&script_path, HOOK_SCRIPT).ok()?;

    // Make executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755));
    }

    Some(script_path.to_string_lossy().into_owned())
}

/// Check if a hook entry (in Claude Code format) contains a codemux hook.
fn entry_contains_codemux_hook(entry: &serde_json::Value) -> bool {
    // Check the nested format: { "hooks": [{ "command": "...codemux..." }] }
    if let Some(hooks) = entry.get("hooks").and_then(|h| h.as_array()) {
        return hooks.iter().any(|h| {
            h.get("command")
                .and_then(|c| c.as_str())
                .map(|c| c.contains(".codemux/hooks/notify.sh"))
                .unwrap_or(false)
        });
    }
    // Also check legacy flat format for cleanup: { "command": "...codemux..." }
    entry
        .get("command")
        .and_then(|c| c.as_str())
        .map(|c| c.contains(".codemux/hooks/notify.sh"))
        .unwrap_or(false)
}

/// Register hooks with Claude Code's settings.json (~/.claude/settings.json).
/// Only modifies the hooks section; preserves all other settings.
pub fn register_claude_code_hooks() {
    let Some(script_path) = ensure_hook_script() else {
        eprintln!("[codemux::hooks] Failed to create hook script");
        return;
    };

    let home = match std::env::var("HOME") {
        Ok(h) => h,
        Err(_) => return,
    };

    let settings_path = std::path::PathBuf::from(&home).join(".claude/settings.json");

    // Read existing settings or create empty object
    let mut settings: serde_json::Value = if settings_path.exists() {
        match std::fs::read_to_string(&settings_path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or(serde_json::json!({})),
            Err(_) => serde_json::json!({}),
        }
    } else {
        // Ensure directory exists
        if let Some(parent) = settings_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        serde_json::json!({})
    };

    // Build hook commands
    let hook_events = [
        ("UserPromptSubmit", "UserPromptSubmit"),
        ("Stop", "Stop"),
        ("PermissionRequest", "PermissionRequest"),
        ("SessionEnd", "sessionEnd"),
    ];

    let hooks = settings
        .as_object_mut()
        .unwrap()
        .entry("hooks")
        .or_insert(serde_json::json!({}));

    for (event_name, event_type) in &hook_events {
        let hook_cmd = format!("{script_path} {event_type}");

        let hook_array = hooks
            .as_object_mut()
            .unwrap()
            .entry(*event_name)
            .or_insert(serde_json::json!([]));

        // Claude Code hook format: each entry in the array must be
        // { "matcher": "<pattern>", "hooks": [{ "type": "command", "command": "..." }] }
        let codemux_entry = serde_json::json!({
            "matcher": "",
            "hooks": [{ "type": "command", "command": hook_cmd }]
        });

        if let Some(arr) = hook_array.as_array_mut() {
            // Find existing codemux hook entry (check inside hooks[].command)
            let existing_idx = arr.iter().position(|entry| {
                entry_contains_codemux_hook(entry)
            });

            if let Some(idx) = existing_idx {
                // Update in place
                arr[idx] = codemux_entry;
            } else {
                arr.push(codemux_entry);
            }
        }
    }

    // Write back
    match serde_json::to_string_pretty(&settings) {
        Ok(json) => {
            let _ = std::fs::write(&settings_path, json);
        }
        Err(e) => eprintln!("[codemux::hooks] Failed to serialize settings: {e}"),
    }
}

/// Remove all Codemux hook entries from ~/.claude/settings.json.
/// Preserves all other settings and non-Codemux hooks.
pub fn unregister_claude_code_hooks() {
    let home = match std::env::var("HOME") {
        Ok(h) => h,
        Err(_) => return,
    };

    let settings_path = std::path::PathBuf::from(&home).join(".claude/settings.json");
    if !settings_path.exists() {
        return;
    }

    let mut settings: serde_json::Value = match std::fs::read_to_string(&settings_path) {
        Ok(content) => match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => return,
        },
        Err(_) => return,
    };

    let Some(hooks) = settings.get_mut("hooks").and_then(|h| h.as_object_mut()) else {
        return;
    };

    // Remove codemux entries from each hook event array
    let event_keys: Vec<String> = hooks.keys().cloned().collect();
    for key in &event_keys {
        if let Some(arr) = hooks.get_mut(key).and_then(|v| v.as_array_mut()) {
            arr.retain(|entry| !entry_contains_codemux_hook(entry));
        }
    }

    // Remove empty event arrays
    hooks.retain(|_, v| {
        v.as_array().map(|a| !a.is_empty()).unwrap_or(true)
    });

    // Remove empty hooks object
    if hooks.is_empty() {
        settings.as_object_mut().unwrap().remove("hooks");
    }

    match serde_json::to_string_pretty(&settings) {
        Ok(json) => {
            let _ = std::fs::write(&settings_path, json);
        }
        Err(e) => eprintln!("[codemux::hooks] Failed to serialize settings: {e}"),
    }
}

/// Build the hooks JSON that would be written to ~/.claude/settings.json.
/// Useful for testing the format without touching the filesystem.
pub fn build_claude_hooks_json(script_path: &str) -> serde_json::Value {
    let hook_events = [
        ("UserPromptSubmit", "UserPromptSubmit"),
        ("Stop", "Stop"),
        ("PermissionRequest", "PermissionRequest"),
    ];

    let mut hooks = serde_json::json!({});
    for (event_name, event_type) in &hook_events {
        let hook_cmd = format!("{script_path} {event_type}");
        hooks[event_name] = serde_json::json!([{
            "matcher": "",
            "hooks": [{ "type": "command", "command": hook_cmd }]
        }]);
    }
    hooks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_json_matches_claude_code_format() {
        let hooks = build_claude_hooks_json("/home/test/.codemux/hooks/notify.sh");

        // Each event key must be an array
        for event in ["UserPromptSubmit", "Stop", "PermissionRequest"] {
            let arr = hooks[event].as_array().expect(&format!("{event} must be an array"));
            assert!(!arr.is_empty(), "{event} array must not be empty");

            for entry in arr {
                // Each entry must have "matcher" (string)
                assert!(
                    entry.get("matcher").and_then(|m| m.as_str()).is_some(),
                    "{event} entry must have a string 'matcher' field"
                );
                // Each entry must have "hooks" (array)
                let inner_hooks = entry
                    .get("hooks")
                    .and_then(|h| h.as_array())
                    .expect(&format!("{event} entry must have a 'hooks' array"));
                assert!(!inner_hooks.is_empty(), "{event} hooks array must not be empty");

                for hook in inner_hooks {
                    // Each hook must have "type": "command"
                    assert_eq!(
                        hook.get("type").and_then(|t| t.as_str()),
                        Some("command"),
                        "{event} hook must have type 'command'"
                    );
                    // Each hook must have "command" (string)
                    let cmd = hook
                        .get("command")
                        .and_then(|c| c.as_str())
                        .expect(&format!("{event} hook must have a 'command' string"));
                    assert!(
                        cmd.contains(".codemux/hooks/notify.sh"),
                        "command must reference the codemux hook script"
                    );
                    assert!(
                        cmd.contains(event),
                        "command must pass event type as argument"
                    );
                }
            }
        }
    }

    #[test]
    fn merge_preserves_existing_hooks() {
        let script = "/home/test/.codemux/hooks/notify.sh";

        // Simulate existing settings with a Superset hook
        let mut settings = serde_json::json!({
            "effortLevel": "high",
            "hooks": {
                "UserPromptSubmit": [{
                    "matcher": "*",
                    "hooks": [{"type": "command", "command": "superset-notify.sh"}]
                }]
            }
        });

        // Merge codemux hooks using the same logic as register_claude_code_hooks
        let hooks = settings
            .as_object_mut()
            .unwrap()
            .entry("hooks")
            .or_insert(serde_json::json!({}));

        let hook_events = [
            ("UserPromptSubmit", "UserPromptSubmit"),
            ("Stop", "Stop"),
        ];

        for (event_name, event_type) in &hook_events {
            let hook_cmd = format!("{script} {event_type}");
            let codemux_entry = serde_json::json!({
                "matcher": "",
                "hooks": [{ "type": "command", "command": hook_cmd }]
            });

            let hook_array = hooks
                .as_object_mut()
                .unwrap()
                .entry(*event_name)
                .or_insert(serde_json::json!([]));

            if let Some(arr) = hook_array.as_array_mut() {
                let existing_idx = arr.iter().position(|entry| entry_contains_codemux_hook(entry));
                if let Some(idx) = existing_idx {
                    arr[idx] = codemux_entry;
                } else {
                    arr.push(codemux_entry);
                }
            }
        }

        // Verify: effortLevel preserved
        assert_eq!(settings["effortLevel"], "high");

        // Verify: Superset hook still present in UserPromptSubmit
        let ups = settings["hooks"]["UserPromptSubmit"].as_array().unwrap();
        assert_eq!(ups.len(), 2, "should have superset + codemux hooks");
        assert!(ups[0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("superset"));

        // Verify: codemux hook has correct format
        assert_eq!(ups[1]["matcher"], "");
        assert!(ups[1]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains(".codemux/hooks/notify.sh"));

        // Verify: Stop has only codemux hook (newly created)
        let stop = settings["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 1);
    }

    #[test]
    fn unregister_removes_codemux_hooks_only() {
        // Simulate settings with mixed Superset + Codemux hooks
        let mut settings = serde_json::json!({
            "effortLevel": "high",
            "hooks": {
                "UserPromptSubmit": [
                    {
                        "matcher": "*",
                        "hooks": [{"type": "command", "command": "superset-notify.sh"}]
                    },
                    {
                        "matcher": "",
                        "hooks": [{"type": "command", "command": "/home/user/.codemux/hooks/notify.sh UserPromptSubmit"}]
                    }
                ],
                "Stop": [
                    {
                        "matcher": "",
                        "hooks": [{"type": "command", "command": "/home/user/.codemux/hooks/notify.sh Stop"}]
                    }
                ],
                "PostToolUse": [
                    {
                        "matcher": "*",
                        "hooks": [{"type": "command", "command": "superset-notify.sh"}]
                    }
                ]
            }
        });

        // Run unregister logic (same as unregister_claude_code_hooks but on in-memory value)
        let hooks = settings.get_mut("hooks").unwrap().as_object_mut().unwrap();
        let event_keys: Vec<String> = hooks.keys().cloned().collect();
        for key in &event_keys {
            if let Some(arr) = hooks.get_mut(key).and_then(|v| v.as_array_mut()) {
                arr.retain(|entry| !entry_contains_codemux_hook(entry));
            }
        }
        hooks.retain(|_, v| v.as_array().map(|a| !a.is_empty()).unwrap_or(true));
        if hooks.is_empty() {
            settings.as_object_mut().unwrap().remove("hooks");
        }

        // effortLevel preserved
        assert_eq!(settings["effortLevel"], "high");

        let hooks = settings["hooks"].as_object().unwrap();

        // UserPromptSubmit: superset entry remains, codemux entry removed
        let ups = hooks["UserPromptSubmit"].as_array().unwrap();
        assert_eq!(ups.len(), 1);
        assert!(ups[0]["hooks"][0]["command"].as_str().unwrap().contains("superset"));

        // Stop: was codemux-only, should be removed entirely
        assert!(!hooks.contains_key("Stop"), "Stop should be removed (was codemux-only)");

        // PostToolUse: superset-only, untouched
        let ptu = hooks["PostToolUse"].as_array().unwrap();
        assert_eq!(ptu.len(), 1);
    }

    #[test]
    fn unregister_removes_hooks_key_when_empty() {
        let mut settings = serde_json::json!({
            "effortLevel": "high",
            "hooks": {
                "Stop": [
                    {
                        "matcher": "",
                        "hooks": [{"type": "command", "command": "/home/user/.codemux/hooks/notify.sh Stop"}]
                    }
                ]
            }
        });

        let hooks = settings.get_mut("hooks").unwrap().as_object_mut().unwrap();
        let event_keys: Vec<String> = hooks.keys().cloned().collect();
        for key in &event_keys {
            if let Some(arr) = hooks.get_mut(key).and_then(|v| v.as_array_mut()) {
                arr.retain(|entry| !entry_contains_codemux_hook(entry));
            }
        }
        hooks.retain(|_, v| v.as_array().map(|a| !a.is_empty()).unwrap_or(true));
        if hooks.is_empty() {
            settings.as_object_mut().unwrap().remove("hooks");
        }

        assert_eq!(settings["effortLevel"], "high");
        assert!(settings.get("hooks").is_none(), "hooks key should be removed when empty");
    }

    #[test]
    fn detects_codemux_hook_in_both_formats() {
        // Correct nested format
        let correct = serde_json::json!({
            "matcher": "",
            "hooks": [{"type": "command", "command": "/home/user/.codemux/hooks/notify.sh Stop"}]
        });
        assert!(entry_contains_codemux_hook(&correct));

        // Legacy flat format (should also be detected for cleanup)
        let legacy = serde_json::json!({
            "type": "command",
            "command": "/home/user/.codemux/hooks/notify.sh Stop"
        });
        assert!(entry_contains_codemux_hook(&legacy));

        // Non-codemux entry
        let other = serde_json::json!({
            "matcher": "*",
            "hooks": [{"type": "command", "command": "superset-notify.sh"}]
        });
        assert!(!entry_contains_codemux_hook(&other));
    }
}
