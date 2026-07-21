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

const ACTIVE_HOOK_PORT_FILE: &str = "active-port";

pub fn hook_port() -> Option<u16> {
    HOOK_PORT.get().copied()
}

/// Start the agent hook notification server on a random localhost port.
/// Returns the allocated port number.
pub fn start_hook_server(app: AppHandle) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("Failed to bind hook server");
    let port = listener.local_addr().unwrap().port();
    HOOK_PORT.set(port).ok();
    if write_active_hook_port(port).is_none() {
        eprintln!("[codemux::hooks] Failed to publish active hook server port {port}");
    }

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
                let agent_session_id = params.get("agentSessionId").copied().unwrap_or("");

                if event_type.is_empty() || session_id.is_empty() {
                    let _ = stream.write_all(
                        b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n",
                    );
                    return;
                }

                // Always store the agent session ID — it reflects the CURRENT
                // session running in this pane.
                if !agent_session_id.is_empty() {
                    let state: tauri::State<'_, AppStateStore> = app.state();
                    state.set_terminal_adapter_capture(
                        session_id,
                        "claude_session_id",
                        agent_session_id,
                    );
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
///
/// The vocabulary spans every agent Codemux can register hooks for:
/// Claude Code (`UserPromptSubmit`, `PostToolUse`, `Stop`, ...), Codex
/// (`task_started`, `task_complete`, `exec_approval_request`, ...),
/// Gemini (`BeforeAgent`, `AfterAgent`, `AfterTool`, ...), and OpenCode
/// (the plugin pre-normalizes to `Start` / `Stop` / `PermissionRequest`).
/// This is the single source of truth for event-name → status; the
/// per-agent registration just decides which of these names actually
/// get wired up.
fn map_event_type(event_type: &str) -> Option<PaneStatus> {
    match event_type {
        // Start events → Working
        "Start" | "UserPromptSubmit" | "PostToolUse" | "PostToolUseFailure" | "BeforeAgent"
        | "AfterTool" | "sessionStart" | "session_start" | "userPromptSubmitted"
        | "postToolUse" | "task_started" => Some(PaneStatus::Working),
        // Stop events → Review (caller decides idle vs review)
        "Stop" | "agent-turn-complete" | "AfterAgent" | "task_complete" => {
            Some(PaneStatus::Review)
        }
        // Session end → Idle (agent is exiting, always clear)
        "sessionEnd" | "SessionEnd" | "session_end" => Some(PaneStatus::Idle),
        // Permission events.
        //
        // `Notification` reaches here only for genuine permission/approval
        // prompts: the hook script inspects the payload's `message` and
        // drops Claude Code's 60s idle reminder ("waiting for your input")
        // before it ever hits the server, so the red dot is not raised on
        // a finished, idle agent.
        "PermissionRequest" | "Notification" | "PreToolUse" | "preToolUse" | "permission.ask"
        | "beforeShellExecution" | "beforeMCPExecution" | "exec_approval_request"
        | "apply_patch_approval_request" | "request_user_input" => Some(PaneStatus::Permission),
        _ => None,
    }
}

fn handle_lifecycle_event(app: &AppHandle, session_id: &str, status: PaneStatus) {
    let state: tauri::State<'_, AppStateStore> = app.state();

    // For Stop events, check if the pane is in the active workspace+tab — if so, go idle
    let snapshot = state.snapshot();
    let is_active = if status == PaneStatus::Review {
        is_pane_active_for_session(&snapshot, session_id)
    } else {
        false
    };
    let resolved_status = if status == PaneStatus::Review && is_active {
        PaneStatus::Idle
    } else {
        status.clone()
    };

    let is_active_status =
        matches!(resolved_status, PaneStatus::Working | PaneStatus::Permission);
    state.set_pane_status_by_session(session_id, resolved_status.clone());
    // Coalesced: agent hooks can fire many times per second during a
    // streaming turn (PreToolUse / PostToolUse / Notification). The
    // 16 ms window collapses bursts into one frontend render — the
    // status pill update is not perception-sensitive at that scale.
    state::schedule_emit_app_state(app);

    // Fire desktop notification on agent completion when the user can't already
    // see the pane. Mirrors the "suppress if visible" behavior. A workspace
    // can also be explicitly muted (right-click → Mute notifications) — useful
    // when a pane runs a process that spawns agent subprocesses of its own,
    // whose lifecycle hooks would otherwise pop notifications for this pane.
    if status == PaneStatus::Review
        && !crate::notifications::should_suppress(app, is_active)
        && !state.is_session_workspace_muted(session_id)
    {
        let workspace_title = workspace_title_for_session(&snapshot, session_id)
            .unwrap_or_else(|| "Workspace".to_string());
        crate::notifications::dispatch_agent_complete(app, &workspace_title);
    }

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

/// Return the title of the workspace containing the given session, if any.
fn workspace_title_for_session(
    snapshot: &state::AppStateSnapshot,
    session_id: &str,
) -> Option<String> {
    for ws in &snapshot.workspaces {
        for surface in &ws.surfaces {
            if find_session_in_node(&surface.root, session_id) {
                return Some(ws.title.clone());
            }
        }
    }
    None
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
        state::PaneNodeSnapshot::Browser { .. }
        | state::PaneNodeSnapshot::AgentChat { .. } => false,
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
                // Coalesced: this fires from a polling loop checking
                // shell foreground state; not perception-sensitive at
                // 16 ms.
                state::schedule_emit_app_state(&app);
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

#[cfg(not(target_os = "windows"))]
const HOOK_SCRIPT: &str = r#"#!/bin/sh
# Codemux agent lifecycle hook — notifies the hook server of agent status changes.
# Injected env: CODEMUX_HOOK_PORT, CODEMUX_SESSION_ID
[ -z "$CODEMUX_SESSION_ID" ] && exit 0

# Event type comes from the first arg when the agent's hook config can pass
# one (Claude Code, Gemini). When it's absent — Codex registers a bare
# command and the Pi extension pipes only JSON — fall back to the event
# name carried in the agent's JSON payload.
EVENT_TYPE="${1:-}"

# Drain stdin once: the agent's JSON payload carries session_id and, for
# Codex/Pi, the hook_event_name (Codex's older notify callback uses "type").
INPUT=$(cat 2>/dev/null)

AGENT_SID=""
if command -v jq >/dev/null 2>&1; then
  AGENT_SID=$(printf '%s' "$INPUT" | jq -r '.session_id // empty' 2>/dev/null)
  if [ -z "$EVENT_TYPE" ]; then
    EVENT_TYPE=$(printf '%s' "$INPUT" | jq -r '.hook_event_name // .type // empty' 2>/dev/null)
  fi
fi

[ -z "$EVENT_TYPE" ] && exit 0

# Claude Code fires `Notification` for two unrelated situations: a real
# permission/approval prompt ("Claude needs your permission to use Bash")
# AND a 60s idle reminder ("Claude is waiting for your input"). Only the
# permission case should drive the red needs-input dot — forwarding the
# idle reminder would flip a finished agent's green dot to red with no
# follow-up event left to ever clear it. Inspect the payload's `message`
# and drop anything that is not a permission/approval request.
if [ "$EVENT_TYPE" = "Notification" ]; then
  NOTIF_MSG=""
  if command -v jq >/dev/null 2>&1; then
    NOTIF_MSG=$(printf '%s' "$INPUT" | jq -r '.message // empty' 2>/dev/null)
  fi
  # jq missing, or no `message` field — fall back to the raw payload.
  [ -z "$NOTIF_MSG" ] && NOTIF_MSG="$INPUT"
  case "$NOTIF_MSG" in
    *permission*|*Permission*|*approval*|*Approval*) : ;;
    *) exit 0 ;;
  esac
fi

# Persistent PTYs survive Codemux restarts, but their process environment
# cannot be updated: CODEMUX_HOOK_PORT can therefore point at the previous
# app instance. Try the inherited port first (important if two app instances
# coexist), then retry the port published by the current app. The retry lives
# in the detached subshell so hooks stay non-blocking.
send_hook() {
  PORT="$1"
  [ -n "$PORT" ] || return 1
  URL="http://127.0.0.1:${PORT}/hook?sessionId=${CODEMUX_SESSION_ID}&eventType=${EVENT_TYPE}"
  if [ -n "$AGENT_SID" ]; then
    URL="${URL}&agentSessionId=${AGENT_SID}"
  fi
  curl -fsS --connect-timeout 1 --max-time 2 "$URL" >/dev/null 2>&1
}

(
  send_hook "$CODEMUX_HOOK_PORT" && exit 0
  SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" 2>/dev/null && pwd)
  ACTIVE_PORT=$(cat "$SCRIPT_DIR/active-port" 2>/dev/null)
  [ "$ACTIVE_PORT" = "$CODEMUX_HOOK_PORT" ] && exit 0
  send_hook "$ACTIVE_PORT" || true
) >/dev/null 2>&1 &
exit 0
"#;

/// Windows equivalent of HOOK_SCRIPT — same protocol (read stdin JSON,
/// extract session_id, fire-and-forget GET to the hook server) but in
/// PowerShell so cmd.exe can invoke it without bash/curl/jq.
#[cfg(target_os = "windows")]
const HOOK_SCRIPT_PS1: &str = r#"# Codemux agent lifecycle hook (Windows / PowerShell)
# Injected env: CODEMUX_HOOK_PORT, CODEMUX_SESSION_ID
$ErrorActionPreference = 'SilentlyContinue'
if (-not $env:CODEMUX_SESSION_ID)   { exit 0 }
$eventType = if ($args.Count -gt 0) { $args[0] } else { '' }

# Agents pipe JSON on stdin; extract session_id, and the event name when
# it wasn't passed as an arg (Codex / Pi).
$agentSid = ''
try {
    $raw = [Console]::In.ReadToEnd()
    if ($raw) {
        $obj = $raw | ConvertFrom-Json
        if ($obj.session_id) { $agentSid = $obj.session_id }
        if (-not $eventType) {
            if ($obj.hook_event_name) { $eventType = $obj.hook_event_name }
            elseif ($obj.type)        { $eventType = $obj.type }
        }
    }
} catch {}

if (-not $eventType) { exit 0 }

# Claude Code fires `Notification` for two unrelated situations: a real
# permission/approval prompt AND a 60s idle reminder ("Claude is waiting
# for your input"). Only the permission case should drive the red
# needs-input dot — forwarding the idle reminder would flip a finished
# agent's green dot to red with no follow-up event left to clear it.
if ($eventType -eq 'Notification') {
    $notifMsg = ''
    try { if ($obj -and $obj.message) { $notifMsg = $obj.message } } catch {}
    if (-not $notifMsg) { $notifMsg = $raw }
    if ($notifMsg -notmatch '(?i)permission|approval') { exit 0 }
}

function Send-CodemuxHook([string]$port) {
    if (-not $port) { return $false }
    $url = "http://127.0.0.1:$port/hook?sessionId=$($env:CODEMUX_SESSION_ID)&eventType=$eventType"
    if ($agentSid) { $url = "$url&agentSessionId=$agentSid" }
    try {
        Invoke-WebRequest -Uri $url -UseBasicParsing -TimeoutSec 2 | Out-Null
        return $true
    } catch {
        return $false
    }
}

# A daemon-backed PTY can outlive the app that supplied its environment.
# Prefer that inherited app's port, then fall back to the port published by
# the current Codemux process when the old listener is gone.
if (-not (Send-CodemuxHook $env:CODEMUX_HOOK_PORT)) {
    $activePort = ''
    try { $activePort = (Get-Content (Join-Path $PSScriptRoot 'active-port') -Raw).Trim() } catch {}
    if ($activePort -and $activePort -ne $env:CODEMUX_HOOK_PORT) {
        [void](Send-CodemuxHook $activePort)
    }
}
exit 0
"#;

/// Gemini CLI lifecycle hook. Unlike Claude/Codex, the Gemini CLI *blocks*
/// on a hook until it receives valid JSON on stdout — so this script must
/// `printf '{}'` before doing anything slow. The event type is passed as
/// the literal `$1` arg (same model as `notify.sh`); stdin JSON is only
/// mined for the agent `session_id`.
#[cfg(not(target_os = "windows"))]
const GEMINI_HOOK_SCRIPT: &str = r#"#!/bin/sh
# Codemux Gemini lifecycle hook — notifies the hook server of agent status.
# Injected env: CODEMUX_HOOK_PORT, CODEMUX_SESSION_ID

EVENT_TYPE="${1:-}"

# Drain stdin (the Gemini JSON payload) first, then ALWAYS emit `{}` so the
# Gemini CLI doesn't hang waiting on us — even if the rest is a no-op.
INPUT=$(cat)
printf '{}\n'

[ -z "$CODEMUX_SESSION_ID" ] && exit 0
[ -z "$EVENT_TYPE" ] && exit 0

AGENT_SID=""
if command -v jq >/dev/null 2>&1; then
  AGENT_SID=$(printf '%s' "$INPUT" | jq -r '.session_id // empty' 2>/dev/null)
fi

send_hook() {
  PORT="$1"
  [ -n "$PORT" ] || return 1
  URL="http://127.0.0.1:${PORT}/hook?sessionId=${CODEMUX_SESSION_ID}&eventType=${EVENT_TYPE}"
  if [ -n "$AGENT_SID" ]; then
    URL="${URL}&agentSessionId=${AGENT_SID}"
  fi
  curl -fsS --connect-timeout 1 --max-time 2 "$URL" >/dev/null 2>&1
}

(
  send_hook "$CODEMUX_HOOK_PORT" && exit 0
  SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" 2>/dev/null && pwd)
  ACTIVE_PORT=$(cat "$SCRIPT_DIR/active-port" 2>/dev/null)
  [ "$ACTIVE_PORT" = "$CODEMUX_HOOK_PORT" ] && exit 0
  send_hook "$ACTIVE_PORT" || true
) >/dev/null 2>&1 &
exit 0
"#;

/// Cross-platform "user home directory" lookup. On Unix this is `$HOME`,
/// on Windows we prefer `$USERPROFILE` because `HOME` is not set unless
/// the user explicitly configured it (e.g. for git/MSYS2 compatibility).
fn user_home_dir() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        if let Ok(profile) = std::env::var("USERPROFILE") {
            return Some(profile);
        }
        // Fallback for users who do have HOME set (Git Bash, MSYS2 launches).
        std::env::var("HOME").ok()
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::env::var("HOME").ok()
    }
}

