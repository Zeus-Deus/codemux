//! The `codemux pty-daemon` subprocess.
//!
//! Run as: `codemux pty-daemon --socket /path/to/sock`.
//!
//! Lifetime: started detached by the Tauri app on first need, outlives the
//! app (intentionally — this is the whole point of step 1). Adopted by the
//! next Tauri startup via `manifest::read_manifest` + `Hello` handshake.
//!
//! Concurrency model:
//! - One tokio task per inbound client connection (the Tauri app opens one
//!   per session it cares about, plus a control connection for List/Spawn).
//! - One blocking std::thread per spawned PTY for the read loop, draining
//!   the master fd into the daemon's per-session output buffer; the buffer
//!   fans out to whichever client connection is currently attached.
//!
//! Cross-platform note: today only Unix (tokio `UnixListener`). Windows
//! named-pipe support is the obvious follow-up; the protocol and supervisor
//! are already cfg-agnostic.

use crate::pty_daemon::manifest::{remove_manifest, write_manifest, DaemonManifest};
use crate::pty_daemon::protocol::{
    ClientRequest, DaemonSessionInfo, Frame, ServerEvent, ServerResponse, PROTOCOL_VERSION,
};
use base64::Engine;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, Mutex};

/// Path of the per-socket liveness pidfile, set once in `run()`. A
/// `<socket>.pid` sidecar lets an SSH-tunnel reconnect (and the host-side
/// "is a daemon already serving this socket?" probe) decide whether to
/// reuse a still-running detached daemon or spawn a fresh one — without
/// depending on the single shared `pty-daemon-manifest.json` (which a
/// per-workspace remote daemon would clobber). The daemon owns the file:
/// it writes it on bind and removes it on clean exit (idle-reap /
/// Shutdown).
///
/// Unix-only: the daemon's `run` loop is `#[cfg(unix)]` (Unix-socket
/// based), so these helpers have no consumer on Windows.
#[cfg(unix)]
static PID_FILE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// `<socket>.pid` — the pidfile path for a given daemon socket. Pure so the
/// host-side reconnect shell and tests agree on the location.
#[cfg(unix)]
pub fn pid_file_for(socket_path: &std::path::Path) -> PathBuf {
    let mut raw = socket_path.as_os_str().to_os_string();
    raw.push(".pid");
    PathBuf::from(raw)
}

/// Write `<socket>.pid` containing this daemon's pid and remember it for
/// removal on exit. Best-effort: a failure just means a reconnect can't
/// confirm liveness and will spawn a fresh daemon (correct, if wasteful).
#[cfg(unix)]
fn write_pid_file(socket_path: &std::path::Path) {
    let path = pid_file_for(socket_path);
    if let Err(error) = std::fs::write(&path, std::process::id().to_string()) {
        eprintln!("[codemux::pty_daemon] WARNING: could not write pidfile {path:?}: {error}");
    }
    let _ = PID_FILE.set(path);
}

/// Remove the pidfile written by `write_pid_file`. Idempotent; no-op if the
/// daemon never wrote one.
#[cfg(unix)]
fn remove_pid_file() {
    if let Some(path) = PID_FILE.get() {
        let _ = std::fs::remove_file(path);
    }
}

/// Capacity of the per-session output broadcast channel. Tuned for
/// short-lived disconnects: roughly 30 seconds of typical TUI redraw output
/// at 64KB chunks. Slow consumers that lag past this will drop frames; the
/// daemon logs the lag and the Tauri client treats it as a partial-output
/// signal (worst case: stale xterm cells until the next full redraw).
const OUTPUT_CHANNEL_CAPACITY: usize = 512;

/// Maximum size of the "cold-start replay" buffer per session. Captures
/// recent output so a freshly-attached client sees something on-screen
/// instead of an empty terminal. 256KB is enough for ~one screenful of an
/// alt-screen TUI; for shell scrollback we rely on the existing
/// `scrollback.rs` system.
const REPLAY_BUFFER_BYTES: usize = 256 * 1024;

/// Flow-control backstop: the maximum time the read loop will honour a
/// `flow_paused` flag before force-resuming. A healthy client clears the
/// flag within milliseconds (as soon as its xterm write queue drains), so
/// this only ever fires when the client crashed, wedged, or lost the resume
/// frame. Force-resuming bounds the worst case to "a stuck pane self-heals
/// in ~this long" instead of "the child is blocked on write forever".
const FLOW_MAX_PARK: Duration = Duration::from_secs(10);

