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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{unix::OwnedWriteHalf, UnixStream};
use tokio::sync::{mpsc, oneshot, Mutex as AsyncMutex};

const DEFAULT_RPC_TIMEOUT: Duration = Duration::from_secs(10);

/// Errors returned by every client method. We collapse network, protocol,
/// and daemon-side errors into one type so callers don't have to nest
/// `Result<Result<_, _>, _>`.
#[derive(Debug)]
pub enum PtyDaemonError {
    Io(std::io::Error),
    Serde(serde_json::Error),
    Daemon(String),
    Closed,
    Timeout,
    Base64(String),
}

impl std::fmt::Display for PtyDaemonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Serde(e) => write!(f, "serde: {e}"),
            Self::Daemon(m) => write!(f, "daemon: {m}"),
            Self::Closed => write!(f, "client closed before response"),
            Self::Timeout => write!(f, "daemon RPC timed out"),
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

type PendingMap = Arc<StdMutex<HashMap<u64, oneshot::Sender<ServerResponse>>>>;
type AttachMap = Arc<StdMutex<HashMap<String, mpsc::UnboundedSender<Vec<u8>>>>>;

/// Removes a request sender even when the request future is cancelled. A
/// synchronous mutex keeps Drop deterministic; the map is held only for a
/// single hash operation and never across socket I/O.
struct PendingReservation {
    pending: PendingMap,
    request_id: u64,
}

impl Drop for PendingReservation {
    fn drop(&mut self) {
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.request_id);
    }
}

/// Poisons the connection when a request frame write is abandoned between its
/// first byte and its flush. The RPC deadline (or an outer cancellation)
/// drops the in-flight write future; without this, the next request would
/// append a fresh frame right after the partial one and the daemon's line
/// reader would parse both as a single corrupt line, silently desyncing the
/// stream until EOF. Declared after the writer lock guard so `closed` is
/// published before the lock is released to any waiting writer.
struct WriteInFlightGuard {
    closed: Arc<AtomicBool>,
    armed: bool,
}

impl WriteInFlightGuard {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for WriteInFlightGuard {
    fn drop(&mut self) {
        if self.armed {
            self.closed.store(true, Ordering::Release);
        }
    }
}

/// Attach registers its output sender before sending the RPC so early output
/// cannot race past it. This guard removes that sender on every non-success
/// path (write failure, timeout, disconnect, unexpected response, cancellation).
struct AttachReservation {
    attached: AttachMap,
    session_id: String,
    committed: bool,
}

impl AttachReservation {
    fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for AttachReservation {
    fn drop(&mut self) {
        if !self.committed {
            self.attached
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&self.session_id);
        }
    }
}

/// Long-lived client. Internally maintains a background reader task that
/// demuxes inbound frames to either pending request callers (via oneshot)
/// or attached-session subscribers (via mpsc).
pub struct PtyDaemonClient {
    writer: Arc<AsyncMutex<OwnedWriteHalf>>,
    next_request_id: AtomicU64,
    pending: PendingMap,
    attached: AttachMap,
    /// Set as soon as the reader observes EOF/error (or an RPC observes a
    /// terminal socket error). Supervisors use this to evict a cached client
    /// instead of returning the same permanently-dead connection forever.
    closed: Arc<AtomicBool>,
    rpc_timeout: Duration,
}

impl PtyDaemonClient {
    /// Test-only constructor that produces a real `Arc<PtyDaemonClient>`
    /// with a connected-but-unused socket pair, so unit tests that need
    /// to verify Arc identity (e.g. `Arc::ptr_eq` checks in
    /// `terminal::is_runtime_owned_by_client`) can produce distinct
    /// client allocations without setting up a real daemon process.
    ///
    /// The returned client is functional for `Arc::ptr_eq` but has no reader
    /// task or daemon peer, so requests only fail by the normal RPC deadline —
    /// never use it for actual protocol tests.
    #[cfg(test)]
    pub(crate) async fn new_for_test_arc_identity() -> Arc<Self> {
        use tokio::net::UnixStream;
        // socketpair() guarantees we get two halves we can hold
        // forever without external setup; the other half is dropped
        // immediately to avoid leaking fds, since we don't actually
        // exchange frames in these tests.
        let (a, _b) = UnixStream::pair().expect("socketpair");
        let (_read_half, write_half) = a.into_split();
        let pending: PendingMap = Arc::new(StdMutex::new(HashMap::new()));
        let attached: AttachMap = Arc::new(StdMutex::new(HashMap::new()));
        Arc::new(Self {
            writer: Arc::new(AsyncMutex::new(write_half)),
            next_request_id: AtomicU64::new(1),
            pending,
            attached,
            closed: Arc::new(AtomicBool::new(false)),
            rpc_timeout: DEFAULT_RPC_TIMEOUT,
        })
    }

