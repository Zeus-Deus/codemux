//! Long-lived JSON-RPC-over-stdio child-process helper.
//!
//! Many of the CLI-backed agents the chat runtime talks to follow the same
//! pattern: a long-running subprocess that speaks newline-delimited JSON-RPC
//! 2.0 on its stdin/stdout. Codex's `app-server` uses exactly this
//! protocol, the Agent Client Protocol does, and future sidecars likely
//! will too.
//!
//! Rather than re-implement the framing + routing + timeout logic inside
//! each adapter, this module provides a reusable [`JsonRpcChild`] handle:
//!
//! * [`JsonRpcChild::request`] — send a request, await a typed response.
//! * [`JsonRpcChild::notify`] — fire-and-forget outgoing notification.
//! * [`JsonRpcChild::notifications`] — subscribe to incoming notifications.
//! * [`JsonRpcChild::incoming_requests`] — receive server-initiated
//!   requests (tool approvals, etc.) and answer them with
//!   [`JsonRpcChild::respond`].
//! * [`JsonRpcChild::shutdown`] — graceful EOF-then-kill teardown.
//!
//! The module is intentionally protocol-layer only. It knows nothing about
//! Codex's method names, Claude's SDK events, or any other provider; higher
//! layers decide what methods mean.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{broadcast, mpsc, oneshot};

/// Maximum number of stderr bytes the helper retains as diagnostic context
/// for [`RpcChildError::ChildExited`].
const STDERR_TAIL_CAPACITY: usize = 8 * 1024;
/// How long `shutdown()` waits for a cooperative exit after closing stdin
/// before resorting to `kill()`.
const GRACEFUL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
/// Depth of the incoming-request mpsc queue. Higher values absorb bursty
/// providers at the cost of more memory; the value is a conservative ceiling
/// well above what any current CLI uses.
const INCOMING_REQUEST_CHANNEL_CAPACITY: usize = 256;
/// Depth of the notifications broadcast channel. Lagging subscribers
/// observe a `Lagged` error rather than blocking the reader task.
const NOTIFICATION_CHANNEL_CAPACITY: usize = 512;

/// Parameters for spawning a JSON-RPC child process.
#[derive(Debug, Clone)]
pub struct SpawnConfig {
    /// Executable path to spawn.
    pub program: PathBuf,
    /// Arguments passed to the executable.
    pub args: Vec<String>,
    /// Environment variables overlaid onto the inherited env.
    pub env: HashMap<String, String>,
    /// Working directory, or inherit when `None`.
    pub cwd: Option<PathBuf>,
    /// Default per-request timeout used by
    /// [`JsonRpcChild::request`]. Individual callers can override via
    /// [`JsonRpcChild::request_with_timeout`].
    pub default_timeout: Duration,
}

/// JSON-RPC notification (a message with a `method` but no `id`) received
/// from the child.
#[derive(Debug, Clone)]
pub struct Notification {
    pub method: String,
    pub params: Value,
}

/// A server-initiated JSON-RPC request (has both `method` and `id`). The
/// consumer answers by calling [`JsonRpcChild::respond`] with the same `id`.
#[derive(Debug, Clone)]
pub struct IncomingRequest {
    pub id: Value,
    pub method: String,
    pub params: Value,
}

/// JSON-RPC error payload as sent over the wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "rpc error {}: {}", self.code, self.message)
    }
}

impl std::error::Error for RpcError {}

/// Every way [`JsonRpcChild`] operations can fail.
#[derive(Debug)]
pub enum RpcChildError {
    /// Spawning the subprocess failed before stdio could be attached.
    SpawnFailed(std::io::Error),
    /// The subprocess exited before a pending operation could complete.
    ChildExited {
        /// Exit code when available.
        code: Option<i32>,
        /// Last ~8KB of stderr captured for diagnostics.
        stderr_tail: String,
    },
    /// Generic I/O failure on stdin/stdout.
    IoError(std::io::Error),
    /// The child emitted a line that failed to parse as JSON.
    JsonParseError {
        line: String,
        source: serde_json::Error,
    },
    /// An awaited request exceeded its timeout budget.
    Timeout { method: String, elapsed: Duration },
    /// The child violated the JSON-RPC framing we expect (e.g. a response to
    /// an id we never issued, missing required field, ...).
    ProtocolError(String),
    /// The remote responded with a structured JSON-RPC error payload.
    RpcError(RpcError),
    /// The handle has already been shut down.
    AlreadyShutdown,
}