/// Publish the current app instance's hook-server port next to the shared
/// notifier scripts. Daemon-backed terminals retain the environment of the
/// app instance that originally spawned them, so their inherited
/// `CODEMUX_HOOK_PORT` becomes stale after an app restart. The scripts try
/// that inherited port first, then read this file and retry against the
/// current listener.
fn write_active_hook_port(port: u16) -> Option<std::path::PathBuf> {
    let home = user_home_dir()?;
    write_active_hook_port_in(&std::path::PathBuf::from(home).join(".codemux/hooks"), port)
}

fn write_active_hook_port_in(hooks_dir: &std::path::Path, port: u16) -> Option<std::path::PathBuf> {
    std::fs::create_dir_all(hooks_dir).ok()?;
    let path = hooks_dir.join(ACTIVE_HOOK_PORT_FILE);
    // One short write during startup. A failed write leaves the prior port in
    // place, while fresh terminals still use their inherited env value.
    std::fs::write(&path, format!("{port}\n")).ok()?;
    Some(path)
}

/// Write a file into `~/.codemux/hooks/` with the given name and body,
/// making it executable on Unix. Returns the absolute path.
///
/// Shared by every agent hook artifact — the main `notify.sh`, the
/// Gemini wrapper script, and the OpenCode plugin.
fn write_hook_file(name: &str, body: &str, executable: bool) -> Option<String> {
    let home = user_home_dir()?;
    let hooks_dir = std::path::PathBuf::from(&home).join(".codemux/hooks");
    std::fs::create_dir_all(&hooks_dir).ok()?;

    let path = hooks_dir.join(name);
    std::fs::write(&path, body).ok()?;

    #[cfg(unix)]
    if executable {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
    }
    #[cfg(not(unix))]
    let _ = executable;

    Some(path.to_string_lossy().into_owned())
}

