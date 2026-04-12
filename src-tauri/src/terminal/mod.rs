use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::env;
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::io::RawFd;

/// Opaque poll handle passed to `batched_reader_loop`. On Unix this is the
/// raw fd of the PTY master (used by `poll()`); on Windows it is unused because
/// the placeholder reader does blocking reads.
#[cfg(unix)]
type PollFd = RawFd;
#[cfg(windows)]
type PollFd = ();
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{ipc::Channel, AppHandle, Emitter, Manager, State};

use crate::project::current_project_root;
use crate::settings_sync;
use crate::state::{self, AppStateStore, TerminalSessionState};

static COMM_LOG_LOCKS: std::sync::OnceLock<Arc<Mutex<HashMap<String, Arc<Mutex<std::fs::File>>>>>> =
    std::sync::OnceLock::new();

pub fn get_comm_log_lock(path: &str) -> Arc<Mutex<std::fs::File>> {
    let locks = COMM_LOG_LOCKS.get_or_init(|| Arc::new(Mutex::new(HashMap::new())));
    let mut locks_guard = locks.lock().unwrap_or_else(|e| e.into_inner());
    locks_guard
        .entry(path.to_string())
        .or_insert_with(|| {
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .expect("Failed to open comm log for locking");
            Arc::new(Mutex::new(file))
        })
        .clone()
}

pub fn release_comm_log_lock(path: &str) {
    if let Some(locks) = COMM_LOG_LOCKS.get() {
        let mut locks_guard = locks.lock().unwrap_or_else(|e| e.into_inner());
        locks_guard.remove(path);
    }
}

fn strip_ansi_codes(s: &str) -> String {
    let mut result = String::new();
    let mut in_escape = false;
    let mut escape_buf = String::new();

    for c in s.chars() {
        if c == '\x1b' {
            in_escape = true;
            escape_buf.clear();
        } else if in_escape {
            if c.is_ascii_alphanumeric()
                || c == '@'
                || c == '['
                || c == ']'
                || c == ';'
                || c == '?'
                || c == ' '
            {
                escape_buf.push(c);
                // CSI sequences end with letters, OSC with bell/ST
                if c.is_ascii_lowercase() || c.is_ascii_uppercase() || c == '@' || c == '`' {
                    in_escape = false;
                }
            } else if c == '\\' || c == '\x07' {
                // ST (String Terminator) or BEL
                in_escape = false;
            }
        } else {
            result.push(c);
        }
    }
    result
}

const DEFAULT_ROWS: u16 = 24;
const DEFAULT_COLS: u16 = 80;
const OUTPUT_BUFFER_LIMIT: usize = 1024;
/// PTY output batching: flush after this many accumulated bytes.
const PTY_BATCH_SIZE: usize = 32_768;
/// PTY output batching: flush after this much time since last flush (~1 frame at 60 Hz).
const PTY_BATCH_INTERVAL: Duration = Duration::from_millis(16);
/// Safety cap so we never spawn hundreds of PTYs on startup (e.g. after corrupted or stale persisted state).
const MAX_STARTUP_SESSIONS: usize = 50;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalLifecycleState {
    Starting,
    Ready,
    Exited,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct TerminalStatusPayload {
    pub session_id: String,
    pub state: TerminalLifecycleState,
    pub message: Option<String>,
    pub exit_code: Option<u32>,
}

pub struct SessionRuntime {
    pub writer: Option<Box<dyn Write + Send>>,
    pub master: Option<Box<dyn MasterPty + Send>>,
    pub output_channel: Option<Channel<Vec<u8>>>,
    pub pending_output: VecDeque<Vec<u8>>,
    pub last_status: TerminalStatusPayload,
    pub child_pid: Option<u32>,
    pub skip_preset_launch: bool,
    /// Optional full launch command for restored sessions.
    /// When present, the preset readiness helper injects this command instead
    /// of the normal preset command.
    pub resume_command: Option<String>,
}

impl SessionRuntime {
    pub(crate) fn new(session_id: &str) -> Self {
        Self {
            writer: None,
            master: None,
            output_channel: None,
            pending_output: VecDeque::new(),
            last_status: TerminalStatusPayload {
                session_id: session_id.to_string(),
                state: TerminalLifecycleState::Starting,
                message: Some("Starting shell...".into()),
                exit_code: None,
            },
            child_pid: None,
            skip_preset_launch: false,
            resume_command: None,
        }
    }
}

#[derive(Default)]
pub struct PtyState {
    pub sessions: Arc<Mutex<HashMap<String, SessionRuntime>>>,
}

impl PtyState {
    /// Returns a snapshot of session_id -> child PID for all active sessions.
    pub fn get_session_pids(&self) -> HashMap<String, u32> {
        let guard = self.sessions.lock().unwrap_or_else(|e| e.into_inner());
        guard
            .iter()
            .filter_map(|(id, runtime)| runtime.child_pid.map(|pid| (id.clone(), pid)))
            .collect()
    }
}

fn remove_session_runtime(
    sessions: &Arc<Mutex<HashMap<String, SessionRuntime>>>,
    session_id: &str,
) -> Option<SessionRuntime> {
    sessions
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(session_id)
}

fn map_status_state(state: &TerminalLifecycleState) -> TerminalSessionState {
    match state {
        TerminalLifecycleState::Starting => TerminalSessionState::Starting,
        TerminalLifecycleState::Ready => TerminalSessionState::Ready,
        TerminalLifecycleState::Exited => TerminalSessionState::Exited,
        TerminalLifecycleState::Failed => TerminalSessionState::Failed,
    }
}

/// Pick the best CWD for a restored PTY session.  Prefers the scrollback
/// metadata's `working_directory` when it is non-empty and still exists on
/// disk, so that CWD-scoped tools (`claude --resume`) find their sessions.
/// Falls back to `fallback` otherwise.
fn resolve_session_cwd(scrollback_working_dir: &str, fallback: &str) -> String {
    if !scrollback_working_dir.is_empty()
        && std::path::Path::new(scrollback_working_dir).is_dir()
    {
        scrollback_working_dir.to_string()
    } else {
        fallback.to_string()
    }
}

fn build_resume_launch_command(original_command: &str, resume_args: &str) -> String {
    let original = original_command.trim();
    let args = resume_args.trim();

    if original.is_empty() {
        return args.to_string();
    }
    if args.is_empty() {
        return original.to_string();
    }

    format!("{original} {args}")
}

fn resolve_resume_command(
    snapshot: &crate::state::AppStateSnapshot,
    meta: &crate::scrollback::ScrollbackMeta,
    adapter_state: &crate::session_adapters::AdapterState,
) -> Option<String> {
    let original_command = meta.original_command.clone().or_else(|| {
        snapshot
            .terminal_sessions
            .iter()
            .find(|session| session.session_id.0 == meta.session_id)
            .and_then(|session| session.original_command.clone())
    })?;

    let adapter_id = meta
        .adapter_id
        .clone()
        .or_else(|| adapter_state.match_adapter_id_for_command(&original_command))?;

    let adapter_match = adapter_state.get_adapter_match(&adapter_id, &meta.adapter_captures)?;
    let resume_args = adapter_match.resume_args?;

    Some(build_resume_launch_command(&original_command, &resume_args))
}

fn with_session_runtime<T>(
    sessions: &Arc<Mutex<HashMap<String, SessionRuntime>>>,
    session_id: &str,
    default: impl FnOnce() -> SessionRuntime,
    f: impl FnOnce(&mut SessionRuntime) -> T,
) -> T {
    let mut guard = sessions.lock().unwrap_or_else(|e| e.into_inner());
    let runtime = guard.entry(session_id.to_string()).or_insert_with(default);
    f(runtime)
}

fn with_existing_session_runtime<T>(
    sessions: &Arc<Mutex<HashMap<String, SessionRuntime>>>,
    session_id: &str,
    f: impl FnOnce(&mut SessionRuntime) -> T,
) -> Option<T> {
    let mut guard = sessions.lock().unwrap_or_else(|e| e.into_inner());
    guard.get_mut(session_id).map(f)
}

fn emit_terminal_status(
    app: &AppHandle,
    sessions: &Arc<Mutex<HashMap<String, SessionRuntime>>>,
    payload: TerminalStatusPayload,
) {
    let app_state: State<'_, AppStateStore> = app.state();
    app_state.update_terminal_session_status(
        &payload.session_id,
        map_status_state(&payload.state),
        payload.message.clone(),
        payload.exit_code,
    );

    // For terminal states (Failed/Exited), don't create a new SessionRuntime
    // entry — the session is done. For Starting/Ready, use or_insert to ensure
    // the entry exists.
    match payload.state {
        TerminalLifecycleState::Failed | TerminalLifecycleState::Exited => {
            with_existing_session_runtime(sessions, &payload.session_id, |runtime| {
                runtime.last_status = payload.clone();
            });
        }
        _ => {
            with_session_runtime(
                sessions,
                &payload.session_id,
                || SessionRuntime::new(&payload.session_id),
                |runtime| {
                    runtime.last_status = payload.clone();
                },
            );
        }
    }

    // On terminal exit, clear transient pane status (working/permission → idle)
    if matches!(
        payload.state,
        TerminalLifecycleState::Exited | TerminalLifecycleState::Failed
    ) {
        app_state.clear_transient_pane_status_by_session(&payload.session_id);
    }

    if let Err(error) = app.emit("terminal-status", payload) {
        eprintln!("[codemux::terminal] Failed to emit terminal status: {error}");
    }
}

fn queue_or_send_output(
    sessions: &Arc<Mutex<HashMap<String, SessionRuntime>>>,
    session_id: &str,
    chunk: Vec<u8>,
) {
    with_session_runtime(
        sessions,
        session_id,
        || SessionRuntime::new(session_id),
        |runtime| {
            runtime.pending_output.push_back(chunk.clone());
            while runtime.pending_output.len() > OUTPUT_BUFFER_LIMIT {
                runtime.pending_output.pop_front();
            }

            if let Some(channel) = runtime.output_channel.clone() {
                if let Err(error) = channel.send(chunk) {
                    eprintln!("[codemux::terminal] Failed to send terminal output: {error}");
                    runtime.output_channel = None;
                }
            }
        },
    );
}

/// Flush accumulated PTY output as a single batched chunk.
/// Resets `batch` and `last_flush` after flushing.
fn flush_pty_batch(
    sessions: &Arc<Mutex<HashMap<String, SessionRuntime>>>,
    session_id: &str,
    batch: &mut Vec<u8>,
    last_flush: &mut Instant,
) {
    if !batch.is_empty() {
        let chunk = std::mem::take(batch);
        queue_or_send_output(sessions, session_id, chunk);
        *last_flush = Instant::now();
    }
}

/// Poll a file descriptor for read-readiness with a timeout.
/// Returns true if data is available (POLLIN) or the fd is dead (POLLHUP/POLLERR),
/// meaning the caller should attempt read() to get data or discover the error.
/// Returns false on timeout (no events within timeout_ms).
/// Retries on EINTR (signal interruption).
#[cfg(unix)]
fn poll_read_ready(fd: RawFd, timeout_ms: i32) -> bool {
    let mut pfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        // Safety: pfd is a valid pollfd struct on the stack.
        let ret = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
        if ret < 0 {
            let errno = std::io::Error::last_os_error();
            if errno.raw_os_error() == Some(libc::EINTR) {
                continue; // Interrupted by signal, retry
            }
            // Other poll error (e.g. EBADF) — return true so caller
            // hits read() which will surface the actual error.
            return true;
        }
        return ret > 0 && (pfd.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR)) != 0;
    }
}