/// Poll interval for the read loop's pause gate. Small enough that a resume
/// is observed promptly, large enough that a parked thread costs ~nothing.
const FLOW_PARK_POLL: Duration = Duration::from_millis(10);

/// Frames pushed through a session's broadcast channel. The reader thread
/// emits `Output` for every PTY chunk; the waiter thread emits `Exited`
/// exactly once when the child finally exits. Connection handlers map each
/// variant to the matching `ServerEvent`.
#[derive(Clone, Debug)]
enum SessionFrame {
    Output(Vec<u8>),
    Exited(i32),
}

struct DaemonSession {
    session_id: String,
    workspace_id: String,
    pid: u32,
    argv: Vec<String>,
    cwd: String,
    rows: u16,
    cols: u16,
    created_at: i64,
    /// PTY master, behind a Mutex so the resize path (request handler) and
    /// the writer path (also request handler) don't race. The reader runs
    /// on a dedicated std::thread holding its own `try_clone_reader`.
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    /// Writer half, also mutex-guarded for the same reason.
    writer: Arc<Mutex<Box<dyn std::io::Write + Send>>>,
    /// Broadcast channel for output AND exit frames. Each attached client
    /// owns one receiver; the read thread and waiter thread are the only
    /// senders.
    frame_tx: broadcast::Sender<SessionFrame>,
    /// Replay buffer for cold-start. Ring-buffered: when full, oldest bytes
    /// are evicted in 4KB chunks so the trim cost stays bounded.
    replay: Arc<Mutex<Vec<u8>>>,
    /// Final exit code once the waiter thread has reaped the child. Used
    /// by late attachers who connect after the child exited: they see this
    /// value in the `Listed` response instead of getting silence.
    exit_code: Arc<Mutex<Option<i32>>>,
    /// Terminal output flow control. When `true`, the per-session read
    /// thread stops draining the master fd, so the child blocks on write
    /// once the kernel PTY buffer fills. Set/cleared by `SetFlowPaused`;
    /// force-cleared on `Attach` and `Close` and by the read loop's
    /// `FLOW_MAX_PARK` backstop. Shared (Arc) with the read thread.
    flow_paused: Arc<AtomicBool>,
}

#[derive(Default)]
struct DaemonState {
    sessions: HashMap<String, Arc<DaemonSession>>,
}

type SharedState = Arc<Mutex<DaemonState>>;

/// Entry point for `codemux pty-daemon`. Binds the Unix socket, writes the
/// manifest, then accepts client connections until shutdown.
///
/// Windows path is not implemented yet — the binary's CLI dispatcher
/// returns a clear error and exits. The Tauri-side supervisor's
/// `circuit_is_open()` check + `daemon_path_viable()` on Windows already
/// make this unreachable on Windows in practice, but we keep the
/// cfg-gate so a careless user running `codemux pty-daemon` by hand on
/// Windows gets a readable failure instead of a link error.
#[cfg(not(unix))]
pub async fn run(_socket_path: PathBuf) -> Result<(), String> {
    Err("codemux pty-daemon is not yet implemented on Windows".into())
}