/// Write the hook notification script. On Unix this is `~/.codemux/hooks/notify.sh`;
/// on Windows it's `~/.codemux/hooks/notify.ps1` (cmd.exe cannot execute `.sh`
/// without explicit `bash` invocation, which would force a Git Bash dependency).
pub fn ensure_hook_script() -> Option<String> {
    #[cfg(target_os = "windows")]
    let (script_name, script_body) = ("notify.ps1", HOOK_SCRIPT_PS1);
    #[cfg(not(target_os = "windows"))]
    let (script_name, script_body) = ("notify.sh", HOOK_SCRIPT);

    write_hook_file(script_name, script_body, true)
}

/// Check if a hook entry (in Claude Code format) contains a codemux hook.
/// Matches both the Unix `notify.sh` form and the Windows `notify.ps1` form
/// so cleanup works regardless of which platform last registered the hook
/// (e.g. dotfiles synced between Unix and Windows machines).
fn entry_contains_codemux_hook(entry: &serde_json::Value) -> bool {
    let is_codemux = |cmd: &str| {
        cmd.contains(".codemux") && (cmd.contains("notify.sh") || cmd.contains("notify.ps1"))
    };
    // Check the nested format: { "hooks": [{ "command": "...codemux..." }] }
    if let Some(hooks) = entry.get("hooks").and_then(|h| h.as_array()) {
        return hooks.iter().any(|h| {
            h.get("command")
                .and_then(|c| c.as_str())
                .map(is_codemux)
                .unwrap_or(false)
        });
    }
    // Also check legacy flat format for cleanup: { "command": "...codemux..." }
    entry
        .get("command")
        .and_then(|c| c.as_str())
        .map(is_codemux)
        .unwrap_or(false)
}

