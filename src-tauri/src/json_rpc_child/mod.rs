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
//!
//! # Exit-drain guarantee
//!
//! **Messages the child successfully wrote before dying are always routed;
//! pending requests are failed only after the stdout pipe is fully
//! drained.** A child's final response/notification can still be sitting in
//! the kernel pipe buffer when `child.wait()` returns — the watchdog used to
//! fail all pending requests the instant the child died, which could drop a
//! response whose bytes had already been written (e.g. a sidecar that emits
//! its final turn result and then exits). The watchdog now waits for the
//! reader task to reach EOF (bounded by [`PIPE_DRAIN_TIMEOUT`], protecting
//! against a grandchild inheriting the stdout fd and holding the pipe open)
//! before failing whatever is genuinely still outstanding. This applies to
//! both the voluntary-exit and graceful-shutdown paths.

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
/// How long the watchdog waits for the reader task to drain stdout to EOF
/// after the child exits, before failing outstanding requests (see the
/// module-level "Exit-drain guarantee"). Once the child is dead EOF arrives
/// promptly — the bound only matters when a grandchild process inherited
/// the stdout fd and is holding the pipe open.
const PIPE_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);
/// Depth of the incoming-request mpsc queue. Higher values absorb bursty
/// providers at the cost of more memory; the value is a conservative ceiling
/// well above what any current CLI uses.
const INCOMING_REQUEST_CHANNEL_CAPACITY: usize = 256;
/// Depth of the notifications broadcast channel. Lagging subscribers
/// observe a `Lagged` error rather than blocking the reader task.
const NOTIFICATION_CHANNEL_CAPACITY: usize = 512;
/// Shared by the managed sidecar readers. Includes JSON escaping, envelope and LF.
const MANAGED_MAX_FRAME_BYTES: usize = 2 * 1024 * 1024;

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
    /// Command::spawn failed: no executable started. Post-spawn setup failures
    /// use a different variant and must not be classified as known-zero.
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

/// Removes a request from the routing table when its awaiting future is
/// dropped. Most calls live until a response or timeout, but vendor ACP
/// protocols can provide a notification-based completion fallback (xAI's
/// prompt-complete extension does); abandoning the standard response future
/// must not leak its sender for the lifetime of the subprocess.
struct PendingRequestGuard {
    pending: Arc<Mutex<PendingMap>>,
    id: u64,
}

impl Drop for PendingRequestGuard {
    fn drop(&mut self) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(self.id);
        }
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
/// Cheaply shareable — all internal state is `Arc`-guarded — so adapters can
/// wrap the handle in an `Arc` and hand it to background tasks freely.
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
    /// Set to `true` the first time [`shutdown`](Self::shutdown) runs. A
    /// second call observes `true` via `swap` and returns early; the
    /// original cleanup has already fired.
    shutdown_started: Arc<AtomicBool>,
    /// Only workflow-owned children get their own process group. Regular
    /// chat sessions retain their existing lifecycle.
    managed_group: Option<u32>,
}

/// Created before spawning the watchdog, so even an unpolled task owns group
/// cleanup. Drop runs before the inner Child can reap/release its leader PID.
struct OwnedRpcProcess {
    inner: tokio::process::Child,
    managed_group: Option<u32>,
}

impl Drop for OwnedRpcProcess {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        if let Some(group) = self.managed_group {
            if self.inner.id() == Some(group) {
                let _ = signal_owned_managed_group(group);
            }
        }
    }
}

/// Cancelling startup must stop its owned child even if a callback/task still
/// holds an Arc. Successful insertion explicitly transfers lifecycle ownership.
pub(crate) struct ManagedStartupGuard {
    child: Arc<JsonRpcChild>,
    task_shutdown: Option<broadcast::Sender<()>>,
    armed: bool,
}

impl ManagedStartupGuard {
    pub(crate) fn set_task_shutdown(&mut self, sender: broadcast::Sender<()>) {
        self.task_shutdown = Some(sender);
    }
    pub(crate) fn disarm(&mut self) { self.armed = false; }
}