/// Batched PTY reader loop. Uses poll() to guarantee pending data is flushed
/// within PTY_BATCH_INTERVAL even when no more output arrives.
///
/// `poll_fd` is the raw fd for the PTY master (used for poll readiness checks).
/// `reader` is the cloned reader (a dup of the same fd, used for actual reads).
/// `pre_read_hook` is called with each read's raw data before it's batched,
/// allowing per-read processing (e.g. comm log in the agent loop).
#[cfg(unix)]
fn batched_reader_loop(
    reader: &mut dyn Read,
    poll_fd: PollFd,
    sessions: &Arc<Mutex<HashMap<String, SessionRuntime>>>,
    session_id: &str,
    mut pre_read_hook: impl FnMut(&[u8]),
) {
    let mut buf = [0u8; 4096];
    let mut batch: Vec<u8> = Vec::with_capacity(PTY_BATCH_SIZE);
    let mut last_flush = Instant::now();
    let timeout_ms = PTY_BATCH_INTERVAL.as_millis() as i32;

    loop {
        // If the batch has data, use a timed poll so we flush on timeout.
        // If the batch is empty, block indefinitely until data arrives.
        let poll_timeout = if batch.is_empty() { -1 } else { timeout_ms };

        if !poll_read_ready(poll_fd, poll_timeout) {
            // Timeout with no new data — flush pending batch.
            flush_pty_batch(sessions, session_id, &mut batch, &mut last_flush);
            continue;
        }

        match reader.read(&mut buf) {
            Ok(n) if n > 0 => {
                let data = &buf[..n];
                pre_read_hook(data);
                batch.extend_from_slice(data);
                if batch.len() >= PTY_BATCH_SIZE || last_flush.elapsed() >= PTY_BATCH_INTERVAL {
                    flush_pty_batch(sessions, session_id, &mut batch, &mut last_flush);
                }
            }
            Ok(_) => {
                flush_pty_batch(sessions, session_id, &mut batch, &mut last_flush);
                break;
            }
            Err(error) => {
                flush_pty_batch(sessions, session_id, &mut batch, &mut last_flush);
                eprintln!("[codemux::terminal] PTY read error: {error}");
                break;
            }
        }
    }
}

#[cfg(windows)]
fn batched_reader_loop(
    reader: &mut dyn Read,
    _poll_fd: PollFd,
    sessions: &Arc<Mutex<HashMap<String, SessionRuntime>>>,
    session_id: &str,
    mut pre_read_hook: impl FnMut(&[u8]),
) {
    // Windows placeholder: Windows pipes are blocking with no poll() equivalent,
    // so we use a simple blocking read loop that flushes every read. No 16ms
    // batching yet — that needs a Tokio-based rewrite.
    let mut buf = [0u8; 4096];
    let mut batch: Vec<u8> = Vec::with_capacity(PTY_BATCH_SIZE);
    let mut last_flush = Instant::now();
    loop {
        match reader.read(&mut buf) {
            Ok(n) if n > 0 => {
                let data = &buf[..n];
                pre_read_hook(data);
                batch.extend_from_slice(data);
                flush_pty_batch(sessions, session_id, &mut batch, &mut last_flush);
            }
            Ok(_) => {
                flush_pty_batch(sessions, session_id, &mut batch, &mut last_flush);
                break;
            }
            Err(error) => {
                flush_pty_batch(sessions, session_id, &mut batch, &mut last_flush);
                eprintln!("[codemux::terminal] PTY read error: {error}");
                break;
            }
        }
    }
}

#[cfg(unix)]
fn default_shell() -> String {
    env::var("SHELL")
        .ok()
        .filter(|shell| !shell.trim().is_empty())
        .unwrap_or_else(|| "/bin/bash".to_string())
}

#[cfg(windows)]
fn default_shell() -> String {
    env::var("COMSPEC")
        .ok()
        .filter(|shell| !shell.trim().is_empty())
        .unwrap_or_else(|| "cmd.exe".to_string())
}

#[cfg(unix)]
fn ensure_openflow_cli_shims() -> Option<(String, String)> {
    let current_exe = std::env::current_exe().ok()?;
    let current_exe = current_exe.display().to_string();
    let shim_dir = std::env::temp_dir().join("codemux-openflow-shims");
    std::fs::create_dir_all(&shim_dir).ok()?;

    let shim_path = shim_dir.join("codemux");
    let script = format!(
        "#!/bin/sh\nexec \"{}\" \"$@\"\n",
        current_exe.replace('"', "\\\"")
    );
    std::fs::write(&shim_path, script).ok()?;

    let mut perms = std::fs::metadata(&shim_path).ok()?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&shim_path, perms).ok()?;

    Some((shim_dir.display().to_string(), current_exe))
}

#[cfg(windows)]
fn ensure_openflow_cli_shims() -> Option<(String, String)> {
    let current_exe = std::env::current_exe().ok()?;
    let current_exe = current_exe.display().to_string();
    let shim_dir = std::env::temp_dir().join("codemux-openflow-shims");
    std::fs::create_dir_all(&shim_dir).ok()?;

    let shim_path = shim_dir.join("codemux.bat");
    let script = format!(
        "@echo off\r\n\"{}\" %*\r\n",
        current_exe.replace('"', "\\\"")
    );
    std::fs::write(&shim_path, script).ok()?;

    Some((shim_dir.display().to_string(), current_exe))
}

#[cfg(not(any(unix, windows)))]
fn ensure_openflow_cli_shims() -> Option<(String, String)> {
    None
}

/// Find the workspace that owns a terminal session by walking each workspace's
/// pane tree.  Returns `None` for orphaned sessions (no workspace references them).
fn find_owning_workspace<'a>(
    snapshot: &'a crate::state::AppStateSnapshot,
    session_id: &str,
) -> Option<&'a crate::state::WorkspaceSnapshot> {
    snapshot.workspaces.iter().find(|ws| {
        ws.surfaces
            .iter()
            .any(|s| crate::state::find_terminal_pane_id(&s.root, session_id).is_some())
    })
}

fn session_working_dir(app_state: &State<'_, AppStateStore>, session_id: &str) -> String {
    app_state
        .snapshot()
        .terminal_sessions
        .into_iter()
        .find(|session| session.session_id.0 == session_id)
        .map(|session| session.cwd)
        .unwrap_or_else(|| current_project_root().display().to_string())
}

/// RAII guard that kills and waits on a PTY child process if not explicitly
/// disarmed. Prevents zombie processes when spawn_pty_for_session or
/// spawn_pty_for_agent encounters an error after the child has been spawned.
struct ChildGuard {
    child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
}

impl ChildGuard {
    fn new(child: Box<dyn portable_pty::Child + Send + Sync>) -> Self {
        Self { child: Some(child) }
    }

    /// Take ownership of the child, disarming the guard.
    /// Call this once the child is handed off to the waiter thread.
    fn disarm(mut self) -> Box<dyn portable_pty::Child + Send + Sync> {
        self.child.take().expect("ChildGuard already disarmed")
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            eprintln!("[codemux::terminal] Killing orphaned child process");
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Build worktree environment variables and dynamic agent context for a PTY session.
/// Used by both `spawn_pty_for_session()` and `spawn_pty_for_agent()` to ensure
/// consistent env var injection across user shells and agent processes.
fn workspace_pty_env(ws: &crate::state::WorkspaceSnapshot) -> Vec<(String, String)> {
    let root = ws.project_root.clone().unwrap_or_else(|| {
        crate::scripts::resolve_root_path(std::path::Path::new(&ws.cwd))
            .to_string_lossy()
            .to_string()
    });
    let port = crate::scripts::allocate_workspace_port(&ws.workspace_id.0);

    let mut vars: Vec<(String, String)> = crate::scripts::script_env(
        std::path::Path::new(&ws.cwd),
        std::path::Path::new(&root),
        ws.git_branch.as_deref(),
        Some(port),
    )
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();

    vars.push(("CODEMUX_WORKSPACE_NAME".to_string(), ws.title.clone()));
    vars.push((
        "CODEMUX_AGENT_CONTEXT".to_string(),
        crate::agent_context::build_agent_context(
            Some(&ws.title),
            ws.worktree_path.as_deref(),
            ws.git_branch.as_deref(),
            Some(&root),
        ),
    ));

    vars
}

pub fn spawn_pty_for_session(app: AppHandle, session_id: String) {
    let terminal_state: State<'_, PtyState> = app.state();
    let app_state: State<'_, AppStateStore> = app.state();
    let sessions = terminal_state.sessions.clone();

    let already_running = sessions
        .lock()
        .unwrap()
        .get(&session_id)
        .map(|runtime| runtime.writer.is_some() || runtime.master.is_some())
        .unwrap_or(false);

    if already_running {
        return;
    }

    emit_terminal_status(
        &app,
        &sessions,
        TerminalStatusPayload {
            session_id: session_id.clone(),
            state: TerminalLifecycleState::Starting,
            message: Some("Starting shell...".into()),
            exit_code: None,
        },
    );

    let pty_system = native_pty_system();
    let pty_pair = match pty_system.openpty(PtySize {
        rows: DEFAULT_ROWS,
        cols: DEFAULT_COLS,
        pixel_width: 0,
        pixel_height: 0,
    }) {
        Ok(pair) => pair,
        Err(error) => {
            emit_terminal_status(
                &app,
                &sessions,
                TerminalStatusPayload {
                    session_id,
                    state: TerminalLifecycleState::Failed,
                    message: Some(format!("Failed to open PTY: {error}")),
                    exit_code: None,
                },
            );
            return;
        }
    };

    let shell = default_shell();
    app_state.update_terminal_session_shell(&session_id, shell.clone());

    let cwd = session_working_dir(&app_state, &session_id);
    let mut cmd = CommandBuilder::new(shell.clone());
    cmd.cwd(cwd);

    // Declare terminal capabilities — Codemux is the terminal emulator, so it
    // must advertise what it supports.  Without these, CLI tools launched from a
    // desktop shortcut (no parent terminal) lose ANSI color output.
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env("TERM_PROGRAM", "codemux");
    cmd.env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));