/// Build the shell command an agent will invoke for a hook event.
///
/// Unix: bash interprets the `.sh` script directly when run via the shell.
/// Windows: cmd.exe cannot execute `.ps1` directly, so we wrap it in a
/// `powershell -NoProfile -ExecutionPolicy Bypass -File ...` invocation
/// (matches the way other Tauri/Electron tools register PS hooks).
///
/// An empty `event_type` produces a *bare* command (just the script path)
/// — Codex registers one bare command for all events and the notify
/// script recovers the event name from the JSON payload on stdin.
fn build_hook_command(script_path: &str, event_type: &str) -> String {
    #[cfg(target_os = "windows")]
    {
        let base =
            format!("powershell -NoProfile -ExecutionPolicy Bypass -File \"{script_path}\"");
        if event_type.is_empty() {
            base
        } else {
            format!("{base} {event_type}")
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        if event_type.is_empty() {
            script_path.to_string()
        } else {
            format!("{script_path} {event_type}")
        }
    }
}

/// Merge Codemux hook entries into a nested-format hook config file.
///
/// Claude Code (`~/.claude/settings.json`), Codex (`~/.codex/hooks.json`),
/// and Gemini (`~/.gemini/settings.json`) all share the same shape:
///
/// ```json
/// { "hooks": { "<EventName>": [ { "matcher"?: "", "hooks": [ {type, command} ] } ] } }
/// ```
///
/// Only the `hooks` section is touched; every other user setting is
/// preserved. A prior Codemux entry for an event is replaced in place so
/// repeated startups never accumulate duplicates. `hook_events` maps each
/// agent's event name to the canonical event-type arg passed to the
/// notify script.
fn merge_nested_hooks_file(
    settings_path: &std::path::Path,
    hook_events: &[(&str, &str)],
    script_path: &str,
) {
    // Read existing settings or start from an empty object.
    let mut settings: serde_json::Value = if settings_path.exists() {
        match std::fs::read_to_string(settings_path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or(serde_json::json!({})),
            Err(_) => serde_json::json!({}),
        }
    } else {
        if let Some(parent) = settings_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        serde_json::json!({})
    };

    // Defensive: if the file existed but held a non-object (corrupt /
    // hand-edited), don't clobber it — bail rather than overwrite.
    if !settings.is_object() {
        eprintln!(
            "[codemux::hooks] {} is not a JSON object; skipping hook merge",
            settings_path.display()
        );
        return;
    }

    let hooks = settings
        .as_object_mut()
        .unwrap()
        .entry("hooks")
        .or_insert(serde_json::json!({}));

    for (event_name, event_type) in hook_events {
        let hook_cmd = build_hook_command(script_path, event_type);

        let hook_array = hooks
            .as_object_mut()
            .unwrap()
            .entry(*event_name)
            .or_insert(serde_json::json!([]));

        // Nested hook format: each entry is
        // { "matcher": "<pattern>", "hooks": [{ "type": "command", "command": "..." }] }
        let codemux_entry = serde_json::json!({
            "matcher": "",
            "hooks": [{ "type": "command", "command": hook_cmd }]
        });

        if let Some(arr) = hook_array.as_array_mut() {
            // Replace an existing codemux entry in place; otherwise append.
            let existing_idx = arr.iter().position(entry_contains_codemux_hook);
            if let Some(idx) = existing_idx {
                arr[idx] = codemux_entry;
            } else {
                arr.push(codemux_entry);
            }
        }
    }

    match serde_json::to_string_pretty(&settings) {
        Ok(json) => {
            let _ = std::fs::write(settings_path, json);
        }
        Err(e) => eprintln!("[codemux::hooks] Failed to serialize settings: {e}"),
    }
}

/// Register hooks with Claude Code's settings.json (~/.claude/settings.json).
/// Only modifies the hooks section; preserves all other settings.
///
/// PostToolUse is registered so that when the user answers a
/// PermissionRequest / Notification (e.g. picks an option from an
/// AskUserQuestion menu), the tool's resolution fires PostToolUse →
/// Working — clearing the stuck red pulse. Notification drives the red
/// pulse, but Claude Code fires it both for permission prompts and for a
/// 60s idle reminder; the hook script forwards only the permission case
/// (see HOOK_SCRIPT) so a finished agent's green dot is never flipped to
/// a red dot that has no follow-up event to clear it.
pub fn register_claude_code_hooks() {
    let Some(script_path) = ensure_hook_script() else {
        eprintln!("[codemux::hooks] Failed to create hook script");
        return;
    };
    let Some(home) = user_home_dir() else { return };

    let settings_path = std::path::PathBuf::from(&home).join(".claude/settings.json");
    merge_nested_hooks_file(&settings_path, &CLAUDE_HOOK_EVENTS, &script_path);
}

/// Hook events registered with Claude Code. See `register_claude_code_hooks`.
const CLAUDE_HOOK_EVENTS: [(&str, &str); 6] = [
    ("UserPromptSubmit", "UserPromptSubmit"),
    ("Stop", "Stop"),
    ("PermissionRequest", "PermissionRequest"),
    ("Notification", "Notification"),
    ("PostToolUse", "PostToolUse"),
    ("SessionEnd", "sessionEnd"),
];

/// Hook events registered with Codex via `~/.codex/hooks.json`.
///
/// Codex (≥0.129) auto-loads `~/.codex/hooks.json` and uses the same
/// nested hook shape as Claude. The event names are verified against the
/// `HookEventNameWire` enum in the Codex 0.130 binary — the full set is
/// `PreToolUse`, `PermissionRequest`, `PostToolUse`, `PreCompact`,
/// `PostCompact`, `SessionStart`, `UserPromptSubmit`, `Stop`. Note there
/// is **no `SessionEnd`** (unlike Claude/Gemini).
///
/// Codex pipes its event JSON on stdin with `hook_event_name`, so all
/// events register the *bare* notify-script path (the empty event-type
/// arg) and `notify.sh` recovers the event name from the payload — this
/// matches the format Codex itself writes when other tools register.
///
/// `PermissionRequest` is a first-class Codex hook event, so Codex gets
/// the red "needs input" pulse, not just the amber/green cadence.
const CODEX_HOOK_EVENTS: [(&str, &str); 4] = [
    ("UserPromptSubmit", ""),
    ("PostToolUse", ""),
    ("PermissionRequest", ""),
    ("Stop", ""),
];

/// Register hooks with Codex via `~/.codex/hooks.json`.
///
/// Only runs if `~/.codex` already exists — i.e. the user actually has
/// Codex — so Codemux never creates config dirs for tools that aren't
/// installed. The merge preserves any existing Codex hooks.
pub fn register_codex_hooks() {
    let Some(script_path) = ensure_hook_script() else {
        eprintln!("[codemux::hooks] Failed to create hook script");
        return;
    };
    let Some(home) = user_home_dir() else { return };

    let codex_dir = std::path::PathBuf::from(&home).join(".codex");
    if !codex_dir.exists() {
        return; // Codex not installed — don't create stray config.
    }

    let hooks_path = codex_dir.join("hooks.json");
    merge_nested_hooks_file(&hooks_path, &CODEX_HOOK_EVENTS, &script_path);
}

/// Hook events registered with Gemini via `~/.gemini/settings.json`.
///
/// Gemini (verified against the CLI 0.42 bundle) exposes the hook events
/// `SessionStart`, `SessionEnd`, `BeforeAgent`, `AfterAgent`, `BeforeTool`,
/// `AfterTool`, `BeforeModel`, `AfterModel`, `Notification`, `Stop`. We
/// register `BeforeAgent`/`AfterTool` (amber), `AfterAgent` (green),
/// `Notification` (red — needs input) and `SessionEnd` (clear). We skip
/// `SessionStart` since it fires on boot while the agent is still idle.
/// The settings file uses the same nested hook shape as Claude/Codex, and
/// Gemini even auto-migrates Claude-format hook entries.
#[cfg(not(target_os = "windows"))]
const GEMINI_HOOK_EVENTS: [(&str, &str); 5] = [
    ("BeforeAgent", "BeforeAgent"),
    ("AfterTool", "AfterTool"),
    ("AfterAgent", "AfterAgent"),
    ("Notification", "Notification"),
    ("SessionEnd", "SessionEnd"),
];

/// Write the Gemini hook script to `~/.codemux/hooks/gemini-notify.sh`.
#[cfg(not(target_os = "windows"))]
fn ensure_gemini_hook_script() -> Option<String> {
    write_hook_file("gemini-notify.sh", GEMINI_HOOK_SCRIPT, true)
}

/// Register hooks with Gemini via `~/.gemini/settings.json`.
///
/// Only runs if `~/.gemini` already exists — i.e. the user actually has
/// the Gemini CLI. Unix-only: the Gemini hook is a POSIX `sh` script
/// (Windows Gemini support would need a PowerShell variant, same as the
/// main `notify.ps1`).
#[cfg(not(target_os = "windows"))]
pub fn register_gemini_hooks() {
    let Some(home) = user_home_dir() else { return };

    let gemini_dir = std::path::PathBuf::from(&home).join(".gemini");
    if !gemini_dir.exists() {
        return; // Gemini CLI not installed — don't create stray config.
    }

    let Some(script_path) = ensure_gemini_hook_script() else {
        eprintln!("[codemux::hooks] Failed to create Gemini hook script");
        return;
    };

    let settings_path = gemini_dir.join("settings.json");
    merge_nested_hooks_file(&settings_path, &GEMINI_HOOK_EVENTS, &script_path);
}

/// No-op on Windows — the Gemini hook script is POSIX `sh` only.
#[cfg(target_os = "windows")]
pub fn register_gemini_hooks() {}

/// OpenCode notify plugin. OpenCode has no hook-config file like Claude or
/// Codex — instead it loads JS plugins from its plugin directory. This
/// plugin watches OpenCode's `session.status` busy/idle transitions and
/// `permission.ask` events and shells out to `notify.sh`, normalizing to
/// the canonical `Start` / `Stop` / `PermissionRequest` vocabulary.
///
/// `{{NOTIFY_PATH}}` is substituted with the absolute `notify.sh` path at
/// install time. The plugin is a no-op unless `CODEMUX_SESSION_ID` is in
/// the environment, so a user running OpenCode outside Codemux is
/// unaffected.
#[cfg(not(target_os = "windows"))]
const OPENCODE_PLUGIN_TEMPLATE: &str = r#"// Codemux OpenCode notify plugin
// Installed by Codemux — bridges OpenCode session lifecycle to the
// Codemux agent status indicator (the working/idle/needs-input dot).
// Safe to delete: Codemux rewrites it on startup.

export const CodemuxNotifyPlugin = async ({ $ }) => {
  // Only active inside a Codemux-managed terminal.
  if (!process?.env?.CODEMUX_SESSION_ID) return {};

  const notifyPath = "{{NOTIFY_PATH}}";

  let currentState = "idle"; // "idle" | "busy"
  let rootSessionID = null;
  let stopSent = false;

  const notify = async (eventType) => {
    try {
      // Event type is the literal argv arg; stdin is closed so notify.sh's
      // session-id capture returns empty without blocking.
      await $`bash ${notifyPath} ${eventType} < /dev/null`.quiet();
    } catch {
      // Best-effort — never break the agent over a status ping.
    }
  };

  const isChildSession = (event) =>
    Boolean(event?.properties?.info?.parentID);

  const handleBusy = async (sessionID) => {
    if (!rootSessionID) rootSessionID = sessionID;
    if (sessionID !== rootSessionID) return;
    if (currentState === "idle") {
      currentState = "busy";
      stopSent = false;
      await notify("Start");
    }
  };

  const handleStop = async (sessionID) => {
    if (rootSessionID && sessionID !== rootSessionID) return;
    if (currentState === "busy" && !stopSent) {
      currentState = "idle";
      stopSent = true;
      rootSessionID = null;
      await notify("Stop");
    }
  };

  return {
    event: async ({ event }) => {
      const sessionID =
        event?.properties?.sessionID ?? event?.properties?.info?.id ?? null;

      // Ignore subagent/child sessions so background spawns don't flip the dot.
      if (isChildSession(event)) return;

      // Verified against @opencode-ai/sdk: session.status carries
      // { properties: { sessionID, status: { type: "idle"|"busy"|"retry" } } };
      // session.idle / session.error carry { properties: { sessionID } }.
      if (event.type === "session.status") {
        const status = event.properties?.status?.type;
        if (status === "busy") await handleBusy(sessionID);
        else if (status === "idle") await handleStop(sessionID);
      }
      if (event.type === "session.idle") await handleStop(sessionID);
      if (event.type === "session.error") await handleStop(sessionID);
    },
    "permission.ask": async () => {
      await notify("PermissionRequest");
    },
  };
};
"#;

/// Resolve OpenCode's config directory: `$XDG_CONFIG_HOME/opencode` or
/// `~/.config/opencode`.
#[cfg(not(target_os = "windows"))]
fn opencode_config_dir() -> Option<std::path::PathBuf> {
    let config_home = match std::env::var("XDG_CONFIG_HOME") {
        Ok(x) if !x.trim().is_empty() => std::path::PathBuf::from(x),
        _ => std::path::PathBuf::from(user_home_dir()?).join(".config"),
    };
    Some(config_home.join("opencode"))
}

/// Render the OpenCode plugin JS with the notify script path inlined.
#[cfg(not(target_os = "windows"))]
fn build_opencode_plugin(notify_script_path: &str) -> String {
    OPENCODE_PLUGIN_TEMPLATE.replace("{{NOTIFY_PATH}}", notify_script_path)
}

/// Install the OpenCode notify plugin into OpenCode's plugin directory.
///
/// Only runs if OpenCode's config dir already exists — i.e. the user
/// actually has OpenCode. Unix-only (the plugin shells out to `notify.sh`).
#[cfg(not(target_os = "windows"))]
pub fn register_opencode_plugin() {
    let Some(config_dir) = opencode_config_dir() else { return };
    if !config_dir.exists() {
        return; // OpenCode not installed — don't create stray config.
    }

    let Some(script_path) = ensure_hook_script() else {
        eprintln!("[codemux::hooks] Failed to create hook script");
        return;
    };

    let plugin_dir = config_dir.join("plugin");
    if std::fs::create_dir_all(&plugin_dir).is_err() {
        eprintln!("[codemux::hooks] Failed to create OpenCode plugin dir");
        return;
    }

    let plugin_path = plugin_dir.join("codemux-notify.js");
    let content = build_opencode_plugin(&script_path);
    let _ = std::fs::write(&plugin_path, content);
}

/// No-op on Windows — the OpenCode plugin shells out to a POSIX script.
#[cfg(target_os = "windows")]
pub fn register_opencode_plugin() {}

/// Pi notify extension. Like OpenCode, Pi has no hook-config file — it
/// auto-discovers TypeScript extensions in `~/.pi/agent/extensions/` at
/// session start. This extension subscribes to Pi's agent lifecycle
/// events and shells out to `notify.sh`.
///
/// Event names verified against the installed `@mariozechner/pi-coding-agent`
/// `ExtensionAPI`: `before_agent_start`, `tool_execution_end`, `agent_end`,
/// `session_shutdown` (note: there is no `session_end` event in this API,
/// despite what older integrations assumed). Pi has no distinct
/// permission/approval event, so Pi gets the amber/green working cadence
/// but not the red needs-input pulse.
///
/// `{{NOTIFY_PATH}}` is substituted with the absolute `notify.sh` path.
/// The extension no-ops unless `CODEMUX_SESSION_ID` is set.
#[cfg(not(target_os = "windows"))]
const PI_EXTENSION_TEMPLATE: &str = r#"// Codemux Pi notify extension
// Installed by Codemux — bridges Pi's agent lifecycle to the Codemux
// agent status indicator (the working/idle dot). Pi auto-discovers
// extensions in ~/.pi/agent/extensions/ at session start, so no
// registration step is needed. Safe to delete: Codemux rewrites it.

import { spawn } from "node:child_process";

const NOTIFY_PATH = "{{NOTIFY_PATH}}";

export default function (pi: any) {
  // Only active inside a Codemux-managed terminal.
  if (!process.env.CODEMUX_SESSION_ID) return;

  // Fire-and-forget: pipe the event name as JSON on stdin — notify.sh
  // recovers it from hook_event_name when no arg is passed.
  const fire = (eventName: string) => {
    try {
      const child = spawn(NOTIFY_PATH, [], {
        stdio: ["pipe", "ignore", "ignore"],
        detached: true,
      });
      child.on("error", () => {});
      child.stdin?.on("error", () => {});
      child.stdin?.end(JSON.stringify({ hook_event_name: eventName }));
      child.unref();
    } catch {
      // spawn() can throw synchronously (EACCES / ENOENT) — stay silent.
    }
  };

  // Skip non-interactive sessions (print / RPC / subagent): those have
  // ctx.hasUI === false and must not drive the host status dot. The
  // `=== false` check keeps pre-hasUI Pi versions firing.
  const skip = (ctx: any) => ctx && ctx.hasUI === false;

  pi.on("before_agent_start", (_e: any, ctx: any) => {
    if (skip(ctx)) return;
    fire("UserPromptSubmit");
  });
  pi.on("tool_execution_end", (_e: any, ctx: any) => {
    if (skip(ctx)) return;
    fire("PostToolUse");
  });
  pi.on("agent_end", (_e: any, ctx: any) => {
    if (skip(ctx)) return;
    fire("Stop");
  });
  // Fires on Ctrl+C, /quit, /reload — make sure the dot doesn't get
  // stuck "working" if Pi exits mid-run.
  pi.on("session_shutdown", (_e: any, ctx: any) => {
    if (skip(ctx)) return;
    fire("Stop");
  });
}
"#;

/// Render the Pi extension TypeScript with the notify script path inlined.
#[cfg(not(target_os = "windows"))]
fn build_pi_extension(notify_script_path: &str) -> String {
    PI_EXTENSION_TEMPLATE.replace("{{NOTIFY_PATH}}", notify_script_path)
}

/// Install the Pi notify extension into `~/.pi/agent/extensions/`.
///
/// Only runs if `~/.pi` already exists — i.e. the user actually has Pi.
/// Unix-only (the extension shells out to `notify.sh`).
#[cfg(not(target_os = "windows"))]
pub fn register_pi_extension() {
    let Some(home) = user_home_dir() else { return };

    let pi_dir = std::path::PathBuf::from(&home).join(".pi");
    if !pi_dir.exists() {
        return; // Pi not installed — don't create stray config.
    }

    let Some(script_path) = ensure_hook_script() else {
        eprintln!("[codemux::hooks] Failed to create hook script");
        return;
    };

    let ext_dir = pi_dir.join("agent").join("extensions");
    if std::fs::create_dir_all(&ext_dir).is_err() {
        eprintln!("[codemux::hooks] Failed to create Pi extensions dir");
        return;
    }

    let ext_path = ext_dir.join("codemux-notify.ts");
    let content = build_pi_extension(&script_path);
    let _ = std::fs::write(&ext_path, content);
}

/// No-op on Windows — the Pi extension shells out to a POSIX script.
#[cfg(target_os = "windows")]
pub fn register_pi_extension() {}

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

/// Build the nested hooks JSON for a given event set, without touching
/// the filesystem. Useful for asserting the on-disk format in tests.
pub fn build_nested_hooks_json(
    script_path: &str,
    hook_events: &[(&str, &str)],
) -> serde_json::Value {
    let mut hooks = serde_json::json!({});
    for (event_name, event_type) in hook_events {
        let hook_cmd = build_hook_command(script_path, event_type);
        hooks[event_name] = serde_json::json!([{
            "matcher": "",
            "hooks": [{ "type": "command", "command": hook_cmd }]
        }]);
    }
    hooks
}

/// Build the hooks JSON that would be written to ~/.claude/settings.json.
pub fn build_claude_hooks_json(script_path: &str) -> serde_json::Value {
    build_nested_hooks_json(script_path, &CLAUDE_HOOK_EVENTS)
}

/// Build the hooks JSON that would be written to ~/.codex/hooks.json.
pub fn build_codex_hooks_json(script_path: &str) -> serde_json::Value {
    build_nested_hooks_json(script_path, &CODEX_HOOK_EVENTS)
}

/// Build the hooks JSON that would be written to ~/.gemini/settings.json.
#[cfg(not(target_os = "windows"))]
pub fn build_gemini_hooks_json(script_path: &str) -> serde_json::Value {
    build_nested_hooks_json(script_path, &GEMINI_HOOK_EVENTS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_event_type_covers_all_agent_vocabularies() {
        // Claude Code
        assert_eq!(map_event_type("UserPromptSubmit"), Some(PaneStatus::Working));
        assert_eq!(map_event_type("PostToolUse"), Some(PaneStatus::Working));
        assert_eq!(map_event_type("Stop"), Some(PaneStatus::Review));
        assert_eq!(map_event_type("SessionEnd"), Some(PaneStatus::Idle));
        assert_eq!(
            map_event_type("PermissionRequest"),
            Some(PaneStatus::Permission)
        );
        assert_eq!(map_event_type("Notification"), Some(PaneStatus::Permission));

        // Codex
        assert_eq!(map_event_type("task_started"), Some(PaneStatus::Working));
        assert_eq!(map_event_type("task_complete"), Some(PaneStatus::Review));
        assert_eq!(map_event_type("session_end"), Some(PaneStatus::Idle));
        assert_eq!(
            map_event_type("exec_approval_request"),
            Some(PaneStatus::Permission)
        );
        assert_eq!(
            map_event_type("apply_patch_approval_request"),
            Some(PaneStatus::Permission)
        );
        assert_eq!(
            map_event_type("request_user_input"),
            Some(PaneStatus::Permission)
        );

        // Gemini
        assert_eq!(map_event_type("BeforeAgent"), Some(PaneStatus::Working));
        assert_eq!(map_event_type("AfterTool"), Some(PaneStatus::Working));
        assert_eq!(map_event_type("AfterAgent"), Some(PaneStatus::Review));

        // OpenCode plugin pre-normalizes to these.
        assert_eq!(map_event_type("Start"), Some(PaneStatus::Working));
        assert_eq!(map_event_type("permission.ask"), Some(PaneStatus::Permission));

        // Unknown / empty → no status change.
        assert_eq!(map_event_type("totally-made-up"), None);
        assert_eq!(map_event_type(""), None);
    }

    #[test]
    fn codex_hooks_json_uses_bare_command() {
        // Verified against the Codex 0.130 HookEventNameWire enum + the
        // ~/.codex/hooks.json format Codex itself writes: every event
        // registers the *bare* notify.sh path (no event arg) and Codex
        // pipes the event name on stdin.
        let hooks = build_codex_hooks_json("/home/test/.codemux/hooks/notify.sh");

        // SessionEnd is NOT a Codex hook event — must not be registered.
        assert!(
            hooks.get("SessionEnd").is_none(),
            "SessionEnd is not in Codex's HookEventNameWire enum"
        );
        // PermissionRequest IS a Codex hook event — Codex gets the red pulse.
        assert!(
            hooks.get("PermissionRequest").is_some(),
            "PermissionRequest must be registered for Codex"
        );

        // The bare command is the script path with no event-type arg. On
        // Windows it's wrapped in the `powershell ... -File` invocation, so
        // compare against `build_hook_command` rather than the literal path.
        let bare = build_hook_command("/home/test/.codemux/hooks/notify.sh", "");
        for (event_name, _) in CODEX_HOOK_EVENTS {
            let cmd = hooks[event_name][0]["hooks"][0]["command"]
                .as_str()
                .unwrap_or_else(|| panic!("{event_name} must have a command string"));
            assert_eq!(
                cmd, bare,
                "{event_name} must register the bare notify.sh path (no event arg)"
            );
        }
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn gemini_hooks_json_uses_nested_format() {
        let hooks = build_gemini_hooks_json("/home/test/.codemux/hooks/gemini-notify.sh");

        for (event_name, event_type) in GEMINI_HOOK_EVENTS {
            let cmd = hooks[event_name][0]["hooks"][0]["command"]
                .as_str()
                .unwrap_or_else(|| panic!("{event_name} must have a command string"));
            assert!(
                cmd.contains(".codemux/hooks/gemini-notify.sh"),
                "{event_name} command must reference the Gemini hook script"
            );
            assert!(
                cmd.contains(event_type),
                "{event_name} command must pass '{event_type}' as the event arg"
            );
        }
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn gemini_hook_script_emits_json_before_work() {
        // The Gemini CLI blocks on the hook until it gets JSON on stdout —
        // the `printf '{}'` must come before any network call.
        let printf_idx = GEMINI_HOOK_SCRIPT
            .find("printf '{}")
            .expect("Gemini hook must emit {} on stdout");
        let curl_idx = GEMINI_HOOK_SCRIPT
            .find("curl")
            .expect("Gemini hook must dispatch via curl");
        assert!(
            printf_idx < curl_idx,
            "Gemini hook must print {{}} before the curl dispatch"
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn opencode_plugin_inlines_notify_path_and_guards_env() {
        let plugin = build_opencode_plugin("/home/test/.codemux/hooks/notify.sh");
        assert!(
            plugin.contains("/home/test/.codemux/hooks/notify.sh"),
            "plugin must inline the notify script path"
        );
        assert!(
            !plugin.contains("{{NOTIFY_PATH}}"),
            "plugin must not leave the placeholder unsubstituted"
        );
        assert!(
            plugin.contains("CODEMUX_SESSION_ID"),
            "plugin must no-op unless running inside a Codemux terminal"
        );
        // Normalizes to the canonical vocabulary map_event_type understands.
        for token in ["\"Start\"", "\"Stop\"", "\"PermissionRequest\""] {
            assert!(
                plugin.contains(token),
                "plugin must emit the {token} canonical event"
            );
        }
        // session.busy is not a real OpenCode event — must not be referenced.
        assert!(
            !plugin.contains("session.busy"),
            "plugin must not reference the non-existent session.busy event"
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn pi_extension_inlines_notify_path_and_uses_verified_events() {
        let ext = build_pi_extension("/home/test/.codemux/hooks/notify.sh");
        assert!(
            ext.contains("/home/test/.codemux/hooks/notify.sh"),
            "extension must inline the notify script path"
        );
        assert!(
            !ext.contains("{{NOTIFY_PATH}}"),
            "extension must not leave the placeholder unsubstituted"
        );
        assert!(
            ext.contains("CODEMUX_SESSION_ID"),
            "extension must no-op unless running inside a Codemux terminal"
        );
        // Verified @mariozechner/pi-coding-agent ExtensionAPI events.
        for ev in [
            "before_agent_start",
            "tool_execution_end",
            "agent_end",
            "session_shutdown",
        ] {
            assert!(ext.contains(ev), "extension must subscribe to {ev}");
        }
        // `session_end` is NOT a real Pi event (older integrations assumed it).
        assert!(
            !ext.contains("\"session_end\""),
            "extension must not subscribe to the non-existent session_end event"
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn notify_script_recovers_event_from_stdin_when_no_arg() {
        // Codex registers a bare command and the Pi extension pipes only
        // JSON — both rely on notify.sh reading hook_event_name from stdin.
        assert!(
            HOOK_SCRIPT.contains("hook_event_name"),
            "notify.sh must fall back to hook_event_name from the JSON payload"
        );
        assert!(
            HOOK_SCRIPT.contains("EVENT_TYPE=\"${1:-}\""),
            "notify.sh must still accept the event type as the first arg"
        );
    }

    #[test]
    fn active_hook_port_file_tracks_the_current_listener() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_active_hook_port_in(dir.path(), 43123).unwrap();

        assert_eq!(path, dir.path().join(ACTIVE_HOOK_PORT_FILE));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "43123\n");
    }

    #[test]
    fn every_notifier_can_retry_the_published_active_port() {
        #[cfg(not(target_os = "windows"))]
        {
            assert!(HOOK_SCRIPT.contains("$SCRIPT_DIR/active-port"));
            assert!(GEMINI_HOOK_SCRIPT.contains("$SCRIPT_DIR/active-port"));
        }
        #[cfg(target_os = "windows")]
        assert!(HOOK_SCRIPT_PS1.contains("Join-Path $PSScriptRoot 'active-port'"));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn notify_script_retries_the_current_port_after_a_stale_inherited_port() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let script_path = dir.path().join("notify.sh");
        std::fs::write(&script_path, HOOK_SCRIPT).unwrap();
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        write_active_hook_port_in(dir.path(), 43210).unwrap();

        // Replace curl with a deterministic probe: the inherited port fails,
        // the active-port retry succeeds, and both attempted URLs are logged.
        let bin_dir = dir.path().join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let fake_curl = bin_dir.join("curl");
        std::fs::write(
            &fake_curl,
            r#"#!/bin/sh
for ARG in "$@"; do
  case "$ARG" in http://*) URL="$ARG" ;; esac
done
printf '%s\n' "$URL" >> "$HOOK_TEST_LOG"
case "$URL" in *:43199/*) exit 7 ;; *) exit 0 ;; esac
"#,
        )
        .unwrap();
        std::fs::set_permissions(&fake_curl, std::fs::Permissions::from_mode(0o755)).unwrap();
        let log_path = dir.path().join("curl.log");

        let status = std::process::Command::new(&script_path)
            .arg("UserPromptSubmit")
            .env("CODEMUX_HOOK_PORT", "43199")
            .env("CODEMUX_SESSION_ID", "session-resumed")
            .env("HOOK_TEST_LOG", &log_path)
            .env("PATH", format!("{}:/usr/bin:/bin", bin_dir.display()))
            .stdin(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let log = loop {
            let log = std::fs::read_to_string(&log_path).unwrap_or_default();
            if log.lines().count() >= 2 || std::time::Instant::now() >= deadline {
                break log;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let attempts: Vec<_> = log.lines().collect();
        assert_eq!(
            attempts.len(),
            2,
            "expected inherited + active-port attempts: {log}"
        );
        assert!(attempts[0].contains(":43199/hook?"));
        assert!(attempts[1].contains(":43210/hook?"));
        assert!(attempts[1].contains("sessionId=session-resumed"));
        assert!(attempts[1].contains("eventType=UserPromptSubmit"));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn notify_script_drops_claude_idle_notification() {
        // Claude Code fires `Notification` both for permission prompts and
        // for a 60s idle reminder. notify.sh must inspect the payload's
        // `message` and forward only the permission case — otherwise a
        // finished agent's green dot flips to a red dot that never clears.
        assert!(
            HOOK_SCRIPT.contains("$EVENT_TYPE\" = \"Notification\""),
            "notify.sh must special-case the Notification event"
        );
        assert!(
            HOOK_SCRIPT.contains(".message"),
            "notify.sh must read the Notification payload's message field"
        );
        assert!(
            HOOK_SCRIPT.contains("*permission*"),
            "notify.sh must forward Notification only for permission/approval messages"
        );

        // The message guard must run before the curl dispatch so the idle
        // reminder is dropped, not merely classified after the fact.
        let guard_idx = HOOK_SCRIPT
            .find("*permission*")
            .expect("notify.sh must guard the Notification event");
        let curl_idx = HOOK_SCRIPT
            .find("curl")
            .expect("notify.sh must dispatch via curl");
        assert!(
            guard_idx < curl_idx,
            "the Notification message guard must run before the curl dispatch"
        );
    }

    #[test]
    fn hook_json_matches_claude_code_format() {
        let hooks = build_claude_hooks_json("/home/test/.codemux/hooks/notify.sh");

        // Each event key must be an array
        for event in [
            "UserPromptSubmit",
            "Stop",
            "PermissionRequest",
            "Notification",
            "PostToolUse",
        ] {
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

        // Simulate existing settings with an external hook
        let mut settings = serde_json::json!({
            "effortLevel": "high",
            "hooks": {
                "UserPromptSubmit": [{
                    "matcher": "*",
                    "hooks": [{"type": "command", "command": "external-notify.sh"}]
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

        // Verify: external hook still present in UserPromptSubmit
        let ups = settings["hooks"]["UserPromptSubmit"].as_array().unwrap();
        assert_eq!(ups.len(), 2, "should have external + codemux hooks");
        assert!(ups[0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("external"));

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
        // Simulate settings with mixed external + Codemux hooks
        let mut settings = serde_json::json!({
            "effortLevel": "high",
            "hooks": {
                "UserPromptSubmit": [
                    {
                        "matcher": "*",
                        "hooks": [{"type": "command", "command": "external-notify.sh"}]
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
                        "hooks": [{"type": "command", "command": "external-notify.sh"}]
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

        // UserPromptSubmit: external entry remains, codemux entry removed
        let ups = hooks["UserPromptSubmit"].as_array().unwrap();
        assert_eq!(ups.len(), 1);
        assert!(ups[0]["hooks"][0]["command"].as_str().unwrap().contains("external"));

        // Stop: was codemux-only, should be removed entirely
        assert!(!hooks.contains_key("Stop"), "Stop should be removed (was codemux-only)");

        // PostToolUse: external-only, untouched
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
            "hooks": [{"type": "command", "command": "external-notify.sh"}]
        });
        assert!(!entry_contains_codemux_hook(&other));
    }
}