impl Drop for ManagedStartupGuard {
    fn drop(&mut self) {
        if self.armed {
            if let Some(sender) = &self.task_shutdown { let _ = sender.send(()); }
            self.child.request_managed_shutdown();
        }
    }
}

impl JsonRpcChild {
    pub(crate) fn managed_startup_guard(self: &Arc<Self>) -> Option<ManagedStartupGuard> {
        self.managed_group.map(|_| ManagedStartupGuard {
            child: Arc::clone(self), task_shutdown: None, armed: true,
        })
    }

    pub(crate) fn request_managed_shutdown(&self) {
        if self.managed_group.is_some() {
            if let Some(sender) = self.shutdown_tx.lock().ok().and_then(|mut slot| slot.take()) {
                let _ = sender.send(());
            }
        }
    }
    /// Spawn a child process and return a handle attached to its stdio.
    ///
    /// Fails with [`RpcChildError::SpawnFailed`] if the executable cannot start.
    /// An unexpected pipe-capture failure after spawn is a protocol error.
    pub async fn spawn(config: SpawnConfig) -> Result<Self, RpcChildError> {
        Self::spawn_internal(config, false).await
    }

    pub async fn spawn_managed(config: SpawnConfig) -> Result<Self, RpcChildError> {
        Self::spawn_internal(config, true).await
    }