impl std::fmt::Display for RpcChildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SpawnFailed(err) => write!(f, "failed to spawn child process: {err}"),
            Self::ChildExited { code, stderr_tail } => {
                let code_display = code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "<signal>".into());
                if stderr_tail.is_empty() {
                    write!(f, "child process exited with code {code_display}")
                } else {
                    write!(
                        f,
                        "child process exited with code {code_display}; stderr tail: {stderr_tail}"
                    )
                }
            }
            Self::IoError(err) => write!(f, "io error: {err}"),
            Self::JsonParseError { line, source } => {
                write!(f, "failed to parse JSON line {line:?}: {source}")
            }
            Self::Timeout { method, elapsed } => {
                write!(f, "request {method:?} timed out after {elapsed:?}")
            }
            Self::ProtocolError(msg) => write!(f, "protocol error: {msg}"),
            Self::RpcError(err) => write!(f, "{err}"),
            Self::AlreadyShutdown => write!(f, "rpc child has already been shut down"),
        }
    }
}

impl std::error::Error for RpcChildError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::SpawnFailed(err) | Self::IoError(err) => Some(err),
            Self::JsonParseError { source, .. } => Some(source),
            Self::RpcError(err) => Some(err),
            _ => None,
        }
    }
}

type PendingResult = Result<Value, RpcError>;

/// Internal pending-request bookkeeping.
#[derive(Default)]
struct PendingMap {
    inner: HashMap<u64, oneshot::Sender<PendingResult>>,
}

impl PendingMap {
    fn insert(&mut self, id: u64, tx: oneshot::Sender<PendingResult>) {
        self.inner.insert(id, tx);
    }

    fn remove(&mut self, id: u64) -> Option<oneshot::Sender<PendingResult>> {
        self.inner.remove(&id)
    }

    /// Drain all outstanding senders so the reader can notify everyone that
    /// the child died.
    fn drain(&mut self) -> Vec<oneshot::Sender<PendingResult>> {
        self.inner.drain().map(|(_, tx)| tx).collect()
    }
}

/// Bounded FIFO buffer of the last-N stderr bytes.
#[derive(Default)]
struct StderrTail {
    buf: VecDeque<u8>,
}

impl StderrTail {
    fn push(&mut self, chunk: &[u8]) {
        for &b in chunk {
            if self.buf.len() == STDERR_TAIL_CAPACITY {
                self.buf.pop_front();
            }
            self.buf.push_back(b);
        }
    }

    fn snapshot(&self) -> String {
        String::from_utf8_lossy(&self.buf.iter().copied().collect::<Vec<u8>>()).into_owned()
    }
}

/// Reasons the reader task may signal that the child has gone away. Used to
/// route a uniform `ChildExited` into every outstanding caller.
#[derive(Debug, Clone)]
struct ExitInfo {
    code: Option<i32>,
    stderr_tail: String,
}

/// Handle to a running JSON-RPC child.
///
/// Cheaply cloneable — all internal state is `Arc`-shared — so adapters can
/// hand clones to background tasks freely.
pub struct JsonRpcChild {
    writer: Arc<tokio::sync::Mutex<Option<ChildStdin>>>,
    pending: Arc<Mutex<PendingMap>>,
    next_id: Arc<AtomicU64>,
    default_timeout: Duration,
    notifications_tx: broadcast::Sender<Notification>,
    incoming_rx: Arc<Mutex<Option<mpsc::Receiver<IncomingRequest>>>>,
    alive: Arc<AtomicBool>,
    exit_info: Arc<Mutex<Option<ExitInfo>>>,
    shutdown_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

impl JsonRpcChild {
    /// Spawn a child process and return a handle attached to its stdio.
    ///
    /// Fails with [`RpcChildError::SpawnFailed`] if the executable cannot be
    /// started or its stdio pipes cannot be captured.
    pub async fn spawn(config: SpawnConfig) -> Result<Self, RpcChildError> {
        let mut cmd = Command::new(&config.program);
        cmd.args(&config.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        for (k, v) in &config.env {
            cmd.env(k, v);
        }
        if let Some(cwd) = &config.cwd {
            cmd.current_dir(cwd);
        }

        let mut child = cmd.spawn().map_err(RpcChildError::SpawnFailed)?;
        let stdin = child.stdin.take().ok_or_else(|| {
            RpcChildError::SpawnFailed(std::io::Error::other("stdin pipe not captured"))
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            RpcChildError::SpawnFailed(std::io::Error::other("stdout pipe not captured"))
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            RpcChildError::SpawnFailed(std::io::Error::other("stderr pipe not captured"))
        })?;

        let writer = Arc::new(tokio::sync::Mutex::new(Some(stdin)));
        let pending: Arc<Mutex<PendingMap>> = Arc::new(Mutex::new(PendingMap::default()));
        let next_id = Arc::new(AtomicU64::new(1));
        let (notifications_tx, _) = broadcast::channel(NOTIFICATION_CHANNEL_CAPACITY);
        let (incoming_tx, incoming_rx) = mpsc::channel(INCOMING_REQUEST_CHANNEL_CAPACITY);
        let alive = Arc::new(AtomicBool::new(true));
        let exit_info: Arc<Mutex<Option<ExitInfo>>> = Arc::new(Mutex::new(None));
        let stderr_tail = Arc::new(Mutex::new(StderrTail::default()));
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        // Stderr drain task: accumulates the tail buffer and finishes on EOF.
        {
            let stderr_tail = Arc::clone(&stderr_tail);
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                let mut stderr = stderr;
                loop {
                    match stderr.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => {
                            if let Ok(mut tail) = stderr_tail.lock() {
                                tail.push(&buf[..n]);
                            }
                        }
                        Err(_) => break,
                    }
                }
            });
        }

