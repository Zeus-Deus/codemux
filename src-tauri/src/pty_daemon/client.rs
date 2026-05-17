//! Tauri-side client for the PTY daemon. One client owns one socket
//! connection. Use `PtyDaemonClient::connect` to dial; then `spawn`,
//! `attach`, `write`, etc. map to wire requests.
//!
//! `attach` returns a `tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>`
//! the caller drains in a background task — this is the "PTY output stream"
//! that the existing terminal code expects.

use crate::pty_daemon::protocol::{
    ClientRequest, DaemonSessionInfo, Frame, ServerEvent, ServerResponse,
};
use base64::Engine;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{unix::OwnedWriteHalf, UnixStream};
use tokio::sync::{mpsc, oneshot, Mutex};

/// Errors returned by every client method. We collapse network, protocol,
/// and daemon-side errors into one type so callers don't have to nest
/// `Result<Result<_, _>, _>`.
#[derive(Debug)]
pub enum PtyDaemonError {
    Io(std::io::Error),
    Serde(serde_json::Error),
    Daemon(String),
    Closed,
    Base64(String),
}

impl std::fmt::Display for PtyDaemonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Serde(e) => write!(f, "serde: {e}"),
            Self::Daemon(m) => write!(f, "daemon: {m}"),
            Self::Closed => write!(f, "client closed before response"),
            Self::Base64(m) => write!(f, "base64 decode: {m}"),
        }
    }
}

impl std::error::Error for PtyDaemonError {}

impl From<std::io::Error> for PtyDaemonError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for PtyDaemonError {
    fn from(e: serde_json::Error) -> Self {
        Self::Serde(e)
    }
}

type PendingMap = Arc<Mutex<HashMap<u64, oneshot::Sender<ServerResponse>>>>;
type AttachMap = Arc<Mutex<HashMap<String, mpsc::UnboundedSender<Vec<u8>>>>>;

/// Long-lived client. Internally maintains a background reader task that
/// demuxes inbound frames to either pending request callers (via oneshot)
/// or attached-session subscribers (via mpsc).
pub struct PtyDaemonClient {
    writer: Arc<Mutex<OwnedWriteHalf>>,
    next_request_id: AtomicU64,
    pending: PendingMap,
    attached: AttachMap,
}

impl PtyDaemonClient {
    /// Test-only constructor that produces a real `Arc<PtyDaemonClient>`
    /// with a connected-but-unused socket pair, so unit tests that need
    /// to verify Arc identity (e.g. `Arc::ptr_eq` checks in
    /// `terminal::is_runtime_owned_by_client`) can produce distinct
    /// client allocations without setting up a real daemon process.
    ///
    /// The returned client is functional for `Arc::ptr_eq` but will hang
    /// indefinitely on any request — never use it for actual RPC in
    /// tests.
    #[cfg(test)]
    pub(crate) async fn new_for_test_arc_identity() -> Arc<Self> {
        use tokio::net::UnixStream;
        // socketpair() guarantees we get two halves we can hold
        // forever without external setup; the other half is dropped
        // immediately to avoid leaking fds, since we don't actually
        // exchange frames in these tests.
        let (a, _b) = UnixStream::pair().expect("socketpair");
        let (_read_half, write_half) = a.into_split();
        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let attached: AttachMap = Arc::new(Mutex::new(HashMap::new()));
        Arc::new(Self {
            writer: Arc::new(Mutex::new(write_half)),
            next_request_id: AtomicU64::new(1),
            pending,
            attached,
        })
    }