    async fn spawn_internal(config: SpawnConfig, managed: bool) -> Result<Self, RpcChildError> {
        let mut cmd = Command::new(&config.program);
        cmd.args(&config.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        if managed {
            cmd.process_group(0);
        }
        #[cfg(not(target_os = "linux"))]
        if managed {
            return Err(RpcChildError::ProtocolError(
                "managed process containment is unavailable on this platform".into(),
            ));
        }

        // Agent CLIs are ordinary host binaries: under an AppImage they would
        // otherwise inherit AppRun's LD_LIBRARY_PATH and link against our
        // bundled libraries. Applied before the caller's overlay so an explicit
        // env pair from `config` still wins. No-op outside an AppImage.
        crate::execution::sanitize_appimage_env_tokio(&mut cmd);

        for (k, v) in &config.env {
            cmd.env(k, v);
        }
        if let Some(cwd) = &config.cwd {
            cmd.current_dir(cwd);
        }

        let child = cmd.spawn().map_err(RpcChildError::SpawnFailed)?;
        let managed_group = if managed { child.id() } else { None };
        let mut child = OwnedRpcProcess { inner: child, managed_group };
        let stdin = child.inner.stdin.take().ok_or_else(|| {
            RpcChildError::ProtocolError("spawned child stdin pipe not captured".into())
        })?;
        let stdout = child.inner.stdout.take().ok_or_else(|| {
            RpcChildError::ProtocolError("spawned child stdout pipe not captured".into())
        })?;
        let stderr = child.inner.stderr.take().ok_or_else(|| {
            RpcChildError::ProtocolError("spawned child stderr pipe not captured".into())
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

        // Reader task: parses stdout lines and routes them. The JoinHandle
        // is handed to the watchdog so it can await EOF before failing
        // pending requests (the module-level "Exit-drain guarantee").
        let reader_handle = {
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
            })
        };

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
                let exit_status = {
                    #[cfg(target_os = "linux")]
                    if let Some(group) = managed_group {
                        wait_managed_child(&mut child, group, shutdown_rx).await
                    } else {
                        tokio::select! {
                            status = child.inner.wait() => status,
                            _ = shutdown_rx => {
                                match tokio::time::timeout(GRACEFUL_SHUTDOWN_TIMEOUT, child.inner.wait()).await {
                                    Ok(s) => s,
                                    Err(_) => {
                                        let _ = child.inner.kill().await;
                                        child.inner.wait().await
                                    }
                                }
                            }
                        }
                    }
                    #[cfg(not(target_os = "linux"))]
                    {
                        tokio::select! {
                            status = child.inner.wait() => status,
                            _ = shutdown_rx => {
                                match tokio::time::timeout(GRACEFUL_SHUTDOWN_TIMEOUT, child.inner.wait()).await {
                                    Ok(s) => s,
                                    Err(_) => {
                                        let _ = child.inner.kill().await;
                                        child.inner.wait().await
                                    }
                                }
                            }
                        }
                    }
                };

                alive_watchdog.store(false, Ordering::SeqCst);

                let code = exit_status.ok().and_then(|s| s.code());
                // Publish exit info immediately so requests arriving from
                // here on fail with the informative `ChildExited` rather
                // than `AlreadyShutdown` while the drain below runs.
                {
                    let tail_snapshot = stderr_tail_watchdog
                        .lock()
                        .map(|t| t.snapshot())
                        .unwrap_or_default();
                    if let Ok(mut slot) = exit_info_watchdog.lock() {
                        *slot = Some(ExitInfo {
                            code,
                            stderr_tail: tail_snapshot,
                        });
                    }
                }

                // Exit-drain guarantee (see module docs): a response the
                // child wrote just before exiting can still be sitting in
                // the kernel pipe buffer. Failing pending requests NOW would
                // drop it — the caller would see "child process exited" for
                // a request the child actually answered (and the reader
                // would later log the orphaned response as an unknown id).
                // Wait for the reader to reach EOF so every buffered message
                // is routed first; bounded because a grandchild inheriting
                // the stdout fd can hold the pipe open indefinitely.
                let _ = tokio::time::timeout(PIPE_DRAIN_TIMEOUT, reader_handle).await;

                // Refresh the stderr tail: the drain window may have let the
                // stderr task capture the child's final diagnostics too.
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

                // Fail every request that is GENUINELY still outstanding —
                // anything the child answered has already been resolved and
                // removed from the map by the reader during the drain.
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
            shutdown_started: Arc::new(AtomicBool::new(false)),
            managed_group,
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
        let _pending_guard = PendingRequestGuard {
            pending: Arc::clone(&self.pending),
            id,
        };

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
        let response = if self.managed_group.is_some() {
            bound_managed_response(response)?
        } else {
            response
        };
        self.write_line(&response).await
    }

    /// Subscribe to the broadcast stream of incoming notifications.
    pub fn notifications(&self) -> broadcast::Receiver<Notification> {
        self.notifications_tx.subscribe()
    }

    /// Close stdin, wait up to 2s for the child to exit on its own, and
    /// then kill it if it has not.
    ///
    /// Idempotent: a second call after the first one started observes the
    /// internal `shutdown_started` flag and returns `Ok(())` immediately
    /// without re-running the shutdown sequence. This lets `Arc<Self>`
    /// holders coordinate cleanup without needing to decide who "owns"
    /// the single shutdown call.
    pub async fn shutdown(&self) -> Result<(), RpcChildError> {
        if self.shutdown_started.swap(true, Ordering::SeqCst) {
            // Someone else already kicked off shutdown. Nothing to do.
            return Ok(());
        }

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
        let deadline =
            std::time::Instant::now() + GRACEFUL_SHUTDOWN_TIMEOUT + Duration::from_secs(1);
        while self.alive.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        Ok(())
    }

    /// Whether the child process is still running (best-effort).
    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// Verify the owned process group is gone. A stop RPC or a closed
    /// session map is insufficient evidence for releasing workflow leases.
    pub async fn shutdown_managed(&self) -> Result<(), RpcChildError> {
        let Some(group) = self.managed_group else {
            return Err(RpcChildError::ProtocolError(
                "child was not spawned as a managed attempt".into(),
            ));
        };
        #[cfg(unix)]
        {
            // The watchdog signals the group before reaping its owned leader,
            // on both voluntary exit and shutdown. Never signal a numeric PGID
            // here: the leader may already be reaped and its PID reused.
            // Request stop before waiting for the writer lock: a sidecar that
            // stopped reading can leave a write blocked on pipe backpressure.
            self.request_managed_shutdown();
            self.shutdown().await?;
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            loop {
                let exists = unsafe { libc::kill(-(group as i32), 0) } == 0
                    || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH);
                if !self.is_alive() && !exists {
                    return Ok(());
                }
                if std::time::Instant::now() >= deadline {
                    return Err(RpcChildError::ProtocolError(
                        "managed process group quiescence is unconfirmed".into(),
                    ));
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
        #[cfg(not(unix))]
        {
            let _ = group;
            Err(RpcChildError::ProtocolError(
                "managed containment unavailable".into(),
            ))
        }
    }

    pub fn managed_evidence(&self) -> Result<Value, RpcChildError> {
        let group = self.managed_group.ok_or_else(|| {
            RpcChildError::ProtocolError("unmanaged child has no workflow evidence".into())
        })?;
        #[cfg(target_os = "linux")]
        {
            fn start(pid: u32) -> Result<u64, RpcChildError> {
                let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))
                    .map_err(RpcChildError::IoError)?;
                stat.rsplit_once(") ")
                    .and_then(|(_, tail)| tail.split_whitespace().nth(19))
                    .and_then(|v| v.parse().ok())
                    .ok_or_else(|| {
                        RpcChildError::ProtocolError("process start identity is unavailable".into())
                    })
            }
            let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
                .map_err(RpcChildError::IoError)?;
            Ok(
                serde_json::json!({"kind":"linux_process_group","pid":group,"start_time_ticks":start(group)?,"boot_id":boot_id.trim(),
                "host_pid":std::process::id(),"host_start_time_ticks":start(std::process::id())?}),
            )
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = group;
            Err(RpcChildError::ProtocolError(
                "durable managed process identity is unavailable on this platform".into(),
            ))
        }
    }

    async fn write_line(&self, value: &Value) -> Result<(), RpcChildError> {
        let mut guard = self.writer.lock().await;
        let writer = guard.as_mut().ok_or(RpcChildError::AlreadyShutdown)?;
        let line = encode_frame(value, self.managed_group.is_some())?;
        writer
            .write_all(&line)
            .await
            .map_err(RpcChildError::IoError)?;
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

fn encode_frame(value: &Value, managed: bool) -> Result<Vec<u8>, RpcChildError> {
    let mut line = serde_json::to_vec(value).map_err(|err| {
        RpcChildError::ProtocolError(format!("failed to encode outgoing message: {err}"))
    })?;
    line.push(b'\n');
    if managed && line.len() > MANAGED_MAX_FRAME_BYTES {
        return Err(RpcChildError::ProtocolError(
            "managed RPC frame exceeds 2 MiB wire limit".into(),
        ));
    }
    Ok(line)
}

fn bound_managed_response(response: Value) -> Result<Value, RpcChildError> {
    if encode_frame(&response, false)?.len() <= MANAGED_MAX_FRAME_BYTES {
        return Ok(response);
    }
    let error = serde_json::json!({"jsonrpc":"2.0","id":response.get("id"),
        "error":{"code":-32000,"message":"Managed tool response exceeds 2 MiB wire limit; reduce response size or split requests"}});
    // A hostile oversized request ID must not turn the bounded error into
    // another oversized frame.
    encode_frame(&error, true)?;
    Ok(error)
}

#[cfg(target_os = "linux")]
fn managed_leader_exited(pid: u32) -> std::io::Result<bool> {
    loop {
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let result = unsafe {
            libc::waitid(libc::P_PID, pid as libc::id_t, &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT)
        };
        if result == 0 {
            return Ok(unsafe { info.si_pid() } == pid as i32);
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

#[cfg(target_os = "linux")]
async fn wait_managed_child(
    child: &mut OwnedRpcProcess,
    group: u32,
    shutdown: oneshot::Receiver<()>,
) -> std::io::Result<std::process::ExitStatus> {
    let exited = async {
        loop {
            if managed_leader_exited(group)? { return Ok::<_, std::io::Error>(()); }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    };
    tokio::select! {
        result = exited => result?,
        _ = shutdown => { managed_leader_exited(group)?; }
    }
    // WNOWAIT keeps the owned leader (including a zombie) unreaped. Its PID
    // cannot be reused until this sole owner calls child.wait() below, so the
    // negative-PGID signal cannot target a recycled, unrelated process group.
    signal_owned_managed_group(group)?;
    let status = child.inner.wait().await;
    if status.is_ok() { child.managed_group = None; }
    status
}

#[cfg(target_os = "linux")]
fn signal_owned_managed_group(group: u32) -> std::io::Result<()> {
    managed_leader_exited(group)?;
    let result = unsafe { libc::kill(-(group as i32), libc::SIGKILL) };
    if result != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) { return Err(error); }
    }
    Ok(())
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
            eprintln!("[json_rpc_child] dropping non-object JSON: {}", value);
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
                eprintln!("[json_rpc_child] response for unknown id {id_u64} (possibly timed out)");
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

#[cfg(all(test, target_os = "linux"))]
mod managed_tests {
    use super::*;

    fn fake_config() -> SpawnConfig {
        SpawnConfig {
            program: PathBuf::from("/bin/cat"),
            args: Vec::new(),
            env: HashMap::new(),
            cwd: None,
            default_timeout: Duration::from_secs(1),
        }
    }

    #[tokio::test]
    async fn managed_child_group_shutdown_is_verified() {
        let child = JsonRpcChild::spawn_managed(fake_config()).await.unwrap();
        let evidence = child.managed_evidence().unwrap();
        let group = evidence["pid"].as_u64().unwrap() as i32;
        assert_ne!(group, unsafe { libc::getpgrp() });
        assert_eq!(unsafe { libc::getpgid(group) }, group);
        assert!(evidence["start_time_ticks"].as_u64().is_some());
        assert_eq!(
            evidence["host_pid"].as_u64(),
            Some(std::process::id().into())
        );
        child.shutdown_managed().await.unwrap();
        assert!(!child.is_alive());
        assert_eq!(unsafe { libc::kill(-group, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );

        let ordinary = JsonRpcChild::spawn(fake_config()).await.unwrap();
        assert!(ordinary.managed_evidence().is_err());
        assert!(ordinary.shutdown_managed().await.is_err());
        assert!(ordinary.is_alive());
        ordinary.shutdown().await.unwrap();
    }

    #[test]
    fn managed_frames_bound_escaped_content_and_full_envelopes() {
        for content in ["\0".repeat(1024 * 1024), "x".repeat(MANAGED_MAX_FRAME_BYTES - 1)] {
            let response = bound_managed_response(serde_json::json!({"jsonrpc":"2.0","id":"tool","result":{"content":content}})).unwrap();
            assert!(response.get("error").is_some());
            assert!(encode_frame(&response, true).unwrap().len() <= MANAGED_MAX_FRAME_BYTES);
        }
        let response = bound_managed_response(serde_json::json!({"jsonrpc":"2.0","id":"tool","result":{"content":"x".repeat(MANAGED_MAX_FRAME_BYTES - 1024)}})).unwrap();
        assert!(response.get("result").is_some());
        assert!(encode_frame(&response, true).unwrap().len() <= MANAGED_MAX_FRAME_BYTES);
        assert!(encode_frame(&serde_json::json!({"text":"\0".repeat(1024 * 1024)}), true).is_err());
        assert!(managed_leader_exited(std::process::id()).is_err(), "an unrelated PID is not waitable ownership");
    }

    #[tokio::test]
    async fn managed_oversized_tool_reply_is_recoverable_on_same_transport() {
        let config = SpawnConfig { program: "python3".into(), args: vec!["-u".into(), "-c".into(), r#"
import json,sys
for line in sys.stdin:
 request=json.loads(line)
 for name in ['escaped','near-limit','within-limit']:
  print(json.dumps({'jsonrpc':'2.0','id':name,'method':'tool','params':{}}),flush=True)
  response=json.loads(sys.stdin.readline())
  assert ('error' in response) == (name != 'within-limit')
  if name=='within-limit':assert response['result']['content'].startswith('x')
 print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'usable':True}}),flush=True)
"#.into()], env: HashMap::new(), cwd: None, default_timeout: Duration::from_secs(5) };
        let child = Arc::new(JsonRpcChild::spawn_managed(config).await.unwrap());
        let mut incoming = child.incoming_requests().unwrap();
        let request_child = Arc::clone(&child);
        let request = tokio::spawn(async move { request_child.request("exercise", Value::Null).await });
        for content in ["\0".repeat(1024 * 1024), "x".repeat(MANAGED_MAX_FRAME_BYTES - 1), "x".repeat(MANAGED_MAX_FRAME_BYTES - 1024)] {
            let call = incoming.recv().await.unwrap();
            child.respond(call.id, Ok(serde_json::json!({"content":content}))).await.unwrap();
        }
        assert_eq!(request.await.unwrap().unwrap(), serde_json::json!({"usable":true}));
        assert!(child.is_alive());
        child.shutdown_managed().await.unwrap();
    }

    #[tokio::test]
    async fn managed_shutdown_stops_child_with_blocked_pipe_writer() {
        let child = Arc::new(JsonRpcChild::spawn_managed(SpawnConfig {
            program: "python3".into(), args: vec!["-c".into(), "import time;time.sleep(600)".into()],
            env: HashMap::new(), cwd: None, default_timeout: Duration::from_secs(5),
        }).await.unwrap());
        let group = child.managed_group.unwrap() as i32;
        let mut cleanup = child.managed_startup_guard().unwrap();
        let writer_child = Arc::clone(&child);
        let writer = tokio::spawn(async move {
            writer_child.notify("backpressure", serde_json::json!({"content":"x".repeat(1024 * 1024)})).await
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while child.writer.try_lock().is_ok() {
            assert!(!writer.is_finished(), "fixture write completed without pipe backpressure");
            assert!(std::time::Instant::now() < deadline, "fixture did not acquire the writer");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(!writer.is_finished());
        tokio::time::timeout(Duration::from_secs(3), child.shutdown_managed()).await
            .expect("managed shutdown waited behind the blocked writer").unwrap();
        assert!(writer.await.unwrap().is_err());
        assert_eq!(unsafe { libc::kill(-group, 0) }, -1);
        assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
        cleanup.disarm();
    }

    #[tokio::test]
    async fn managed_cancelled_startup_stops_child_despite_retained_arc() {
        let child = Arc::new(JsonRpcChild::spawn_managed(fake_config()).await.unwrap());
        let retained = Arc::clone(&child);
        let group = child.managed_group.unwrap() as i32;
        drop(child.managed_startup_guard().unwrap());
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while unsafe { libc::kill(-group, 0) } == 0 {
            assert!(std::time::Instant::now() < deadline, "cancelled startup retained its group");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
        retained.shutdown_managed().await.unwrap();
    }

    async fn run_under_isolated_subreaper(name: &str) -> bool {
        if std::env::var("CODEMUX_MANAGED_ORPHAN_TEST_CHILD").ok().as_deref() != Some(name) {
            // An isolated subreaper owns fixture orphans. Never change the
            // application's/global test process reaping policy or leave zombies.
            let script = r#"
import ctypes,os,signal,subprocess,sys,time
assert ctypes.CDLL(None,use_errno=True).prctl(36,1,0,0,0)==0
child=subprocess.Popen([sys.argv[1],'--exact',sys.argv[2],'--nocapture'],env={**os.environ,'CODEMUX_MANAGED_ORPHAN_TEST_CHILD':sys.argv[2]})
result=1;deadline=time.monotonic()+15
try:
 while time.monotonic()<deadline:
  try:pid,status=os.waitpid(-1,os.WNOHANG)
  except ChildProcessError:break
  if pid==child.pid:result=os.waitstatus_to_exitcode(status);break
  time.sleep(.01)
finally:
 # These are direct unreaped children of this fixture's subreaper only.
 with open('/proc/self/task/%s/children'%os.getpid()) as source:owned=source.read().split()
 for pid in owned:
  try:os.kill(int(pid),signal.SIGKILL)
  except ProcessLookupError:pass
 while True:
  try:os.waitpid(-1,0)
  except ChildProcessError:break
sys.exit(result)
"#;
            let status = Command::new("python3").args(["-c", script]).arg(std::env::current_exe().unwrap()).arg(name).status().await.unwrap();
            assert!(status.success());
            return true;
        }
        false
    }

    #[tokio::test]
    async fn managed_unpolled_watchdog_drop_stops_leader_and_descendant() {
        if run_under_isolated_subreaper("json_rpc_child::managed_tests::managed_unpolled_watchdog_drop_stops_leader_and_descendant").await { return; }
        let script = r#"
import os,sys,time
descendant=os.fork()
if descendant==0:
 time.sleep(600)
 os._exit(0)
print(descendant,flush=True)
sys.stdin.readline()
"#;
        let child = Command::new("python3").args(["-u", "-c", script])
            .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped())
            .kill_on_drop(true).process_group(0).spawn().unwrap();
        let group = child.id().unwrap();
        let mut child = OwnedRpcProcess { inner:child,managed_group:Some(group) };
        let mut output = BufReader::new(child.inner.stdout.take().unwrap());
        let mut line = String::new(); output.read_line(&mut line).await.unwrap();
        let descendant: u32 = line.trim().parse().unwrap();
        let (_shutdown, shutdown_rx) = oneshot::channel();
        let unpolled = async move { wait_managed_child(&mut child, group, shutdown_rx).await };
        // The async body never runs. Its captured ownership wrapper must stop
        // the group before Child's drop can release the original PID.
        drop(unpolled);
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            // There is deliberately no watchdog left to await Child::wait().
            // Reap only this fixture's direct child, whose dropped Tokio handle
            // may otherwise leave it queued as a zombie until runtime teardown.
            let mut status = 0;
            let reaped = unsafe { libc::waitpid(group as i32, &mut status, libc::WNOHANG) };
            if reaped == group as i32 {
                assert!(libc::WIFSIGNALED(status));
                assert_eq!(libc::WTERMSIG(status), libc::SIGKILL);
            } else if reaped == -1 {
                assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::ECHILD));
            }
            if unsafe { libc::kill(-(group as i32), 0) } != 0 { break; }
            assert!(std::time::Instant::now() < deadline, "unpolled watchdog left an owned group");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
        assert!(!PathBuf::from(format!("/proc/{descendant}")).exists());
    }

    #[tokio::test]
    async fn managed_group_stops_descendants_after_leader_exit() {
        if run_under_isolated_subreaper("json_rpc_child::managed_tests::managed_group_stops_descendants_after_leader_exit").await { return; }
        let script = r#"
import json,os,sys,time
request=json.loads(sys.stdin.readline())
descendant=os.fork()
if descendant==0:
 time.sleep(600)
 os._exit(0)
print(json.dumps({'jsonrpc':'2.0','id':request['id'],'result':{'descendant':descendant}}),flush=True)
os._exit(0)
"#;
        let child = JsonRpcChild::spawn_managed(SpawnConfig { program:"python3".into(), args:vec!["-u".into(),"-c".into(),script.into()],env:HashMap::new(),cwd:None,default_timeout:Duration::from_secs(5) }).await.unwrap();
        let group = child.managed_group.unwrap() as i32;
        let response = child.request("fork", Value::Null).await.unwrap();
        child.shutdown_managed().await.unwrap();
        assert_eq!(unsafe { libc::kill(-group, 0) }, -1);
        assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
        assert!(!PathBuf::from(format!("/proc/{}", response["descendant"].as_u64().unwrap())).exists());
    }
}