    pub async fn connect(socket_path: &Path) -> Result<Arc<Self>, PtyDaemonError> {
        let stream = UnixStream::connect(socket_path).await?;
        Ok(Self::from_stream(stream, DEFAULT_RPC_TIMEOUT))
    }

    fn from_stream(stream: UnixStream, rpc_timeout: Duration) -> Arc<Self> {
        let (read_half, write_half) = stream.into_split();

        let pending: PendingMap = Arc::new(StdMutex::new(HashMap::new()));
        let attached: AttachMap = Arc::new(StdMutex::new(HashMap::new()));
        let closed = Arc::new(AtomicBool::new(false));

        let client = Arc::new(Self {
            writer: Arc::new(AsyncMutex::new(write_half)),
            next_request_id: AtomicU64::new(1),
            pending: pending.clone(),
            attached: attached.clone(),
            closed: closed.clone(),
            rpc_timeout,
        });

        // Background reader task. Owns the read half exclusively.
        let bg_pending = pending.clone();
        let bg_attached = attached.clone();
        let bg_closed = closed;
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
                        eprintln!("[codemux::pty_daemon::client] bad frame {trimmed:?}: {error}");
                        continue;
                    }
                };
                match frame {
                    Frame::Response(resp) => {
                        let request_id = response_request_id(&resp);
                        let sender = {
                            let mut guard = bg_pending.lock().unwrap_or_else(|e| e.into_inner());
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
                        let bytes =
                            match base64::engine::general_purpose::STANDARD.decode(&data_b64) {
                                Ok(b) => b,
                                Err(error) => {
                                    eprintln!(
                                    "[codemux::pty_daemon::client] bad b64 from daemon: {error}"
                                );
                                    continue;
                                }
                            };
                        let sender = {
                            let guard = bg_attached.lock().unwrap_or_else(|e| e.into_inner());
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
                        let mut guard = bg_attached.lock().unwrap_or_else(|e| e.into_inner());
                        guard.remove(&session_id);
                    }
                }
            }
            // Publish liveness before draining reservations. A caller racing
            // this teardown either sees `Closed` immediately or has its
            // already-installed oneshot drained below; it can never register a
            // new pending sender after the one-time drain and wait forever.
            bg_closed.store(true, Ordering::Release);
            // Reader ended — clear pending so callers don't hang.
            {
                let mut guard = bg_pending.lock().unwrap_or_else(|e| e.into_inner());
                for (_, sender) in guard.drain() {
                    drop(sender); // recv() will see RecvError → ::Closed
                }
            }
            // Drop attach senders too. Otherwise output receivers remain open
            // forever after a daemon disconnect and pane tasks cannot fail/retry.
            bg_attached
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clear();
        });

        client
    }

    #[cfg(test)]
    pub(crate) fn from_test_stream(stream: UnixStream, rpc_timeout: Duration) -> Arc<Self> {
        Self::from_stream(stream, rpc_timeout)
    }

    /// Whether this socket connection has permanently closed. A timeout while
    /// waiting for a response does not close the client — a slow daemon can
    /// still answer a later retry — but a timeout that cancels a partially
    /// written request frame does, because the wire framing is desynced.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    fn next_id(&self) -> u64 {
        self.next_request_id.fetch_add(1, Ordering::Relaxed)
    }

    async fn send_request(
        &self,
        request: ClientRequest,
        request_id: u64,
    ) -> Result<ServerResponse, PtyDaemonError> {
        if self.is_closed() {
            return Err(PtyDaemonError::Closed);
        }
        // Serialize before reserving a pending slot. Although today's request
        // fields are infallible JSON values, this ordering makes future custom
        // payloads incapable of leaking a sender on serialization failure.
        let mut bytes = serde_json::to_vec(&request)?;
        bytes.push(b'\n');
        let (tx, rx) = oneshot::channel();
        {
            let mut guard = self.pending.lock().unwrap_or_else(|e| e.into_inner());
            guard.insert(request_id, tx);
        }
        let _reservation = PendingReservation {
            pending: self.pending.clone(),
            request_id,
        };
        // Close can race the first check. The reader publishes `closed` before
        // draining pending senders, so this second check closes the remaining
        // gap without relying on a write to discover EOF.
        if self.is_closed() {
            return Err(PtyDaemonError::Closed);
        }
        let started = std::time::Instant::now();
        let operation = async {
            {
                let mut writer = self.writer.lock().await;
                // A cancelled peer write may have poisoned the stream while
                // this request was waiting on the lock; re-check before
                // appending a frame after its partial bytes.
                if self.is_closed() {
                    return Err(PtyDaemonError::Closed);
                }
                let mut in_flight = WriteInFlightGuard {
                    closed: self.closed.clone(),
                    armed: true,
                };
                writer.write_all(&bytes).await?;
                writer.flush().await?;
                // Fully flushed: a later deadline only abandons the wait for
                // the response, which leaves the wire framing intact, so the
                // connection stays reusable for a retry.
                in_flight.disarm();
            }
            match rx.await {
                Ok(resp) => Ok(resp),
                Err(_) => Err(PtyDaemonError::Closed),
            }
        };
        let result = match tokio::time::timeout(self.rpc_timeout, operation).await {
            Ok(result) => result,
            Err(_) => Err(PtyDaemonError::Timeout),
        };
        if matches!(
            &result,
            Err(PtyDaemonError::Io(_)) | Err(PtyDaemonError::Closed)
        ) {
            self.closed.store(true, Ordering::Release);
        }
        crate::diagnostics::record_perf_timing("pty.daemon-rpc", started.elapsed());
        result
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
            let mut guard = self.attached.lock().unwrap_or_else(|e| e.into_inner());
            guard.insert(session_id.clone(), tx);
        }
        let mut reservation = AttachReservation {
            attached: self.attached.clone(),
            session_id: session_id.clone(),
            committed: false,
        };
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
            ServerResponse::Attached { .. } => {
                reservation.commit();
                Ok(rx)
            }
            ServerResponse::Error { message, .. } => Err(PtyDaemonError::Daemon(message)),
            other => Err(PtyDaemonError::Daemon(format!(
                "unexpected response to Attach: {other:?}"
            ))),
        }
    }

    pub async fn detach(&self, session_id: String) -> Result<(), PtyDaemonError> {
        {
            let mut guard = self.attached.lock().unwrap_or_else(|e| e.into_inner());
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

    /// Pause or resume the daemon's read loop for a session (terminal output
    /// flow control — see `ClientRequest::SetFlowPaused`). Best-effort: an
    /// unknown session id (the session may have just exited) surfaces as a
    /// `Daemon` error the caller is expected to log-and-ignore.
    pub async fn set_flow_paused(
        &self,
        session_id: String,
        paused: bool,
    ) -> Result<(), PtyDaemonError> {
        let id = self.next_id();
        match self
            .send_request(
                ClientRequest::SetFlowPaused {
                    request_id: id,
                    session_id,
                    paused,
                },
                id,
            )
            .await?
        {
            ServerResponse::FlowPaused { .. } => Ok(()),
            ServerResponse::Error { message, .. } => Err(PtyDaemonError::Daemon(message)),
            other => Err(PtyDaemonError::Daemon(format!(
                "unexpected response to SetFlowPaused: {other:?}"
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    async fn read_request(
        reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
    ) -> ClientRequest {
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        serde_json::from_str(line.trim()).unwrap()
    }

    async fn write_response(
        writer: &mut tokio::net::unix::OwnedWriteHalf,
        response: ServerResponse,
    ) {
        let mut bytes = serde_json::to_vec(&Frame::Response(response)).unwrap();
        bytes.push(b'\n');
        writer.write_all(&bytes).await.unwrap();
        writer.flush().await.unwrap();
    }

    #[tokio::test]
    async fn silent_server_times_out_clears_pending_and_retry_succeeds() {
        let (client_stream, server_stream) = UnixStream::pair().unwrap();
        let client = PtyDaemonClient::from_stream(client_stream, Duration::from_millis(80));
        let (server_read, mut server_write) = server_stream.into_split();
        let server = tokio::spawn(async move {
            let mut reader = BufReader::new(server_read);
            let first = read_request(&mut reader).await;
            assert!(matches!(first, ClientRequest::List { .. }));
            // Keep the connection open but deliberately omit the first reply.
            tokio::time::sleep(Duration::from_millis(100)).await;
            let second = read_request(&mut reader).await;
            let ClientRequest::List { request_id } = second else {
                panic!("expected retry List")
            };
            write_response(
                &mut server_write,
                ServerResponse::Listed {
                    request_id,
                    sessions: Vec::new(),
                },
            )
            .await;
        });

        assert!(matches!(client.list().await, Err(PtyDaemonError::Timeout)));
        assert!(client
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty());
        assert!(client.list().await.unwrap().is_empty());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn timeout_during_partial_frame_write_poisons_connection() {
        let (client_stream, server_stream) = UnixStream::pair().unwrap();
        let client = PtyDaemonClient::from_stream(client_stream, Duration::from_millis(100));
        // Hold the peer open but never read: the socket buffers fill and
        // write_all parks mid-frame until the deadline cancels it, leaving a
        // partial frame with no trailing newline on the wire.
        let _stalled_peer = server_stream;
        // Large enough that a single Write frame cannot fit in the socket
        // buffers, guaranteeing the deadline fires during the write.
        let payload = vec![b'x'; 4 * 1024 * 1024];
        assert!(matches!(
            client.write("session".into(), &payload).await,
            Err(PtyDaemonError::Timeout)
        ));
        assert!(
            client.is_closed(),
            "a deadline that cancels a partially-written frame must poison \
             the connection so callers reconnect instead of appending the \
             next frame to a desynced stream"
        );
        assert!(
            client
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty(),
            "the abandoned request must not leave a pending sender"
        );
        // The poisoned client refuses reuse; the retry path reconnects.
        assert!(matches!(client.list().await, Err(PtyDaemonError::Closed)));
        let (client_stream, server_stream) = UnixStream::pair().unwrap();
        let client = PtyDaemonClient::from_stream(client_stream, Duration::from_secs(1));
        let (server_read, mut server_write) = server_stream.into_split();
        let server = tokio::spawn(async move {
            let mut reader = BufReader::new(server_read);
            let ClientRequest::List { request_id } = read_request(&mut reader).await else {
                panic!("expected List on the fresh connection")
            };
            write_response(
                &mut server_write,
                ServerResponse::Listed {
                    request_id,
                    sessions: Vec::new(),
                },
            )
            .await;
        });
        assert!(client.list().await.unwrap().is_empty());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn disconnect_drops_attach_sender_and_closes_output_receiver() {
        let (client_stream, server_stream) = UnixStream::pair().unwrap();
        let client = PtyDaemonClient::from_stream(client_stream, Duration::from_secs(1));
        let (server_read, mut server_write) = server_stream.into_split();
        tokio::spawn(async move {
            let mut reader = BufReader::new(server_read);
            let request = read_request(&mut reader).await;
            let ClientRequest::Attach {
                request_id,
                session_id,
            } = request
            else {
                panic!("expected Attach")
            };
            write_response(
                &mut server_write,
                ServerResponse::Attached {
                    request_id,
                    session_id,
                },
            )
            .await;
            // Dropping both halves simulates a daemon disconnect.
        });

        let mut output = client.attach("session".into()).await.unwrap();
        let closed = tokio::time::timeout(Duration::from_secs(1), output.recv())
            .await
            .expect("output receiver should close on disconnect");
        assert!(closed.is_none());
        assert!(client
            .attached
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty());
        assert!(client.is_closed(), "reader EOF must mark the client closed");
    }

    #[tokio::test]
    async fn attach_timeout_clears_output_sender_reservation() {
        let (client_stream, server_stream) = UnixStream::pair().unwrap();
        let client = PtyDaemonClient::from_stream(client_stream, Duration::from_millis(50));
        let (server_read, server_write) = server_stream.into_split();
        let server = tokio::spawn(async move {
            let _keep_open = server_write;
            let mut reader = BufReader::new(server_read);
            assert!(matches!(
                read_request(&mut reader).await,
                ClientRequest::Attach { .. }
            ));
            tokio::time::sleep(Duration::from_millis(80)).await;
        });

        assert!(matches!(
            client.attach("session".into()).await,
            Err(PtyDaemonError::Timeout)
        ));
        assert!(client
            .attached
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn write_failure_never_leaves_a_pending_sender() {
        let (client_stream, server_stream) = UnixStream::pair().unwrap();
        drop(server_stream);
        let client = PtyDaemonClient::from_stream(client_stream, Duration::from_millis(100));
        assert!(client.list().await.is_err());
        assert!(client
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty());
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
        | ServerResponse::FlowPaused { request_id }
        | ServerResponse::Error { request_id, .. } => *request_id,
    }
}
