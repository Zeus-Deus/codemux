use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use serde::Serialize;
use std::collections::{HashMap, VecDeque};
use std::env;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use tauri::{ipc::Channel, AppHandle, Emitter, Manager, State};

use crate::state::{self, AppStateStore, TerminalSessionState};

const DEFAULT_ROWS: u16 = 24;
const DEFAULT_COLS: u16 = 80;
const OUTPUT_BUFFER_LIMIT: usize = 512;

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
    sessions.lock().unwrap().remove(session_id)
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
    let mut guard = sessions.lock().unwrap();
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

    state::emit_app_state(app);
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
            if let Some(channel) = runtime.output_channel.clone() {
                if let Err(error) = channel.send(chunk.clone()) {
                    eprintln!("[codemux::terminal] Failed to send terminal output: {error}");
                    runtime.pending_output.push_back(chunk);
                }
            } else {
                runtime.pending_output.push_back(chunk);
            }

            while runtime.pending_output.len() > OUTPUT_BUFFER_LIMIT {
                runtime.pending_output.pop_front();
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

pub fn spawn_pty_for_session(app: AppHandle, session_id: String) {
    let terminal_state: State<'_, PtyState> = app.state();
    let app_state: State<'_, AppStateStore> = app.state();
    let sessions = terminal_state.sessions.clone();

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
    state::emit_app_state(&app);

    let cmd = CommandBuilder::new(shell.clone());
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
    });
}

pub fn spawn_initial_pty(app: AppHandle) {
    let app_state: State<'_, AppStateStore> = app.state();
    if let Some(session_id) = app_state.active_terminal_session_id() {
        spawn_pty_for_session(app, session_id.0);
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
            runtime.pending_output.drain(..).collect::<Vec<_>>()
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
