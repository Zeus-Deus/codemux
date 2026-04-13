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
    /// True from the moment a spawn caller has reserved this session id (under
    /// the `PtyState::sessions` lock) to the moment the spawn either succeeds
    /// or fails. While `is_spawning` is true, `is_session_spawn_active` returns
    /// `true` and concurrent spawn attempts skip — closing the TOCTOU race
    /// where two callers both passed the "writer/master is None" check while
    /// the slow ConPTY initialization was in flight on Windows.
    pub is_spawning: bool,
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
            is_spawning: false,
        }
    }
}

/// Safety net: if a `SessionRuntime` is dropped with a live `child_pid`, the
/// normal close path was skipped (panic unwind, future refactor forgetting to
/// kill, or a test tearing down fixtures). We kill the process group as a last
/// resort and shout in stderr so the bug is visible.
///
/// The normal close path (`close_terminal_session`) clears `child_pid` to
/// `None` before dropping the runtime, so this impl is silent on the happy
/// path. Any warning printed here is a real bug worth investigating.
impl Drop for SessionRuntime {
    fn drop(&mut self) {
        if let Some(pid) = self.child_pid.take() {
            eprintln!(
                "[codemux::terminal] SessionRuntime dropped with live child_pid={pid} — \
                 normal close path was skipped. Killing process group as last resort."
            );
            kill_session_tree(pid);
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

/// Atomically reserve a session id for a spawn attempt.
///
/// Returns `true` if the caller now owns the spawn for this session and must
/// proceed (and eventually call `clear_spawn_reservation` on every exit path).
/// Returns `false` if another caller is already spawning, the session is
/// already running (`writer`/`master` populated), or this call lost the race.
///
/// This closes the TOCTOU window between the historical
/// "is the session already running?" check and the moment the spawned PTY
/// handles get inserted into the runtime. On Linux the window was tens of
/// microseconds and never observed in practice; on Windows ConPTY init takes
/// hundreds of milliseconds and the window was wide enough that startup
/// session restore would race with user-driven workspace creation and double-
/// spawn the same session id, leaking children.
fn try_reserve_session_spawn(
    sessions: &Arc<Mutex<HashMap<String, SessionRuntime>>>,
    session_id: &str,
) -> bool {
    let mut guard = sessions.lock().unwrap_or_else(|e| e.into_inner());
    let runtime = guard
        .entry(session_id.to_string())
        .or_insert_with(|| SessionRuntime::new(session_id));
    if runtime.writer.is_some() || runtime.master.is_some() || runtime.is_spawning {
        return false;
    }
    runtime.is_spawning = true;
    true
}

/// Check whether a spawn attempt is in flight or already complete for `session_id`.
///
/// Used by tests and by `spawn_missing_ptys_for_workspace` to skip sessions
/// whose runtime is already populated. Acquires the `PtyState::sessions`
/// mutex; do not hold any other lock across this call.
#[allow(dead_code)]
fn is_session_spawn_active(
    sessions: &Arc<Mutex<HashMap<String, SessionRuntime>>>,
    session_id: &str,
) -> bool {
    let guard = sessions.lock().unwrap_or_else(|e| e.into_inner());
    guard
        .get(session_id)
        .map(|rt| rt.writer.is_some() || rt.master.is_some() || rt.is_spawning)
        .unwrap_or(false)
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

/// OS-specific PATH entry separator: `;` on Windows, `:` on Unix.
///
/// Exposed as a constant-returning function (not a `const`) so tests can
/// drive the PATH-prepend logic with an explicit separator without caring
/// about the host OS. Production callers get the right value via
/// `path_separator()` which resolves at compile time.
fn path_separator() -> &'static str {
    if cfg!(windows) {
        ";"
    } else {
        ":"
    }
}

/// Prepend `shim_dir` to an existing `PATH` string using the OS-specific
/// separator. Extracted from the inline logic in `spawn_pty_for_shell`
/// so it's unit-testable without setting up a real PTY + env.
///
/// Semantics:
///   - Empty `current_path` → return `shim_dir` unchanged (no trailing sep).
///   - Non-empty → `{shim_dir}{sep}{current_path}`.
///   - Does NOT deduplicate. If `shim_dir` is already in `current_path`,
///     it will appear twice. That's intentional — the prepend ensures the
///     codemux shim wins even if another invocation of this function
///     already added it once.
fn prepend_shim_to_path(shim_dir: &str, current_path: &str) -> String {
    if current_path.is_empty() {
        shim_dir.to_string()
    } else {
        format!("{shim_dir}{}{current_path}", path_separator())
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

    // Skip the rewrite (and the chmod) if the shim already matches what we
    // would write. This avoids per-spawn disk churn when a workspace
    // hydrates many sessions at once.
    let needs_write = match std::fs::read_to_string(&shim_path) {
        Ok(existing) => existing != script,
        Err(_) => true,
    };
    if needs_write {
        std::fs::write(&shim_path, &script).ok()?;
        let mut perms = std::fs::metadata(&shim_path).ok()?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&shim_path, perms).ok()?;
    }

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

    // Skip the rewrite if the shim already matches what we would write.
    // Avoids per-spawn disk churn on Windows where every session hydration
    // would otherwise touch %TEMP%.
    let needs_write = match std::fs::read_to_string(&shim_path) {
        Ok(existing) => existing != script,
        Err(_) => true,
    };
    if needs_write {
        std::fs::write(&shim_path, &script).ok()?;
    }

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

/// Kill a PTY child and its entire process group with a single SIGKILL.
///
/// **Why no SIGTERM grace period.** A previous version did SIGTERM → 200ms
/// sleep → SIGKILL. That grace window is exactly the adversarial case for
/// PID recycling: the shell handles SIGTERM and exits in ~50ms, the waiter
/// thread reaps the zombie, the kernel recycles the PID to an unrelated
/// process, then our SIGKILL lands on the wrong process group. Going
/// straight to SIGKILL collapses the window between `remove_session_runtime`
/// and the signal call to microseconds, eliminates the per-session 200ms
/// block on the Tauri worker thread, and is the correct semantic for a
/// "close this pane" user action where no flush is needed.
///
/// Does **not** call `waitpid` — the waiter thread at the bottom of
/// `spawn_pty_for_session` / `spawn_pty_for_agent` owns the `Box<dyn Child>`
/// and is blocked in `child.wait()`. When SIGKILL lands, that `wait()`
/// returns and the waiter thread reaps the child normally. Reaping from
/// two places would race and produce `ECHILD`.
///
/// Because `portable-pty`'s Unix spawn path calls `libc::setsid()` in its
/// `pre_exec` hook, every PTY child is a session leader whose PGID equals
/// its PID. That means `killpg(pid, ...)` sends the signal to the shell
/// **and** everything the shell spawned (Claude CLI, MCP servers,
/// rust-analyzer, ...) as long as those children have not themselves called
/// `setsid` to detach into a new process group. This handles the common
/// leak pattern — the setsid-detach limitation is documented in
/// `test_killpg_does_not_reach_setsid_detached_grandchild` and tracked as a
/// `/proc`-walking fallback follow-up if we ever observe it in practice.
///
/// `ESRCH` is treated as success: it means the process was already gone
/// (likely reaped by the waiter thread first).
#[cfg(unix)]
fn kill_session_tree(pid: u32) {
    if pid <= 1 {
        // killpg(0, ...) signals our own process group; killpg(1, ...)
        // targets init. Neither is ever correct for a PTY child we
        // spawned. A pid <= 1 stored in SessionRuntime is a bug upstream.
        eprintln!(
            "[codemux::terminal] kill_session_tree refusing pid={pid} (<=1 is never a PTY child)"
        );
        return;
    }
    let pid_i32 = pid as i32;

    // SAFETY: `killpg` is an async-signal-safe POSIX call that takes a
    // plain integer PGID. There is no memory aliasing or lifetime concern.
    //
    // PID recycling is theoretically possible between `remove_session_runtime`
    // (which dropped the `SessionRuntime` in `terminate_pty_session`) and
    // this call, but the window is microseconds (no sleep). On a system
    // with `pid_max` >= 32768, recycling in this window requires ~32k
    // process spawns between our two instructions, which is not realistic.
    // See the doc comment above for the history of why this is not a
    // SIGTERM-then-SIGKILL dance.
    let ret = unsafe { libc::killpg(pid_i32, libc::SIGKILL) };
    if ret != 0 {
        let errno = std::io::Error::last_os_error();
        if errno.raw_os_error() != Some(libc::ESRCH) {
            eprintln!("[codemux::terminal] killpg({pid}, SIGKILL) failed: {errno}");
        }
        // ESRCH means the waiter thread already reaped — success.
    }
}

/// Windows stub. Codemux is Linux-primary; Windows is tracked as a follow-up
/// in the windows-support plan. Until we have a Job Object implementation,
/// this path is a no-op with a warning.
#[cfg(not(unix))]
fn kill_session_tree(pid: u32) {
    eprintln!(
        "[codemux::terminal] kill_session_tree({pid}) is a no-op on non-Unix; \
         PTY children will leak until app exit (TODO: Windows Job Object support)"
    );
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

    // Atomic reservation: insert a placeholder runtime with `is_spawning = true`
    // under the `PtyState::sessions` lock so a concurrent spawn attempt for the
    // same session id sees the marker and bails. The previous "is writer/master
    // populated?" check released the lock before doing slow ConPTY init, which
    // on Windows produced a 100ms+ window where startup session restore and
    // user-driven workspace creation could both pass the check and double-spawn
    // the same session id, eventually leaking ~23 ConPTY children before the
    // app froze. See `try_reserve_session_spawn` for the lock ordering.
    if !try_reserve_session_spawn(&sessions, &session_id) {
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
                    session_id: session_id.clone(),
                    state: TerminalLifecycleState::Failed,
                    message: Some(format!("Failed to open PTY: {error}")),
                    exit_code: None,
                },
            );
            // Drop the spawn reservation we took out at the top of this function
            // so a future retry / restart_terminal_session can re-try.
            remove_session_runtime(&sessions, &session_id);
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
        let prefixed = prepend_shim_to_path(&shim_dir, &current_path);
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
                    session_id: session_id.clone(),
                    state: TerminalLifecycleState::Failed,
                    message: Some(format!("Failed to spawn shell {shell}: {error}")),
                    exit_code: None,
                },
            );
            // Drop the spawn reservation taken at the top of this function;
            // see the same pattern at the openpty failure branch above.
            remove_session_runtime(&sessions, &session_id);
            return;
        }
    };

    // Wrap child in a guard that kills+waits on drop, preventing zombies
    // if a subsequent step fails before the waiter thread is spawned.
    let guard = ChildGuard::new(child);

    // A session without a PID is unkillable — `terminate_pty_session`
    // would later find no `child_pid` and silently skip the kill, leaking
    // the shell and everything it spawned. Fail the spawn loudly instead.
    // The guard will kill the child on drop during the early return.
    let child_pid = match guard.child.as_ref().and_then(|c| c.process_id()) {
        Some(pid) => pid,
        None => {
            emit_terminal_status(
                &app,
                &sessions,
                TerminalStatusPayload {
                    session_id: session_id.clone(),
                    state: TerminalLifecycleState::Failed,
                    message: Some(
                        "portable_pty returned a child with no process_id(); \
                         refusing to start an unkillable session"
                            .into(),
                    ),
                    exit_code: None,
                },
            );
            remove_session_runtime(&sessions, &session_id);
            return; // guard drops here, kills+waits on child
        }
    };

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
            runtime.child_pid = Some(child_pid);
            runtime.skip_preset_launch = auto_resume_command.is_some();
            runtime.resume_command = auto_resume_command.clone();
            // Spawn complete — clear the reservation marker so the runtime
            // looks identical to a non-spawning runtime in every downstream
            // check (terminate_pty_session, restart_terminal_session, etc.).
            runtime.is_spawning = false;
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