    let snapshot = app_state.snapshot();

    // Find the workspace that owns this session by walking each workspace's
    // pane tree.  Orphaned sessions (no workspace references them) get only
    // session-level env vars — no workspace-specific injection, to avoid
    // silently using the wrong workspace's context.
    let owning_ws = find_owning_workspace(&snapshot, &session_id);

    cmd.env("CODEMUX", "1");
    cmd.env("CODEMUX_VERSION", env!("CARGO_PKG_VERSION"));
    cmd.env("CODEMUX_SURFACE_ID", session_id.clone());
    cmd.env("CODEMUX_SESSION_ID", session_id.clone());
    cmd.env("CODEMUX_BROWSER_CMD", "codemux browser");
    cmd.env("BROWSER", "codemux browser open");

    if let Some(ws) = owning_ws {
        cmd.env("CODEMUX_WORKSPACE_ID", &ws.workspace_id.0);
        for (key, val) in workspace_pty_env(ws) {
            cmd.env(&key, val);
        }
    } else {
        eprintln!(
            "[codemux::terminal] No owning workspace found for session {session_id}; \
             skipping workspace env injection"
        );
        cmd.env(
            "CODEMUX_AGENT_CONTEXT",
            crate::agent_context::build_agent_context(None, None, None, None),
        );
    }

    // Inject pane ID and hook server port for agent status notifications.
    // Also detect restored sessions so preset auto-launch can be skipped and
    // the full resume command can be injected later.
    let mut auto_resume_command: Option<String> = None;
    if let Some((_ws_id, pane_id)) = snapshot.workspaces.iter().find_map(|ws| {
        ws.surfaces
            .iter()
            .find_map(|s| crate::state::find_terminal_pane_id(&s.root, &session_id))
            .map(|pane_id| (ws.workspace_id.0.clone(), pane_id.0))
    }) {
        cmd.env("CODEMUX_PANE_ID", &pane_id);
    }

    let session_restore_enabled = settings_sync::load_cache()
        .map(|s| s.session_restore.enabled)
        .unwrap_or(true);
    if session_restore_enabled {
        if let Some(adapter_state) = app.try_state::<crate::session_adapters::AdapterState>() {
            if let Some((ws_id, pane_id, meta)) =
                crate::scrollback::find_scrollback_meta_for_session(&session_id)
            {
                // Override CWD with the original working directory so CWD-scoped
                // tools (e.g. `claude --resume`) find their sessions.
                let effective_cwd = resolve_session_cwd(
                    &meta.working_directory,
                    &session_working_dir(&app_state, &session_id),
                );
                cmd.cwd(&effective_cwd);
                cmd.env("CODEMUX_PANE_ID", &pane_id);
                if let Some(resume_command) =
                    resolve_resume_command(&snapshot, &meta, &adapter_state)
                {
                    eprintln!(
                        "[codemux::terminal] Skipping auto-launch for {session_id}: restored session found at {ws_id}/{pane_id}"
                    );
                    auto_resume_command = Some(resume_command);
                } else {
                    eprintln!(
                        "[codemux::terminal] Restored session metadata found for {session_id} at {ws_id}/{pane_id} but no resume command could be built"
                    );
                }
            } else {
                eprintln!(
                    "[codemux::terminal] No scrollback metadata found for restored session {session_id}"
                );
            }
        }
    }
    if let Some(port) = crate::hooks::hook_port() {
        cmd.env("CODEMUX_HOOK_PORT", port.to_string());
    }

    // Add codemux CLI shim to PATH so `codemux` commands work in user terminals.
    if let Some((shim_dir, current_exe)) = ensure_openflow_cli_shims() {
        let current_path = env::var("PATH").unwrap_or_default();
        let sep = if cfg!(windows) { ";" } else { ":" };
        let prefixed = if current_path.is_empty() {
            shim_dir
        } else {
            format!("{shim_dir}{sep}{current_path}")
        };
        cmd.env("PATH", prefixed);
        cmd.env("CODEMUX_CLI_SAFE_PATH", current_exe);
    }

    let child = match pty_pair.slave.spawn_command(cmd) {
        Ok(child) => child,
        Err(error) => {
            emit_terminal_status(
                &app,
                &sessions,
                TerminalStatusPayload {
                    session_id,
                    state: TerminalLifecycleState::Failed,
                    message: Some(format!("Failed to spawn shell {shell}: {error}")),
                    exit_code: None,
                },
            );
            return;
        }
    };

    // Wrap child in a guard that kills+waits on drop, preventing zombies
    // if a subsequent step fails before the waiter thread is spawned.
    let guard = ChildGuard::new(child);
    let child_pid = guard.child.as_ref().and_then(|c| c.process_id());

    drop(pty_pair.slave);

    let mut reader = match pty_pair.master.try_clone_reader() {
        Ok(reader) => reader,
        Err(error) => {
            emit_terminal_status(
                &app,
                &sessions,
                TerminalStatusPayload {
                    session_id: session_id.clone(),
                    state: TerminalLifecycleState::Failed,
                    message: Some(format!("Failed to clone PTY reader: {error}")),
                    exit_code: None,
                },
            );
            remove_session_runtime(&sessions, &session_id);
            return; // guard drops here, kills+waits on child
        }
    };

    let writer = match pty_pair.master.take_writer() {
        Ok(writer) => writer,
        Err(error) => {
            emit_terminal_status(
                &app,
                &sessions,
                TerminalStatusPayload {
                    session_id: session_id.clone(),
                    state: TerminalLifecycleState::Failed,
                    message: Some(format!("Failed to take PTY writer: {error}")),
                    exit_code: None,
                },
            );
            remove_session_runtime(&sessions, &session_id);
            return; // guard drops here, kills+waits on child
        }
    };

    // All resources acquired — disarm the guard and hand child to the waiter thread.
    let mut child = guard.disarm();

    // Extract the raw fd for poll() before moving master into SessionRuntime.
    // The reader fd is a dup of this fd, so polling either detects data on both.
    #[cfg(unix)]
    let poll_fd: PollFd = pty_pair.master.as_raw_fd().unwrap_or(-1);
    #[cfg(windows)]
    let poll_fd: PollFd = ();

    with_session_runtime(
        &sessions,
        &session_id,
        || SessionRuntime::new(&session_id),
        |runtime| {
            runtime.writer = Some(writer);
            runtime.master = Some(pty_pair.master);
            runtime.child_pid = child_pid;
            runtime.skip_preset_launch = auto_resume_command.is_some();
            runtime.resume_command = auto_resume_command.clone();
        },
    );

    if let Some(command) = auto_resume_command {
        let sessions_for_command = sessions.clone();
        let session_id_for_command = session_id.clone();
        crate::commands::presets::write_command_when_ready(
            sessions_for_command,
            session_id_for_command,
            command,
            120,
        );
    }

    emit_terminal_status(
        &app,
        &sessions,
        TerminalStatusPayload {
            session_id: session_id.clone(),
            state: TerminalLifecycleState::Ready,
            message: Some(format!("Shell ready: {shell}")),
            exit_code: None,
        },
    );

    // Wire up the session adapter output scanner if this session has an original_command.
    let adapter_clone: Option<crate::session_adapters::AdapterState> = app
        .try_state::<crate::session_adapters::AdapterState>()
        .map(|s| s.inner().clone());
    let snapshot = app_state.snapshot();
    let original_cmd = snapshot
        .terminal_sessions
        .iter()
        .find(|s| s.session_id.0 == session_id)
        .and_then(|s| s.original_command.clone());

    // Only create a scanner (and the per-byte hook) when a preset command was used.
    // Plain shell panes get no scanner and a no-op hook — zero overhead in the hot path.
    let has_scanner = if let (Some(ref adapter), Some(ref cmd)) = (&adapter_clone, &original_cmd) {
        adapter.start_scanner(&session_id, cmd).is_some()
    } else {
        false
    };

    let read_sessions = sessions.clone();
    let read_session_id = session_id.clone();
    let scanner_session_id = session_id.clone();
    let mut line_buf = Vec::<u8>::new();