#[cfg(unix)]
pub async fn run(socket_path: PathBuf) -> Result<(), String> {
    use tokio::net::UnixListener;

    // Tear down any stale socket file from a previous crashed daemon. If the
    // file is still alive and bound by another process, the bind below will
    // fail with EADDRINUSE — that's the correct behavior (we don't double-
    // bind).
    if socket_path.exists() {
        let _ = std::fs::remove_file(&socket_path);
    }

    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create socket parent {:?}: {e}", parent))?;
    }

    let listener = UnixListener::bind(&socket_path)
        .map_err(|e| format!("bind {:?}: {e}", socket_path))?;

    // Restrict socket to the current user. Tokio doesn't expose this on
    // bind, so we chmod after the fact.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600));
    }

    let manifest = DaemonManifest {
        pid: std::process::id(),
        socket_path: socket_path.clone(),
        daemon_version: env!("CARGO_PKG_VERSION").to_string(),
        protocol_version: PROTOCOL_VERSION,
        started_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
    };
    if let Err(error) = write_manifest(&manifest) {
        eprintln!(
            "[codemux::pty_daemon] WARNING: could not write manifest: {error} (adoption from this run will fail)"
        );
    }

    // Per-socket liveness pidfile. The shared manifest above is a single
    // file that a second per-workspace remote daemon would clobber; the
    // pidfile is keyed to this socket so the SSH-tunnel reconnect probe can
    // tell whether THIS socket already has a live (detached) daemon to
    // reuse — the mechanism behind "close the app, the host agent keeps
    // running, reopen reattaches."
    write_pid_file(&socket_path);

    let state: SharedState = Arc::new(Mutex::new(DaemonState::default()));

    eprintln!(
        "[codemux::pty_daemon] listening on {:?} pid={} version={}",
        socket_path,
        std::process::id(),
        env!("CARGO_PKG_VERSION"),
    );

    // Idle reaper: a daemon with zero live sessions continuously for
    // `IDLE_REAP` removes its manifest and exits, so an abandoned daemon
    // doesn't linger forever and stale manifests don't confuse the next
    // adoption. HARD GUARD: it re-checks the session count under the same
    // lock immediately before exit, so it can NEVER reap with a live
    // session — and any spawn/attach naturally resets the idle clock by
    // making `sessions` non-empty on the next check.
    {
        let reaper_state = state.clone();
        let reaper_socket = socket_path.clone();
        tokio::spawn(async move {
            const CHECK_INTERVAL: std::time::Duration =
                std::time::Duration::from_secs(60);
            const IDLE_REAP: std::time::Duration =
                std::time::Duration::from_secs(3600);
            let mut idle_since: Option<tokio::time::Instant> =
                Some(tokio::time::Instant::now());
            loop {
                tokio::time::sleep(CHECK_INTERVAL).await;
                if !reaper_state.lock().await.sessions.is_empty() {
                    idle_since = None;
                    continue;
                }
                let since =
                    idle_since.get_or_insert_with(tokio::time::Instant::now);
                if since.elapsed() < IDLE_REAP {
                    continue;
                }
                // Re-check under the lock right before exit — never reap a
                // daemon that just gained a session between the poll and now.
                if !reaper_state.lock().await.sessions.is_empty() {
                    idle_since = None;
                    continue;
                }
                eprintln!(
                    "[codemux::pty_daemon] idle with no sessions for {IDLE_REAP:?} — \
                     removing manifest and exiting"
                );
                remove_manifest();
                remove_pid_file();
                let _ = std::fs::remove_file(&reaper_socket);
                std::process::exit(0);
            }
        });
    }

    loop {
        match listener.accept().await {
            Ok((stream, _addr)) => {
                let conn_state = state.clone();
                tokio::spawn(async move {
                    if let Err(error) = handle_connection(stream, conn_state).await {
                        eprintln!("[codemux::pty_daemon] connection ended: {error}");
                    }
                });
            }
            Err(error) => {
                eprintln!("[codemux::pty_daemon] accept failed: {error}");
                // Brief backoff; a tight loop on EMFILE would burn CPU.
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        }
    }
}