        // Reader task: parses stdout lines and routes them.
        {
            let pending_reader = Arc::clone(&pending);
            let notifications_tx_reader = notifications_tx.clone();
            let incoming_tx_reader = incoming_tx.clone();
            let alive_reader = Arc::clone(&alive);
            tokio::spawn(async move {
                let mut reader = BufReader::new(stdout);
                let mut line = String::new();
                loop {
                    line.clear();
                    match reader.read_line(&mut line).await {
                        Ok(0) => break, // EOF
                        Ok(_) => {
                            let trimmed = line.trim_end_matches(&['\r', '\n'][..]);
                            if trimmed.is_empty() {
                                continue;
                            }
                            route_incoming_line(
                                trimmed,
                                &pending_reader,
                                &notifications_tx_reader,
                                &incoming_tx_reader,
                            )
                            .await;
                        }
                        Err(_) => break,
                    }
                }
                alive_reader.store(false, Ordering::SeqCst);
                // Dropping `incoming_tx_reader` (the only remaining sender
                // besides the watchdog-held clone) will eventually close the
                // channel once watchdog drops its clone too.
            });
        }

        // Watchdog task: owns the Child, waits for either a voluntary exit or
        // a shutdown request, then populates exit_info and wakes pending
        // waiters.
        {
            let pending_watchdog = Arc::clone(&pending);
            let alive_watchdog = Arc::clone(&alive);
            let exit_info_watchdog = Arc::clone(&exit_info);
            let stderr_tail_watchdog = Arc::clone(&stderr_tail);
            // We intentionally keep the incoming sender inside the watchdog
            // so that the incoming-request channel does not close until the
            // child has truly gone away.
            let _incoming_keepalive = incoming_tx;
            tokio::spawn(async move {
                let exit_status = tokio::select! {
                    status = child.wait() => status,
                    _ = shutdown_rx => {
                        match tokio::time::timeout(GRACEFUL_SHUTDOWN_TIMEOUT, child.wait()).await {
                            Ok(s) => s,
                            Err(_) => {
                                let _ = child.kill().await;
                                child.wait().await
                            }
                        }
                    }
                };

                alive_watchdog.store(false, Ordering::SeqCst);

                let code = exit_status.ok().and_then(|s| s.code());
                let tail_snapshot = stderr_tail_watchdog
                    .lock()
                    .map(|t| t.snapshot())
                    .unwrap_or_default();
                if let Ok(mut slot) = exit_info_watchdog.lock() {
                    *slot = Some(ExitInfo {
                        code,
                        stderr_tail: tail_snapshot.clone(),
                    });
                }

                // Fail every outstanding request so callers unblock.
                let pending_list = pending_watchdog
                    .lock()
                    .map(|mut m| m.drain())
                    .unwrap_or_default();
                for tx in pending_list {
                    let _ = tx.send(Err(RpcError {
                        code: -32000,
                        message: format!(
                            "child process exited (code={:?}); stderr: {}",
                            code, tail_snapshot
                        ),
                        data: None,
                    }));
                }
                // Dropping `_incoming_keepalive` now closes the
                // incoming-request channel.
            });
        }