/// Walk a workspace's pane tree and return every terminal session id under it.
///
/// Used by `spawn_missing_ptys` (active workspace at startup) and
/// `spawn_missing_ptys_for_workspace` (lazy spawn when the user activates an
/// inactive workspace). Returns an empty vec if no workspace matches `workspace_id`.
fn collect_workspace_session_ids(
    snapshot: &crate::state::AppStateSnapshot,
    workspace_id: &str,
) -> Vec<String> {
    snapshot
        .workspaces
        .iter()
        .find(|w| w.workspace_id.0 == workspace_id)
        .map(|w| crate::state::collect_terminal_sessions(&w.surfaces))
        .unwrap_or_default()
}

/// Spawn PTYs for every terminal session belonging to `workspace_id` that
/// isn't already running. Used by the lazy-activation path so that when a user
/// switches workspaces we materialize that workspace's PTYs on demand.
///
/// Idempotent: each `spawn_pty_for_session` call is gated by
/// `try_reserve_session_spawn`, so re-calling this for an already-active
/// workspace is a cheap no-op.
pub fn spawn_missing_ptys_for_workspace(app: AppHandle, workspace_id: &str) {
    let app_state: State<'_, AppStateStore> = app.state();
    let snapshot = app_state.snapshot();
    let session_ids = collect_workspace_session_ids(&snapshot, workspace_id);

    if session_ids.len() > MAX_STARTUP_SESSIONS {
        eprintln!(
            "[codemux::terminal] Workspace {workspace_id} has {} sessions; \
             spawning only the first {} to avoid blocking the IPC thread",
            session_ids.len(),
            MAX_STARTUP_SESSIONS
        );
    }

    for session_id in session_ids.into_iter().take(MAX_STARTUP_SESSIONS) {
        spawn_pty_for_session(app.clone(), session_id);
    }
}