#[cfg(unix)]
async fn handle_connection(
    stream: tokio::net::UnixStream,
    state: SharedState,
) -> Result<(), String> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    // Each client connection holds receivers for whatever sessions it's
    // attached to. When the receiver yields a frame, we forward to the
    // socket. Detach removes the entry.
    let mut attached: HashMap<String, broadcast::Receiver<SessionFrame>> = HashMap::new();

    // Persistent accumulation buffer for the request line currently being
    // read. It MUST survive across loop iterations: the read below times out
    // periodically to go drain output, and any bytes already pulled off the
    // socket stay buffered here until the terminating newline arrives.
    let mut pending: Vec<u8> = Vec::new();
    loop {
        // Multiplex: either read a new request line OR forward any pending
        // output from attached sessions. tokio::select! across the attach
        // receivers requires they all be polled — we sequentially poll each
        // attached session's recv (non-blocking) then yield to a read.
        //
        // For MVP simplicity we use a serial drain instead of select!:
        // - try_recv each attached channel until empty,
        // - then wait for next request line with a short timeout to keep
        //   the drain loop snappy.
        let mut drained_any = false;
        let mut to_detach: Vec<String> = Vec::new();
        for (sid, rx) in attached.iter_mut() {
            loop {
                match rx.try_recv() {
                    Ok(SessionFrame::Output(data)) => {
                        let frame = Frame::Event(ServerEvent::Output {
                            session_id: sid.clone(),
                            data_b64: base64::engine::general_purpose::STANDARD.encode(&data),
                        });
                        write_frame(&mut write_half, &frame).await?;
                        drained_any = true;
                    }
                    Ok(SessionFrame::Exited(code)) => {
                        let frame = Frame::Event(ServerEvent::Exited {
                            session_id: sid.clone(),
                            exit_code: code,
                        });
                        write_frame(&mut write_half, &frame).await?;
                        drained_any = true;
                        // The session will be removed by the waiter
                        // thread; we just detach our local receiver.
                        to_detach.push(sid.clone());
                        break;
                    }
                    Err(broadcast::error::TryRecvError::Empty) => break,
                    Err(broadcast::error::TryRecvError::Lagged(_)) => {
                        // We dropped frames — keep going, the client's
                        // xterm will recover on the next full redraw.
                        eprintln!(
                            "[codemux::pty_daemon] client lagged on session {sid}, dropping frames"
                        );
                    }
                    Err(broadcast::error::TryRecvError::Closed) => {
                        // Sender (reader + waiter) dropped. Session is
                        // definitely gone; detach.
                        to_detach.push(sid.clone());
                        break;
                    }
                }
            }
        }
        for sid in to_detach {
            attached.remove(&sid);
        }

        let read_timeout = if drained_any {
            std::time::Duration::from_millis(1)
        } else {
            std::time::Duration::from_millis(10)
        };
        // Accumulate bytes until a full newline-terminated line is available.
        // `fill_buf` + `consume` is cancellation-safe — both the BufReader's
        // internal buffer and `pending` persist if the timeout cancels this
        // future — so a request that straddles the poll timeout is preserved.
        // (`read_line` is NOT cancellation-safe: on timeout it drops the
        // partial bytes it had already consumed, corrupting large or chunked
        // requests such as a big paste forwarded as one `Write`.)
        let fill = tokio::time::timeout(read_timeout, reader.fill_buf()).await;
        let chunk = match fill {
            Ok(Ok(buf)) => buf,
            Ok(Err(error)) => return Err(format!("read: {error}")),
            Err(_elapsed) => continue, // timeout — go back to draining
        };
        if chunk.is_empty() {
            return Ok(()); // EOF — client closed
        }
        let newline = chunk.iter().position(|&b| b == b'\n');
        let take = newline.map_or(chunk.len(), |i| i + 1);
        pending.extend_from_slice(&chunk[..take]);
        reader.consume(take);
        if newline.is_none() {
            continue; // partial line — keep accumulating across iterations
        }

        let line_bytes = std::mem::take(&mut pending);
        let line = match std::str::from_utf8(&line_bytes) {
            Ok(text) => text,
            Err(_) => {
                let frame = Frame::Response(ServerResponse::Error {
                    request_id: 0,
                    message: "invalid request: non-utf8 line".to_string(),
                });
                write_frame(&mut write_half, &frame).await?;
                continue;
            }
        };
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if trimmed.is_empty() {
            continue;
        }
        let req: ClientRequest = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(error) => {
                eprintln!("[codemux::pty_daemon] invalid request: {error}: {trimmed}");
                let frame = Frame::Response(ServerResponse::Error {
                    request_id: 0,
                    message: format!("invalid request: {error}"),
                });
                write_frame(&mut write_half, &frame).await?;
                continue;
            }
        };

        let resp = handle_request(req, state.clone(), &mut attached).await;
        write_frame(&mut write_half, &Frame::Response(resp)).await?;
    }
}

#[cfg(unix)]
async fn write_frame(
    write_half: &mut tokio::net::unix::OwnedWriteHalf,
    frame: &Frame,
) -> Result<(), String> {
    let mut bytes = serde_json::to_vec(frame).map_err(|e| format!("serialize: {e}"))?;
    bytes.push(b'\n');
    write_half
        .write_all(&bytes)
        .await
        .map_err(|e| format!("write: {e}"))
}

