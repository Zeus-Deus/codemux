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
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, Mutex};

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

    let state: SharedState = Arc::new(Mutex::new(DaemonState::default()));

    eprintln!(
        "[codemux::pty_daemon] listening on {:?} pid={} version={}",
        socket_path,
        std::process::id(),
        env!("CARGO_PKG_VERSION"),
    );

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

    let mut line = String::new();
    loop {
        line.clear();
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
        let read_result =
            tokio::time::timeout(read_timeout, reader.read_line(&mut line)).await;
        let read_n = match read_result {
            Ok(Ok(n)) => n,
            Ok(Err(error)) => return Err(format!("read_line: {error}")),
            Err(_elapsed) => {
                // Timeout — go back to draining.
                continue;
            }
        };
        if read_n == 0 {
            return Ok(()); // client closed cleanly
        }

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
            // Spawn the exit after replying so the client gets the
            // ShuttingDown frame.
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                std::process::exit(0);
            });
            ServerResponse::ShuttingDown { request_id }
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
    let resolved_cwd = expand_tilde(&cwd);
    let cwd_exists = std::path::Path::new(&resolved_cwd).exists();
    eprintln!(
        "[daemon::spawn] session={session_id} input_cwd={cwd:?} \
         resolved_cwd={resolved_cwd:?} exists={cwd_exists} \
         HOME={:?}",
        std::env::var("HOME").ok()
    );
    cmd.cwd(&resolved_cwd);
    for (k, v) in &env {
        cmd.env(k, v);
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
    });

    {
        let mut guard = state.lock().await;
        guard.sessions.insert(session_id.clone(), session.clone());
    }

    // Read loop on a blocking thread — portable-pty's reader is sync.
    let read_session_id = session_id.clone();
    let read_tx = tx.clone();
    let read_replay = replay;
    std::thread::spawn(move || {
        let mut reader = reader;
        let mut buf = [0u8; 8192];
        loop {
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

/// Expand a leading `~/` (or bare `~`) in a path-as-string into the
/// process's `$HOME`. No-op for paths without a leading tilde or when
/// `$HOME` is unset.
///
/// Why this lives on the daemon side: tunneled spawns from a remote
/// workspace pass `~/.codemux/worktrees/<project>/<branch>` as cwd
/// because the laptop side doesn't know the remote's `$HOME`. The
/// daemon DOES know its own `$HOME`. Resolving here means the laptop
/// stays portable and we avoid a round trip to ask "what's your HOME".
fn expand_tilde(path: &str) -> String {
    if path == "~" {
        return std::env::var("HOME").unwrap_or_else(|_| path.to_string());
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{home}/{rest}");
        }
    }
    path.to_string()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_tilde_slash_uses_home_env() {
        // SAFETY: setting env in a single-threaded #[test] is fine
        // because cargo test runs each test in its own thread but the
        // env mutation here is scoped to assertion-checking only and
        // doesn't outlive this test.
        std::env::set_var("HOME", "/fake/home");
        assert_eq!(
            expand_tilde("~/.codemux/worktrees/proj/branch"),
            "/fake/home/.codemux/worktrees/proj/branch"
        );
    }

    #[test]
    fn expand_tilde_bare_returns_home() {
        std::env::set_var("HOME", "/another/home");
        assert_eq!(expand_tilde("~"), "/another/home");
    }

    #[test]
    fn expand_tilde_absolute_path_unchanged() {
        std::env::set_var("HOME", "/whatever");
        assert_eq!(expand_tilde("/usr/local/bin"), "/usr/local/bin");
    }

    #[test]
    fn expand_tilde_relative_path_unchanged() {
        std::env::set_var("HOME", "/whatever");
        assert_eq!(expand_tilde("relative/path"), "relative/path");
    }

    #[test]
    fn expand_tilde_mid_path_tilde_unchanged() {
        // We only handle a LEADING tilde — `foo/~/bar` is not a
        // tilde-expansion form; treat it as a literal path.
        std::env::set_var("HOME", "/whatever");
        assert_eq!(expand_tilde("foo/~/bar"), "foo/~/bar");
    }
}