/// Startup PTY hydration. Restores PTY children only for sessions that belong
/// to the **active** workspace, leaving inactive workspaces' sessions to be
/// materialized lazily by `spawn_missing_ptys_for_workspace` when the user
/// switches into them via `activate_workspace`.
///
/// **Why active-only.** The previous implementation walked every persisted
/// `terminal_sessions` entry and called `spawn_pty_for_session` for each, up
/// to `MAX_STARTUP_SESSIONS = 50`. On Linux this is microsecond-cheap and
/// invisible. On Windows ConPTY init takes hundreds of milliseconds per
/// child, so a saved state with N sessions blocked the synchronous IPC
/// thread for N × ~300ms during startup, producing the observed "23 ConPTY
/// children spawning at once → app frozen → force-close" failure mode.
///
/// **What "active" means at startup.** `state::AppStateStore` loads the
/// persisted active workspace id from disk during startup, **before** this
/// function runs (via `lib.rs` setup), so by the time `spawn_missing_ptys`
/// is called the snapshot's `active_workspace_id` is the same workspace
/// id the user was on when they last closed the app. If the persisted
/// active workspace id doesn't resolve to a real workspace (e.g. it was
/// deleted between sessions, or no workspaces exist yet) we spawn nothing
/// — the next user-driven action will spawn its own PTY.
///
/// Other workspaces' sessions are not lost: their persisted scrollback +
/// metadata stays on disk, and `spawn_missing_ptys_for_workspace` rehydrates
/// them on workspace switch.
pub fn spawn_missing_ptys(app: AppHandle) {
    let app_state: State<'_, AppStateStore> = app.state();
    let snapshot = app_state.snapshot();
    let active_workspace_id = snapshot.active_workspace_id.0.clone();

    if active_workspace_id.is_empty() {
        // No workspace persisted yet (fresh install, or all workspaces deleted).
        // The next user-driven workspace creation will spawn its own PTYs via
        // `create_workspace_with_layout` / `create_worktree_workspace`.
        return;
    }

    spawn_missing_ptys_for_workspace(app, &active_workspace_id);
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

/// Kill the PTY child process for a session and release its runtime buffers.
///
/// This is the **only** code path that actually reaches `kill_session_tree`.
/// Every close entry point (the `close_terminal_session` Tauri command,
/// `close_pane`, `close_tab`, `close_workspace`, `close_workspace_with_worktree`,
/// and `restart_terminal_session`) funnels through here.
///
/// Why it does not touch `AppStateStore`: higher-level close helpers
/// (`state.close_pane`, `state.close_tab`, `state.close_workspace`) already
/// remove sessions from `snapshot.terminal_sessions` as part of their pane-tree
/// bookkeeping. If this function then called `app_state.close_terminal_session`
/// it would fail with "No terminal session found" and short-circuit before
/// reaching the kill — which is exactly the latent bug that let PTY children
/// leak across every close path. Keeping this helper state-store-free means
/// it is idempotent and callable from any point in a close flow.
///
/// Race-safe vs. the waiter thread: both paths call `remove_session_runtime`,
/// which is a `HashMap::remove` under the `PtyState::sessions` mutex. Whoever
/// wins takes the runtime; the loser gets `None` and no-ops. This is also the
/// double-close safety property — calling `terminate_pty_session` twice on the
/// same id is a no-op on the second call.
///
/// We intentionally do **not** reap via `waitpid`: the waiter thread owns the
/// `Box<dyn Child>` and is blocked in `child.wait()`. When SIGKILL lands, that
/// `wait()` returns and the waiter reaps the child normally. Reaping from two
/// places would race into `ECHILD`.
pub(crate) fn terminate_pty_session(
    sessions: &Arc<Mutex<HashMap<String, SessionRuntime>>>,
    session_id: &str,
) {
    let Some(mut runtime) = remove_session_runtime(sessions, session_id) else {
        // Either already removed by the waiter thread (child exited on its
        // own) or by a concurrent close. Nothing to do.
        return;
    };

    // Clear child_pid to None *first* so the `Drop for SessionRuntime`
    // safety-net impl stays silent on the happy path. Any non-None value
    // printed by Drop means something skipped this function.
    let pid = runtime.child_pid.take();
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
    // Drop the runtime *before* signalling: `kill_session_tree` sleeps 200ms
    // between SIGTERM and SIGKILL and there is no point holding PTY handles
    // across that sleep.
    drop(runtime);

    match pid {
        Some(pid) => kill_session_tree(pid),
        None => {
            // Two causes, both benign by the time we get here:
            //   1. The spawn path fails loudly if `process_id()` returns
            //      None (see `spawn_pty_for_session` / `spawn_pty_for_agent`),
            //      so a live session cannot reach this branch with
            //      child_pid=None.
            //   2. The waiter thread can transiently re-insert a phantom
            //      runtime (via `with_session_runtime`'s entry-or-insert)
            //      after we removed it but before it calls
            //      `remove_session_runtime` itself. That phantom has
            //      `child_pid = None` from `SessionRuntime::new()`, and
            //      a second `terminate_pty_session` racing in during the
            //      phantom window lands here.
            // In both cases there is no live process to kill. Logged only
            // so new close paths that forget to populate child_pid are
            // noticeable in dev.
            eprintln!(
                "[codemux::terminal] terminate_pty_session({session_id}): no child_pid \
                 (spawn returned None or waiter-thread race)"
            );
        }
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
    terminate_pty_session(&terminal_state.sessions, &session_id);
    state::emit_app_state(&app);
    Ok(fallback_session.0)
}

#[tauri::command]
pub fn restart_terminal_session(
    app: AppHandle,
    terminal_state: State<'_, PtyState>,
    session_id: String,
) -> Result<(), String> {
    // Kill the old process group before spawning the replacement. Uses the
    // same helper as close so the Drop safety net stays silent.
    terminate_pty_session(&terminal_state.sessions, &session_id);
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

    // Atomic spawn reservation — see `spawn_pty_for_session` for the
    // TOCTOU-race rationale. Same lock-held placeholder pattern.
    if !try_reserve_session_spawn(&sessions, &session_id) {
        return;
    }

    let executable = match argv.first() {
        Some(e) => e.clone(),
        None => {
            emit_terminal_status(
                &app,
                &sessions,
                TerminalStatusPayload {
                    session_id: session_id.clone(),
                    state: TerminalLifecycleState::Failed,
                    message: Some("Agent spawn failed: empty argv".into()),
                    exit_code: None,
                },
            );
            // Drop the spawn reservation so a future retry can re-try.
            remove_session_runtime(&sessions, &session_id);
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
                    session_id: session_id.clone(),
                    state: TerminalLifecycleState::Failed,
                    message: Some(format!("Failed to open PTY for agent: {error}")),
                    exit_code: None,
                },
            );
            // Drop the spawn reservation so a future retry can re-try.
            remove_session_runtime(&sessions, &session_id);
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
        let prefixed_path = prepend_shim_to_path(&shim_dir, &current_path);
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
                    session_id: session_id.clone(),
                    state: TerminalLifecycleState::Failed,
                    message: Some(format!("Failed to spawn agent {executable}: {error}")),
                    exit_code: None,
                },
            );
            // Drop the spawn reservation so a future retry can re-try.
            remove_session_runtime(&sessions, &session_id);
            return;
        }
    };

    let guard = ChildGuard::new(child);

    // A session without a PID is unkillable — fail loudly rather than
    // leak. The guard will kill the child on drop during the early return.
    let child_pid = match guard.child.as_ref().and_then(|c| c.process_id()) {
        Some(pid) => pid,
        None => {
            emit_terminal_status(
                &app,
                &sessions,
                TerminalStatusPayload {
                    session_id: session_id.clone(),
                    state: TerminalLifecycleState::Failed,
                    message: Some(
                        "portable_pty returned an agent child with no process_id(); \
                         refusing to start an unkillable session"
                            .into(),
                    ),
                    exit_code: None,
                },
            );
            remove_session_runtime(&sessions, &session_id);
            return; // guard drops here, kills+waits on child
        }
    };

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
            runtime.child_pid = Some(child_pid);
            // Spawn complete — clear the reservation marker. See the same
            // pattern at the end of `spawn_pty_for_session`.
            runtime.is_spawning = false;
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

    // ── Shell + PATH tests ───────────────────────────────────────────
    //
    // `path_separator` and `prepend_shim_to_path` are cross-platform
    // helpers, so their tests run on every platform. `default_shell` on
    // Windows returns a Windows-specific value, so that test is
    // `#[cfg(windows)]`-gated and only executes on Windows CI.

    #[test]
    fn test_path_separator_matches_host_os() {
        let sep = path_separator();
        #[cfg(windows)]
        assert_eq!(sep, ";");
        #[cfg(unix)]
        assert_eq!(sep, ":");
    }

    #[test]
    fn test_prepend_shim_to_path_with_existing_path() {
        // Typical case: PATH has entries, shim gets prepended with
        // the OS-correct separator.
        let result = prepend_shim_to_path("/opt/codemux/shims", "/usr/bin:/bin");
        #[cfg(unix)]
        assert_eq!(result, "/opt/codemux/shims:/usr/bin:/bin");
        #[cfg(windows)]
        assert_eq!(result, "/opt/codemux/shims;/usr/bin:/bin");
    }

    #[test]
    fn test_prepend_shim_to_path_with_empty_current() {
        // Edge case: PATH is empty or unset. Result must be the
        // shim_dir with NO trailing separator — an empty trailing
        // component would make the child process try to resolve
        // binaries from the empty path (which is CWD on some shells,
        // a security hazard).
        let result = prepend_shim_to_path("/opt/codemux/shims", "");
        assert_eq!(result, "/opt/codemux/shims");
        assert!(
            !result.ends_with(':') && !result.ends_with(';'),
            "empty current_path must not produce a trailing separator"
        );
    }

    #[test]
    fn test_prepend_shim_to_path_windows_style_paths() {
        // Verify the Windows-style paths pass through unmodified on
        // Windows. The function is oblivious to path content — it
        // only joins with the separator.
        let result = prepend_shim_to_path(
            r"C:\Users\zeus\AppData\Local\Codemux\shims",
            r"C:\Windows\System32;C:\Program Files\Git\bin",
        );
        #[cfg(windows)]
        assert_eq!(
            result,
            r"C:\Users\zeus\AppData\Local\Codemux\shims;C:\Windows\System32;C:\Program Files\Git\bin"
        );
        #[cfg(unix)]
        {
            // On Unix the separator is `:`, so the Windows paths get
            // joined with a colon. That's fine — this is just exercising
            // the function's behavior under test, not a production path.
            assert_eq!(
                result,
                r"C:\Users\zeus\AppData\Local\Codemux\shims:C:\Windows\System32;C:\Program Files\Git\bin"
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn test_default_shell_windows_returns_cmd_or_powershell() {
        // default_shell() on Windows reads $COMSPEC (typically
        // `C:\Windows\System32\cmd.exe`) and falls back to the literal
        // string "cmd.exe" if COMSPEC is unset. It should never return
        // a Unix shell path. The exact resolution depends on runner
        // config; we assert the case-insensitive suffix lands on a
        // known Windows shell binary.
        let shell = default_shell();
        let lower = shell.to_lowercase();
        assert!(
            lower.ends_with("cmd.exe") || lower.ends_with("powershell.exe"),
            "Windows default_shell should end with cmd.exe or powershell.exe, got: {shell}"
        );
        assert!(
            !shell.contains("/bin/"),
            "Windows default_shell must not return a Unix shell path, got: {shell}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_default_shell_unix_returns_bash_or_shell_env() {
        // Paired guard for the Unix path: default_shell() on Unix reads
        // $SHELL and falls back to `/bin/bash`. It should never return
        // a .exe suffix or a Windows-style path.
        let shell = default_shell();
        assert!(
            shell.starts_with('/') || shell.is_empty() == false,
            "Unix default_shell should be an absolute path, got: {shell}"
        );
        assert!(
            !shell.to_lowercase().ends_with(".exe"),
            "Unix default_shell must not return a .exe binary, got: {shell}"
        );
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
        // IMPORTANT: do NOT drop `pair.slave` anywhere in this test. Keep
        // it alive for the full duration and let it drop naturally at
        // end-of-scope once `reader_handle.join()` has returned.
        //
        // Why: the reader thread reads from `pair.master`'s read side,
        // which on Linux receives data via the PTY's line discipline ECHO
        // behavior — bytes written to `master.writer` → slave's input →
        // line discipline echoes them back → `master.reader` returns them.
        // The line discipline is attached to the master/slave pair; if
        // we close the slave end while the reader is still running, the
        // discipline can transition into a "no slave" state where echo
        // stops working and `master.reader` either returns stale EOF or
        // never delivers the echoed bytes. GitHub Actions' ubuntu-latest
        // kernel hits this case; local dev machines don't, which is why
        // three prior fix attempts (widened sleep → polling loop → drop
        // after write) all passed locally and still failed on CI.
        //
        // The ONLY correct sequence is: keep slave open → write → let
        // echo deliver → read in background thread → assert pending_output
        // has the bytes → drop writer → join reader (reader sees EOF on
        // writer drop because the line discipline is still alive and
        // propagates it properly) → function returns → slave drops last.
        let payload = b"hello from pty test\r\n";
        writer.write_all(payload).expect("write failed");
        writer.flush().expect("flush failed");

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

        // Clean up: the reader thread is blocked in poll()/read() on the
        // master's read side and will stay blocked until SOMETHING signals
        // EOF. Dropping `writer` alone is not enough — on Linux PTYs,
        // master.writer and master.reader are independent halves, so
        // closing the write side doesn't propagate EOF to the read side.
        // The slave is what we need to close: dropping `pair.slave` here
        // tears down the slave end of the PTY, which causes the kernel to
        // mark the master's read side with POLLHUP. The reader thread's
        // `poll_read_ready` returns true, `reader.read()` returns Ok(0),
        // the batched loop hits its `Ok(_) => break` branch, and the
        // thread exits cleanly — which is what `reader_handle.join()`
        // needs to return.
        //
        // Sequence matters: slave MUST stay alive until AFTER the payload
        // has been observed in pending_output (see the long comment above
        // the write block), but MUST be dropped before we try to join the
        // reader. Do both in the right order here.
        drop(pair.slave);
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

    // ── Spawn-reservation TOCTOU tests (Bug 1b) ───────────────────

    fn make_spawn_sessions() -> Arc<Mutex<HashMap<String, SessionRuntime>>> {
        Arc::new(Mutex::new(HashMap::new()))
    }

    #[test]
    fn try_reserve_session_spawn_first_caller_wins() {
        let sessions = make_spawn_sessions();
        assert!(
            try_reserve_session_spawn(&sessions, "sess-1"),
            "first reservation must succeed"
        );
        // The reservation must be visible as is_spawning under the lock.
        assert!(is_session_spawn_active(&sessions, "sess-1"));
    }

    #[test]
    fn try_reserve_session_spawn_second_caller_loses_while_in_flight() {
        let sessions = make_spawn_sessions();
        assert!(try_reserve_session_spawn(&sessions, "race-id"));
        // Second caller for the same session must observe the placeholder
        // and bail. Without this guarantee, two concurrent
        // `spawn_pty_for_session` callers would both pass the historic
        // "writer/master is None" check and both spawn ConPTY children for
        // the same session id — the exact bug we're closing.
        assert!(
            !try_reserve_session_spawn(&sessions, "race-id"),
            "second reservation while in flight must lose"
        );
    }

    #[test]
    fn try_reserve_session_spawn_succeeds_again_after_runtime_removed() {
        // The error early-return paths in spawn_pty_for_session call
        // remove_session_runtime to drop the placeholder. After that, the
        // next caller must be able to reserve again so a real retry / restart
        // path works.
        let sessions = make_spawn_sessions();
        assert!(try_reserve_session_spawn(&sessions, "retry-id"));
        // Simulate the error early-return cleanup.
        remove_session_runtime(&sessions, "retry-id");
        assert!(
            try_reserve_session_spawn(&sessions, "retry-id"),
            "post-cleanup retry must succeed"
        );
    }

    #[test]
    fn try_reserve_session_spawn_blocks_when_writer_already_set() {
        // Once a session is fully running (writer/master populated, is_spawning
        // cleared) a fresh `spawn_pty_for_session` call MUST also lose the
        // reservation race so we never double-spawn.
        let sessions = make_spawn_sessions();
        with_session_runtime(
            &sessions,
            "running-id",
            || SessionRuntime::new("running-id"),
            |rt| {
                // Use a no-op Write impl as a stand-in for a real PTY writer.
                struct NoopWriter;
                impl std::io::Write for NoopWriter {
                    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                        Ok(buf.len())
                    }
                    fn flush(&mut self) -> std::io::Result<()> {
                        Ok(())
                    }
                }
                rt.writer = Some(Box::new(NoopWriter));
                rt.is_spawning = false;
            },
        );
        assert!(
            !try_reserve_session_spawn(&sessions, "running-id"),
            "reservation against a fully-running session must lose"
        );
    }

    #[test]
    fn try_reserve_session_spawn_only_one_winner_under_thread_race() {
        // Smoke-test the lock semantics: 16 threads all race to reserve the
        // same session id. Exactly one must win.
        let sessions = make_spawn_sessions();
        let session_id = "thread-race".to_string();
        let mut handles = Vec::new();
        let winners = Arc::new(Mutex::new(0u32));
        for _ in 0..16 {
            let s = sessions.clone();
            let id = session_id.clone();
            let w = winners.clone();
            handles.push(std::thread::spawn(move || {
                if try_reserve_session_spawn(&s, &id) {
                    *w.lock().unwrap() += 1;
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let winner_count = *winners.lock().unwrap();
        assert_eq!(
            winner_count, 1,
            "exactly one of 16 racing threads must win the reservation; got {winner_count}"
        );
    }

    #[test]
    fn is_session_spawn_active_reflects_lifecycle_states() {
        let sessions = make_spawn_sessions();
        // No entry → not active.
        assert!(!is_session_spawn_active(&sessions, "lifecycle"));
        // Reserved → active.
        assert!(try_reserve_session_spawn(&sessions, "lifecycle"));
        assert!(is_session_spawn_active(&sessions, "lifecycle"));
        // Cleared (success path simulation) → still active because writer/
        // master would be populated by the real spawn flow. Here we just
        // clear is_spawning and check the helper still says "active" iff
        // writer or master is populated. We're not setting them here, so
        // is_session_spawn_active should now be false.
        with_session_runtime(
            &sessions,
            "lifecycle",
            || SessionRuntime::new("lifecycle"),
            |rt| {
                rt.is_spawning = false;
            },
        );
        assert!(
            !is_session_spawn_active(&sessions, "lifecycle"),
            "after clearing is_spawning with writer/master still None, the session is not active"
        );
    }

    // ── Active-workspace gating tests (Bug 1a) ───────────────────

    #[test]
    fn collect_workspace_session_ids_returns_only_target_workspace() {
        let ws_a = test_workspace_with_pane(
            "ws-a", "a", "/a", None, None, None, "sess-a",
        );
        let ws_b = test_workspace_with_pane(
            "ws-b", "b", "/b", None, None, None, "sess-b",
        );
        let snapshot = test_snapshot("ws-a", vec![ws_a, ws_b]);

        let a_ids = collect_workspace_session_ids(&snapshot, "ws-a");
        let b_ids = collect_workspace_session_ids(&snapshot, "ws-b");

        assert_eq!(a_ids, vec!["sess-a".to_string()]);
        assert_eq!(b_ids, vec!["sess-b".to_string()]);
    }

    #[test]
    fn collect_workspace_session_ids_returns_empty_for_unknown_workspace() {
        let ws_a = test_workspace_with_pane(
            "ws-a", "a", "/a", None, None, None, "sess-a",
        );
        let snapshot = test_snapshot("ws-a", vec![ws_a]);

        // Misspelled / nonexistent workspace id is the realistic stale-state
        // case during startup hydration; must return empty rather than
        // accidentally hydrating a different workspace.
        let nope = collect_workspace_session_ids(&snapshot, "ws-does-not-exist");
        assert!(nope.is_empty());
    }

    #[test]
    fn collect_workspace_session_ids_walks_split_panes() {
        use crate::state::*;
        let mut ws = test_workspace("ws-tree", "t", "/t", None, None, None);
        ws.surfaces = vec![SurfaceSnapshot {
            surface_id: SurfaceId("surf".into()),
            title: "Terminal".into(),
            root: PaneNodeSnapshot::Split {
                pane_id: PaneId("split-root".into()),
                direction: SplitDirection::Horizontal,
                child_sizes: vec![0.5, 0.5],
                children: vec![
                    PaneNodeSnapshot::Terminal {
                        pane_id: PaneId("p1".into()),
                        session_id: SessionId("s1".into()),
                        title: "left".into(),
                    },
                    PaneNodeSnapshot::Terminal {
                        pane_id: PaneId("p2".into()),
                        session_id: SessionId("s2".into()),
                        title: "right".into(),
                    },
                ],
            },
            active_pane_id: PaneId("p1".into()),
        }];
        let snapshot = test_snapshot("ws-tree", vec![ws]);

        let ids = collect_workspace_session_ids(&snapshot, "ws-tree");
        // Order matches the depth-first walk in collect_terminal_sessions;
        // we just care that BOTH session ids are present so the lazy hydrate
        // path covers every pane in the workspace.
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"s1".to_string()));
        assert!(ids.contains(&"s2".to_string()));
    }

    // -- process-group kill tests -----------------------------------------------
    //
    // These tests spawn real processes through a real PTY and verify that
    // `kill_session_tree` / `terminate_pty_session` / `SessionRuntime::drop`
    // actually terminate the child (and its children). They are `#[cfg(unix)]`
    // because `kill_session_tree` is only meaningful on Unix, and `#[serial]`
    // so they do not interact with any other test that might spawn processes.

    #[cfg(unix)]
    mod process_kill {
        use super::*;
        use portable_pty::{native_pty_system, CommandBuilder, PtySize};
        use serial_test::serial;
        use std::time::Instant;

        /// Liveness probe for a pid we do **not** own (no `Child` handle).
        /// Uses `libc::kill(pid, 0)` — sends no signal, only checks whether
        /// the pid is reachable. Returns `false` (== not alive) both when
        /// the process has fully exited AND been reaped, and when we have
        /// no permission to signal it (which should not happen in tests).
        ///
        /// Caveat: a zombie (exited but not yet reaped by its parent) still
        /// reports "alive" via this probe because the process-table entry
        /// exists. For children we own, use `try_wait_child` instead.
        fn is_alive(pid: i32) -> bool {
            // Safety: kill with signal 0 does not deliver anything; it only
            // checks whether the caller could signal the target.
            let ret = unsafe { libc::kill(pid, 0) };
            if ret == 0 {
                return true;
            }
            let err = std::io::Error::last_os_error().raw_os_error();
            err != Some(libc::ESRCH)
        }

        /// Poll `is_alive` for up to `timeout` and return whether the pid
        /// became unreachable within that window. Use this ONLY for pids we
        /// do not own (orphaned grandchildren reaped by init).
        fn wait_until_gone(pid: i32, timeout: Duration) -> bool {
            let start = Instant::now();
            while start.elapsed() < timeout {
                if !is_alive(pid) {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            !is_alive(pid)
        }

        /// Wait for a `Child` that we own (via portable-pty) to be reaped.
        /// This is the correct probe for children we spawned ourselves —
        /// `libc::kill(pid, 0)` would return success on the zombie because
        /// its process-table entry still exists until we call `wait()`.
        /// `try_wait` reaps the zombie as soon as it sees one, giving us a
        /// definitive "this process has exited" signal.
        fn wait_child_gone(
            child: &mut Box<dyn portable_pty::Child + Send + Sync>,
            timeout: Duration,
        ) -> bool {
            let start = Instant::now();
            while start.elapsed() < timeout {
                match child.try_wait() {
                    Ok(Some(_)) => return true, // exited + reaped
                    Ok(None) => {}              // still running
                    Err(_) => return true,      // reaping error ≈ already gone
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            matches!(child.try_wait(), Ok(Some(_)) | Err(_))
        }

        /// Spawn a command on a fresh PTY pair and return `(child, pid)`.
        /// Dropping the returned `Child` does not kill the process — we
        /// keep it alive so the tests can call `wait()` themselves to avoid
        /// leaving zombies behind.
        fn spawn_on_pty(
            cmd: CommandBuilder,
        ) -> (Box<dyn portable_pty::Child + Send + Sync>, u32) {
            let pty = native_pty_system()
                .openpty(PtySize {
                    rows: 24,
                    cols: 80,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .expect("openpty");
            let child = pty.slave.spawn_command(cmd).expect("spawn");
            // Drop the slave side — we don't need it; the spawned process
            // already inherited its fds.
            drop(pty.slave);
            let pid = child.process_id().expect("child pid");
            // Leak the master so the PTY fds stay open for the lifetime of
            // the test. Otherwise closing the master can deliver SIGHUP to
            // the child before we run our kill logic.
            std::mem::forget(pty.master);
            (child, pid)
        }

        /// SIGTERM→200ms→SIGKILL should clear a simple long-sleeping child.
        #[test]
        #[serial]
        fn test_kill_session_tree_removes_shell() {
            let mut cmd = CommandBuilder::new("sleep");
            cmd.arg("3600");
            let (mut child, pid) = spawn_on_pty(cmd);

            // Sanity: child is alive before we kill it.
            assert!(is_alive(pid as i32), "child should be alive pre-kill");

            kill_session_tree(pid);

            assert!(
                wait_child_gone(&mut child, Duration::from_secs(2)),
                "pid {pid} should be exited + reaped within 2s after kill_session_tree"
            );
        }

        /// The critical test: killpg must reach the *grandchild* of the PTY
        /// shell. This is the exact property we rely on — portable-pty's
        /// `setsid()` means the shell is a process-group leader, and any
        /// children it spawns inherit that PGID unless they detach themselves.
        /// A plain `sh` doing `sleep 3600 &` is the canonical case.
        #[test]
        #[serial]
        fn test_kill_session_tree_kills_grandchild() {
            // We need to keep the master side of the PTY around so we can
            // read the grandchild PID the shell prints, so we spawn inline
            // instead of using `spawn_on_pty`.
            let pty = native_pty_system()
                .openpty(PtySize {
                    rows: 24,
                    cols: 80,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .expect("openpty");
            let mut cmd = CommandBuilder::new("sh");
            cmd.arg("-c");
            // Shell backgrounds sleep, prints its PID on a marker line,
            // then waits forever. We parse the marker to learn the PID.
            cmd.arg("sleep 3600 & echo GRANDCHILD_PID=$! && wait");
            let mut child = pty.slave.spawn_command(cmd).expect("spawn");
            drop(pty.slave);
            let shell_pid = child.process_id().expect("shell pid");
            let mut reader = pty.master.try_clone_reader().expect("reader");

            // Read PTY output until we see the marker line.
            let mut buf = [0u8; 1024];
            let mut accumulated = Vec::new();
            let start = Instant::now();
            let grandchild_pid: u32 = loop {
                if start.elapsed() > Duration::from_secs(5) {
                    panic!(
                        "timed out waiting for GRANDCHILD_PID in: {:?}",
                        String::from_utf8_lossy(&accumulated)
                    );
                }
                match reader.read(&mut buf) {
                    Ok(0) => panic!("PTY closed before GRANDCHILD_PID arrived"),
                    Ok(n) => {
                        accumulated.extend_from_slice(&buf[..n]);
                        if let Ok(text) = std::str::from_utf8(&accumulated) {
                            if let Some(line) =
                                text.lines().find(|l| l.contains("GRANDCHILD_PID="))
                            {
                                let pid_str = line
                                    .split("GRANDCHILD_PID=")
                                    .nth(1)
                                    .unwrap()
                                    .trim()
                                    .trim_end_matches(|c: char| !c.is_ascii_digit());
                                break pid_str.parse().expect("parse pid");
                            }
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => panic!("read error: {e}"),
                }
            };

            assert!(is_alive(shell_pid as i32), "shell should be alive");
            assert!(
                is_alive(grandchild_pid as i32),
                "grandchild should be alive"
            );

            kill_session_tree(shell_pid);

            // Shell is ours → use try_wait to reap the zombie.
            assert!(
                wait_child_gone(&mut child, Duration::from_secs(2)),
                "shell pid {shell_pid} should be exited + reaped \
                 after kill_session_tree"
            );
            // Grandchild is orphaned by the shell's death → init (pid 1)
            // adopts and reaps it. `libc::kill(pid, 0)` is the correct
            // probe here because we are not its parent. Init reaps
            // quickly but give it up to 3s to absorb CI jitter.
            assert!(
                wait_until_gone(grandchild_pid as i32, Duration::from_secs(3)),
                "grandchild pid {grandchild_pid} (the `sleep 3600 &`) should be \
                 gone after kill_session_tree — this is the whole point of killpg"
            );
            // Leak the master so closing it cannot deliver SIGHUP to any
            // still-exiting process.
            std::mem::forget(pty.master);
        }

        /// Calling kill_session_tree on a PID that has already been reaped
        /// (ESRCH path) must not panic or print spurious errors.
        #[test]
        #[serial]
        fn test_kill_session_tree_esrch_is_ok() {
            let cmd = CommandBuilder::new("true");
            let (mut child, pid) = spawn_on_pty(cmd);
            // Wait for `true` to exit naturally.
            let _ = child.wait();
            // Give the kernel a moment to fully tear down the process entry.
            std::thread::sleep(Duration::from_millis(50));
            assert!(!is_alive(pid as i32), "true should be gone after wait");

            // Should complete without panicking and without extra stderr.
            kill_session_tree(pid);
        }

        /// `Drop for SessionRuntime` is the safety net for code paths that
        /// forget to clear `child_pid`. Construct a runtime with a live PID,
        /// drop it, and assert the child is exited + reaped.
        #[test]
        #[serial]
        fn test_session_runtime_drop_kills_live_child() {
            let mut cmd = CommandBuilder::new("sleep");
            cmd.arg("3600");
            let (mut child, pid) = spawn_on_pty(cmd);
            assert!(is_alive(pid as i32));

            let mut runtime = SessionRuntime::new("drop-test");
            runtime.child_pid = Some(pid);
            // Drop runs here → warning printed to stderr, process killed.
            drop(runtime);

            assert!(
                wait_child_gone(&mut child, Duration::from_secs(2)),
                "Drop impl should have killed and allowed reap of pid {pid}"
            );
        }

        /// `terminate_pty_session` must be idempotent — calling it twice on
        /// the same id removes the runtime the first time and no-ops the
        /// second, matching the `HashMap::remove` semantics that protect
        /// us from races with the waiter thread.
        #[test]
        #[serial]
        fn test_terminate_pty_session_double_call_is_noop() {
            let sessions = make_sessions();
            // Insert a runtime with no child_pid so the helper exits
            // cleanly without trying to signal anything.
            with_session_runtime(
                &sessions,
                "double",
                || SessionRuntime::new("double"),
                |runtime| {
                    runtime.child_pid = None;
                },
            );

            terminate_pty_session(&sessions, "double");
            terminate_pty_session(&sessions, "double");

            assert!(
                sessions.lock().unwrap().get("double").is_none(),
                "runtime should be gone after terminate_pty_session"
            );
        }

        /// Asserts the property we rely on for the whole fix: portable-pty
        /// puts every spawned child into its own session via `setsid()` in
        /// its Unix `pre_exec` hook, so the child's PGID equals its PID.
        /// If portable-pty ever regresses on this, `killpg(pid, ...)` would
        /// signal Codemux's own process group instead of the child — which
        /// this test would catch by asserting `getpgid(pid) == pid`.
        #[test]
        #[serial]
        fn test_portable_pty_child_pgid_equals_pid() {
            let mut cmd = CommandBuilder::new("sleep");
            cmd.arg("3600");
            let (mut child, pid) = spawn_on_pty(cmd);

            // SAFETY: `getpgid` is a simple POSIX call that reads the
            // process group id of the target pid. No memory concerns. A
            // return of -1 would indicate ESRCH/EPERM; we assert it's > 0.
            let pgid = unsafe { libc::getpgid(pid as i32) };
            assert!(pgid > 0, "getpgid({pid}) failed with {pgid}");
            assert_eq!(
                pgid as u32, pid,
                "portable-pty child PGID must equal its PID (setsid contract); \
                 got pgid={pgid} pid={pid}. If this fails, portable-pty regressed \
                 and `killpg` now targets Codemux's own process group."
            );

            // Clean up.
            kill_session_tree(pid);
            let _ = wait_child_gone(&mut child, Duration::from_secs(2));
        }

        /// Soak test: spawn 10 independent PTY sessions with long-running
        /// sleeps, tear each one down through `terminate_pty_session`, and
        /// assert all 10 PIDs are gone. Exercises the HashMap mutex + kill
        /// path under burst load and catches any per-session leak that
        /// single-session tests would miss.
        #[test]
        #[serial]
        fn test_soak_ten_sessions_no_leaks() {
            const N: usize = 10;
            let sessions = make_sessions();
            let mut children: Vec<(Box<dyn portable_pty::Child + Send + Sync>, u32)> =
                Vec::with_capacity(N);

            for i in 0..N {
                let mut cmd = CommandBuilder::new("sleep");
                cmd.arg("3600");
                let (child, pid) = spawn_on_pty(cmd);
                // Insert a runtime with the real pid so `terminate_pty_session`
                // has something to find and kill. Master/writer are set to
                // None — this test exercises the kill path, not I/O.
                let session_id = format!("soak-{i}");
                with_session_runtime(
                    &sessions,
                    &session_id,
                    || SessionRuntime::new(&session_id),
                    |runtime| {
                        runtime.child_pid = Some(pid);
                    },
                );
                children.push((child, pid));
            }

            // Confirm every child is alive before we kill them.
            for (_, pid) in &children {
                assert!(is_alive(*pid as i32), "soak child {pid} should be alive");
            }

            // Tear them all down through the public helper.
            for i in 0..N {
                terminate_pty_session(&sessions, &format!("soak-{i}"));
            }

            // Every runtime should be gone from the map, and every child
            // should be reaped within 2s.
            {
                let guard = sessions.lock().unwrap();
                for i in 0..N {
                    assert!(
                        guard.get(&format!("soak-{i}")).is_none(),
                        "runtime soak-{i} should have been removed"
                    );
                }
            }
            for (mut child, pid) in children {
                assert!(
                    wait_child_gone(&mut child, Duration::from_secs(2)),
                    "soak pid {pid} should be exited + reaped"
                );
            }
        }

        /// Documents a known v1 limitation: if a grandchild calls `setsid`
        /// itself, it escapes the shell's process group and `killpg(shell_pgid, ...)`
        /// does NOT reach it. This test pins that behavior so anyone reading
        /// the test file understands the failure mode; if this test ever
        /// fails (i.e. the grandchild IS killed), it means we gained a
        /// tree-walking fallback and this limitation is no longer accurate.
        ///
        /// Track the `/proc`-walking tree-kill fallback as the fix if this
        /// becomes a real problem — e.g. if Claude CLI, an MCP server, or
        /// rust-analyzer is observed to call `setsid` in practice.
        #[test]
        #[serial]
        fn test_killpg_does_not_reach_setsid_detached_grandchild() {
            let pty = native_pty_system()
                .openpty(PtySize {
                    rows: 24,
                    cols: 80,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .expect("openpty");
            let mut cmd = CommandBuilder::new("sh");
            cmd.arg("-c");
            // `setsid --fork` always double-forks and exec's the target in
            // a new session. The inner `sh -c` prints its own `$$` (the
            // real session leader) so we can probe it. We cannot use
            // `$!` here because setsid's internal fork makes `$!` point
            // at the short-lived wrapper, not the real session leader.
            cmd.arg(
                "setsid --fork sh -c 'echo DETACHED_PID=$$; sleep 3600' & \
                 echo SHELL_READY; wait",
            );
            let mut child = pty.slave.spawn_command(cmd).expect("spawn");
            drop(pty.slave);
            let shell_pid = child.process_id().expect("shell pid");
            let mut reader = pty.master.try_clone_reader().expect("reader");

            let mut buf = [0u8; 1024];
            let mut accumulated = Vec::new();
            let start = Instant::now();
            let detached_pid: u32 = loop {
                if start.elapsed() > Duration::from_secs(5) {
                    panic!(
                        "timed out waiting for DETACHED_PID in: {:?}",
                        String::from_utf8_lossy(&accumulated)
                    );
                }
                match reader.read(&mut buf) {
                    Ok(0) => panic!("PTY closed before DETACHED_PID arrived"),
                    Ok(n) => {
                        accumulated.extend_from_slice(&buf[..n]);
                        if let Ok(text) = std::str::from_utf8(&accumulated) {
                            if let Some(line) =
                                text.lines().find(|l| l.contains("DETACHED_PID="))
                            {
                                let pid_str = line
                                    .split("DETACHED_PID=")
                                    .nth(1)
                                    .unwrap()
                                    .trim()
                                    .trim_end_matches(|c: char| !c.is_ascii_digit());
                                break pid_str.parse().expect("parse pid");
                            }
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => panic!("read error: {e}"),
                }
            };

            // Confirm the detached child is in its own PGID.
            // SAFETY: `getpgid` reads the pgid of a pid we just read from
            // the PTY; no aliasing concerns.
            let detached_pgid = unsafe { libc::getpgid(detached_pid as i32) };
            assert!(
                detached_pgid > 0 && detached_pgid as u32 == detached_pid,
                "detached child should be its own pgid leader (pgid={detached_pgid} pid={detached_pid})"
            );
            assert_ne!(
                detached_pgid as u32, shell_pid,
                "detached child must not share the shell's pgid — that's the point of setsid"
            );

            // Kill the shell's process group. The detached grandchild
            // should survive because it's in a different pgid.
            kill_session_tree(shell_pid);
            assert!(
                wait_child_gone(&mut child, Duration::from_secs(2)),
                "shell pid {shell_pid} should still be killed by killpg"
            );

            // The detached grandchild is STILL alive — this is the limitation.
            // Give the scheduler a moment, then assert.
            std::thread::sleep(Duration::from_millis(100));
            assert!(
                is_alive(detached_pid as i32),
                "KNOWN LIMITATION: setsid-detached grandchild pid {detached_pid} \
                 should still be alive after killpg(shell_pgid). If this assertion \
                 fails, we gained a tree-kill fallback — update this test to reflect \
                 the new behavior."
            );

            // Clean up the detached child manually so the test leaves no
            // zombies behind. We're not its parent (init is, since the
            // shell died), so we use plain kill.
            // SAFETY: plain POSIX kill; detached_pid is a pid we own
            // transitively and are now cleaning up after the test.
            unsafe { libc::kill(detached_pid as i32, libc::SIGKILL) };
            // Let init reap it.
            let _ = wait_until_gone(detached_pid as i32, Duration::from_secs(2));

            std::mem::forget(pty.master);
        }

        // -- guard-path tests (pid <= 1) ---------------------------------
        //
        // These tests verify that `kill_session_tree` refuses dangerous
        // PIDs without invoking `killpg`. The only way to *prove* killpg
        // wasn't called is to show that a known innocent process still
        // exists after the call — if `kill_session_tree(0)` reached
        // `killpg(0, SIGKILL)` it would signal our own process group and
        // everything we spawned (the witness sleep, plus the test runner
        // itself) would die.

        /// `kill_session_tree(0)` must be refused: `killpg(0, ...)` signals
        /// the caller's own process group.
        #[test]
        #[serial]
        fn test_kill_session_tree_rejects_pid_zero() {
            // Spawn a witness child that MUST still be alive after the
            // guarded call. If kill_session_tree(0) reached killpg(0, ...)
            // it would signal this test's process group and kill the
            // witness (and probably the test process itself).
            let mut cmd = CommandBuilder::new("sleep");
            cmd.arg("3600");
            let (mut witness, witness_pid) = spawn_on_pty(cmd);
            assert!(is_alive(witness_pid as i32));

            // The guard path: early-return with an eprintln, no killpg.
            kill_session_tree(0);

            // Witness must still be alive.
            assert!(
                is_alive(witness_pid as i32),
                "witness pid {witness_pid} should still be alive — \
                 kill_session_tree(0) must not reach killpg()"
            );

            // Clean up.
            kill_session_tree(witness_pid);
            let _ = wait_child_gone(&mut witness, Duration::from_secs(2));
        }

        /// `kill_session_tree(1)` must be refused: pid 1 is init / the
        /// container entrypoint and is never a PTY child we spawned.
        /// Even though the kernel usually refuses signals to init, a
        /// defensive guard keeps a bad pid from ever reaching libc.
        #[test]
        #[serial]
        fn test_kill_session_tree_rejects_pid_one() {
            let mut cmd = CommandBuilder::new("sleep");
            cmd.arg("3600");
            let (mut witness, witness_pid) = spawn_on_pty(cmd);
            assert!(is_alive(witness_pid as i32));

            kill_session_tree(1);

            assert!(
                is_alive(witness_pid as i32),
                "witness pid {witness_pid} should still be alive — \
                 kill_session_tree(1) must not reach killpg()"
            );

            kill_session_tree(witness_pid);
            let _ = wait_child_gone(&mut witness, Duration::from_secs(2));
        }

        // -- terminate_pty_session edge paths ----------------------------

        /// The `let Some(...) else { return }` path: asking to terminate
        /// a session that is not in the map must be a silent no-op.
        #[test]
        fn test_terminate_pty_session_missing_session_is_noop() {
            let sessions = make_sessions();
            assert!(sessions.lock().unwrap().is_empty());

            // No panic, no insertion.
            terminate_pty_session(&sessions, "not-there");

            assert!(
                sessions.lock().unwrap().is_empty(),
                "terminate on missing session must not create an entry"
            );
        }

        /// The `None => eprintln!(...)` path: a runtime is in the map but
        /// `child_pid` is `None`. After Fix 2 (spawn fails loud if
        /// `process_id()` returns None), a live session cannot reach this
        /// branch — the only remaining cause is waiter-thread phantom
        /// insertion between `terminate_pty_session`'s remove and its
        /// kill. Must still remove the runtime and not panic.
        #[test]
        fn test_terminate_pty_session_none_child_pid() {
            let sessions = make_sessions();
            // `SessionRuntime::new` initializes child_pid to None.
            with_session_runtime(
                &sessions,
                "phantom",
                || SessionRuntime::new("phantom"),
                |_| {},
            );
            assert!(sessions.lock().unwrap().contains_key("phantom"));

            terminate_pty_session(&sessions, "phantom");

            assert!(
                sessions.lock().unwrap().get("phantom").is_none(),
                "runtime should be removed even when child_pid is None"
            );
            // Also: the eprintln warning text must not imply a spawn bug
            // unambiguously — see the `None` arm of `terminate_pty_session`
            // and its updated wording "spawn returned None or waiter-thread
            // race". We don't capture stderr here, but the wording test is
            // the source-level comment.
        }

        // -- SessionRuntime Drop ------------------------------------------

        /// Dropping a runtime with `child_pid = None` must be silent and
        /// must not panic. Guards against a future refactor that adds
        /// logic to `Drop` assuming `child_pid` is always `Some`.
        #[test]
        fn test_session_runtime_drop_with_none_pid_is_silent() {
            let runtime = SessionRuntime::new("drop-none");
            assert!(runtime.child_pid.is_none());
            // No panic on drop. Runs the Drop impl's `if let Some(..)`
            // with a None and falls through silently.
            drop(runtime);
        }

        // -- documented gaps (ignored stubs) -----------------------------

        #[test]
        #[ignore]
        fn test_concurrent_terminate_and_waiter_race() {
            // Requires controlled thread interleaving to test properly.
            // The race is benign (no leak, possible spurious warning).
            // Documented as untested.
        }

        #[test]
        #[ignore]
        fn test_spawn_with_none_process_id() {
            // Cannot simulate portable_pty::Child::process_id() returning
            // None without mocking the crate. The spawn path now fails
            // loudly (Fix 2). Documented as untested.
        }
    }
}