async fn handle_request(
    req: ClientRequest,
    state: SharedState,
    attached: &mut HashMap<String, broadcast::Receiver<SessionFrame>>,
) -> ServerResponse {
    match req {
        ClientRequest::Hello { request_id } => ServerResponse::Hello {
            request_id,
            protocol_version: PROTOCOL_VERSION,
            daemon_pid: std::process::id(),
            daemon_version: env!("CARGO_PKG_VERSION").to_string(),
        },
        ClientRequest::Spawn {
            request_id,
            session_id,
            workspace_id,
            argv,
            cwd,
            env,
            rows,
            cols,
        } => match spawn_pty(&state, session_id.clone(), workspace_id, argv, cwd, env, rows, cols)
            .await
        {
            Ok(pid) => ServerResponse::Spawned {
                request_id,
                session_id,
                pid,
            },
            Err(error) => ServerResponse::Error {
                request_id,
                message: error,
            },
        },
        ClientRequest::Attach {
            request_id,
            session_id,
        } => {
            let guard = state.lock().await;
            let session = match guard.sessions.get(&session_id) {
                Some(s) => s.clone(),
                None => {
                    return ServerResponse::Error {
                        request_id,
                        message: format!("unknown session {session_id}"),
                    };
                }
            };
            drop(guard);
            // Fail-safe: a fresh attach always resumes the read loop. If a
            // previous client paused this session and then crashed or
            // dropped its connection without resuming, the new client would
            // otherwise inherit a wedged (paused) PTY. Clearing here means a
            // stuck pause self-heals on the next attach (e.g. app restart,
            // reopening the pane).
            session.flow_paused.store(false, Ordering::Relaxed);
            // Subscribe to live output (after replay so we don't drop
            // anything in the gap).
            let rx = session.frame_tx.subscribe();
            attached.insert(session_id.clone(), rx);
            // Flush replay buffer first so the freshly-attached xterm
            // has something to render.
            let replay = { session.replay.lock().await.clone() };
            if !replay.is_empty() {
                let _ = session.frame_tx.send(SessionFrame::Output(replay));
            }
            // Late-attachers to an exited session: emit Exited
            // immediately so they don't sit waiting on a dead channel.
            if let Some(code) = *session.exit_code.lock().await {
                let _ = session.frame_tx.send(SessionFrame::Exited(code));
            }
            ServerResponse::Attached {
                request_id,
                session_id,
            }
        }
        ClientRequest::Detach {
            request_id,
            session_id,
        } => {
            attached.remove(&session_id);
            ServerResponse::Detached {
                request_id,
                session_id,
            }
        }
        ClientRequest::Write {
            request_id,
            session_id,
            data_b64,
        } => {
            let session = {
                let guard = state.lock().await;
                guard.sessions.get(&session_id).cloned()
            };
            let session = match session {
                Some(s) => s,
                None => {
                    return ServerResponse::Error {
                        request_id,
                        message: format!("unknown session {session_id}"),
                    };
                }
            };
            let bytes = match base64::engine::general_purpose::STANDARD.decode(&data_b64) {
                Ok(b) => b,
                Err(error) => {
                    return ServerResponse::Error {
                        request_id,
                        message: format!("invalid base64: {error}"),
                    };
                }
            };
            let mut writer = session.writer.lock().await;
            if let Err(error) = writer.write_all(&bytes) {
                return ServerResponse::Error {
                    request_id,
                    message: format!("pty write: {error}"),
                };
            }
            let _ = writer.flush();
            ServerResponse::Written { request_id }
        }
        ClientRequest::Resize {
            request_id,
            session_id,
            rows,
            cols,
        } => {
            let session = {
                let guard = state.lock().await;
                guard.sessions.get(&session_id).cloned()
            };
            let session = match session {
                Some(s) => s,
                None => {
                    return ServerResponse::Error {
                        request_id,
                        message: format!("unknown session {session_id}"),
                    };
                }
            };
            let master = session.master.lock().await;
            if let Err(error) = master.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            }) {
                return ServerResponse::Error {
                    request_id,
                    message: format!("resize: {error}"),
                };
            }
            ServerResponse::Resized { request_id }
        }
        ClientRequest::Close {
            request_id,
            session_id,
        } => {
            let session = {
                let mut guard = state.lock().await;
                guard.sessions.remove(&session_id)
            };
            if let Some(session) = session {
                // Clear any pause first so a read thread parked in the
                // flow-control gate wakes, reads EOF on the now-killed
                // master, and exits — otherwise it would leak (parked
                // forever holding the broadcast sender).
                session.flow_paused.store(false, Ordering::Relaxed);
                kill_session_pid(session.pid);
            }
            ServerResponse::Closed { request_id }
        }
        ClientRequest::List { request_id } => {
            let guard = state.lock().await;
            let sessions: Vec<DaemonSessionInfo> = guard
                .sessions
                .values()
                .map(|s| DaemonSessionInfo {
                    session_id: s.session_id.clone(),
                    workspace_id: s.workspace_id.clone(),
                    pid: s.pid,
                    argv: s.argv.clone(),
                    cwd: s.cwd.clone(),
                    rows: s.rows,
                    cols: s.cols,
                    created_at: s.created_at,
                })
                .collect();
            ServerResponse::Listed {
                request_id,
                sessions,
            }
        }
        ClientRequest::Shutdown { request_id } => {
            // Best-effort: kill everything, drop the manifest, exit.
            let mut guard = state.lock().await;
            for (_, session) in guard.sessions.drain() {
                kill_session_pid(session.pid);
            }
            drop(guard);
            remove_manifest();
            remove_pid_file();
            // Spawn the exit after replying so the client gets the
            // ShuttingDown frame.
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                std::process::exit(0);
            });
            ServerResponse::ShuttingDown { request_id }
        }
        ClientRequest::SetFlowPaused {
            request_id,
            session_id,
            paused,
        } => {
            let session = {
                let guard = state.lock().await;
                guard.sessions.get(&session_id).cloned()
            };
            match session {
                Some(session) => {
                    session.flow_paused.store(paused, Ordering::Relaxed);
                    ServerResponse::FlowPaused { request_id }
                }
                None => ServerResponse::Error {
                    request_id,
                    message: format!("unknown session {session_id}"),
                },
            }
        }
    }
}