    std::thread::spawn(move || {
        batched_reader_loop(
            &mut reader,
            poll_fd,
            &read_sessions,
            &read_session_id,
            |data: &[u8]| {
                if !has_scanner {
                    return;
                }
                let Some(ref adapter) = adapter_clone else {
                    return;
                };

                // Feed bytes into line buffer, scan complete lines
                for &byte in data {
                    if byte == b'\n' {
                        let line = String::from_utf8_lossy(&line_buf);
                        let clean = strip_ansi_codes(&line);
                        adapter.scan_line(&scanner_session_id, &clean);
                        line_buf.clear();
                    } else if byte != b'\r' {
                        line_buf.push(byte);
                    }
                }
            },
        );
    });

    let wait_app = app.clone();
    let wait_sessions = sessions.clone();
    let wait_session_id = session_id.clone();
    std::thread::spawn(move || {
        let payload = match child.wait() {
            Ok(status) => TerminalStatusPayload {
                session_id: wait_session_id.clone(),
                state: TerminalLifecycleState::Exited,
                message: Some(if status.success() {
                    "Shell exited successfully".into()
                } else {
                    format!("Shell exited with code {}", status.exit_code())
                }),
                exit_code: Some(status.exit_code()),
            },
            Err(error) => TerminalStatusPayload {
                session_id: wait_session_id.clone(),
                state: TerminalLifecycleState::Failed,
                message: Some(format!("Failed while waiting for shell: {error}")),
                exit_code: None,
            },
        };

        // Send terminal reset sequences to xterm.js via the IPC channel
        // before tearing down the session. This restores xterm.js to a clean
        // state after apps that enable mouse tracking, alt screen, etc.
        const TERMINAL_RESET: &[u8] =
            b"\x1b[?1049l\x1b[?1006l\x1b[?1003l\x1b[?1002l\x1b[?1000l\x1b[?2004l\x1b[?25h\x1b[0m";
        queue_or_send_output(&wait_sessions, &wait_session_id, TERMINAL_RESET.to_vec());

        with_session_runtime(
            &wait_sessions,
            &wait_session_id,
            || SessionRuntime::new(&wait_session_id),
            |runtime| {
                runtime.writer = None;
                runtime.master = None;
            },
        );

        emit_terminal_status(&wait_app, &wait_sessions, payload);

        // Clean up the session runtime to prevent memory leak
        remove_session_runtime(&wait_sessions, &wait_session_id);

        state::emit_app_state(&wait_app);
    });
}

pub fn spawn_missing_ptys(app: AppHandle) {
    let app_state: State<'_, AppStateStore> = app.state();
    let mut session_ids = app_state
        .snapshot()
        .terminal_sessions
        .into_iter()
        .map(|session| session.session_id.0)
        .collect::<Vec<_>>();

    if session_ids.len() > MAX_STARTUP_SESSIONS {
        eprintln!(
            "[codemux::terminal] Too many persisted sessions ({}); spawning only the first {}",
            session_ids.len(),
            MAX_STARTUP_SESSIONS
        );
        session_ids.truncate(MAX_STARTUP_SESSIONS);
    }

    for session_id in session_ids {
        spawn_pty_for_session(app.clone(), session_id);
    }
}

#[tauri::command]
pub fn create_terminal_session(
    app: AppHandle,
    app_state: State<'_, AppStateStore>,
) -> Result<String, String> {
    let session_id = app_state.create_terminal_session()?;
    state::emit_app_state(&app);
    spawn_pty_for_session(app, session_id.0.clone());
    Ok(session_id.0)
}

#[tauri::command]
pub fn activate_terminal_session(
    app: AppHandle,
    app_state: State<'_, AppStateStore>,
    session_id: String,
) -> Result<(), String> {
    if app_state.activate_terminal_session(&session_id) {
        state::emit_app_state(&app);
        Ok(())
    } else {
        Err(format!("No terminal session found for {session_id}"))
    }
}

#[tauri::command]
pub fn close_terminal_session(
    app: AppHandle,
    terminal_state: State<'_, PtyState>,
    app_state: State<'_, AppStateStore>,
    session_id: String,
) -> Result<String, String> {
    let fallback_session = app_state.close_terminal_session(&session_id)?;

    if let Some(mut runtime) = remove_session_runtime(&terminal_state.sessions, &session_id) {
        runtime.output_channel = None;
        runtime.pending_output.clear();
        if let Some(master) = runtime.master.as_mut() {
            let _ = master.resize(PtySize {
                rows: 1,
                cols: 1,
                pixel_width: 0,
                pixel_height: 0,
            });
        }
    }

    state::emit_app_state(&app);
    Ok(fallback_session.0)
}

#[tauri::command]
pub fn restart_terminal_session(
    app: AppHandle,
    terminal_state: State<'_, PtyState>,
    session_id: String,
) -> Result<(), String> {
    if let Some(mut runtime) = remove_session_runtime(&terminal_state.sessions, &session_id) {
        runtime.output_channel = None;
        runtime.pending_output.clear();
    }

    spawn_pty_for_session(app, session_id);
    Ok(())
}

#[tauri::command]
pub fn get_terminal_status(
    terminal_state: State<'_, PtyState>,
    app_state: State<'_, AppStateStore>,
    session_id: Option<String>,
) -> Result<TerminalStatusPayload, String> {
    let session_id = session_id
        .or_else(|| {
            app_state
                .active_terminal_session_id()
                .map(|session| session.0)
        })
        .ok_or_else(|| "No active terminal session found".to_string())?;

    let status = with_session_runtime(
        &terminal_state.sessions,
        &session_id,
        || SessionRuntime::new(&session_id),
        |runtime| runtime.last_status.clone(),
    );

    Ok(status)
}

#[tauri::command]
pub fn attach_pty_output(
    terminal_state: State<'_, PtyState>,
    app_state: State<'_, AppStateStore>,
    channel: Channel<Vec<u8>>,
    session_id: Option<String>,
    skip_pending: Option<bool>,
) -> Result<(), String> {
    let session_id = session_id
        .or_else(|| {
            app_state
                .active_terminal_session_id()
                .map(|session| session.0)
        })
        .ok_or_else(|| "No active terminal session found".to_string())?;

    let pending_chunks = with_session_runtime(
        &terminal_state.sessions,
        &session_id,
        || SessionRuntime::new(&session_id),
        |runtime| {
            runtime.output_channel = Some(channel.clone());
            if skip_pending.unwrap_or(false) {
                vec![]
            } else {
                runtime.pending_output.iter().cloned().collect::<Vec<_>>()
            }
        },
    );

    for chunk in pending_chunks {
        channel
            .send(chunk)
            .map_err(|error| format!("Failed to flush buffered PTY output: {error}"))?;
    }

    Ok(())
}

#[tauri::command]
pub fn detach_pty_output(
    terminal_state: State<'_, PtyState>,
    app_state: State<'_, AppStateStore>,
    session_id: Option<String>,
) -> Result<(), String> {
    let session_id = session_id
        .or_else(|| {
            app_state
                .active_terminal_session_id()
                .map(|session| session.0)
        })
        .ok_or_else(|| "No active terminal session found".to_string())?;

    with_session_runtime(
        &terminal_state.sessions,
        &session_id,
        || SessionRuntime::new(&session_id),
        |runtime| {
            runtime.output_channel = None;
        },
    );

    Ok(())
}

#[tauri::command]
pub fn write_to_pty(
    terminal_state: State<'_, PtyState>,
    app_state: State<'_, AppStateStore>,
    data: String,
    session_id: Option<String>,
) -> Result<(), String> {
    let session_id = session_id
        .or_else(|| {
            app_state
                .active_terminal_session_id()
                .map(|session| session.0)
        })
        .ok_or_else(|| "No active terminal session found".to_string())?;

    with_session_runtime(
        &terminal_state.sessions,
        &session_id,
        || SessionRuntime::new(&session_id),
        |runtime| {
            let writer = runtime
                .writer
                .as_mut()
                .ok_or_else(|| format!("Terminal shell {session_id} is not currently writable"))?;

            writer
                .write_all(data.as_bytes())
                .map_err(|error| format!("Failed to write to PTY: {error}"))?;
            writer
                .flush()
                .map_err(|error| format!("Failed to flush PTY writer: {error}"))
        },
    )
}

/// Write data to a PTY by session ID (non-Tauri helper for internal use).
pub fn write_to_pty_by_session(
    pty_state: &State<'_, PtyState>,
    session_id: &str,
    data: &str,
) -> Result<(), String> {
    with_session_runtime(
        &pty_state.sessions,
        session_id,
        || SessionRuntime::new(session_id),
        |runtime| {
            let writer = runtime
                .writer
                .as_mut()
                .ok_or_else(|| format!("Terminal shell {session_id} is not currently writable"))?;
            writer
                .write_all(data.as_bytes())
                .map_err(|e| format!("Failed to write to PTY: {e}"))?;
            writer
                .flush()
                .map_err(|e| format!("Failed to flush PTY writer: {e}"))
        },
    )
}

/// Write data to a PTY by session ID using a PtyState directly (for spawned threads).
pub fn write_to_pty_by_session_direct(
    pty_state: &PtyState,
    session_id: &str,
    data: &str,
) -> Result<(), String> {
    with_session_runtime(
        &pty_state.sessions,
        session_id,
        || SessionRuntime::new(session_id),
        |runtime| {
            let writer = runtime
                .writer
                .as_mut()
                .ok_or_else(|| format!("Terminal shell {session_id} is not currently writable"))?;
            writer
                .write_all(data.as_bytes())
                .map_err(|e| format!("Failed to write to PTY: {e}"))?;
            writer
                .flush()
                .map_err(|e| format!("Failed to flush PTY writer: {e}"))
        },
    )
}

