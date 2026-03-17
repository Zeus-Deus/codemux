use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::env;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{ipc::Channel, AppHandle, Emitter, Manager, State};

use crate::project::current_project_root;
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
}

impl SessionRuntime {
    fn new(session_id: &str) -> Self {
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
        }
    }
}

#[derive(Default)]
pub struct PtyState {
    pub sessions: Arc<Mutex<HashMap<String, SessionRuntime>>>,
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

    with_session_runtime(
        sessions,
        &payload.session_id,
        || SessionRuntime::new(&payload.session_id),
        |runtime| {
            runtime.last_status = payload.clone();
        },
    );

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

fn default_shell() -> String {
    env::var("SHELL")
        .ok()
        .filter(|shell| !shell.trim().is_empty())
        .unwrap_or_else(|| "/bin/bash".to_string())
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

    let snapshot = app_state.snapshot();
    let active_workspace_id = snapshot.active_workspace_id.0.clone();
    cmd.env("CODEMUX_WORKSPACE_ID", active_workspace_id);
    cmd.env("CODEMUX_SURFACE_ID", session_id.clone());

    let mut child = match pty_pair.slave.spawn_command(cmd) {
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

    drop(pty_pair.slave);

    let mut reader = match pty_pair.master.try_clone_reader() {
        Ok(reader) => reader,
        Err(error) => {
            emit_terminal_status(
                &app,
                &sessions,
                TerminalStatusPayload {
                    session_id,
                    state: TerminalLifecycleState::Failed,
                    message: Some(format!("Failed to clone PTY reader: {error}")),
                    exit_code: None,
                },
            );
            return;
        }
    };

    let writer = match pty_pair.master.take_writer() {
        Ok(writer) => writer,
        Err(error) => {
            emit_terminal_status(
                &app,
                &sessions,
                TerminalStatusPayload {
                    session_id,
                    state: TerminalLifecycleState::Failed,
                    message: Some(format!("Failed to take PTY writer: {error}")),
                    exit_code: None,
                },
            );
            return;
        }
    };

    with_session_runtime(
        &sessions,
        &session_id,
        || SessionRuntime::new(&session_id),
        |runtime| {
            runtime.writer = Some(writer);
            runtime.master = Some(pty_pair.master);
        },
    );

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

    let read_sessions = sessions.clone();
    let read_session_id = session_id.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(n) if n > 0 => {
                    let chunk = buf[..n].to_vec();
                    queue_or_send_output(&read_sessions, &read_session_id, chunk);
                }
                Ok(_) => break,
                Err(error) => {
                    eprintln!("[codemux::terminal] PTY read error: {error}");
                    break;
                }
            }
        }
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

#[tauri::command]
pub fn resize_pty(
    app: AppHandle,
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
    state::emit_app_state(&app);

    Ok(())
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
) {
    let terminal_state: State<'_, PtyState> = app.state();
    let app_state: State<'_, AppStateStore> = app.state();
    let sessions = terminal_state.sessions.clone();

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

    emit_terminal_status(
        &app,
        &sessions,
        TerminalStatusPayload {
            session_id: session_id.clone(),
            state: TerminalLifecycleState::Starting,
            message: Some(format!("Starting agent: {executable}")),
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
    let mut cmd = CommandBuilder::new(&executable);
    for arg in argv.iter().skip(1) {
        cmd.arg(arg);
    }
    cmd.cwd(cwd);

    // Standard Codemux env vars.
    cmd.env("CODEMUX_WORKSPACE_ID", &workspace_id);
    cmd.env("CODEMUX_SURFACE_ID", &session_id);

    // Agent-specific env vars from the adapter.
    for (key, val) in &extra_env {
        cmd.env(key, val);
    }

    let mut child = match pty_pair.slave.spawn_command(cmd) {
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

    drop(pty_pair.slave);

    let mut reader = match pty_pair.master.try_clone_reader() {
        Ok(r) => r,
        Err(error) => {
            emit_terminal_status(
                &app,
                &sessions,
                TerminalStatusPayload {
                    session_id,
                    state: TerminalLifecycleState::Failed,
                    message: Some(format!("Failed to clone PTY reader for agent: {error}")),
                    exit_code: None,
                },
            );
            return;
        }
    };

    let writer = match pty_pair.master.take_writer() {
        Ok(w) => w,
        Err(error) => {
            emit_terminal_status(
                &app,
                &sessions,
                TerminalStatusPayload {
                    session_id,
                    state: TerminalLifecycleState::Failed,
                    message: Some(format!("Failed to take PTY writer for agent: {error}")),
                    exit_code: None,
                },
            );
            return;
        }
    };

    with_session_runtime(
        &sessions,
        &session_id,
        || SessionRuntime::new(&session_id),
        |runtime| {
            runtime.writer = Some(writer);
            runtime.master = Some(pty_pair.master);
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
        let mut buf = [0u8; 4096];
        let mut comm_log_buffer: Vec<String> = Vec::new();
        let mut last_flush = Instant::now();

        loop {
            match reader.read(&mut buf) {
                Ok(n) if n > 0 => {
                    let chunk = buf[..n].to_vec();

                    // Buffer agent output for communication log (cleaned); flush periodically
                    if let Some((ref log_lock, ref role)) = log_lock_opt {
                        if let Ok(text) = String::from_utf8(chunk.clone()) {
                            let cleaned = strip_ansi_codes(&text);
                            let trimmed = cleaned.trim();

                            if !trimmed.is_empty()
                                && trimmed.len() > 2
                                && !trimmed.starts_with('\x1b')
                                && !trimmed.chars().all(|c| {
                                    c.is_whitespace() || c == '▀' || c == '▄' || c == '█' || c == ' '
                                })
                            {
                                let timestamp =
                                    chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
                                let entry = format!(
                                    "[{}] [{}] {}\n",
                                    timestamp,
                                    role.to_uppercase(),
                                    trimmed
                                );
                                comm_log_buffer.push(entry);
                                if comm_log_buffer.len() >= COMM_LOG_FLUSH_BATCH_SIZE
                                    || last_flush.elapsed() >= COMM_LOG_FLUSH_INTERVAL
                                {
                                    if let Ok(mut file) = log_lock.lock() {
                                        for e in &comm_log_buffer {
                                            let _ = file.write_all(e.as_bytes());
                                        }
                                        let _ = file.flush();
                                    }
                                    comm_log_buffer.clear();
                                    last_flush = Instant::now();
                                }
                            }
                        }
                    }

                    queue_or_send_output(&read_sessions, &read_session_id, chunk);
                }
                Ok(_) => break,
                Err(error) => {
                    eprintln!("[codemux::terminal] Agent PTY read error: {error}");
                    break;
                }
            }
        }

        // Flush any remaining buffered entries
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
    std::thread::spawn(move || {
        let payload = match child.wait() {
            Ok(status) => TerminalStatusPayload {
                session_id: wait_session_id.clone(),
                state: TerminalLifecycleState::Exited,
                message: Some(if status.success() {
                    "Agent exited successfully".into()
                } else {
                    format!("Agent exited with code {}", status.exit_code())
                }),
                exit_code: Some(status.exit_code()),
            },
            Err(error) => TerminalStatusPayload {
                session_id: wait_session_id.clone(),
                state: TerminalLifecycleState::Failed,
                message: Some(format!("Failed to wait for agent: {error}")),
                exit_code: None,
            },
        };

        crate::diagnostics::openflow_breadcrumb(&format!(
            "agent_exited session_id={} state={:?}",
            wait_session_id,
            payload.state
        ));

        emit_terminal_status(&wait_app, &wait_sessions, payload);

        // Clean up the session runtime to prevent memory leak
        remove_session_runtime(&wait_sessions, &wait_session_id);

        state::emit_app_state(&wait_app);
    });
}