        Ok(Self {
            writer,
            pending,
            next_id,
            default_timeout: config.default_timeout,
            notifications_tx,
            incoming_rx: Arc::new(Mutex::new(Some(incoming_rx))),
            alive,
            exit_info,
            shutdown_tx: Arc::new(Mutex::new(Some(shutdown_tx))),
        })
    }

    /// Send a request, awaiting a response bounded by the handle's default
    /// timeout.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, RpcChildError> {
        self.request_with_timeout(method, params, self.default_timeout)
            .await
    }

    /// Send a request with an explicit timeout override.
    pub async fn request_with_timeout(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, RpcChildError> {
        if !self.alive.load(Ordering::SeqCst) {
            return Err(self.exit_or_shutdown());
        }

        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel::<PendingResult>();
        {
            let mut map = self
                .pending
                .lock()
                .map_err(|_| RpcChildError::ProtocolError("pending map poisoned".into()))?;
            map.insert(id, tx);
        }

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        if let Err(err) = self.write_line(&request).await {
            // Roll back the pending entry so it does not leak.
            if let Ok(mut map) = self.pending.lock() {
                map.remove(id);
            }
            return Err(err);
        }

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(Ok(result))) => Ok(result),
            Ok(Ok(Err(err))) => Err(RpcChildError::RpcError(err)),
            Ok(Err(_canceled)) => Err(self.exit_or_shutdown()),
            Err(_elapsed) => {
                if let Ok(mut map) = self.pending.lock() {
                    map.remove(id);
                }
                Err(RpcChildError::Timeout {
                    method: method.to_string(),
                    elapsed: timeout,
                })
            }
        }
    }

    /// Send an outgoing notification (no response expected).
    pub async fn notify(&self, method: &str, params: Value) -> Result<(), RpcChildError> {
        if !self.alive.load(Ordering::SeqCst) {
            return Err(self.exit_or_shutdown());
        }
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });
        self.write_line(&notification).await
    }

    /// Take ownership of the single receiver that yields server-initiated
    /// requests. Only the first call returns `Some`; later calls get `None`
    /// so the receiver is not aliased.
    pub fn incoming_requests(&self) -> Option<mpsc::Receiver<IncomingRequest>> {
        self.incoming_rx
            .lock()
            .ok()
            .and_then(|mut slot| slot.take())
    }

    /// Answer a server-initiated request, mirroring JSON-RPC 2.0's response
    /// shape: either `result` or `error` is populated. The caller provides
    /// the original request id verbatim.
    pub async fn respond(
        &self,
        incoming_id: Value,
        result: Result<Value, RpcError>,
    ) -> Result<(), RpcChildError> {
        if !self.alive.load(Ordering::SeqCst) {
            return Err(self.exit_or_shutdown());
        }
        let response = match result {
            Ok(value) => serde_json::json!({
                "jsonrpc": "2.0",
                "id": incoming_id,
                "result": value,
            }),
            Err(err) => serde_json::json!({
                "jsonrpc": "2.0",
                "id": incoming_id,
                "error": err,
            }),
        };
        self.write_line(&response).await
    }

    /// Subscribe to the broadcast stream of incoming notifications.
    pub fn notifications(&self) -> broadcast::Receiver<Notification> {
        self.notifications_tx.subscribe()
    }

    /// Close stdin, wait up to 2s for the child to exit on its own, and
    /// then kill it if it has not.
    pub async fn shutdown(self) -> Result<(), RpcChildError> {
        // Close stdin so the child observes EOF.
        {
            let mut guard = self.writer.lock().await;
            *guard = None;
        }

        // Trigger the watchdog timer if it has not fired already.
        let tx = self
            .shutdown_tx
            .lock()
            .ok()
            .and_then(|mut slot| slot.take());
        if let Some(tx) = tx {
            let _ = tx.send(());
        }

        // Poll alive for a short while to give the watchdog a chance to
        // actually reap the process; this keeps the caller's ordering
        // predictable.
        let deadline = std::time::Instant::now() + GRACEFUL_SHUTDOWN_TIMEOUT + Duration::from_secs(1);
        while self.alive.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        Ok(())
    }

    /// Whether the child process is still running (best-effort).
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    async fn write_line(&self, value: &Value) -> Result<(), RpcChildError> {
        let mut guard = self.writer.lock().await;
        let writer = guard.as_mut().ok_or(RpcChildError::AlreadyShutdown)?;
        let mut line = serde_json::to_vec(value).map_err(|err| {
            RpcChildError::ProtocolError(format!("failed to encode outgoing message: {err}"))
        })?;
        line.push(b'\n');
        writer.write_all(&line).await.map_err(RpcChildError::IoError)?;
        writer.flush().await.map_err(RpcChildError::IoError)?;
        Ok(())
    }

    /// Pick the most informative error given that [`alive`] has flipped
    /// false: either a real `ChildExited` with diagnostics, or the generic
    /// `AlreadyShutdown` if exit info is not yet populated.
    fn exit_or_shutdown(&self) -> RpcChildError {
        if let Ok(slot) = self.exit_info.lock() {
            if let Some(info) = slot.clone() {
                return RpcChildError::ChildExited {
                    code: info.code,
                    stderr_tail: info.stderr_tail,
                };
            }
        }
        RpcChildError::AlreadyShutdown
    }
}