#[tauri::command]
pub fn resize_pty(
    _app: AppHandle,
    terminal_state: State<'_, PtyState>,
    app_state: State<'_, AppStateStore>,
    rows: u16,
    cols: u16,
    session_id: Option<String>,
) -> Result<(), String> {
    if rows == 0 || cols == 0 {
        return Ok(());
    }

    let session_id = session_id
        .or_else(|| {
            app_state
                .active_terminal_session_id()
                .map(|session| session.0)
        })
        .ok_or_else(|| "No active terminal session found".to_string())?;

    with_session_runtime(
        &terminal_state.sessions,
        &session_id,
        || SessionRuntime::new(&session_id),
        |runtime| {
            let master = runtime
                .master
                .as_mut()
                .ok_or_else(|| format!("Terminal shell {session_id} is not currently resizable"))?;

            master
                .resize(PtySize {
                    rows,
                    cols,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(|error| format!("Failed to resize PTY: {error}"))
        },
    )?;

    app_state.update_terminal_session_size(&session_id, cols, rows);

    Ok(())
}

/// Clear stuck Working/Permission status for a terminal session.
/// Called from the frontend when the user presses Escape in a terminal
/// where an agent was processing — the agent stops but stays alive,
/// so the PTY exit cleanup never fires.
#[tauri::command]
pub fn clear_agent_status(session_id: String, app_state: State<'_, AppStateStore>, app: AppHandle) {
    app_state.clear_transient_pane_status_by_session(&session_id);
    state::emit_app_state(&app);
}

/// Spawn a PTY for an OpenFlow agent terminal session.
///
/// Unlike `spawn_pty_for_session` (which launches the user's default shell),
/// this function runs a specific command (e.g. `opencode`) with extra
/// environment variables injected for the agent role, run ID, and communication
/// log path.
///
/// `argv` must be non-empty; the first element is the executable and the rest
/// are arguments.  `extra_env` is a list of `(key, value)` pairs that will be
/// set on the spawned process on top of the normal Codemux env vars.
pub fn spawn_pty_for_agent(
    app: AppHandle,
    session_id: String,
    workspace_id: String,
    argv: Vec<String>,
    extra_env: Vec<(String, String)>,
    execution_policy: crate::execution::ExecutionPolicy,
) {
    let terminal_state: State<'_, PtyState> = app.state();
    let app_state: State<'_, AppStateStore> = app.state();
    let agent_store: State<'_, crate::openflow::AgentSessionStore> = app.state();
    let sessions = terminal_state.sessions.clone();
    let agent_store_inner = agent_store.clone_inner();

    let already_running = sessions
        .lock()
        .unwrap()
        .get(&session_id)
        .map(|r| r.writer.is_some() || r.master.is_some())
        .unwrap_or(false);

    if already_running {
        return;
    }

    let executable = match argv.first() {
        Some(e) => e.clone(),
        None => {
            emit_terminal_status(
                &app,
                &sessions,
                TerminalStatusPayload {
                    session_id,
                    state: TerminalLifecycleState::Failed,
                    message: Some("Agent spawn failed: empty argv".into()),
                    exit_code: None,
                },
            );
            return;
        }
    };

    let prepared = crate::execution::prepare_agent_command(
        executable.clone(),
        argv.iter().skip(1).cloned().collect(),
        &session_working_dir(&app_state, &session_id),
        &execution_policy,
    );

    emit_terminal_status(
        &app,
        &sessions,
        TerminalStatusPayload {
            session_id: session_id.clone(),
            state: TerminalLifecycleState::Starting,
            message: Some(format!(
                "Starting agent: {} [{}]",
                prepared.executable,
                match prepared.backend {
                    crate::execution::ExecutionBackendKind::HostPassthrough => "host_passthrough",
                    crate::execution::ExecutionBackendKind::LinuxBubblewrap => "linux_bubblewrap",
                    crate::execution::ExecutionBackendKind::MacOsSandbox => "macos_sandbox",
                    crate::execution::ExecutionBackendKind::WindowsRestricted =>
                        "windows_restricted",
                }
            )),
            exit_code: None,
        },
    );

    let pty_system = native_pty_system();
    let pty_pair = match pty_system.openpty(PtySize {
        rows: DEFAULT_ROWS,
        cols: DEFAULT_COLS,
        pixel_width: 0,
        pixel_height: 0,
    }) {
        Ok(pair) => pair,
        Err(error) => {
            emit_terminal_status(
                &app,
                &sessions,
                TerminalStatusPayload {
                    session_id,
                    state: TerminalLifecycleState::Failed,
                    message: Some(format!("Failed to open PTY for agent: {error}")),
                    exit_code: None,
                },
            );
            return;
        }
    };

    app_state.update_terminal_session_shell(&session_id, executable.clone());

    let cwd = session_working_dir(&app_state, &session_id);
    let mut cmd = CommandBuilder::new(&prepared.executable);
    for arg in &prepared.args {
        cmd.arg(arg);
    }
    cmd.cwd(cwd);

    // Declare terminal capabilities (see spawn_pty_for_session for rationale).
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env("TERM_PROGRAM", "codemux");
    cmd.env("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));

    // Standard Codemux env vars.
    cmd.env("CODEMUX", "1");
    cmd.env("CODEMUX_VERSION", env!("CARGO_PKG_VERSION"));
    cmd.env("CODEMUX_WORKSPACE_ID", &workspace_id);
    cmd.env("CODEMUX_SURFACE_ID", &session_id);
    cmd.env("CODEMUX_BROWSER_CMD", "codemux browser");
    cmd.env("BROWSER", "codemux browser open");

    // Worktree environment variables and dynamic agent context
    {
        let ws_snapshot = app_state.snapshot();
        if let Some(ws) = ws_snapshot
            .workspaces
            .iter()
            .find(|w| w.workspace_id.0 == workspace_id)
        {
            for (key, val) in workspace_pty_env(ws) {
                cmd.env(&key, val);
            }
        } else {
            cmd.env(
                "CODEMUX_AGENT_CONTEXT",
                crate::agent_context::build_agent_context(None, None, None, None),
            );
        }
    }

    if let Some((shim_dir, current_exe)) = ensure_openflow_cli_shims() {
        let current_path = env::var("PATH").unwrap_or_default();
        let prefixed_path = if current_path.is_empty() {
            shim_dir.clone()
        } else {
            format!("{shim_dir}:{current_path}")
        };
        cmd.env("PATH", prefixed_path);
        cmd.env("CODEMUX_CLI_SAFE_PATH", current_exe);
    }

    // Agent-specific env vars from the adapter.
    for (key, val) in &extra_env {
        cmd.env(key, val);
    }

    // This is wiring for the future sandbox backend selection. For now we pass
    // the intent through env so agent-side tooling and logs can see which
    // execution profile was selected, even before platform sandboxes are active.
    cmd.env(
        "CODEMUX_EXECUTION_BACKEND",
        match prepared.backend {
            crate::execution::ExecutionBackendKind::HostPassthrough => "host_passthrough",
            crate::execution::ExecutionBackendKind::LinuxBubblewrap => "linux_bubblewrap",
            crate::execution::ExecutionBackendKind::MacOsSandbox => "macos_sandbox",
            crate::execution::ExecutionBackendKind::WindowsRestricted => "windows_restricted",
        },
    );
    cmd.env(
        "CODEMUX_ALLOW_DESKTOP_GUI",
        if execution_policy.allow_desktop_gui {
            "1"
        } else {
            "0"
        },
    );
    cmd.env(
        "CODEMUX_ALLOW_BROWSER_AUTOMATION",
        if execution_policy.allow_browser_automation {
            "1"
        } else {
            "0"
        },
    );
    cmd.env(
        "CODEMUX_ALLOW_NETWORK",
        if execution_policy.allow_network {
            "1"
        } else {
            "0"
        },
    );

    let child = match pty_pair.slave.spawn_command(cmd) {
        Ok(child) => child,
        Err(error) => {
            emit_terminal_status(
                &app,
                &sessions,
                TerminalStatusPayload {
                    session_id,
                    state: TerminalLifecycleState::Failed,
                    message: Some(format!("Failed to spawn agent {executable}: {error}")),
                    exit_code: None,
                },
            );
            return;
        }
    };

    let guard = ChildGuard::new(child);
    let child_pid = guard.child.as_ref().and_then(|c| c.process_id());

    drop(pty_pair.slave);

    let mut reader = match pty_pair.master.try_clone_reader() {
        Ok(r) => r,
        Err(error) => {
            emit_terminal_status(
                &app,
                &sessions,
                TerminalStatusPayload {
                    session_id: session_id.clone(),
                    state: TerminalLifecycleState::Failed,
                    message: Some(format!("Failed to clone PTY reader for agent: {error}")),
                    exit_code: None,
                },
            );
            remove_session_runtime(&sessions, &session_id);
            return; // guard drops here, kills+waits on child
        }
    };

    let writer = match pty_pair.master.take_writer() {
        Ok(w) => w,
        Err(error) => {
            emit_terminal_status(
                &app,
                &sessions,
                TerminalStatusPayload {
                    session_id: session_id.clone(),
                    state: TerminalLifecycleState::Failed,
                    message: Some(format!("Failed to take PTY writer for agent: {error}")),
                    exit_code: None,
                },
            );
            remove_session_runtime(&sessions, &session_id);
            return; // guard drops here, kills+waits on child
        }
    };

    let mut child = guard.disarm();

    #[cfg(unix)]
    let poll_fd: PollFd = pty_pair.master.as_raw_fd().unwrap_or(-1);
    #[cfg(windows)]
    let poll_fd: PollFd = ();

    with_session_runtime(
        &sessions,
        &session_id,
        || SessionRuntime::new(&session_id),
        |runtime| {
            runtime.writer = Some(writer);
            runtime.master = Some(pty_pair.master);
            runtime.child_pid = child_pid;
        },
    );

    emit_terminal_status(
        &app,
        &sessions,
        TerminalStatusPayload {
            session_id: session_id.clone(),
            state: TerminalLifecycleState::Ready,
            message: Some(format!("Agent ready: {executable}")),
            exit_code: None,
        },
    );

    // Get communication log path from env vars
    let comm_log_path = extra_env
        .iter()
        .find(|(k, _)| k == "CODEMUX_COMMUNICATION_LOG")
        .map(|(_, v)| v.clone());
    // Prefer instance-specific ID (e.g. "builder-0") over bare role ("builder") so that
    // parallel agents of the same role are distinguishable in the comm log.
    let agent_role = extra_env
        .iter()
        .find(|(k, _)| k == "CODEMUX_AGENT_INSTANCE_ID")
        .or_else(|| extra_env.iter().find(|(k, _)| k == "CODEMUX_AGENT_ROLE"))
        .map(|(_, v)| v.clone());

    const COMM_LOG_FLUSH_INTERVAL: Duration = Duration::from_millis(500);
    const COMM_LOG_FLUSH_BATCH_SIZE: usize = 50;

    let read_sessions = sessions.clone();
    let read_session_id = session_id.clone();
    let log_lock_opt: Option<(Arc<Mutex<std::fs::File>>, String)> =
        match (comm_log_path.as_ref(), agent_role.as_ref()) {
            (Some(path), Some(role)) => Some((get_comm_log_lock(path), role.clone())),
            _ => None,
        };

    std::thread::spawn(move || {
        let mut comm_log_buffer: Vec<String> = Vec::new();
        let mut comm_last_flush = Instant::now();

        batched_reader_loop(
            &mut reader,
            poll_fd,
            &read_sessions,
            &read_session_id,
            |data| {
                // Buffer agent output for communication log (cleaned); flush periodically
                if let Some((ref log_lock, ref role)) = log_lock_opt {
                    if let Ok(text) = std::str::from_utf8(data) {
                        let cleaned = strip_ansi_codes(text);
                        let trimmed = cleaned.trim();

                        if !trimmed.is_empty()
                            && trimmed.len() > 2
                            && !trimmed.starts_with('\x1b')
                            && !trimmed.starts_with("No orchestration progress detected")
                            && !trimmed.starts_with("STOP: General Agent")
                            && !trimmed.chars().all(|c| {
                                c.is_whitespace()
                                    || c == '\u{2580}'
                                    || c == '\u{2584}'
                                    || c == '\u{2588}'
                                    || c == ' '
                            })
                        {
                            let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
                            let entry =
                                format!("[{}] [{}] {}\n", timestamp, role.to_uppercase(), trimmed);
                            comm_log_buffer.push(entry);
                            if comm_log_buffer.len() >= COMM_LOG_FLUSH_BATCH_SIZE
                                || comm_last_flush.elapsed() >= COMM_LOG_FLUSH_INTERVAL
                            {
                                if let Ok(mut file) = log_lock.lock() {
                                    for e in &comm_log_buffer {
                                        let _ = file.write_all(e.as_bytes());
                                    }
                                    let _ = file.flush();
                                }
                                comm_log_buffer.clear();
                                comm_last_flush = Instant::now();
                            }
                        }
                    }
                }
            },
        );

        // Flush any remaining comm log entries
        if let Some((ref log_lock, _)) = log_lock_opt {
            if !comm_log_buffer.is_empty() {
                if let Ok(mut file) = log_lock.lock() {
                    for e in &comm_log_buffer {
                        let _ = file.write_all(e.as_bytes());
                    }
                    let _ = file.flush();
                }
            }
        }
    });

    let wait_app = app.clone();
    let wait_sessions = sessions.clone();
    let wait_session_id = session_id.clone();
    let wait_agent_store = agent_store_inner.clone();

    fn decode_exit_status(exit_code: i32) -> (crate::openflow::agent::AgentSessionStatus, String) {
        match exit_code {
            0 => (
                crate::openflow::agent::AgentSessionStatus::Done,
                "Agent exited successfully".to_string(),
            ),
            1..=125 => (
                crate::openflow::agent::AgentSessionStatus::Failed,
                format!("Agent exited with code {}", exit_code),
            ),
            126 => (
                crate::openflow::agent::AgentSessionStatus::Failed,
                "Command not executable (permission denied or not executable)".to_string(),
            ),
            127 => (
                crate::openflow::agent::AgentSessionStatus::Failed,
                "Command not found".to_string(),
            ),
            128..=255 => {
                let signal = exit_code - 128;
                let signal_name = match signal {
                    1 => "SIGHUP",
                    2 => "SIGINT",
                    3 => "SIGQUIT",
                    4 => "SIGILL",
                    5 => "SIGTRAP",
                    6 => "SIGABRT",
                    7 => "SIGBUS",
                    8 => "SIGFPE",
                    9 => "SIGKILL",
                    10 => "SIGUSR1",
                    11 => "SIGSEGV",
                    12 => "SIGUSR2",
                    13 => "SIGPIPE",
                    14 => "SIGALRM",
                    15 => "SIGTERM",
                    16 => "SIGSTKFLT",
                    17 => "SIGCHLD",
                    18 => "SIGCONT",
                    19 => "SIGSTOP",
                    20 => "SIGTSTP",
                    21 => "SIGTTIN",
                    22 => "SIGTTOU",
                    23 => "SIGURG",
                    24 => "SIGXCPU",
                    25 => "SIGXFSZ",
                    26 => "SIGVTALRM",
                    27 => "SIGPROF",
                    28 => "SIGWINCH",
                    29 => "SIGIO",
                    30 => "SIGPWR",
                    31 => "SIGSYS",
                    _ => "UNKNOWN",
                };
                let reason = if signal == 9 {
                    "SIGKILL (likely OOM or explicit kill -9)".to_string()
                } else if signal == 15 {
                    "SIGTERM (terminated by signal)".to_string()
                } else {
                    format!("killed by signal {} ({})", signal, signal_name)
                };
                (crate::openflow::agent::AgentSessionStatus::Killed, reason)
            }
            _ => (
                crate::openflow::agent::AgentSessionStatus::Failed,
                format!("Agent exited with unexpected code {}", exit_code),
            ),
        }
    }

    std::thread::spawn(move || {
        let payload = match child.wait() {
            Ok(status) => {
                let (decoded_status, decoded_msg) = decode_exit_status(status.exit_code() as i32);
                if let Some(entry) = wait_agent_store
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get_mut(&wait_session_id)
                {
                    entry.status = decoded_status;
                }
                TerminalStatusPayload {
                    session_id: wait_session_id.clone(),
                    state: TerminalLifecycleState::Exited,
                    message: Some(decoded_msg),
                    exit_code: Some(status.exit_code()),
                }
            }
            Err(error) => {
                if let Some(entry) = wait_agent_store
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get_mut(&wait_session_id)
                {
                    entry.status = crate::openflow::agent::AgentSessionStatus::Failed;
                }
                TerminalStatusPayload {
                    session_id: wait_session_id.clone(),
                    state: TerminalLifecycleState::Failed,
                    message: Some(format!("Failed to wait for agent: {error}")),
                    exit_code: None,
                }
            }
        };

        crate::diagnostics::openflow_breadcrumb(&format!(
            "agent_exited session_id={} state={:?}",
            wait_session_id, payload.state
        ));

        // Send terminal reset sequences to xterm.js before tearing down.
        const TERMINAL_RESET: &[u8] =
            b"\x1b[?1049l\x1b[?1006l\x1b[?1003l\x1b[?1002l\x1b[?1000l\x1b[?2004l\x1b[?25h\x1b[0m";
        queue_or_send_output(&wait_sessions, &wait_session_id, TERMINAL_RESET.to_vec());

        emit_terminal_status(&wait_app, &wait_sessions, payload);

        // Clean up the session runtime to prevent memory leak
        remove_session_runtime(&wait_sessions, &wait_session_id);

        state::emit_app_state(&wait_app);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_sessions() -> Arc<Mutex<HashMap<String, SessionRuntime>>> {
        Arc::new(Mutex::new(HashMap::new()))
    }

    #[test]
    fn test_failed_status_does_not_create_ghost_entry() {
        let sessions = make_sessions();

        // with_existing_session_runtime should not create an entry
        let result = with_existing_session_runtime(&sessions, "ghost-session", |runtime| {
            runtime.last_status.state = TerminalLifecycleState::Failed;
        });

        assert!(
            result.is_none(),
            "should not create entry for non-existent session"
        );
        let guard = sessions.lock().unwrap();
        assert!(
            !guard.contains_key("ghost-session"),
            "no ghost entry should exist"
        );
    }

    #[test]
    fn test_starting_status_creates_entry() {
        let sessions = make_sessions();

        with_session_runtime(
            &sessions,
            "new-session",
            || SessionRuntime::new("new-session"),
            |runtime| {
                runtime.last_status = TerminalStatusPayload {
                    session_id: "new-session".to_string(),
                    state: TerminalLifecycleState::Starting,
                    message: Some("Starting shell...".into()),
                    exit_code: None,
                };
            },
        );

        let guard = sessions.lock().unwrap();
        assert!(
            guard.contains_key("new-session"),
            "Starting should create entry"
        );
    }

    #[test]
    fn test_get_session_pids_no_stale_after_cleanup() {
        let pty_state = PtyState::default();
        let sessions = pty_state.sessions.clone();

        with_session_runtime(
            &sessions,
            "sess-1",
            || SessionRuntime::new("sess-1"),
            |runtime| {
                runtime.child_pid = Some(12345);
            },
        );
        assert_eq!(pty_state.get_session_pids().len(), 1);

        // Simulate cleanup: update status then remove
        with_existing_session_runtime(&sessions, "sess-1", |runtime| {
            runtime.last_status.state = TerminalLifecycleState::Failed;
        });
        remove_session_runtime(&sessions, "sess-1");

        assert!(
            pty_state.get_session_pids().is_empty(),
            "no stale pids after cleanup"
        );
    }

    #[test]
    fn test_queue_or_send_output_buffers_chunks() {
        let sessions = make_sessions();
        with_session_runtime(&sessions, "sess", || SessionRuntime::new("sess"), |_| {});

        queue_or_send_output(&sessions, "sess", vec![1, 2, 3]);
        queue_or_send_output(&sessions, "sess", vec![4, 5, 6]);

        let guard = sessions.lock().unwrap();
        let runtime = guard.get("sess").unwrap();
        assert_eq!(runtime.pending_output.len(), 2);
        assert_eq!(runtime.pending_output[0], vec![1, 2, 3]);
        assert_eq!(runtime.pending_output[1], vec![4, 5, 6]);
    }

    #[test]
    fn test_ring_buffer_eviction() {
        let sessions = make_sessions();
        with_session_runtime(&sessions, "sess", || SessionRuntime::new("sess"), |_| {});

        for i in 0..OUTPUT_BUFFER_LIMIT + 10 {
            queue_or_send_output(&sessions, "sess", vec![i as u8]);
        }

        let guard = sessions.lock().unwrap();
        let runtime = guard.get("sess").unwrap();
        assert_eq!(runtime.pending_output.len(), OUTPUT_BUFFER_LIMIT);
        // Oldest 10 evicted; first remaining is chunk #10
        assert_eq!(runtime.pending_output[0], vec![10]);
    }

    #[test]
    fn test_flush_pty_batch_sends_and_resets() {
        let sessions = make_sessions();
        with_session_runtime(&sessions, "sess", || SessionRuntime::new("sess"), |_| {});

        let mut batch = vec![1u8, 2, 3, 4, 5];
        let mut last_flush = Instant::now() - Duration::from_secs(1);

        flush_pty_batch(&sessions, "sess", &mut batch, &mut last_flush);

        assert!(batch.is_empty(), "batch should be cleared after flush");
        assert!(
            last_flush.elapsed() < Duration::from_millis(100),
            "last_flush should be recent"
        );

        let guard = sessions.lock().unwrap();
        let runtime = guard.get("sess").unwrap();
        assert_eq!(runtime.pending_output.len(), 1);
        assert_eq!(runtime.pending_output[0], vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_flush_pty_batch_noop_on_empty() {
        let sessions = make_sessions();
        with_session_runtime(&sessions, "sess", || SessionRuntime::new("sess"), |_| {});

        let mut batch: Vec<u8> = Vec::new();
        let original_time = Instant::now() - Duration::from_secs(10);
        let mut last_flush = original_time;

        flush_pty_batch(&sessions, "sess", &mut batch, &mut last_flush);

        assert_eq!(
            last_flush, original_time,
            "last_flush should not change on empty batch"
        );
        let guard = sessions.lock().unwrap();
        let runtime = guard.get("sess").unwrap();
        assert_eq!(
            runtime.pending_output.len(),
            0,
            "no output should be queued"
        );
    }

    /// Verify that data written to a PTY appears in pending_output within
    /// PTY_BATCH_INTERVAL even when no further writes occur. This tests the
    /// poll()-based flush timeout in batched_reader_loop.
    #[cfg(unix)]
    #[test]
    fn test_batch_flushes_on_timeout_without_further_writes() {
        use portable_pty::{native_pty_system, PtySize};

        let sessions = make_sessions();
        with_session_runtime(
            &sessions,
            "test-pty",
            || SessionRuntime::new("test-pty"),
            |_| {},
        );

        // Open a real PTY pair.
        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 2,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("failed to open pty");

        let poll_fd = pair.master.as_raw_fd().expect("no raw fd");
        let mut reader = pair.master.try_clone_reader().expect("clone reader");
        let mut writer = pair.master.take_writer().expect("take writer");

        // Start the batched reader loop in a background thread.
        let read_sessions = sessions.clone();
        let reader_handle = std::thread::spawn(move || {
            batched_reader_loop(&mut reader, poll_fd, &read_sessions, "test-pty", |_| {});
        });

        // Write a small payload — well below PTY_BATCH_SIZE.
        //
        // IMPORTANT: keep `pair.slave` alive until AFTER the write. If we
        // drop it before the reader thread is scheduled, the PTY master
        // immediately sees POLLHUP and `poll_read_ready` returns true with
        // an empty read buffer. `reader.read()` then returns Ok(0) (EOF),
        // the loop flushes its empty batch and exits via the `Ok(_) =>`
        // break branch — BEFORE our write ever reaches it. That race is
        // why this test kept failing on loaded CI runners while passing
        // locally: fast dev machines hit the write first, slow CI runners
        // hit the poll first. Dropping the slave after the write
        // eliminates the race: by the time the slave closes, the reader
        // has already consumed the payload.
        let payload = b"hello from pty test\r\n";
        writer.write_all(payload).expect("write failed");
        writer.flush().expect("flush failed");
        drop(pair.slave);

        // Poll for up to `deadline` waiting for the batched reader thread to
        // flush. The key assertion is that data arrives WITHOUT another write
        // — not that it arrives in exactly 16ms. Previous iterations used a
        // single `sleep(PTY_BATCH_INTERVAL + 200ms)` which kept tripping on
        // loaded CI runners where thread scheduling jitter blew past the
        // margin. Polling gives fast machines a typical finish time of
        // ~16-50ms while still tolerating up to 3s of CI jitter — strictly
        // more headroom than a fixed sleep at no cost to local dev runs.
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let mut found = false;
        while std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
            {
                let guard = sessions.lock().unwrap();
                let runtime = guard.get("test-pty").unwrap();
                if !runtime.pending_output.is_empty() {
                    found = true;
                    break;
                }
            }
        }
        assert!(
            found,
            "data should appear in pending_output within 3s (PTY_BATCH_INTERVAL = {PTY_BATCH_INTERVAL:?})"
        );

        // Verify the content includes our payload.
        let content = {
            let guard = sessions.lock().unwrap();
            let runtime = guard.get("test-pty").unwrap();
            runtime
                .pending_output
                .iter()
                .flat_map(|c| c.iter().copied())
                .collect::<Vec<u8>>()
        };
        let content_str = String::from_utf8_lossy(&content);
        assert!(
            content_str.contains("hello from pty test"),
            "pending_output should contain the written payload, got: {content_str:?}"
        );

        // Clean up: drop the writer to close the PTY, which causes the reader loop
        // to see EOF and exit.
        drop(writer);
        reader_handle.join().expect("reader thread panicked");
    }

    // -- workspace_pty_env tests -------------------------------------------------

    fn test_workspace(
        id: &str,
        title: &str,
        cwd: &str,
        git_branch: Option<&str>,
        worktree_path: Option<&str>,
        project_root: Option<&str>,
    ) -> crate::state::WorkspaceSnapshot {
        use crate::state::*;
        WorkspaceSnapshot {
            workspace_id: WorkspaceId(id.to_string()),
            title: title.to_string(),
            workspace_type: WorkspaceType::Standard,
            cwd: cwd.to_string(),
            git_branch: git_branch.map(String::from),
            git_ahead: 0,
            git_behind: 0,
            git_additions: 0,
            git_deletions: 0,
            git_changed_files: 0,
            notification_count: 0,
            latest_agent_state: None,
            worktree_path: worktree_path.map(String::from),
            project_root: project_root.map(String::from),
            pr_number: None,
            pr_state: None,
            pr_url: None,
            linked_issue: None,
            tabs: Vec::new(),
            active_tab_id: String::new(),
            active_surface_id: SurfaceId(String::new()),
            surfaces: Vec::new(),
        }
    }

    fn env_map(ws: &crate::state::WorkspaceSnapshot) -> std::collections::HashMap<String, String> {
        workspace_pty_env(ws).into_iter().collect()
    }

    #[test]
    fn pty_env_sets_worktree_vars() {
        let ws = test_workspace(
            "ws-123",
            "my-feature",
            "/home/user/.codemux/worktrees/repo/my-feature",
            Some("feat/my-feature"),
            Some("/home/user/.codemux/worktrees/repo/my-feature"),
            Some("/home/user/projects/repo"),
        );
        let m = env_map(&ws);

        assert_eq!(m["CODEMUX_ROOT_PATH"], "/home/user/projects/repo");
        assert_eq!(
            m["CODEMUX_WORKSPACE_PATH"],
            "/home/user/.codemux/worktrees/repo/my-feature"
        );
        assert_eq!(m["CODEMUX_BRANCH"], "feat/my-feature");
        assert!(m.contains_key("CODEMUX_PORT"));
        assert_eq!(m["CODEMUX_WORKSPACE_NAME"], "my-feature");
    }

    #[test]
    fn pty_env_omits_branch_when_none() {
        let ws = test_workspace(
            "ws-456",
            "main",
            "/home/user/projects/repo",
            None,
            None,
            Some("/home/user/projects/repo"),
        );
        let m = env_map(&ws);

        assert!(!m.contains_key("CODEMUX_BRANCH"));
        assert_eq!(m["CODEMUX_ROOT_PATH"], "/home/user/projects/repo");
        assert_eq!(m["CODEMUX_WORKSPACE_PATH"], "/home/user/projects/repo");
    }

    #[test]
    fn pty_env_resolves_root_from_cwd_when_project_root_missing() {
        // When project_root is None, resolve_root_path falls back to cwd
        // if no .git directory is found.
        let ws = test_workspace("ws-789", "test-ws", "/tmp/no-git-here", None, None, None);
        let m = env_map(&ws);

        assert_eq!(m["CODEMUX_ROOT_PATH"], "/tmp/no-git-here");
    }

    #[test]
    fn pty_env_port_is_deterministic() {
        let ws1 = test_workspace("ws-abc", "ws", "/tmp", None, None, None);
        let ws2 = test_workspace("ws-abc", "ws", "/tmp", None, None, None);
        let m1 = env_map(&ws1);
        let m2 = env_map(&ws2);

        assert_eq!(m1["CODEMUX_PORT"], m2["CODEMUX_PORT"]);
    }

    #[test]
    fn pty_env_agent_context_contains_workspace_info() {
        let ws = test_workspace(
            "ws-123",
            "analyze-db",
            "/home/zeus/.codemux/worktrees/proj/analyze-db",
            Some("analyze-db"),
            Some("/home/zeus/.codemux/worktrees/proj/analyze-db"),
            Some("/home/zeus/projects/proj"),
        );
        let m = env_map(&ws);
        let ctx = &m["CODEMUX_AGENT_CONTEXT"];

        assert!(ctx.contains("Your workspace: analyze-db"));
        assert!(ctx.contains("Your working directory: /home/zeus/.codemux/worktrees/proj/analyze-db"));
        assert!(ctx.contains("Your branch: analyze-db"));
        assert!(ctx.contains("Original repo (reference only): /home/zeus/projects/proj"));
        assert!(ctx.contains("Do NOT create additional git worktrees"));
        assert!(ctx.contains("Do NOT cd to the original repo path"));
    }

    #[test]
    fn pty_env_agent_context_omits_worktree_when_not_worktree() {
        let ws = test_workspace(
            "ws-456",
            "main",
            "/home/user/projects/repo",
            Some("main"),
            None,
            Some("/home/user/projects/repo"),
        );
        let m = env_map(&ws);
        let ctx = &m["CODEMUX_AGENT_CONTEXT"];

        assert!(ctx.contains("Your workspace: main"));
        assert!(!ctx.contains("Your worktree:"));
        assert!(ctx.contains("Your branch: main"));
        assert!(ctx.contains("codemux browser"));
    }

    #[test]
    fn pty_env_always_includes_agent_context() {
        // Even with minimal workspace info, CODEMUX_AGENT_CONTEXT is present
        let ws = test_workspace("ws-min", "ws", "/tmp", None, None, None);
        let m = env_map(&ws);

        assert!(m.contains_key("CODEMUX_AGENT_CONTEXT"));
        assert!(m["CODEMUX_AGENT_CONTEXT"].contains("Codemux"));
        assert!(m["CODEMUX_AGENT_CONTEXT"].contains("codemux browser"));
    }

    // ── Bug 1: find_owning_workspace tests ────────────────────────

    /// Build a workspace with a single terminal pane in its surface tree.
    fn test_workspace_with_pane(
        id: &str,
        title: &str,
        cwd: &str,
        git_branch: Option<&str>,
        worktree_path: Option<&str>,
        project_root: Option<&str>,
        session_id: &str,
    ) -> crate::state::WorkspaceSnapshot {
        use crate::state::*;
        let mut ws = test_workspace(id, title, cwd, git_branch, worktree_path, project_root);
        ws.surfaces = vec![SurfaceSnapshot {
            surface_id: SurfaceId(format!("surf-{id}")),
            title: "Terminal".into(),
            root: PaneNodeSnapshot::Terminal {
                pane_id: PaneId(format!("pane-{session_id}")),
                session_id: SessionId(session_id.into()),
                title: "shell".into(),
            },
            active_pane_id: PaneId(format!("pane-{session_id}")),
        }];
        ws
    }

    fn test_snapshot(
        active_id: &str,
        workspaces: Vec<crate::state::WorkspaceSnapshot>,
    ) -> crate::state::AppStateSnapshot {
        use crate::state::*;
        AppStateSnapshot {
            schema_version: 1,
            active_workspace_id: WorkspaceId(active_id.into()),
            workspaces,
            terminal_sessions: Vec::new(),
            browser_sessions: Vec::new(),
            persistence: PersistenceSchema {
                schema_version: 1,
                stores_layout_metadata: true,
                stores_terminal_metadata: true,
                stores_live_process_state: false,
            },
            config: CodemuxConfigSnapshot {
                config_version: 1,
                default_shell: None,
                theme_source: "system".into(),
                linux_first: false,
                notification_sound_enabled: false,
                ai_commit_message_enabled: false,
                ai_commit_message_model: None,
                ai_resolver_enabled: false,
                ai_resolver_cli: None,
                ai_resolver_model: None,
                ai_resolver_strategy: "smart_merge".into(),
            },
            detected_ports: Vec::new(),
            notifications: Vec::new(),
            pane_statuses: std::collections::HashMap::new(),
            agent_browser_sessions: Vec::new(),
        }
    }

    #[test]
    fn find_owning_workspace_returns_correct_workspace() {
        let ws_a = test_workspace_with_pane(
            "ws-a", "ws-a", "/a", None, None, None, "sess-a",
        );
        let ws_b = test_workspace_with_pane(
            "ws-b", "ws-b", "/b", Some("feat"), Some("/b"), Some("/repo"), "sess-b",
        );
        let snapshot = test_snapshot("ws-a", vec![ws_a, ws_b]);

        // Session in workspace B should resolve to workspace B, not active workspace A
        let owner = find_owning_workspace(&snapshot, "sess-b");
        assert!(owner.is_some());
        assert_eq!(owner.unwrap().workspace_id.0, "ws-b");

        // Session in workspace A
        let owner_a = find_owning_workspace(&snapshot, "sess-a");
        assert!(owner_a.is_some());
        assert_eq!(owner_a.unwrap().workspace_id.0, "ws-a");
    }

    #[test]
    fn find_owning_workspace_returns_none_for_orphan() {
        let ws_a = test_workspace_with_pane(
            "ws-a", "ws-a", "/a", None, None, None, "sess-a",
        );
        let snapshot = test_snapshot("ws-a", vec![ws_a]);

        assert!(find_owning_workspace(&snapshot, "sess-orphan").is_none());
    }

    #[test]
    fn find_owning_workspace_handles_empty_surfaces() {
        let ws = test_workspace("ws-empty", "empty", "/e", None, None, None);
        let snapshot = test_snapshot("ws-empty", vec![ws]);

        // Workspace has no surfaces at all
        assert!(find_owning_workspace(&snapshot, "sess-any").is_none());
    }

    #[test]
    fn find_owning_workspace_searches_split_panes() {
        use crate::state::*;
        let mut ws = test_workspace("ws-split", "split", "/s", None, None, None);
        // Nested: split > split > terminal (3 levels deep)
        ws.surfaces = vec![SurfaceSnapshot {
            surface_id: SurfaceId("surf-split".into()),
            title: "Terminal".into(),
            root: PaneNodeSnapshot::Split {
                pane_id: PaneId("split-root".into()),
                direction: SplitDirection::Horizontal,
                child_sizes: vec![0.5, 0.5],
                children: vec![
                    PaneNodeSnapshot::Split {
                        pane_id: PaneId("split-inner".into()),
                        direction: SplitDirection::Vertical,
                        child_sizes: vec![0.5, 0.5],
                        children: vec![
                            PaneNodeSnapshot::Terminal {
                                pane_id: PaneId("pane-deep".into()),
                                session_id: SessionId("sess-deep".into()),
                                title: "deep".into(),
                            },
                            PaneNodeSnapshot::Terminal {
                                pane_id: PaneId("pane-1".into()),
                                session_id: SessionId("sess-1".into()),
                                title: "left-bottom".into(),
                            },
                        ],
                    },
                    PaneNodeSnapshot::Terminal {
                        pane_id: PaneId("pane-2".into()),
                        session_id: SessionId("sess-2".into()),
                        title: "right".into(),
                    },
                ],
            },
            active_pane_id: PaneId("pane-1".into()),
        }];
        let snapshot = test_snapshot("ws-split", vec![ws]);

        // Deeply nested terminal (split > split > terminal) is reachable
        assert!(find_owning_workspace(&snapshot, "sess-deep").is_some());
        assert!(find_owning_workspace(&snapshot, "sess-1").is_some());
        assert!(find_owning_workspace(&snapshot, "sess-2").is_some());
        assert!(find_owning_workspace(&snapshot, "sess-3").is_none());
    }

    #[test]
    fn owning_workspace_env_vars_use_correct_workspace() {
        // The actual bug scenario: workspace A is active, session belongs to workspace B.
        // Verify env vars come from workspace B, not workspace A.
        let ws_a = test_workspace_with_pane(
            "ws-a",
            "main-ws",
            "/home/user/projects/repo",
            Some("main"),
            None,
            Some("/home/user/projects/repo"),
            "sess-a",
        );
        let ws_b = test_workspace_with_pane(
            "ws-b",
            "feature-ws",
            "/home/user/.codemux/worktrees/repo/feature",
            Some("feat/feature"),
            Some("/home/user/.codemux/worktrees/repo/feature"),
            Some("/home/user/projects/repo"),
            "sess-b",
        );
        let snapshot = test_snapshot("ws-a", vec![ws_a, ws_b]);

        // Look up workspace B's session
        let owner = find_owning_workspace(&snapshot, "sess-b").unwrap();
        assert_eq!(owner.workspace_id.0, "ws-b");

        // Verify env vars come from workspace B
        let env = env_map(owner);
        assert_eq!(
            env["CODEMUX_WORKSPACE_PATH"],
            "/home/user/.codemux/worktrees/repo/feature"
        );
        assert_eq!(env["CODEMUX_BRANCH"], "feat/feature");
        assert_eq!(env["CODEMUX_ROOT_PATH"], "/home/user/projects/repo");
        assert_eq!(env["CODEMUX_WORKSPACE_NAME"], "feature-ws");

        // Verify workspace A would produce different values
        let owner_a = find_owning_workspace(&snapshot, "sess-a").unwrap();
        let env_a = env_map(owner_a);
        assert_eq!(env_a["CODEMUX_WORKSPACE_PATH"], "/home/user/projects/repo");
        assert_eq!(env_a["CODEMUX_BRANCH"], "main");
    }

    // ── resolve_session_cwd tests ─────────────────────────────────

    #[test]
    fn resolve_session_cwd_uses_scrollback_dir_when_exists() {
        // Use the platform temp dir as the "known-to-exist" path — `/tmp`
        // exists on Unix but not on Windows, and `std::env::temp_dir()`
        // resolves correctly on both (e.g. `/tmp` on Linux,
        // `C:\Users\<user>\AppData\Local\Temp` on Windows).
        let temp = std::env::temp_dir();
        let temp_str = temp.to_string_lossy().into_owned();
        let result = resolve_session_cwd(&temp_str, "/fallback");
        assert_eq!(result, temp_str);
    }

    #[test]
    fn resolve_session_cwd_falls_back_when_dir_missing() {
        let result = resolve_session_cwd(
            "/nonexistent/worktree/that/was/deleted",
            "/home/fallback",
        );
        assert_eq!(result, "/home/fallback");
    }

    #[test]
    fn resolve_session_cwd_falls_back_on_empty_string() {
        let result = resolve_session_cwd("", "/home/fallback");
        assert_eq!(result, "/home/fallback");
    }
}