async fn spawn_pty(
    state: &SharedState,
    session_id: String,
    workspace_id: String,
    argv: Vec<String>,
    cwd: String,
    env: Vec<(String, String)>,
    rows: u16,
    cols: u16,
) -> Result<u32, String> {
    if argv.is_empty() {
        return Err("argv is empty".into());
    }

    // Refuse to double-spawn the same id — the Tauri side is supposed to
    // generate fresh ids per request, but tests and panics can break that
    // invariant.
    {
        let guard = state.lock().await;
        if guard.sessions.contains_key(&session_id) {
            return Err(format!("session {session_id} already exists in daemon"));
        }
    }

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("openpty: {e}"))?;

    let mut cmd = CommandBuilder::new(&argv[0]);
    for arg in argv.iter().skip(1) {
        cmd.arg(arg);
    }
    // Tilde expansion: `cmd.cwd` calls `chdir` (libc), which does NOT
    // expand `~/` — that's a shell-only thing. Tunneled spawns from
    // remote workspaces pass `~/.codemux/worktrees/...` as cwd because
    // the laptop side doesn't know the remote's HOME. If we passed the
    // literal `~` to chdir, the child would fail to enter its cwd and
    // (on some shells) exit immediately, killing the session before a
    // single byte of prompt rendered. Expand here on the daemon side
    // where we know the local HOME.
    let resolved_cwd = crate::project::expand_tilde(&cwd);
    let cwd_exists = std::path::Path::new(&resolved_cwd).exists();
    crate::trace_cloud_push!(
        "[daemon::spawn] session={session_id} input_cwd={cwd:?} \
         resolved_cwd={resolved_cwd:?} exists={cwd_exists} \
         HOME={:?}",
        std::env::var("HOME").ok()
    );
    cmd.cwd(&resolved_cwd);

    // Undo AppRun's loader/toolkit rewrites before the caller's env is layered
    // on. The daemon is the Codemux binary re-executed, so it still needs the
    // AppDir libraries itself and cannot be sanitized at launch — the strip has
    // to happen here, on the leaf child. No-op outside an AppImage.
    //
    // Ordering: this runs BEFORE the loop below so an explicit PATH from the
    // app process (already sanitized, plus the CLI shim dir) takes precedence
    // over the PATH this derives from the daemon's own environment.
    crate::execution::sanitize_appimage_env_pty(&mut cmd);

    for (k, v) in &env {
        cmd.env(k, v);
    }

    // Belt-and-braces with the supervisor's own strip: a daemon adopted from
    // an older app process may still carry the WebKitGTK renderer transport
    // vars, which are an app-process concern and must never reach a shell or
    // any GTK/WebKit app launched from one.
    for key in crate::webview_tuning::RENDERER_ENV_VARS {
        cmd.env_remove(key);
    }

    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("spawn: {e}"))?;
    let pid = child
        .process_id()
        .ok_or_else(|| "spawned child has no pid".to_string())?;
    // Keep the Child handle so we can reap it and report an honest exit
    // code via the Exited event. The child moves into the waiter thread
    // spawned below.

    // Drop the slave handle in the parent so EOF propagates correctly once
    // the child exits (same invariant as the in-process spawn path).
    drop(pair.slave);

    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|e| format!("clone reader: {e}"))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|e| format!("take writer: {e}"))?;

    let (tx, _rx) = broadcast::channel::<SessionFrame>(OUTPUT_CHANNEL_CAPACITY);
    let replay = Arc::new(Mutex::new(Vec::with_capacity(REPLAY_BUFFER_BYTES)));
    let exit_code = Arc::new(Mutex::new(None));
    let flow_paused = Arc::new(AtomicBool::new(false));

    let session = Arc::new(DaemonSession {
        session_id: session_id.clone(),
        workspace_id,
        pid,
        argv,
        cwd,
        rows,
        cols,
        created_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
        master: Arc::new(Mutex::new(pair.master)),
        writer: Arc::new(Mutex::new(writer)),
        frame_tx: tx.clone(),
        replay: replay.clone(),
        exit_code: exit_code.clone(),
        flow_paused: flow_paused.clone(),
    });

    {
        let mut guard = state.lock().await;
        guard.sessions.insert(session_id.clone(), session.clone());
    }

    // Read loop on a blocking thread — portable-pty's reader is sync.
    let read_session_id = session_id.clone();
    let read_tx = tx.clone();
    let read_replay = replay;
    let read_flow_paused = flow_paused;
    std::thread::spawn(move || {
        let mut reader = reader;
        let mut buf = [0u8; 8192];
        // Tracks how long we've been continuously parked, for the
        // `FLOW_MAX_PARK` backstop.
        let mut paused_since: Option<Instant> = None;
        loop {
            // Flow-control gate. While the attached client signals it's
            // behind (its xterm parser can't keep up), we stop draining the
            // master fd. The kernel PTY buffer then fills and the child's
            // next `write()` blocks — backpressure straight to the producer,
            // instead of us overflowing the broadcast channel and dropping
            // frames (which corrupts the rendered terminal until a redraw).
            //
            // We poll the flag rather than block on a condvar: it guarantees
            // we re-check even if a wake is somehow missed, and it lets the
            // `FLOW_MAX_PARK` backstop fire so a wedged/crashed client can
            // never block the child forever. Exit detection is unaffected —
            // the waiter thread reaps the child and emits `Exited`
            // independently of this loop.
            while read_flow_paused.load(Ordering::Relaxed) {
                let since = *paused_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= FLOW_MAX_PARK {
                    eprintln!(
                        "[codemux::pty_daemon] session {read_session_id} flow-paused for \
                         >{}s — force-resuming (client wedged or resume frame lost)",
                        FLOW_MAX_PARK.as_secs()
                    );
                    read_flow_paused.store(false, Ordering::Relaxed);
                    break;
                }
                std::thread::sleep(FLOW_PARK_POLL);
            }
            paused_since = None;

            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let chunk = buf[..n].to_vec();
                    // Append to replay buffer; trim oldest bytes if over
                    // capacity. We use blocking_lock here because we're on
                    // a std::thread (not a tokio worker).
                    {
                        let mut rb = read_replay.blocking_lock();
                        rb.extend_from_slice(&chunk);
                        if rb.len() > REPLAY_BUFFER_BYTES {
                            let excess = rb.len() - REPLAY_BUFFER_BYTES;
                            rb.drain(0..excess);
                        }
                    }
                    let _ = read_tx.send(SessionFrame::Output(chunk));
                }
                Err(error) => {
                    eprintln!(
                        "[codemux::pty_daemon] read error on session {read_session_id}: {error}"
                    );
                    break;
                }
            }
        }
        // EOF on the master — child has exited or the slave was closed.
        // We DO NOT touch the session map here; the waiter thread owns
        // teardown so the exit_code lands before the session disappears.
    });

    // Waiter thread: owns the Child, blocks on wait(), publishes the real
    // exit code, then evicts the session from the daemon's state. We pin
    // the rt handle so we can hop back into the tokio world to drop the
    // session under the same `Mutex` everyone else uses.
    let wait_session_id = session_id.clone();
    let wait_state = state.clone();
    let wait_tx = tx;
    let wait_exit_code = exit_code;
    let rt_handle = tokio::runtime::Handle::current();
    std::thread::spawn(move || {
        let mut child = child;
        let code: i32 = match child.wait() {
            Ok(status) => {
                // ExitStatus on Unix encodes signal+code; portable-pty's
                // ExitStatus exposes only the numeric code. Anything other
                // than a clean exit reports as a non-zero code already.
                status.exit_code() as i32
            }
            Err(error) => {
                eprintln!(
                    "[codemux::pty_daemon] wait() failed on session {wait_session_id}: {error}"
                );
                -1
            }
        };
        // Record the exit code so late-attachers see it.
        rt_handle.block_on(async {
            *wait_exit_code.lock().await = Some(code);
        });
        // Emit Exited to any currently-attached client.
        let _ = wait_tx.send(SessionFrame::Exited(code));
        // Evict from the daemon's session map so subsequent
        // Write/Resize/Attach for this id error with "unknown session".
        rt_handle.block_on(async {
            let mut guard = wait_state.lock().await;
            guard.sessions.remove(&wait_session_id);
        });
    });

    Ok(pid)
}