/// Parse and dispatch a single line of stdout.
async fn route_incoming_line(
    line: &str,
    pending: &Arc<Mutex<PendingMap>>,
    notifications_tx: &broadcast::Sender<Notification>,
    incoming_tx: &mpsc::Sender<IncomingRequest>,
) {
    let value: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(err) => {
            // Emit a best-effort log line; do not crash the reader. The
            // protocol_error variant is used both for malformed incoming
            // lines and for local encoding failures.
            eprintln!(
                "[json_rpc_child] dropping malformed line: {err}; line={:?}",
                line
            );
            return;
        }
    };

    // Only object-shaped messages are part of JSON-RPC; ignore anything else.
    let obj = match value.as_object() {
        Some(o) => o.clone(),
        None => {
            eprintln!(
                "[json_rpc_child] dropping non-object JSON: {}",
                value
            );
            return;
        }
    };

    let id = obj.get("id").cloned();
    let method = obj.get("method").and_then(|v| v.as_str()).map(String::from);

    match (id, method) {
        // Response: has id + (result | error), no method.
        (Some(id_val), None) => {
            let id_u64 = match id_val.as_u64() {
                Some(n) => n,
                None => {
                    eprintln!(
                        "[json_rpc_child] ignoring response with non-u64 id: {}",
                        id_val
                    );
                    return;
                }
            };
            let sender = pending.lock().ok().and_then(|mut m| m.remove(id_u64));
            let Some(tx) = sender else {
                eprintln!(
                    "[json_rpc_child] response for unknown id {id_u64} (possibly timed out)"
                );
                return;
            };
            let outcome: PendingResult = if let Some(err_val) = obj.get("error") {
                match serde_json::from_value::<RpcError>(err_val.clone()) {
                    Ok(err) => Err(err),
                    Err(_) => Err(RpcError {
                        code: -32000,
                        message: format!("unparseable error payload: {err_val}"),
                        data: None,
                    }),
                }
            } else {
                Ok(obj.get("result").cloned().unwrap_or(Value::Null))
            };
            let _ = tx.send(outcome);
        }
        // Server-initiated request: has both id and method.
        (Some(id_val), Some(method_name)) => {
            let params = obj.get("params").cloned().unwrap_or(Value::Null);
            let req = IncomingRequest {
                id: id_val,
                method: method_name,
                params,
            };
            // Best-effort: if the consumer has not asked for incoming
            // requests yet, we drop rather than block.
            if let Err(err) = incoming_tx.try_send(req) {
                match err {
                    mpsc::error::TrySendError::Full(_) => {
                        eprintln!("[json_rpc_child] incoming request queue full; dropping");
                    }
                    mpsc::error::TrySendError::Closed(_) => {
                        // No consumer attached — silently drop.
                    }
                }
            }
        }
        // Notification: has method but no id.
        (None, Some(method_name)) => {
            let params = obj.get("params").cloned().unwrap_or(Value::Null);
            let _ = notifications_tx.send(Notification {
                method: method_name,
                params,
            });
        }
        // No method and no id — unusable.
        (None, None) => {
            eprintln!("[json_rpc_child] dropping message with neither id nor method");
        }
    }
}

impl std::fmt::Debug for JsonRpcChild {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsonRpcChild")
            .field("alive", &self.alive.load(Ordering::SeqCst))
            .field("default_timeout", &self.default_timeout)
            .finish()
    }
}
