use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tauri::{AppHandle, Manager, Runtime};

use crate::state::{self, AppStateStore, PaneStatus};

static HOOK_PORT: OnceLock<u16> = OnceLock::new();
static MONITOR_SESSIONS: OnceLock<Mutex<HashMap<String, Arc<AtomicBool>>>> = OnceLock::new();

const ACTIVE_HOOK_PORT_FILE: &str = "active-port";

pub fn hook_port() -> Option<u16> {
    HOOK_PORT.get().copied()
}

/// Start the agent hook notification server on a random localhost port.
/// Returns the allocated port number.
pub fn start_hook_server<R: Runtime>(app: AppHandle<R>) -> u16 {
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
                    let _ =
                        stream.write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n");
                    return;
                }

                // Always store the agent session ID — it reflects the CURRENT
                // session running in this pane.
                if !agent_session_id.is_empty() {
                    let state: tauri::State<'_, AppStateStore> = app.state();
                    // `handle_lifecycle_event` below emits, but only for event
                    // types it recognises — an unrecognised one returns early,
                    // and this write would then never reach the renderer.
                    if state.set_terminal_adapter_capture(
                        session_id,
                        "claude_session_id",
                        agent_session_id,
                    ) {
                        crate::state::schedule_emit_app_state(&app);
                    }
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
        "Stop" | "agent-turn-complete" | "AfterAgent" | "task_complete" => Some(PaneStatus::Review),
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

fn handle_lifecycle_event<R: Runtime>(app: &AppHandle<R>, session_id: &str, status: PaneStatus) {
    let state: tauri::State<'_, AppStateStore> = app.state();

    // For Stop events, check if the pane is in the active workspace+tab — if so, go idle
    let snapshot = state.snapshot();
    let is_active = if status == PaneStatus::Review {
        is_pane_active_for_session(&snapshot, session_id)
    } else {
        false
    };
    crate::web_remote::push::agent_status(app, session_id, status.clone());
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

    // Notify on agent completion. `notify_agent` skips it when the user can
    // already see the pane, or when the workspace is muted (right-click → Mute
    // notifications) — useful when a pane runs a process that spawns agent
    // subprocesses of its own, whose lifecycle hooks would otherwise pop
    // notifications for this pane. Terminal agents notify only on Review: in
    // an approve-each-tool session their hooks alternate Permission
    // (`PermissionRequest` / `Notification`) with Working (`PostToolUse`), so
    // a transition-based rule would notify once per approved tool call. Chat
    // agents report approvals precisely and notify then.
    if status == PaneStatus::Review {
        if let Some(target) = state.notification_target_for_session(session_id) {
            crate::notifications::notify_agent(
                app,
                crate::notifications::AgentNotice::Finished,
                &target,
            );
        }
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

/// Check if the pane for a session is in the currently active workspace.
fn is_pane_active_for_session(snapshot: &state::AppStateSnapshot, session_id: &str) -> bool {
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

fn find_session_in_node(node: &state::PaneNodeSnapshot, target_session_id: &str) -> bool {
    match node {
        state::PaneNodeSnapshot::Terminal { session_id, .. } => session_id.0 == target_session_id,
        state::PaneNodeSnapshot::Split { children, .. } => children
            .iter()
            .any(|c| find_session_in_node(c, target_session_id)),
        state::PaneNodeSnapshot::Browser { .. } | state::PaneNodeSnapshot::AgentChat { .. } => {
            false
        }
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
    matches!(
        crate::terminal::parse_stat_pgrp_tpgid(&stat),
        Some((pgrp, tpgid)) if tpgid == pgrp
    )
}

#[cfg(not(target_os = "linux"))]
fn shell_is_foreground(_shell_pid: u32) -> bool {
    false
}

/// Start a background thread that monitors when an agent exits so that stuck
/// Working/Permission status indicators can be cleared.
fn start_agent_exit_monitor<R: Runtime>(app: AppHandle<R>, session_id: String, shell_pid: u32) {
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
            let still_active = matches!(
                state.pane_status_for_session(&session_id),
                Some(PaneStatus::Working | PaneStatus::Permission)
            );

            if !still_active {
                break; // Status already cleared by a hook or terminal exit
            }

            // Check if the shell is the foreground process (agent has exited)
            if shell_is_foreground(shell_pid) {
                // Clearing the stuck status is this path's entire mutation, so
                // it ships as a `PaneStatus` delta instead of a full snapshot.
                if let Some(delta) = state.clear_transient_pane_status_by_session_delta(&session_id)
                {
                    state::emit_app_state_delta(&app, delta);
                }
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
    write_hook_artifact(&path, &format!("{port}\n"), false)?;
    Some(path)
}

/// A snapshot pins a symlink's target once and detects intervening edits before
/// replacing it. The final comparison is best-effort, not a filesystem CAS.
struct HookFile {
    path: PathBuf,
    original: Option<Vec<u8>>,
    permissions: Option<std::fs::Permissions>,
}

impl HookFile {
    fn read(path: &Path) -> std::io::Result<Self> {
        let path = match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_symlink() => std::fs::canonicalize(path)?,
            Ok(_) => path.to_path_buf(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => path.to_path_buf(),
            Err(error) => return Err(error),
        };
        let original = read_optional(&path)?;
        let permissions = if original.is_some() {
            Some(std::fs::metadata(&path)?.permissions())
        } else {
            None
        };
        Ok(Self {
            path,
            original,
            permissions,
        })
    }

    fn replace(&self, content: &[u8], executable: bool) -> std::io::Result<()> {
        if self.original.as_deref() == Some(content) {
            return Ok(());
        }
        if self
            .permissions
            .as_ref()
            .is_some_and(|mode| mode.readonly())
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "hook file is read-only; skipping",
            ));
        }
        let parent = self.path.parent().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "hook file has no parent")
        })?;
        std::fs::create_dir_all(parent)?;
        let mut builder = tempfile::Builder::new();
        #[cfg(unix)]
        if self.permissions.is_none() {
            use std::os::unix::fs::PermissionsExt;
            // Let creation apply the user's umask, just like a normal file write.
            builder.permissions(std::fs::Permissions::from_mode(if executable {
                0o755
            } else {
                0o666
            }));
        }
        let mut staged = builder.tempfile_in(parent)?;
        staged.write_all(content)?;
        if let Some(permissions) = &self.permissions {
            staged.as_file().set_permissions(permissions.clone())?;
        }
        #[cfg(not(unix))]
        let _ = executable;
        staged.as_file().sync_all()?;
        #[cfg(windows)]
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        loop {
            if read_optional(&self.path)? != self.original {
                return Err(std::io::Error::other(
                    "hook file changed during update; skipping",
                ));
            }
            match staged.persist(&self.path) {
                Ok(_) => return Ok(()),
                Err(error) => {
                    #[cfg(windows)]
                    if matches!(error.error.raw_os_error(), Some(5 | 32))
                        && std::time::Instant::now() < deadline
                    {
                        // Readers and scanners can briefly block replacement on Windows.
                        // Keep the staged file and recheck for edits before each retry.
                        staged = error.file;
                        std::thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    return Err(error.error);
                }
            }
        }
    }
}

fn read_optional(path: &Path) -> std::io::Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn write_hook_artifact(path: &Path, body: &str, executable: bool) -> Option<()> {
    match HookFile::read(path).and_then(|file| file.replace(body.as_bytes(), executable)) {
        Ok(()) => Some(()),
        Err(error) => {
            eprintln!(
                "[codemux::hooks] Failed to update {}: {error}",
                path.display()
            );
            None
        }
    }
}

/// Write a shared notifier only when its content changes, keeping existing mode.
fn write_hook_file(name: &str, body: &str, executable: bool) -> Option<String> {
    let home = user_home_dir()?;
    let path = PathBuf::from(home).join(".codemux/hooks").join(name);
    write_hook_artifact(&path, body, executable)?;
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

fn is_codemux_hook(hook: &serde_json::Value, generated_command: &str) -> bool {
    if hook.get("type").is_some_and(|kind| kind != "command") {
        return false;
    }
    let Some(command) = hook.get("command").and_then(|command| command.as_str()) else {
        return false;
    };
    // Our current generated command is authoritative even when HOME contains
    // shell metacharacters. The heuristic below is only for older invocations.
    if command == generated_command {
        return true;
    }
    // Match generated invocations, not user commands that merely mention the
    // script or combine it with other work in the same shell command.
    let command = command
        .strip_prefix("powershell -NoProfile -ExecutionPolicy Bypass -File \"")
        .unwrap_or(command);
    if !(command.starts_with('/')
        || command.starts_with(r"\\")
        || command.as_bytes().get(1) == Some(&b':'))
        || command.contains([';', '|', '&', '<', '>', '\n'])
    {
        return false;
    }
    let normalized = command.replace('\\', "/");
    let Some((_, suffix)) = normalized.rsplit_once(".codemux/hooks/") else {
        return false;
    };
    ["notify.sh", "notify.ps1", "gemini-notify.sh"]
        .iter()
        .any(|name| {
            let Some(tail) = suffix.strip_prefix(name) else {
                return false;
            };
            let tail = tail.strip_prefix('"').unwrap_or(tail);
            tail.is_empty()
                || tail.strip_prefix(' ').is_some_and(|event| {
                    !event.is_empty() && event.chars().all(|ch| ch.is_ascii_alphabetic())
                })
        })
}

/// Remove only our commands from a mixed entry, keeping its matcher and options.
fn remove_codemux_hooks(entry: &mut serde_json::Value, generated_command: &str) -> bool {
    if let Some(hooks) = entry
        .get_mut("hooks")
        .and_then(|hooks| hooks.as_array_mut())
    {
        let had_codemux = hooks
            .iter()
            .any(|hook| is_codemux_hook(hook, generated_command));
        hooks.retain(|hook| !is_codemux_hook(hook, generated_command));
        return had_codemux
            && hooks.is_empty()
            && entry
                .as_object()
                .is_some_and(|object| object.keys().all(|key| key == "hooks" || key == "matcher"));
    }
    is_codemux_hook(entry, generated_command)
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
/// preserved. Only recognized Codemux commands are refreshed, so mixed user
/// entries survive and repeated startups never accumulate duplicates. `hook_events` maps each
/// agent's event name to the canonical event-type arg passed to the
/// notify script.
fn merge_nested_hooks_file(
    settings_path: &std::path::Path,
    hook_events: &[(&str, &str)],
    script_path: &str,
) {
    if let Err(error) = try_merge_nested_hooks_file(settings_path, hook_events, script_path) {
        eprintln!(
            "[codemux::hooks] Skipping hook merge for {}: {error}",
            settings_path.display()
        );
    }
}

fn try_merge_nested_hooks_file(
    settings_path: &Path,
    hook_events: &[(&str, &str)],
    script_path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let file = HookFile::read(settings_path)?;
    let mut settings: serde_json::Value = match &file.original {
        Some(content) => serde_json::from_slice(content)?,
        None => serde_json::json!({}),
    };
    let original = settings.clone();
    let object = settings
        .as_object_mut()
        .ok_or("settings root must be an object")?;
    let hooks = object
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));
    let hooks = hooks.as_object_mut().ok_or("hooks must be an object")?;
    // Validate only the events we mutate; leave unrelated event shapes alone.
    for (event_name, _) in hook_events {
        let Some(entries) = hooks.get(*event_name) else {
            continue;
        };
        let entries = entries.as_array().ok_or("hook events must be arrays")?;
        for entry in entries {
            let entry = entry.as_object().ok_or("hook entries must be objects")?;
            if let Some(nested) = entry.get("hooks") {
                let nested = nested.as_array().ok_or("entry hooks must be an array")?;
                if nested.iter().any(|hook| !hook.is_object()) {
                    return Err("nested hook entries must be objects".into());
                }
            }
        }
    }
    for (event_name, event_type) in hook_events {
        let entries = hooks
            .entry(*event_name)
            .or_insert_with(|| serde_json::json!([]));
        let entries = entries.as_array_mut().ok_or("hook events must be arrays")?;
        let generated_command = build_hook_command(script_path, event_type);
        let desired = serde_json::json!({
            "matcher": "",
            "hooks": [{ "type": "command", "command": generated_command }]
        });
        let existing_commands = entries
            .iter()
            .map(|entry| {
                entry
                    .get("hooks")
                    .and_then(|hooks| hooks.as_array())
                    .map(|hooks| {
                        hooks
                            .iter()
                            .filter(|hook| is_codemux_hook(hook, &generated_command))
                            .count()
                    })
                    .unwrap_or_else(|| usize::from(is_codemux_hook(entry, &generated_command)))
            })
            .sum::<usize>();
        if existing_commands == 1 && entries.contains(&desired) {
            continue;
        }
        entries.retain_mut(|entry| !remove_codemux_hooks(entry, &generated_command));
        entries.push(desired);
    }
    // Preserve original bytes (including formatting) for semantic no-ops.
    if settings != original {
        file.replace(&serde_json::to_vec_pretty(&settings)?, false)?;
    }
    Ok(())
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

/// JS shared by both OpenCode plugins: reports a canonical event straight to
/// the hook server.
///
/// Reports are chained and each one is awaited: the hook server only answers
/// after it has applied the status, so a quick `Start` → `Stop` can never
/// land reversed (detached `notify.sh` runs give no such ordering). Like
/// `notify.sh`, it tries the inherited port first, then the port the running
/// app published — persistent PTYs keep a stale `CODEMUX_HOOK_PORT` across
/// app restarts.
///
/// `{{ACTIVE_PORT_PATH}}` is substituted with a JSON string literal.
#[cfg(not(target_os = "windows"))]
const OPENCODE_TRANSPORT_JS: &str = r#"import { readFile } from "node:fs/promises";

const ACTIVE_PORT_PATH = {{ACTIVE_PORT_PATH}};

let reports = Promise.resolve();

const postHook = async (port, eventType) => {
  if (!port) return false;
  const sessionId = encodeURIComponent(process.env.CODEMUX_SESSION_ID ?? "");
  try {
    const response = await fetch(
      `http://127.0.0.1:${port}/hook?sessionId=${sessionId}&eventType=${eventType}`,
      { signal: AbortSignal.timeout(2000) },
    );
    return response.ok;
  } catch {
    return false;
  }
};

const notify = (eventType) => {
  reports = reports.then(async () => {
    const inherited = process.env.CODEMUX_HOOK_PORT;
    if (await postHook(inherited, eventType)) return;
    let active = "";
    try {
      active = (await readFile(ACTIVE_PORT_PATH, "utf8")).trim();
    } catch {}
    // Best-effort — never break the agent over a status ping.
    if (active && active !== inherited) await postHook(active, eventType);
  });
};
"#;

/// OpenCode 1.x notify plugin. OpenCode has no hook-config file like Claude
/// or Codex — instead it loads JS plugins from its plugin directory, inside
/// the terminal's own process tree. This plugin watches `session.status`
/// busy/idle transitions plus permission and question requests, normalizing
/// to the canonical `Start` / `Stop` / `PermissionRequest` vocabulary.
///
/// This historical template is kept verbatim to recognize our generated legacy
/// artifact during the 2.x migration. Contrary to its old inline comment,
/// OpenCode 2 also discovers the singular path and rejects its export.
/// Event shapes verified against OpenCode 1.19.0: `permission.asked` /
/// `question.asked` carry `id`, their replies carry `requestID`. The
/// `permission.ask` hook is kept for older 1.x releases; 1.19 never calls it.
///
/// The plugin is a no-op unless `CODEMUX_SESSION_ID` is in the environment,
/// so a user running OpenCode outside Codemux is unaffected.
#[cfg(not(target_os = "windows"))]
const OPENCODE_PLUGIN_TEMPLATE: &str = r#"// Codemux OpenCode notify plugin (OpenCode 1.x)
// Installed by Codemux — bridges OpenCode session lifecycle to the
// Codemux agent status indicator (the working/idle/needs-input dot).
// OpenCode 2 ignores this file and loads plugins/codemux/tui.js instead.
// Safe to delete: Codemux rewrites it on startup.

{{TRANSPORT}}
export const CodemuxNotifyPlugin = async () => {
  // Only active inside a Codemux-managed terminal.
  if (!process?.env?.CODEMUX_SESSION_ID) return {};

  let currentState = "idle"; // "idle" | "busy"
  let rootSessionID = null;
  let stopSent = false;
  // Open permission/question requests. Any session's request blocks the
  // run, including a subagent's.
  const pending = new Set();

  const isChildSession = (event) =>
    Boolean(event?.properties?.info?.parentID);

  const handleBusy = (sessionID) => {
    if (!rootSessionID) rootSessionID = sessionID;
    if (sessionID !== rootSessionID) return;
    if (currentState === "idle") {
      currentState = "busy";
      stopSent = false;
      notify("Start");
    }
  };

  const handleStop = (sessionID) => {
    if (rootSessionID && sessionID !== rootSessionID) return;
    if (currentState === "busy" && !stopSent) {
      currentState = "idle";
      stopSent = true;
      rootSessionID = null;
      pending.clear();
      notify("Stop");
    }
  };

  const handleAsked = (requestID) => {
    pending.add(requestID ?? "unknown");
    notify("PermissionRequest");
  };

  // Answering the last open request hands the dot back to the run.
  const handleReplied = (requestID) => {
    pending.delete(requestID);
    if (pending.size === 0 && currentState === "busy") notify("Start");
  };

  return {
    event: async ({ event }) => {
      const properties = event?.properties;
      switch (event?.type) {
        case "permission.asked":
        case "question.asked":
          handleAsked(properties?.id);
          return;
        case "permission.replied":
        case "question.replied":
        case "question.rejected":
          handleReplied(properties?.requestID);
          return;
      }

      // Ignore subagent/child sessions so background spawns don't flip the dot.
      if (isChildSession(event)) return;
      const sessionID = properties?.sessionID ?? properties?.info?.id ?? null;

      // Verified against @opencode-ai/sdk: session.status carries
      // { properties: { sessionID, status: { type: "idle"|"busy"|"retry" } } };
      // session.idle / session.error carry { properties: { sessionID } }.
      if (event.type === "session.status") {
        const status = properties?.status?.type;
        if (status === "busy") handleBusy(sessionID);
        else if (status === "idle") handleStop(sessionID);
      }
      if (event.type === "session.idle") handleStop(sessionID);
      if (event.type === "session.error") handleStop(sessionID);
    },
    "permission.ask": async () => {
      notify("PermissionRequest");
    },
  };
};
"#;

/// OpenCode 2.x terminal plugin.
///
/// OpenCode 2 runs server plugins inside a shared background service
/// (`opencode serve --service`) that every terminal connects to. That
/// process never carries a pane's `CODEMUX_SESSION_ID`, and it no longer
/// loads 1.x-style plugins at all, so 2.x status has to be reported from
/// the terminal side. OpenCode 2 loads `plugins/<name>/tui.js` into each
/// terminal's own process, where the pane environment is intact.
///
/// The service streams every session's events to every terminal, so the
/// plugin follows only the root session this terminal shows. Event shapes
/// verified against OpenCode 2.0.29: `session.execution.{started,
/// succeeded,interrupted,failed}` and `permission.replied` carry
/// `sessionID`; `permission.asked` carries `id`, its reply `requestID`;
/// `form.created` carries `form.{id,sessionID}`, its reply/cancel `id`.
///
/// State is re-derived from the terminal's client cache on every tick, not
/// only when the route changes: a session's status and open requests hydrate
/// asynchronously after the route switches, and hydration replays no events.
/// Events still outrank the cache, because a list read already in flight can
/// land after an event and write back an older snapshot.
#[cfg(not(target_os = "windows"))]
const OPENCODE_TUI_PLUGIN_TEMPLATE: &str = r#"// Codemux OpenCode terminal plugin (OpenCode 2.x)
// Installed by Codemux — bridges the session shown in this terminal to the
// Codemux agent status indicator (the working/idle/needs-input dot).
// Safe to delete: Codemux rewrites it on startup.

{{TRANSPORT}}
const SYNC_INTERVAL_MS = 250;
const REQUEST_KINDS = ["permission", "form"];

export default {
  id: "codemux.notify",
  setup(api) {
    // Only active inside a Codemux-managed terminal.
    if (!process.env.CODEMUX_SESSION_ID) return;

    const parents = new Map();
    // The root session this pane last showed. Kept while the user is on the
    // home screen, so a run they navigated away from still reports its end.
    let selected;
    let running = false;
    // Open requests on the selected session family: "<kind>:<id>" → owner.
    let blockers = new Map();
    // The cache seeds the run state only until this terminal sees the run's
    // own lifecycle event: a late list read can still carry an older status.
    let runningFromEvents = false;
    // Request events outrank the cache: an in-flight list read can land
    // after an event with an older snapshot. Asked requests are held until
    // answered or the run ends; answered ids are unique, so they stay
    // settled for as long as this session is selected.
    const heldRequests = new Map();
    const settledRequests = new Set();
    // Last canonical event sent; "idle" until the first one.
    let reported = "idle";

    const root = (id) => {
      const seen = new Set();
      while (typeof id === "string" && !seen.has(id)) {
        seen.add(id);
        const parentID = api.data.session.get(id)?.parentID ?? parents.get(id);
        if (!parentID) return id;
        id = parentID;
      }
      return undefined;
    };

    const shownRoot = () => {
      const route = api.ui.router.current();
      return route?.type === "session" ? root(route.sessionID) : undefined;
    };

    const listRequests = (kind, member) => {
      try {
        return api.data.session[kind].list(member);
      } catch {
        return undefined;
      }
    };

    const changeRequest = (owner, kind, requestID, present) => {
      if (typeof requestID !== "string") return;
      const key = `${kind}:${requestID}`;
      if (!present) {
        settledRequests.add(key);
        heldRequests.delete(key);
      } else if (!settledRequests.has(key)) {
        heldRequests.set(key, owner);
      }
    };

    // Re-derive the selected session's state from the client cache, which
    // also picks up a run or request that was already open when this
    // terminal attached and only hydrates after the route changed.
    const reconcile = () => {
      if (!runningFromEvents) running = api.data.session.status(selected) === "running";

      let family = [];
      try {
        family = api.data.session.family(selected) ?? [];
      } catch {}
      const next = new Map(heldRequests);
      for (const member of new Set([selected, ...family])) {
        if (root(member) !== selected) continue;
        for (const kind of REQUEST_KINDS) {
          for (const item of listRequests(kind, member) ?? []) {
            next.set(`${kind}:${item.id}`, member);
          }
        }
      }
      for (const key of settledRequests) next.delete(key);
      blockers = next;
    };

    const publish = () => {
      if (!selected) return;
      reconcile();
      const next = blockers.size ? "PermissionRequest" : running ? "Start" : "Stop";
      // A pane that has not run anything stays clear instead of reading "done".
      if (next === reported || (next === "Stop" && reported === "idle")) return;
      reported = next;
      notify(next);
    };

    const sync = () => {
      const id = shownRoot();
      if (id && id !== selected) {
        selected = id;
        running = false;
        blockers = new Map();
        runningFromEvents = false;
        heldRequests.clear();
        settledRequests.clear();
      }
      publish();
    };

    const receive = ({ details: event }) => {
      const data = event?.data;
      if (!data) return;
      if (event.type === "session.created" && data.parentID) {
        parents.set(data.sessionID, data.parentID);
      }
      const id = shownRoot();
      if (id && id !== selected) sync();
      if (!selected) return;
      switch (event.type) {
        case "session.execution.started":
          if (data.sessionID !== selected) return;
          running = true;
          runningFromEvents = true;
          break;
        case "session.execution.succeeded":
        case "session.execution.interrupted":
        case "session.execution.failed":
          if (data.sessionID !== selected) return;
          running = false;
          runningFromEvents = true;
          // The run is over; the cache alone decides what is still open.
          heldRequests.clear();
          break;
        case "permission.asked":
          if (root(data.sessionID) !== selected) return;
          changeRequest(data.sessionID, "permission", data.id, true);
          break;
        case "permission.replied":
          if (root(data.sessionID) !== selected) return;
          changeRequest(data.sessionID, "permission", data.requestID, false);
          break;
        case "form.created":
          if (root(data.form?.sessionID) !== selected) return;
          changeRequest(data.form.sessionID, "form", data.form.id, true);
          break;
        case "form.replied":
        case "form.cancelled": {
          const owner = blockers.get(`form:${data.id}`) ?? data.sessionID;
          if (root(owner) !== selected) return;
          changeRequest(owner, "form", data.id, false);
          break;
        }
        default:
          return;
      }
      publish();
    };

    const unsubscribe = api.data.listen(receive);
    sync();
    // Session switches and cache hydration emit no plugin event; poll both.
    const timer = setInterval(sync, SYNC_INTERVAL_MS);
    return () => {
      clearInterval(timer);
      unsubscribe();
    };
  },
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

/// Render an OpenCode plugin template with the shared transport and the
/// absolute `active-port` path inlined.
#[cfg(not(target_os = "windows"))]
fn render_opencode_plugin(template: &str, active_port_path: &str) -> String {
    let path_literal = serde_json::to_string(active_port_path).unwrap_or_else(|_| "\"\"".into());
    template
        .replace("{{TRANSPORT}}", OPENCODE_TRANSPORT_JS)
        .replace("{{ACTIVE_PORT_PATH}}", &path_literal)
}

/// Always install the terminal plugin; only the legacy file needs a version.
/// Run in the background: passive, bounded discovery must not delay PTY restore.
#[cfg(not(target_os = "windows"))]
pub async fn register_opencode_plugin() {
    let Some(config_dir) = opencode_config_dir() else {
        return;
    };
    if !config_dir.exists() {
        return;
    }
    let Some(home) = user_home_dir() else { return };
    let active_port_path = PathBuf::from(home)
        .join(".codemux/hooks")
        .join(ACTIVE_HOOK_PORT_FILE);
    let active_port_path = active_port_path.to_string_lossy();
    if install_opencode_plugin_in(&config_dir, &active_port_path, None).is_none() {
        return;
    }
    let availability = crate::agent_provider::opencode::check_opencode_availability(None).await;
    let major = availability
        .version
        .as_deref()
        .and_then(|version| version.trim_start_matches('v').split('.').next())
        .and_then(|major| major.parse::<u32>().ok());
    install_opencode_plugin_in(&config_dir, &active_port_path, major);
}

#[cfg(not(target_os = "windows"))]
fn install_opencode_plugin_in(
    config_dir: &Path,
    active_port_path: &str,
    major: Option<u32>,
) -> Option<()> {
    let path = config_dir.join("plugins/codemux/tui.js");
    let content = render_opencode_plugin(OPENCODE_TUI_PLUGIN_TEMPLATE, active_port_path);
    write_hook_artifact(&path, &content, false)?;
    let legacy_path = config_dir.join("plugin/codemux-notify.js");
    let legacy = render_opencode_plugin(OPENCODE_PLUGIN_TEMPLATE, active_port_path);
    match major {
        Some(1) => write_hook_artifact(&legacy_path, &legacy, false),
        Some(2) => {
            // Retire only our exact legacy bytes, after confirming the replacement
            // is on disk. Never follow a symlink or remove a customized plugin.
            if std::fs::read(&path).ok().as_deref() == Some(content.as_bytes())
                && std::fs::symlink_metadata(&legacy_path)
                    .ok()
                    .is_some_and(|meta| meta.is_file() && !meta.permissions().readonly())
                && std::fs::read(&legacy_path).ok().as_deref() == Some(legacy.as_bytes())
            {
                if let Err(error) = std::fs::remove_file(&legacy_path) {
                    eprintln!(
                        "[codemux::hooks] Failed to retire {}: {error}",
                        legacy_path.display()
                    );
                }
            }
            Some(())
        }
        _ => Some(()),
    }
}

/// No-op on Windows, like the other agent hook registrations.
#[cfg(target_os = "windows")]
pub async fn register_opencode_plugin() {}

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
    let _ = write_hook_artifact(&ext_path, &content, false);
}

/// No-op on Windows — the Pi extension shells out to a POSIX script.
#[cfg(target_os = "windows")]
pub fn register_pi_extension() {}

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
    fn opencode_plugins_inline_transport_and_guard_env() {
        let path = "/home/test/.codemux/hooks/active-port";
        for template in [OPENCODE_PLUGIN_TEMPLATE, OPENCODE_TUI_PLUGIN_TEMPLATE] {
            let plugin = render_opencode_plugin(template, path);
            assert!(
                plugin.contains(&format!("const ACTIVE_PORT_PATH = \"{path}\";")),
                "plugin must inline the active-port path as a JS string"
            );
            assert!(
                !plugin.contains("{{"),
                "plugin must not leave a placeholder unsubstituted"
            );
            assert!(
                plugin.contains("CODEMUX_SESSION_ID") && plugin.contains("CODEMUX_HOOK_PORT"),
                "plugin must report this pane and no-op outside Codemux"
            );
            // Normalizes to the canonical vocabulary map_event_type understands.
            for token in ["\"Start\"", "\"Stop\"", "\"PermissionRequest\""] {
                assert!(plugin.contains(token), "plugin must emit the {token} canonical event");
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn opencode_plugin_paths_are_json_escaped() {
        let plugin = render_opencode_plugin(OPENCODE_TUI_PLUGIN_TEMPLATE, "/home/o\"brien/x");
        assert!(plugin.contains(r#"const ACTIVE_PORT_PATH = "/home/o\"brien/x";"#));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn opencode_v1_plugin_uses_verified_events() {
        // OpenCode 1.19 never calls the `permission.ask` hook; requests
        // arrive as events, and answering one must hand the dot back.
        for event in [
            "session.status",
            "session.idle",
            "permission.asked",
            "permission.replied",
            "question.asked",
            "question.replied",
            "question.rejected",
        ] {
            assert!(
                OPENCODE_PLUGIN_TEMPLATE.contains(&format!("\"{event}\"")),
                "1.x plugin must handle {event}"
            );
        }
        assert!(
            !OPENCODE_PLUGIN_TEMPLATE.contains("session.busy"),
            "session.busy is not a real OpenCode event"
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn opencode_v2_plugin_is_a_terminal_plugin_on_verified_events() {
        let plugin = OPENCODE_TUI_PLUGIN_TEMPLATE;
        // OpenCode 2 calls `setup` on a default export in the terminal.
        assert!(plugin.contains("export default {") && plugin.contains("setup(api)"));
        for event in [
            "session.execution.started",
            "session.execution.succeeded",
            "session.execution.interrupted",
            "session.execution.failed",
            "permission.asked",
            "permission.replied",
            "form.created",
            "form.replied",
            "form.cancelled",
        ] {
            assert!(plugin.contains(&format!("\"{event}\"")), "2.x plugin must handle {event}");
        }
        // The shared service streams every terminal's sessions; the plugin
        // must scope itself to the session shown in this one.
        assert!(plugin.contains("api.ui.router.current()"));
    }

    /// Drives the rendered 2.x plugin under Node against a fake terminal API
    /// whose cache hydrates only after the route already shows the session,
    /// with events arriving both before the cache catches up and before an
    /// older snapshot lands. Skipped when `node` is not installed.
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn opencode_v2_plugin_follows_late_cache_hydration() {
        const HARNESS: &str = r#"
import http from "node:http";
const reported = [];
const server = http.createServer((req, res) => {
  reported.push(new URL(req.url, "http://hook").searchParams.get("eventType"));
  res.end();
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
process.env.CODEMUX_SESSION_ID = "pane-1";
process.env.CODEMUX_HOOK_PORT = String(server.address().port);
const { default: plugin } = await import(process.argv[2]);

const cache = { status: {}, permission: {}, form: {} };
let emit;
const dispose = plugin.setup({
  ui: { router: { current: () => ({ type: "session", sessionID: "ses_root" }) } },
  data: {
    listen: (handler) => {
      emit = (type, data) => handler({ details: { type, data } });
      return () => {};
    },
    session: {
      get: () => undefined,
      family: () => [],
      status: (id) => cache.status[id],
      permission: { list: (id) => cache.permission[id] },
      form: { list: (id) => cache.form[id] },
    },
  },
});
const settle = () => new Promise((resolve) => setTimeout(resolve, 400));

await settle(); // Route selected, cache still empty: nothing to report.
cache.status.ses_root = "running";
cache.permission.ses_root = [{ id: "per_1" }];
cache.form.ses_root = [];
await settle(); // Hydration alone surfaces the open request.
cache.permission.ses_root = [];
emit("permission.replied", { sessionID: "ses_root", requestID: "per_1", reply: "once" });
await settle(); // Cache and event agree: back to working.
cache.permission.ses_root = [{ id: "per_1" }];
await settle(); // A pre-reply snapshot landing late must not reopen it.
emit("permission.asked", { id: "per_2", sessionID: "ses_root" });
await settle(); // An ask the cache has not caught up with still blocks.
emit("permission.replied", { sessionID: "ses_root", requestID: "per_2", reply: "once" });
await settle();
emit("session.execution.succeeded", { sessionID: "ses_root" });
await settle(); // The end wins over the stale cached "running".
cache.status.ses_root = "idle";
await settle();
cache.status.ses_root = "running";
await settle(); // Nor does a late pre-end status snapshot restart it.
emit("session.execution.started", { sessionID: "ses_other" });
emit("permission.asked", { id: "per_x", sessionID: "ses_other" });
await settle(); // Another terminal's session never reaches this pane.

dispose();
server.close();
console.log(JSON.stringify(reported));
"#;
        let dir = tempfile::tempdir().unwrap();
        let plugin_path = dir.path().join("tui.mjs");
        let harness_path = dir.path().join("harness.mjs");
        std::fs::write(
            &plugin_path,
            render_opencode_plugin(OPENCODE_TUI_PLUGIN_TEMPLATE, "/nonexistent/active-port"),
        )
        .unwrap();
        std::fs::write(&harness_path, HARNESS).unwrap();

        let output = match std::process::Command::new("node")
            .arg(&harness_path)
            .arg(format!("file://{}", plugin_path.display()))
            .output()
        {
            Ok(output) => output,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("skipping: node is not installed");
                return;
            }
            Err(error) => panic!("failed to run node: {error}"),
        };
        assert!(
            output.status.success(),
            "harness failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            r#"["PermissionRequest","Start","PermissionRequest","Start","Stop"]"#
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

    fn fixture_settings(home: &Path, relative: &str, content: &[u8]) -> PathBuf {
        let path = home.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
        path
    }

    fn freeze_mtime(path: &Path) -> std::time::SystemTime {
        let time = std::time::UNIX_EPOCH + Duration::from_secs(1_234_567);
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(time)
            .unwrap();
        std::fs::metadata(path).unwrap().modified().unwrap()
    }

    #[test]
    fn hook_config_invalid_json_and_shapes_are_untouched() {
        let home = tempfile::tempdir().unwrap();
        for relative in [
            ".claude/settings.json",
            ".codex/hooks.json",
            ".gemini/settings.json",
        ] {
            for content in [
                "",
                "{broken",
                "[]",
                "null",
                r#"{"hooks":[]}"#,
                r#"{"hooks":{"Stop":{}}}"#,
                r#"{"hooks":{"Stop":[],"PermissionRequest":false}}"#,
                r#"{"hooks":{"Stop":[{"hooks":{}}]}}"#,
                r#"{"hooks":{"Stop":[{"hooks":[null]}]}}"#,
                r#"{"hooks":{"Stop":[false]}}"#,
            ] {
                let path = fixture_settings(home.path(), relative, content.as_bytes());
                let modified = freeze_mtime(&path);
                merge_nested_hooks_file(
                    &path,
                    &CLAUDE_HOOK_EVENTS,
                    "/test/.codemux/hooks/notify.sh",
                );
                assert_eq!(std::fs::read(&path).unwrap(), content.as_bytes());
                assert_eq!(
                    std::fs::metadata(&path).unwrap().modified().unwrap(),
                    modified
                );
            }
        }
    }

    #[test]
    fn hook_config_read_failure_is_skipped() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(".claude/settings.json");
        std::fs::create_dir_all(&path).unwrap();
        merge_nested_hooks_file(&path, &CLAUDE_HOOK_EVENTS, "/test/.codemux/hooks/notify.sh");
        assert!(path.is_dir());
        assert_eq!(std::fs::read_dir(&path).unwrap().count(), 0);
    }

    #[test]
    fn hook_config_preserves_unrelated_event_shapes() {
        let home = tempfile::tempdir().unwrap();
        let settings = serde_json::json!({
            "hooks": {"Other":null, "Flag":true, "Future":{"hooks":"custom"}}
        });
        let path = fixture_settings(
            home.path(),
            ".claude/settings.json",
            &serde_json::to_vec(&settings).unwrap(),
        );
        merge_nested_hooks_file(&path, &CLAUDE_HOOK_EVENTS, "/test/.codemux/hooks/notify.sh");
        let result: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        for key in ["Other", "Flag", "Future"] {
            assert_eq!(result["hooks"][key], settings["hooks"][key]);
        }
        assert!(result["hooks"]["Stop"].is_array());
    }

    #[test]
    fn hook_generated_commands_round_trip_without_accumulating_duplicates() {
        let home = tempfile::tempdir().unwrap();
        let mut agents = vec![CLAUDE_HOOK_EVENTS.as_slice(), CODEX_HOOK_EVENTS.as_slice()];
        #[cfg(not(target_os = "windows"))]
        agents.push(GEMINI_HOOK_EVENTS.as_slice());
        for script in [
            "/home/R&D/.codemux/hooks/notify.sh",
            "/home/a;b/.codemux/hooks/notify.sh",
            "/home/a|b/.codemux/hooks/notify.sh",
            "/home/a<b>/.codemux/hooks/notify.sh",
            "/home/with spaces/.codemux/hooks/gemini-notify.sh",
            r"C:\Users\R&D\.codemux\hooks\notify.ps1",
            r"\\server\with spaces\a;b\.codemux\hooks\notify.ps1",
        ] {
            for events in &agents {
                let hooks = build_nested_hooks_json(script, events);
                for (event, event_type) in *events {
                    let command = build_hook_command(script, event_type);
                    assert!(
                        is_codemux_hook(&hooks[event][0]["hooks"][0], &command),
                        "{command}"
                    );
                    assert!(!is_codemux_hook(
                        &serde_json::json!({"command":format!("{command} && user-command")}),
                        &command
                    ));
                }
                // Start with duplicates, including one with a nonempty matcher.
                let mut settings = serde_json::json!({"hooks":hooks});
                for (event, _) in *events {
                    let entries = settings["hooks"][event].as_array_mut().unwrap();
                    let mut duplicate = entries[0].clone();
                    duplicate["matcher"] = serde_json::json!("Bash");
                    entries.push(duplicate);
                }
                let path = fixture_settings(
                    home.path(),
                    "settings.json",
                    &serde_json::to_vec(&settings).unwrap(),
                );
                merge_nested_hooks_file(&path, events, script);
                let bytes = std::fs::read(&path).unwrap();
                let result: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(result["hooks"], hooks);
                let modified = freeze_mtime(&path);
                merge_nested_hooks_file(&path, events, script);
                assert_eq!(std::fs::read(&path).unwrap(), bytes);
                assert_eq!(
                    std::fs::metadata(&path).unwrap().modified().unwrap(),
                    modified
                );
            }
        }
    }

    #[test]
    fn hook_config_preserves_mixed_user_entries_and_settings() {
        let home = tempfile::tempdir().unwrap();
        let script = "/test/.codemux/hooks/notify.sh";
        let user_hook = serde_json::json!({"type":"command", "command":"user-notify", "timeout":9});
        let prompt_hook = serde_json::json!({"type":"prompt", "prompt":"keep this"});
        let mixed = serde_json::json!({
            "matcher":"Bash", "custom":"keep", "hooks":[
                {"type":"command", "command":format!("{script} Stop")}, user_hook, prompt_hook
            ]
        });
        let separate = serde_json::json!({"hooks":[{"type":"command", "command":"another-hook"}]});
        let settings = serde_json::json!({
            "permissions":{"allow":["Read"]}, "other":{"keep":[1,2]},
            "hooks":{"Stop":[mixed, separate,
                {"command":format!("{script} Stop")},
                {"matcher":"Bash", "hooks":[{"type":"command", "command":format!("{script} Stop")}]},
                {"matcher":"", "hooks":[{"type":"command", "command":format!("{script} Stop")}]}],
                "Other":[{"hooks":[{"type":"prompt", "prompt":"untouched"}]}]}
        });
        let path = fixture_settings(
            home.path(),
            ".claude/settings.json",
            &serde_json::to_vec(&settings).unwrap(),
        );
        merge_nested_hooks_file(&path, &[("Stop", "Stop")], script);
        let result: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(result["permissions"], settings["permissions"]);
        assert_eq!(result["other"], settings["other"]);
        assert_eq!(result["hooks"]["Other"], settings["hooks"]["Other"]);
        assert_eq!(
            result["hooks"]["Stop"][0],
            serde_json::json!({
                "matcher":"Bash", "custom":"keep", "hooks":[user_hook, prompt_hook]
            })
        );
        assert_eq!(result["hooks"]["Stop"][1], separate);
        assert_eq!(result["hooks"]["Stop"].as_array().unwrap().len(), 3);
        assert_eq!(
            result["hooks"]["Stop"][2],
            build_nested_hooks_json(script, &[("Stop", "Stop")])["Stop"][0]
        );
        let modified = freeze_mtime(&path);
        merge_nested_hooks_file(&path, &[("Stop", "Stop")], script);
        assert_eq!(
            std::fs::metadata(path).unwrap().modified().unwrap(),
            modified
        );
    }

    #[test]
    fn hook_config_noop_preserves_bytes_and_mtime_for_each_agent() {
        let home = tempfile::tempdir().unwrap();
        let script = "/test/.codemux/hooks/notify.sh";
        let mut agents = vec![
            (".claude/settings.json", CLAUDE_HOOK_EVENTS.as_slice()),
            (".codex/hooks.json", CODEX_HOOK_EVENTS.as_slice()),
        ];
        #[cfg(not(target_os = "windows"))]
        agents.push((".gemini/settings.json", GEMINI_HOOK_EVENTS.as_slice()));
        for (relative, events) in agents {
            let mut settings =
                serde_json::json!({"user":true,"hooks":build_nested_hooks_json(script, events)});
            settings["hooks"][events[0].0]
                .as_array_mut()
                .unwrap()
                .push(serde_json::json!({"hooks":[{"type":"command","command":"user-hook"}]}));
            let content = format!("{}\n", serde_json::to_string(&settings).unwrap());
            let path = fixture_settings(home.path(), relative, content.as_bytes());
            let modified = freeze_mtime(&path);
            merge_nested_hooks_file(&path, events, script);
            assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
            assert_eq!(
                std::fs::metadata(path).unwrap().modified().unwrap(),
                modified
            );
        }
    }

    #[test]
    fn hook_config_creates_missing_file_and_refuses_conflicting_edit() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join(".claude/settings.json");
        merge_nested_hooks_file(&path, &CLAUDE_HOOK_EVENTS, "/test/.codemux/hooks/notify.sh");
        let snapshot = HookFile::read(&path).unwrap();
        std::fs::write(&path, b"user edit").unwrap();
        assert!(snapshot.replace(b"replacement", false).is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"user edit");
        assert_eq!(
            std::fs::read_dir(home.path().join(".claude"))
                .unwrap()
                .count(),
            1
        );
    }

    #[cfg(unix)]
    #[test]
    fn hook_config_respects_readonly_files_and_normal_creation_mode() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("settings.json");
        let reference = home.path().join("normal-file");
        std::fs::write(&reference, b"normal file creation").unwrap();
        merge_nested_hooks_file(&path, &CLAUDE_HOOK_EVENTS, "/test/.codemux/hooks/notify.sh");
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            std::fs::metadata(reference).unwrap().permissions().mode() & 0o777
        );
        std::fs::write(&path, b"{\"user\":true}").unwrap();
        let modified = freeze_mtime(&path);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
        let inode = std::fs::metadata(&path).unwrap().ino();
        merge_nested_hooks_file(&path, &CLAUDE_HOOK_EVENTS, "/test/.codemux/hooks/notify.sh");
        assert_eq!(std::fs::read(&path).unwrap(), b"{\"user\":true}");
        let meta = std::fs::metadata(&path).unwrap();
        assert_eq!(meta.ino(), inode);
        assert_eq!(meta.modified().unwrap(), modified);
        assert_eq!(meta.permissions().mode() & 0o777, 0o444);
    }

    #[cfg(unix)]
    #[test]
    fn hook_config_keeps_mode_and_updates_symlink_target() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let home = tempfile::tempdir().unwrap();
        let target = fixture_settings(home.path(), "dotfiles/settings.json", b"{\"user\":true}");
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o640)).unwrap();
        let path = home.path().join(".claude/settings.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        symlink("../dotfiles/settings.json", &path).unwrap();
        merge_nested_hooks_file(&path, &CLAUDE_HOOK_EVENTS, "/test/.codemux/hooks/notify.sh");
        assert!(std::fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            std::fs::read_link(&path).unwrap(),
            PathBuf::from("../dotfiles/settings.json")
        );
        let settings: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&target).unwrap()).unwrap();
        assert_eq!(settings["user"], true);
        assert!(settings["hooks"]["Stop"].is_array());
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o640
        );
        let dangling = home.path().join(".claude/dangling.json");
        symlink("../missing.json", &dangling).unwrap();
        merge_nested_hooks_file(
            &dangling,
            &CLAUDE_HOOK_EVENTS,
            "/test/.codemux/hooks/notify.sh",
        );
        assert!(!home.path().join("missing.json").exists());
        assert!(std::fs::symlink_metadata(dangling)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn hook_artifacts_are_idempotent_and_keep_existing_mode() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let plugin = render_opencode_plugin(OPENCODE_TUI_PLUGIN_TEMPLATE, "/test/active-port");
        let extension = build_pi_extension("/test/.codemux/hooks/notify.sh");
        for (relative, body, executable) in [
            (".codemux/hooks/notify.sh", HOOK_SCRIPT, true),
            (".codemux/hooks/gemini-notify.sh", GEMINI_HOOK_SCRIPT, true),
            (
                ".config/opencode/plugins/codemux/tui.js",
                plugin.as_str(),
                false,
            ),
            (
                ".pi/agent/extensions/codemux-notify.ts",
                extension.as_str(),
                false,
            ),
        ] {
            let path = home.path().join(relative);
            write_hook_artifact(&path, body, executable).unwrap();
            if executable {
                assert_eq!(
                    std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                    0o755
                );
            }
            let modified = freeze_mtime(&path);
            write_hook_artifact(&path, body, executable).unwrap();
            assert_eq!(
                std::fs::metadata(&path).unwrap().modified().unwrap(),
                modified
            );
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o750)).unwrap();
            write_hook_artifact(&path, &format!("{body}\n"), executable).unwrap();
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o750
            );
        }
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn hook_opencode_unknown_version_installs_plural_and_leaves_legacy_alone() {
        let home = tempfile::tempdir().unwrap();
        let active = "/test/.codemux/hooks/active-port";
        for major in [None, Some(3)] {
            let config = home.path().join(format!("{major:?}"));
            install_opencode_plugin_in(&config, active, major).unwrap();
            let modern = config.join("plugins/codemux/tui.js");
            let legacy = config.join("plugin/codemux-notify.js");
            assert!(modern.is_file());
            assert!(!legacy.exists());
            install_opencode_plugin_in(&config, active, Some(1)).unwrap();
            let bytes = std::fs::read(&legacy).unwrap();
            let modified = freeze_mtime(&legacy);
            install_opencode_plugin_in(&config, active, major).unwrap();
            assert_eq!(std::fs::read(&legacy).unwrap(), bytes);
            assert_eq!(
                std::fs::metadata(&legacy).unwrap().modified().unwrap(),
                modified
            );
        }
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn hook_opencode_always_installs_plural_and_retires_only_generated_legacy() {
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join(".config/opencode");
        let active = "/test/.codemux/hooks/active-port";
        let legacy = config.join("plugin/codemux-notify.js");
        let modern = config.join("plugins/codemux/tui.js");
        install_opencode_plugin_in(&config, active, Some(1)).unwrap();
        let legacy_modified = freeze_mtime(&legacy);
        install_opencode_plugin_in(&config, active, Some(1)).unwrap();
        assert_eq!(
            std::fs::metadata(&legacy).unwrap().modified().unwrap(),
            legacy_modified
        );
        assert!(legacy.exists());
        assert!(modern.exists());
        install_opencode_plugin_in(&config, active, Some(2)).unwrap();
        assert!(modern.exists());
        assert!(!legacy.exists());
        let modified = freeze_mtime(&modern);
        install_opencode_plugin_in(&config, active, Some(2)).unwrap();
        assert_eq!(
            std::fs::metadata(&modern).unwrap().modified().unwrap(),
            modified
        );
        std::fs::write(&legacy, "// user plugin").unwrap();
        install_opencode_plugin_in(&config, active, Some(2)).unwrap();
        assert_eq!(std::fs::read_to_string(&legacy).unwrap(), "// user plugin");
        std::fs::remove_file(&legacy).unwrap();
        let target = home.path().join("legacy.js");
        std::fs::write(
            &target,
            render_opencode_plugin(OPENCODE_PLUGIN_TEMPLATE, active),
        )
        .unwrap();
        std::os::unix::fs::symlink(&target, &legacy).unwrap();
        install_opencode_plugin_in(&config, active, Some(2)).unwrap();
        assert!(std::fs::symlink_metadata(&legacy)
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(target.exists());
        // A failed replacement must leave the generated legacy plugin alone.
        let blocked = home.path().join("blocked");
        fixture_settings(
            home.path(),
            "blocked/plugin/codemux-notify.js",
            render_opencode_plugin(OPENCODE_PLUGIN_TEMPLATE, active).as_bytes(),
        );
        std::fs::write(blocked.join("plugins"), "not a directory").unwrap();
        assert!(install_opencode_plugin_in(&blocked, active, Some(2)).is_none());
        assert!(blocked.join("plugin/codemux-notify.js").exists());
    }

    #[test]
    fn hook_config_does_not_claim_user_commands_that_mention_our_script() {
        for command in [
            "echo /test/.codemux/hooks/notify.sh",
            "/test/.codemux/hooks/notify.sh Stop && user-command",
            "/test/.codemux/hooks/notify.sh.custom",
        ] {
            assert!(!is_codemux_hook(
                &serde_json::json!({"type":"command", "command":command}),
                "/test/.codemux/hooks/notify.sh Stop"
            ));
        }
        assert!(is_codemux_hook(
            &serde_json::json!({
                "command":r#"powershell -NoProfile -ExecutionPolicy Bypass -File "C:\Users\test\.codemux\hooks\notify.ps1" Stop"#
            }),
            "/test/.codemux/hooks/notify.sh Stop"
        ));
    }

    #[test]
    fn hook_artifact_readers_only_observe_complete_replacements() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("notify.sh");
        let first = "a".repeat(64 * 1024);
        let second = "b".repeat(64 * 1024);
        write_hook_artifact(&path, &first, false).unwrap();
        let reading = Arc::new(AtomicBool::new(true));
        let ready = Arc::new(std::sync::Barrier::new(2));
        let reader = {
            let path = path.clone();
            let reading = reading.clone();
            let ready = ready.clone();
            let first = first.clone();
            let second = second.clone();
            std::thread::spawn(move || {
                assert_eq!(std::fs::read_to_string(&path).unwrap(), first);
                ready.wait();
                let mut reads = 1;
                while reading.load(Ordering::Relaxed) {
                    let content = std::fs::read_to_string(&path).unwrap();
                    assert!(
                        content == first || content == second,
                        "reader saw a partial artifact"
                    );
                    reads += 1;
                }
                reads
            })
        };
        ready.wait();
        let written = [&second, &first, &second, &first]
            .into_iter()
            .try_for_each(|body| write_hook_artifact(&path, body, false));
        reading.store(false, Ordering::Relaxed);
        assert!(reader.join().unwrap() > 0);
        written.expect("artifact replacement failed");
    }

    #[cfg(windows)]
    fn hook_reader_without_share_delete(path: &Path) -> std::fs::File {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(path)
            .unwrap()
    }

    #[cfg(windows)]
    #[test]
    fn hook_artifact_windows_retries_until_reader_closes() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("notify.ps1");
        std::fs::write(&path, "old").unwrap();
        let snapshot = HookFile::read(&path).unwrap();
        let reader = hook_reader_without_share_delete(&path);
        let writer = std::thread::spawn(move || snapshot.replace(b"new", false));
        std::thread::sleep(Duration::from_millis(100));
        let was_waiting = !writer.is_finished();
        drop(reader);
        writer.join().unwrap().unwrap();
        assert!(was_waiting);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 1);
    }

    #[cfg(windows)]
    #[test]
    fn hook_artifact_windows_blocked_replace_preserves_file_and_cleans_temp() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("notify.ps1");
        std::fs::write(&path, "old").unwrap();
        let reader = hook_reader_without_share_delete(&path);
        let started = std::time::Instant::now();
        let error = HookFile::read(&path)
            .unwrap()
            .replace(b"new", false)
            .unwrap_err();
        drop(reader);
        assert!(matches!(error.raw_os_error(), Some(5 | 32)));
        assert!(started.elapsed() >= Duration::from_secs(1));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "old");
        assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 1);
    }

    #[cfg(windows)]
    #[test]
    fn hook_artifact_windows_retry_refuses_intervening_edit() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("notify.ps1");
        std::fs::write(&path, "old").unwrap();
        let snapshot = HookFile::read(&path).unwrap();
        let reader = hook_reader_without_share_delete(&path);
        let writer = std::thread::spawn(move || snapshot.replace(b"new", false));
        std::thread::sleep(Duration::from_millis(100));
        std::fs::write(&path, "user edit").unwrap();
        drop(reader);
        let error = writer.join().unwrap().unwrap_err();
        assert!(error.to_string().contains("changed during update"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "user edit");
        assert_eq!(std::fs::read_dir(home.path()).unwrap().count(), 1);
    }

    /// Set CODEMUX_TEST_OPENCODE_BINARY to a prepared native OpenCode 2.x binary.
    /// No launcher, account, real HOME, background service, or model is used.
    #[cfg(unix)]
    #[test]
    #[ignore = "requires an explicitly selected installed OpenCode executable"]
    fn opencode_loader_smoke_isolated_home() {
        use std::os::unix::process::CommandExt;
        let binary = PathBuf::from(
            std::env::var_os("CODEMUX_TEST_OPENCODE_BINARY")
                .expect("select a prepared OpenCode executable"),
        );
        assert!(binary.is_absolute() && binary.is_file());
        let home = tempfile::tempdir().unwrap();
        let config = home.path().join("config/opencode");
        let run = |args: &[&str]| {
            let stdout = tempfile::NamedTempFile::new_in(home.path()).unwrap();
            let stderr = tempfile::NamedTempFile::new_in(home.path()).unwrap();
            let mut command = std::process::Command::new(&binary);
            command
                .args(args)
                .current_dir(home.path())
                .env_clear()
                .env("OPENCODE_DISABLE_AUTOUPDATE", "true")
                .env("OPENCODE_DISABLE_MODELS_FETCH", "true")
                .env("OPENCODE_DISABLE_DEFAULT_PLUGINS", "true")
                .env("HOME", home.path())
                .env("PATH", "/usr/bin:/bin")
                .env("XDG_CONFIG_HOME", home.path().join("config"))
                .env("XDG_DATA_HOME", home.path().join("data"))
                .env("XDG_CACHE_HOME", home.path().join("cache"))
                .env("XDG_STATE_HOME", home.path().join("state"))
                .stdin(std::process::Stdio::null())
                .stdout(stdout.reopen().unwrap())
                .stderr(stderr.reopen().unwrap())
                .process_group(0);
            struct Probe(std::process::Child);
            impl Drop for Probe {
                fn drop(&mut self) {
                    // The private CLI may start its own server; stop our entire
                    // process group even if the CLI already exited.
                    unsafe {
                        libc::kill(-(self.0.id() as i32), libc::SIGKILL);
                    }
                    let _ = self.0.wait();
                }
            }
            let mut probe = Probe(command.spawn().unwrap());
            let deadline = std::time::Instant::now() + Duration::from_secs(20);
            let status = loop {
                if let Some(status) = probe.0.try_wait().unwrap() {
                    break status;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "OpenCode loader timed out"
                );
                std::thread::sleep(Duration::from_millis(20));
            };
            drop(probe);
            let out = std::fs::read_to_string(stdout.path()).unwrap();
            let err = std::fs::read_to_string(stderr.path()).unwrap();
            assert!(status.success(), "OpenCode failed: {out}\n{err}");
            (out, err)
        };
        let (version, _) = run(&["--version"]);
        let version = version
            .split_whitespace()
            .last()
            .unwrap()
            .trim_start_matches('v');
        let major: u32 = version.split('.').next().unwrap().parse().unwrap();
        let active = home.path().join(".codemux/hooks/active-port");
        let active = active.to_str().unwrap();
        install_opencode_plugin_in(&config, active, Some(1)).unwrap();
        assert_eq!(major, 2, "select an installed OpenCode 2.x binary");
        // Reproduce F17 using the actual legacy file, even outside Codemux.
        let (_, errors) = run(&["plugin", "list", "--standalone", "--print-logs"]);
        assert!(errors.contains("failed to load plugin") && errors.contains("codemux-notify.js"));
        install_opencode_plugin_in(&config, active, Some(2)).unwrap();
        let (out, errors) = run(&["plugin", "list", "--standalone", "--print-logs"]);
        assert!(
            out.contains("plugins/codemux"),
            "terminal plugin was not discovered: {out}"
        );
        assert!(!errors.contains("failed to load plugin"), "{errors}");
    }
}