#[cfg(unix)]
fn kill_session_pid(pid: u32) {
    // Same single-SIGKILL killpg policy as the in-process path uses, for
    // the same PID-reuse-race reasons (see terminal::kill_session_tree).
    let pid_i32 = pid as i32;
    if pid_i32 <= 1 {
        return;
    }
    let ret = unsafe { libc::killpg(pid_i32, libc::SIGKILL) };
    if ret != 0 {
        // Try kill() as a fallback — the child may not be a process-group
        // leader if portable-pty didn't setsid on this platform.
        let _ = unsafe { libc::kill(pid_i32, libc::SIGKILL) };
    }
}

#[cfg(not(unix))]
fn kill_session_pid(_pid: u32) {
    // Windows path TBD — TerminateProcess + JobObject. Tracked in
    // the windows-support follow-up; for the MVP we only run on Unix.
}

#[cfg(all(test, unix))]
mod tests {
    use super::pid_file_for;
    use std::path::Path;

    #[test]
    fn pid_file_is_socket_path_plus_pid_suffix() {
        // The host-side SSH reconnect probe and the daemon must agree on
        // the pidfile location: it's exactly `<socket>.pid`. A drift here
        // would silently break daemon reuse (reattach after app close).
        assert_eq!(
            pid_file_for(Path::new("/tmp/codemux-ptyd-abc.sock"))
                .to_str()
                .unwrap(),
            "/tmp/codemux-ptyd-abc.sock.pid"
        );
    }