    pub async fn connect(socket_path: &Path) -> Result<Arc<Self>, PtyDaemonError> {
        let stream = UnixStream::connect(socket_path).await?;
        let (read_half, write_half) = stream.into_split();

        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));
        let attached: AttachMap = Arc::new(Mutex::new(HashMap::new()));

        let client = Arc::new(Self {
            writer: Arc::new(Mutex::new(write_half)),
            next_request_id: AtomicU64::new(1),
            pending: pending.clone(),
            attached: attached.clone(),
        });

        // Background reader task. Owns the read half exclusively.
        let bg_pending = pending.clone();
        let bg_attached = attached.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(read_half);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(error) => {
                        eprintln!("[codemux::pty_daemon::client] read: {error}");
                        break;
                    }
                }
                let trimmed = line.trim_end_matches(['\n', '\r']);
                if trimmed.is_empty() {
                    continue;
                }
                let frame: Frame = match serde_json::from_str(trimmed) {
                    Ok(f) => f,
                    Err(error) => {
                        eprintln!(
                            "[codemux::pty_daemon::client] bad frame {trimmed:?}: {error}"
                        );
                        continue;
                    }
                };
                match frame {
                    Frame::Response(resp) => {
                        let request_id = response_request_id(&resp);
                        let sender = {
                            let mut guard = bg_pending.lock().await;
                            guard.remove(&request_id)
                        };
                        if let Some(sender) = sender {
                            let _ = sender.send(resp);
                        } else {
                            eprintln!(
                                "[codemux::pty_daemon::client] orphan response id={request_id}"
                            );
                        }
                    }
                    Frame::Event(ServerEvent::Output {
                        session_id,
                        data_b64,
                    }) => {
                        let bytes = match base64::engine::general_purpose::STANDARD
                            .decode(&data_b64)
                        {
                            Ok(b) => b,
                            Err(error) => {
                                eprintln!(
                                    "[codemux::pty_daemon::client] bad b64 from daemon: {error}"
                                );
                                continue;
                            }
                        };
                        let sender = {
                            let guard = bg_attached.lock().await;
                            guard.get(&session_id).cloned()
                        };
                        if let Some(sender) = sender {
                            let _ = sender.send(bytes);
                        }
                    }
                    Frame::Event(ServerEvent::Exited {
                        session_id,
                        exit_code: _,
                    }) => {
                        let mut guard = bg_attached.lock().await;
                        guard.remove(&session_id);
                    }
                }
            }
            // Reader ended — clear pending so callers don't hang.
            let mut guard = bg_pending.lock().await;
            for (_, sender) in guard.drain() {
                drop(sender); // recv() will see RecvError → ::Closed
            }
        });

        Ok(client)
    }

    fn next_id(&self) -> u64 {
        self.next_request_id.fetch_add(1, Ordering::Relaxed)
    }

    async fn send_request(
        &self,
        request: ClientRequest,
        request_id: u64,
    ) -> Result<ServerResponse, PtyDaemonError> {
        let (tx, rx) = oneshot::channel();
        {
            let mut guard = self.pending.lock().await;
            guard.insert(request_id, tx);
        }
        let mut bytes = serde_json::to_vec(&request)?;
        bytes.push(b'\n');
        {
            let mut writer = self.writer.lock().await;
            writer.write_all(&bytes).await?;
            writer.flush().await?;
        }
        match rx.await {
            Ok(resp) => Ok(resp),
            Err(_) => Err(PtyDaemonError::Closed),
        }
    }

    pub async fn hello(&self) -> Result<(u32, String, u32), PtyDaemonError> {
        let id = self.next_id();
        match self
            .send_request(ClientRequest::Hello { request_id: id }, id)
            .await?
        {
            ServerResponse::Hello {
                protocol_version,
                daemon_pid,
                daemon_version,
                ..
            } => Ok((daemon_pid, daemon_version, protocol_version)),
            ServerResponse::Error { message, .. } => Err(PtyDaemonError::Daemon(message)),
            other => Err(PtyDaemonError::Daemon(format!(
                "unexpected response to Hello: {other:?}"
            ))),
        }
    }

    pub async fn spawn(
        &self,
        session_id: String,
        workspace_id: String,
        argv: Vec<String>,
        cwd: String,
        env: Vec<(String, String)>,
        rows: u16,
        cols: u16,
    ) -> Result<u32, PtyDaemonError> {
        let id = self.next_id();
        match self
            .send_request(
                ClientRequest::Spawn {
                    request_id: id,
                    session_id,
                    workspace_id,
                    argv,
                    cwd,
                    env,
                    rows,
                    cols,
                },
                id,
            )
            .await?
        {
            ServerResponse::Spawned { pid, .. } => Ok(pid),
            ServerResponse::Error { message, .. } => Err(PtyDaemonError::Daemon(message)),
            other => Err(PtyDaemonError::Daemon(format!(
                "unexpected response to Spawn: {other:?}"
            ))),
        }
    }

    /// Attach to a session and return the output receiver. Drains on a
    /// background task spawned by the caller — every byte the daemon
    /// pushes for this session ends up here.
    pub async fn attach(
        &self,
        session_id: String,
    ) -> Result<mpsc::UnboundedReceiver<Vec<u8>>, PtyDaemonError> {
        let (tx, rx) = mpsc::unbounded_channel::<Vec<u8>>();
        {
            let mut guard = self.attached.lock().await;
            guard.insert(session_id.clone(), tx);
        }
        let id = self.next_id();
        match self
            .send_request(
                ClientRequest::Attach {
                    request_id: id,
                    session_id: session_id.clone(),
                },
                id,
            )
            .await?
        {
            ServerResponse::Attached { .. } => Ok(rx),
            ServerResponse::Error { message, .. } => {
                let mut guard = self.attached.lock().await;
                guard.remove(&session_id);
                Err(PtyDaemonError::Daemon(message))
            }
            other => Err(PtyDaemonError::Daemon(format!(
                "unexpected response to Attach: {other:?}"
            ))),
        }
    }

    pub async fn detach(&self, session_id: String) -> Result<(), PtyDaemonError> {
        {
            let mut guard = self.attached.lock().await;
            guard.remove(&session_id);
        }
        let id = self.next_id();
        match self
            .send_request(
                ClientRequest::Detach {
                    request_id: id,
                    session_id,
                },
                id,
            )
            .await?
        {
            ServerResponse::Detached { .. } => Ok(()),
            ServerResponse::Error { message, .. } => Err(PtyDaemonError::Daemon(message)),
            other => Err(PtyDaemonError::Daemon(format!(
                "unexpected response to Detach: {other:?}"
            ))),
        }
    }

    pub async fn write(&self, session_id: String, data: &[u8]) -> Result<(), PtyDaemonError> {
        let id = self.next_id();
        let data_b64 = base64::engine::general_purpose::STANDARD.encode(data);
        match self
            .send_request(
                ClientRequest::Write {
                    request_id: id,
                    session_id,
                    data_b64,
                },
                id,
            )
            .await?
        {
            ServerResponse::Written { .. } => Ok(()),
            ServerResponse::Error { message, .. } => Err(PtyDaemonError::Daemon(message)),
            other => Err(PtyDaemonError::Daemon(format!(
                "unexpected response to Write: {other:?}"
            ))),
        }
    }

    pub async fn resize(
        &self,
        session_id: String,
        rows: u16,
        cols: u16,
    ) -> Result<(), PtyDaemonError> {
        let id = self.next_id();
        match self
            .send_request(
                ClientRequest::Resize {
                    request_id: id,
                    session_id,
                    rows,
                    cols,
                },
                id,
            )
            .await?
        {
            ServerResponse::Resized { .. } => Ok(()),
            ServerResponse::Error { message, .. } => Err(PtyDaemonError::Daemon(message)),
            other => Err(PtyDaemonError::Daemon(format!(
                "unexpected response to Resize: {other:?}"
            ))),
        }
    }

    pub async fn close(&self, session_id: String) -> Result<(), PtyDaemonError> {
        let id = self.next_id();
        match self
            .send_request(
                ClientRequest::Close {
                    request_id: id,
                    session_id,
                },
                id,
            )
            .await?
        {
            ServerResponse::Closed { .. } => Ok(()),
            ServerResponse::Error { message, .. } => Err(PtyDaemonError::Daemon(message)),
            other => Err(PtyDaemonError::Daemon(format!(
                "unexpected response to Close: {other:?}"
            ))),
        }
    }

    pub async fn list(&self) -> Result<Vec<DaemonSessionInfo>, PtyDaemonError> {
        let id = self.next_id();
        match self
            .send_request(ClientRequest::List { request_id: id }, id)
            .await?
        {
            ServerResponse::Listed { sessions, .. } => Ok(sessions),
            ServerResponse::Error { message, .. } => Err(PtyDaemonError::Daemon(message)),
            other => Err(PtyDaemonError::Daemon(format!(
                "unexpected response to List: {other:?}"
            ))),
        }
    }

    pub async fn shutdown(&self) -> Result<(), PtyDaemonError> {
        let id = self.next_id();
        match self
            .send_request(ClientRequest::Shutdown { request_id: id }, id)
            .await?
        {
            ServerResponse::ShuttingDown { .. } => Ok(()),
            ServerResponse::Error { message, .. } => Err(PtyDaemonError::Daemon(message)),
            other => Err(PtyDaemonError::Daemon(format!(
                "unexpected response to Shutdown: {other:?}"
            ))),
        }
    }
}

fn response_request_id(resp: &ServerResponse) -> u64 {
    match resp {
        ServerResponse::Hello { request_id, .. }
        | ServerResponse::Spawned { request_id, .. }
        | ServerResponse::Attached { request_id, .. }
        | ServerResponse::Detached { request_id, .. }
        | ServerResponse::Written { request_id }
        | ServerResponse::Resized { request_id }
        | ServerResponse::Closed { request_id }
        | ServerResponse::Listed { request_id, .. }
        | ServerResponse::ShuttingDown { request_id }
        | ServerResponse::Error { request_id, .. } => *request_id,
    }
}