    // A request whose bytes arrive split across the server's read timeout must
    // still be parsed (not silently corrupted). Reproduces the read_line
    // cancellation-safety bug: the first half is consumed off the socket, the
    // ~10ms timeout fires, and the old code dropped those bytes.
    #[cfg(unix)]
    #[tokio::test]
    async fn request_split_across_read_timeout_is_not_lost() {
        use super::{handle_connection, DaemonState};
        use std::sync::Arc;
        use std::time::Duration;
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        use tokio::sync::Mutex;

        let state = Arc::new(Mutex::new(DaemonState::default()));
        let (client, server) = tokio::net::UnixStream::pair().unwrap();
        let handle = tokio::spawn(handle_connection(server, state));

        let (client_read, mut client_write) = client.into_split();

        // Send a valid `Hello` request in two halves, with a gap well past the
        // server's ~10ms read timeout so the read times out mid-line.
        client_write
            .write_all(b"{\"type\":\"hello\",\"requ")
            .await
            .unwrap();
        client_write.flush().await.unwrap();
        tokio::time::sleep(Duration::from_millis(60)).await;
        client_write.write_all(b"est_id\":7}\n").await.unwrap();
        client_write.flush().await.unwrap();

        let mut reader = BufReader::new(client_read);
        let mut response = String::new();
        tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut response))
            .await
            .expect("server did not respond in time")
            .expect("read response");

        assert!(
            response.contains("protocol_version") && response.contains("hello"),
            "expected a Hello response, got: {response}"
        );
        assert!(
            !response.contains("invalid request"),
            "request was corrupted by the read timeout: {response}"
        );

        handle.abort();
    }
}
