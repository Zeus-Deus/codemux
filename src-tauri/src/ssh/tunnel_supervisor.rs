//! Tunnel auto-reconnect supervisor.
//!
//! Wraps `TunnelHandle` with retry logic so a transient SSH failure
//! (WiFi flap, laptop sleep/wake, remote sshd restart) doesn't strand
//! a workspace's daemon. Matches the cadence superset-sh uses in
//! their `tunnel-client.ts`:
//!
//! - Exponential backoff from 1 s to 30 s
//! - Watchdog detects SSH death within ~2 s
//! - Crash circuit: 5 reconnect failures in 5 min opens the breaker.
//!   The supervisor then retries at a slow fixed interval so a longer
//!   outage (no network, host rebooting) heals on its own once the
//!   host is reachable again, without hammering it meanwhile.
//!
//! API: `TunnelSupervisor::spawn` returns a supervisor handle that
//! exposes the current local socket path (which stays stable across
//! reconnects — we always re-bind the same path locally) and a status
//! receiver for the UI to show "reconnecting…" indicators.

#![cfg(unix)]

use crate::ssh::tunnel::{build_tunnel_argv, TunnelOptions};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::sync::{watch, Mutex};

/// Observable status of a supervised tunnel. Pushed via a `watch`
/// channel so multiple UI surfaces (workspace header, status bar)
/// can read the same source of truth.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TunnelStatus {
    /// Initial state — supervisor hasn't connected yet.
    Pending,
    /// Tunnel is up and SSH process is alive.
    Connected { ssh_pid: u32 },
    /// SSH died; supervisor is waiting `delay_ms` before next attempt.
    Reconnecting { attempt: u32, delay_ms: u64 },
    /// Crash circuit tripped. The supervisor keeps retrying every
    /// `CIRCUIT_RETRY_INTERVAL` and returns to `Connected` on success.
    CircuitOpen { recent_failures: u32 },
}

/// Backoff schedule used between reconnects. Caps at 30s. The 5-min
/// window inside which `MAX_FAILURES` failures trip the breaker
/// matches the local pty-daemon circuit's policy.
const BACKOFF_FLOOR_MS: u64 = 1_000;
const BACKOFF_CEIL_MS: u64 = 30_000;
const MAX_FAILURES: u32 = 5;
const CIRCUIT_WINDOW: Duration = Duration::from_secs(300);
/// Retry cadence while the circuit is open: slow enough not to hammer
/// a dead host, fast enough to recover soon after the network returns.
const CIRCUIT_RETRY_INTERVAL: Duration = Duration::from_secs(60);
/// How long we wait for the tunnel socket to appear after spawning
/// SSH. The first connect can be slow (DNS, key handshake); later
/// reconnects are usually instant.
const SPAWN_TIMEOUT: Duration = Duration::from_secs(15);
const STDERR_DRAIN_TIMEOUT: Duration = Duration::from_millis(250);
const STDERR_BYTE_LIMIT: u64 = 16 * 1024;

/// Supervisor handle. Dropping it does NOT cancel the supervisor —
/// the tunnel keeps running until `shutdown` is called or the
/// supervisor's own task exits. Sharing the handle is fine.
pub struct TunnelSupervisor {
    inner: Arc<SupervisorInner>,
    /// Supervisor task, awaited by `shutdown` so an ssh that is still
    /// connecting is killed before shutdown returns.
    task: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

struct SupervisorInner {
    local_socket: PathBuf,
    status_tx: watch::Sender<TunnelStatus>,
    shutdown_tx: watch::Sender<bool>,
    /// Latest SSH process. Wrapped so `shutdown` can kill it under
    /// the lock without racing the supervisor's spawn loop.
    current_child: Mutex<Option<tokio::process::Child>>,
}

impl TunnelSupervisor {
    /// Start the supervisor. Returns immediately; the first connect
    /// attempt is asynchronous. Watch the status channel for state.
    pub fn spawn(
        ssh_target: String,
        remote_socket: String,
        local_socket: PathBuf,
        remote_binary: String,
    ) -> Arc<Self> {
        let (status_tx, _status_rx) = watch::channel(TunnelStatus::Pending);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let inner = Arc::new(SupervisorInner {
            local_socket: local_socket.clone(),
            status_tx,
            shutdown_tx,
            current_child: Mutex::new(None),
        });
        let task_inner = inner.clone();
        let mut cmd = Command::new("ssh");
        // The remote side ends in a long `exec sleep`, so an ssh that
        // outlives its owner would hold the tunnel and an sshd session
        // open indefinitely.
        cmd.kill_on_drop(true);
        let task = tokio::spawn(run_supervisor(
            task_inner,
            ssh_target,
            remote_socket,
            local_socket,
            remote_binary,
            shutdown_rx,
            cmd,
        ));
        Arc::new(Self {
            inner,
            task: std::sync::Mutex::new(Some(task)),
        })
    }

    /// Local socket path the `PtyDaemonClient` should dial. Stable
    /// across reconnects.
    pub fn local_socket(&self) -> &Path {
        &self.inner.local_socket
    }

    /// Subscribe to status changes. The first message is the current
    /// status. Drop the receiver to unsubscribe.
    pub fn subscribe(&self) -> watch::Receiver<TunnelStatus> {
        self.inner.status_tx.subscribe()
    }

    /// Stop the supervisor, kill the live SSH process, remove the
    /// local socket. Idempotent.
    pub async fn shutdown(&self) {
        let _ = self.inner.shutdown_tx.send(true);
        if let Some(mut child) = self.inner.current_child.lock().await.take() {
            let _ = child.kill().await;
        }
        // Wait for the task too: it owns any ssh still connecting.
        let task = self.task.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(task) = task {
            let _ = task.await;
        }
        let _ = std::fs::remove_file(&self.inner.local_socket);
    }
}

async fn run_supervisor(
    inner: Arc<SupervisorInner>,
    ssh_target: String,
    remote_socket: String,
    local_socket: PathBuf,
    remote_binary: String,
    mut shutdown_rx: watch::Receiver<bool>,
    mut cmd: Command,
) {
    eprintln!(
        "[tunnel-supervisor] start: ssh_target={ssh_target} \
         local_socket={local_socket:?} remote_socket={remote_socket} \
         remote_binary={remote_binary}"
    );
    let mut retry = RetryState::default();
    let opts = TunnelOptions {
        ssh_target: &ssh_target,
        remote_socket: &remote_socket,
        local_socket: &local_socket,
        remote_binary: &remote_binary,
    };
    let argv = build_tunnel_argv(&opts);
    cmd.args(&argv)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    loop {
        if *shutdown_rx.borrow() {
            return;
        }
        // IMPORTANT: use send_replace, not send. `send` no-ops the
        // update when receiver_count() == 0 — which is the common
        // case here because the supervisor publishes status before
        // any consumer has subscribed (the consumer subscribes
        // lazily from client_for_workspace). With plain `send`, a
        // Connected status published before the consumer subscribes
        // is silently dropped, so the consumer sees Pending forever.
        // While the circuit is open, keep showing CircuitOpen through
        // each slow retry so the UI doesn't flicker.
        if !retry.circuit_open {
            let _ = inner.status_tx.send_replace(TunnelStatus::Pending);
        }

        crate::trace_cloud_push!(
            "[tunnel-supervisor] attempt {} argv: ssh {}",
            retry.attempt + 1,
            argv.join(" ")
        );

        // A dead ssh can leave its -L socket file behind. Readiness is
        // "the socket exists", so clear it first or the next attempt
        // reports Connected before ssh has even authenticated.
        // (StreamLocalBindUnlink lets ssh rebind the path either way.)
        remove_stale_socket(&local_socket);
        let spawn_res = cmd.spawn();
        let mut child = match spawn_res {
            Ok(c) => {
                crate::trace_cloud_push!(
                    "[tunnel-supervisor] ssh spawned ok, pid={:?}",
                    c.id()
                );
                c
            }
            Err(error) => {
                eprintln!("[tunnel-supervisor] spawn failed: {error}");
                let (status, delay) = retry.on_failure();
                let _ = inner.status_tx.send_replace(status);
                // Wait the delay but bail early on shutdown.
                if !sleep_or_shutdown(delay, &mut shutdown_rx).await {
                    return;
                }
                continue;
            }
        };

        // Wait for the socket to appear (or SSH to die before it does).
        let socket_ready = tokio::select! {
            ready = wait_for_socket(&local_socket, &mut child, SPAWN_TIMEOUT) => Some(ready),
            _ = shutdown_rx.wait_for(|stop| *stop) => None,
        };
        let Some(socket_ready) = socket_ready else {
            let _ = child.kill().await;
            return;
        };
        match socket_ready {
            Ok(()) => {
                let ssh_pid = child.id().unwrap_or(0);
                *inner.current_child.lock().await = Some(child);
                let prev = inner
                    .status_tx
                    .send_replace(TunnelStatus::Connected { ssh_pid });
                eprintln!(
                    "[tunnel-supervisor] published Connected via send_replace \
                     (ssh_pid={ssh_pid}, receivers={}, prev={prev:?})",
                    inner.status_tx.receiver_count(),
                );
                retry.on_connected();

                // Watchdog: wait for SSH to exit or shutdown signal.
                let exited = watch_child_until_exit(&inner, &mut shutdown_rx).await;
                if !exited {
                    // Shutdown signalled. `shutdown` may have checked the
                    // slot before this child was stored, so kill it here.
                    if let Some(mut child) = inner.current_child.lock().await.take() {
                        let _ = child.kill().await;
                    }
                    return;
                }
                eprintln!("[tunnel-supervisor] ssh exited; will reconnect");
            }
            Err(reason) => {
                // A live SSH won't close stderr. Kill and reap it before
                // collecting diagnostics so a startup timeout can retry.
                if let Err(error) = child.kill().await {
                    eprintln!("[tunnel-supervisor] failed to stop ssh: {error}");
                }
                let stderr_dump = match child.stderr.take() {
                    Some(stderr) => read_stderr_bounded(stderr).await,
                    None => String::new(),
                };
                eprintln!(
                    "[tunnel-supervisor] tunnel did not come up: {reason}\n\
                     [tunnel-supervisor] ssh stderr: {}",
                    stderr_dump.trim()
                );
            }
        }

        let (status, delay) = retry.on_failure();
        let _ = inner.status_tx.send_replace(status);
        if !sleep_or_shutdown(delay, &mut shutdown_rx).await {
            return;
        }
    }
}

fn remove_stale_socket(local_socket: &Path) {
    match std::fs::remove_file(local_socket) {
        Ok(()) => crate::trace_cloud_push!(
            "[tunnel-supervisor] removed stale local socket {:?}",
            local_socket
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            eprintln!("[tunnel-supervisor] failed to remove stale socket {local_socket:?}: {error}")
        }
    }
}

async fn read_stderr_bounded(stderr: impl AsyncRead + Unpin) -> String {
    // Descendants may keep the pipe open after SSH exits. Keep partial
    // diagnostics even if the drain times out or contains invalid UTF-8.
    let mut bytes = Vec::new();
    let _ = tokio::time::timeout(
        STDERR_DRAIN_TIMEOUT,
        stderr.take(STDERR_BYTE_LIMIT).read_to_end(&mut bytes),
    )
    .await;
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Sleep for `dur` unless a shutdown signal fires. Returns `true` if
/// the sleep completed naturally, `false` if shutdown signalled.
async fn sleep_or_shutdown(
    dur: Duration,
    shutdown_rx: &mut watch::Receiver<bool>,
) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(dur) => true,
        _ = shutdown_rx.changed() => {
            !*shutdown_rx.borrow()
        }
    }
}

/// Poll for the local socket to appear OR for SSH to exit. Returns
/// `Ok(())` if the socket appears in time, `Err(reason)` otherwise.
async fn wait_for_socket(
    local_socket: &Path,
    child: &mut tokio::process::Child,
    deadline: Duration,
) -> Result<(), String> {
    let start = Instant::now();
    let mut last_log_at_secs: u64 = 0;
    crate::trace_cloud_push!(
        "[tunnel-supervisor] waiting for local socket {:?} (deadline {:?})",
        local_socket, deadline
    );
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!(
                "ssh exited before tunnel came up (status={status})"
            ));
        }
        if local_socket.exists() {
            // Tiny grace beat so the daemon's listener is fully up.
            tokio::time::sleep(Duration::from_millis(50)).await;
            crate::trace_cloud_push!(
                "[tunnel-supervisor] local socket appeared after {:?}",
                start.elapsed()
            );
            return Ok(());
        }
        let elapsed_secs = start.elapsed().as_secs();
        if elapsed_secs > last_log_at_secs {
            // Per-second progress so we know the loop is alive.
            crate::trace_cloud_push!(
                "[tunnel-supervisor] still waiting for socket (elapsed {}s, ssh alive)",
                elapsed_secs
            );
            last_log_at_secs = elapsed_secs;
        }
        if start.elapsed() >= deadline {
            return Err(format!(
                "socket {:?} did not appear within {:?}",
                local_socket, deadline
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Block until the supervised SSH child exits or shutdown signals.
/// Returns `true` if child exited (need to reconnect), `false` if
/// shutdown.
async fn watch_child_until_exit(
    inner: &Arc<SupervisorInner>,
    shutdown_rx: &mut watch::Receiver<bool>,
) -> bool {
    loop {
        // Periodically poll the child for exit. We can't await on it
        // directly because the Child is in the mutex; doing a try_wait
        // every 500 ms is the simplest correct pattern. The poll
        // frequency caps detection latency at ~half a second, which
        // matches what a human notices.
        let exited = {
            let mut guard = inner.current_child.lock().await;
            match guard.as_mut() {
                Some(child) => match child.try_wait() {
                    Ok(Some(_status)) => true,
                    Ok(None) => false,
                    Err(_) => true, // wait error → treat as exited
                },
                None => true,
            }
        };
        if exited {
            let mut guard = inner.current_child.lock().await;
            *guard = None;
            return true;
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(500)) => {}
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    return false;
                }
            }
        }
    }
}

/// Reconnect bookkeeping: failure timestamps form a sliding window;
/// `MAX_FAILURES` within `CIRCUIT_WINDOW` opens the circuit.
#[derive(Default)]
struct RetryState {
    failures: Vec<Instant>,
    attempt: u32,
    /// Sticky until a connect succeeds, so the slow retries don't fall
    /// back to fast backoff as old failures age out of the window.
    circuit_open: bool,
}

impl RetryState {
    /// Record a failed attempt; returns the status to publish and how
    /// long to wait before the next attempt.
    fn on_failure(&mut self) -> (TunnelStatus, Duration) {
        record_failure(&mut self.failures);
        self.circuit_open |= circuit_open(&self.failures);
        if self.circuit_open {
            let status = TunnelStatus::CircuitOpen {
                recent_failures: self.failures.len() as u32,
            };
            return (status, CIRCUIT_RETRY_INTERVAL);
        }
        self.attempt += 1;
        let delay = backoff_delay(self.attempt);
        let status = TunnelStatus::Reconnecting {
            attempt: self.attempt,
            delay_ms: delay.as_millis() as u64,
        };
        (status, delay)
    }

    /// A connect succeeded. A recovered circuit starts from a clean
    /// slate; otherwise failures stay in the window so a tunnel that
    /// keeps connecting and dying still trips the breaker.
    fn on_connected(&mut self) {
        self.attempt = 0;
        if self.circuit_open {
            self.circuit_open = false;
            self.failures.clear();
        }
    }
}

fn backoff_delay(attempt: u32) -> Duration {
    // 1s, 2s, 4s, 8s, 16s, 30s, 30s, …
    let raw = BACKOFF_FLOOR_MS.saturating_mul(1u64 << attempt.min(5));
    Duration::from_millis(raw.min(BACKOFF_CEIL_MS))
}

fn record_failure(failures: &mut Vec<Instant>) {
    let now = Instant::now();
    failures.retain(|t| now.duration_since(*t) <= CIRCUIT_WINDOW);
    failures.push(now);
}

fn circuit_open(failures: &[Instant]) -> bool {
    failures.len() >= MAX_FAILURES as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn startup_timeout_reconnects_and_reaps_ssh() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let fake_ssh = dir.path().join("ssh");
        std::fs::write(
            &fake_ssh,
            "#!/bin/sh\necho $$ > \"$0.pid\"\nprintf 'connection stalled\\n' >&2\nexec /bin/sleep 300\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake_ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (status_tx, mut status_rx) = watch::channel(TunnelStatus::Pending);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let local_socket = dir.path().join("tunnel.sock");
        let inner = Arc::new(SupervisorInner {
            local_socket: local_socket.clone(),
            status_tx,
            shutdown_tx,
            current_child: Mutex::new(None),
        });
        let supervisor = TunnelSupervisor {
            inner: inner.clone(),
            task: std::sync::Mutex::new(None),
        };
        let mut cmd = Command::new(&fake_ssh);
        // Keep a failing regression test from leaving its fake SSH alive
        // when the supervisor task is aborted during cleanup.
        cmd.kill_on_drop(true);
        let mut task = tokio::spawn(run_supervisor(
            inner,
            "unused-host".into(),
            "/unused/remote.sock".into(),
            local_socket,
            "/unused/codemux-remote".into(),
            shutdown_rx,
            cmd,
        ));
        let result = tokio::time::timeout(SPAWN_TIMEOUT + Duration::from_secs(2), async {
            loop {
                if matches!(
                    *status_rx.borrow_and_update(),
                    TunnelStatus::Reconnecting { .. } | TunnelStatus::CircuitOpen { .. }
                ) {
                    break;
                }
                status_rx.changed().await.unwrap();
            }
        })
        .await;
        let pid: i32 = std::fs::read_to_string(dir.path().join("ssh.pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let reaped = unsafe { libc::kill(pid, 0) } == -1
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
        supervisor.shutdown().await;
        if result.is_err() {
            task.abort();
            let _ = task.await;
        } else {
            tokio::time::timeout(Duration::from_secs(1), &mut task)
                .await
                .expect("supervisor must stop during reconnect backoff")
                .unwrap();
        }

        result.expect("startup timeout must leave Pending within a bounded deadline");
        assert!(
            reaped,
            "failed SSH must be killed and reaped before reconnecting"
        );
    }

    #[tokio::test]
    async fn shutdown_kills_ssh_that_is_still_connecting() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let fake_ssh = dir.path().join("ssh");
        std::fs::write(
            &fake_ssh,
            "#!/bin/sh\necho $$ > \"$0.pid\"\nexec /bin/sleep 300\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake_ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
        let (status_tx, _status_rx) = watch::channel(TunnelStatus::Pending);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let local_socket = dir.path().join("tunnel.sock");
        let inner = Arc::new(SupervisorInner {
            local_socket: local_socket.clone(),
            status_tx,
            shutdown_tx,
            current_child: Mutex::new(None),
        });
        let mut cmd = Command::new(&fake_ssh);
        cmd.kill_on_drop(true);
        let task = tokio::spawn(run_supervisor(
            inner.clone(),
            "unused-host".into(),
            "/unused/remote.sock".into(),
            local_socket,
            "/unused/codemux-remote".into(),
            shutdown_rx,
            cmd,
        ));
        let supervisor = TunnelSupervisor {
            inner,
            task: std::sync::Mutex::new(Some(task)),
        };
        let pid_file = dir.path().join("ssh.pid");
        let pid: i32 = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(pid) = std::fs::read_to_string(&pid_file)
                    .ok()
                    .and_then(|s| s.trim().parse().ok())
                {
                    return pid;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("fake ssh must start");

        tokio::time::timeout(Duration::from_secs(2), supervisor.shutdown())
            .await
            .expect("shutdown must not wait for the connect timeout");
        let reaped = unsafe { libc::kill(pid, 0) } == -1
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
        assert!(reaped, "shutdown must kill and reap an ssh that is still connecting");
    }

    #[tokio::test]
    async fn stale_local_socket_is_not_reported_as_connected() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        // An ssh that never binds the forward (e.g. stuck authenticating).
        let fake_ssh = dir.path().join("ssh");
        std::fs::write(&fake_ssh, "#!/bin/sh\nexec /bin/sleep 300\n").unwrap();
        std::fs::set_permissions(&fake_ssh, std::fs::Permissions::from_mode(0o755)).unwrap();
        let local_socket = dir.path().join("tunnel.sock");
        // Left behind by a previous ssh that died.
        std::fs::write(&local_socket, b"").unwrap();

        let (status_tx, mut status_rx) = watch::channel(TunnelStatus::Pending);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let inner = Arc::new(SupervisorInner {
            local_socket: local_socket.clone(),
            status_tx,
            shutdown_tx,
            current_child: Mutex::new(None),
        });
        let mut cmd = Command::new(&fake_ssh);
        cmd.kill_on_drop(true);
        let task = tokio::spawn(run_supervisor(
            inner,
            "unused-host".into(),
            "/unused/remote.sock".into(),
            local_socket.clone(),
            "/unused/codemux-remote".into(),
            shutdown_rx,
            cmd,
        ));
        let connected = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if matches!(*status_rx.borrow_and_update(), TunnelStatus::Connected { .. }) {
                    return;
                }
                status_rx.changed().await.unwrap();
            }
        })
        .await;
        task.abort();
        let _ = task.await;

        assert!(
            connected.is_err(),
            "a stale socket file must not make the tunnel report Connected"
        );
        assert!(!local_socket.exists(), "stale socket must be removed before spawning ssh");
    }

    #[tokio::test]
    async fn stderr_drain_preserves_partial_invalid_utf8_without_eof() {
        use tokio::io::AsyncWriteExt;

        let (mut writer, reader) = tokio::io::duplex(64);
        writer
            .write_all(b"permission denied: \xff end\n")
            .await
            .unwrap();
        let diagnostic = tokio::time::timeout(
            STDERR_DRAIN_TIMEOUT + Duration::from_secs(1),
            read_stderr_bounded(reader),
        )
        .await
        .expect("an open stderr pipe must not block diagnostics");
        assert_eq!(diagnostic, "permission denied: \u{fffd} end\n");
        drop(writer);
    }

    #[tokio::test]
    async fn stderr_drain_stops_at_byte_limit_without_eof() {
        let diagnostic = tokio::time::timeout(
            Duration::from_secs(1),
            read_stderr_bounded(tokio::io::repeat(b'x')),
        )
        .await
        .expect("continuous stderr must stop at the byte limit");
        assert_eq!(diagnostic, "x".repeat(STDERR_BYTE_LIMIT as usize));
    }

    #[tokio::test]
    async fn socket_wait_does_not_drain_stderr_after_ssh_exit() {
        let dir = tempfile::tempdir().unwrap();
        // Keep the pipe open independently, as an SSH descendant can,
        // while retaining both process handles for deterministic cleanup.
        let mut holder = Command::new("/bin/sh")
            .args([
                "-c",
                "printf 'permission denied\\n' >&2; exec /bin/sleep 300",
            ])
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let mut child = Command::new("/bin/sh")
            .args(["-c", "exit 7"])
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        child.stderr = holder.stderr.take();
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            wait_for_socket(&dir.path().join("missing.sock"), &mut child, SPAWN_TIMEOUT),
        )
        .await;
        let diagnostic = read_stderr_bounded(child.stderr.take().unwrap()).await;
        holder.kill().await.unwrap();
        child.wait().await.unwrap();

        let reason = result
            .expect("SSH exit must not wait for stderr EOF")
            .unwrap_err();
        assert!(reason.contains("ssh exited before tunnel came up"));
        assert!(reason.contains('7'));
        assert_eq!(diagnostic, "permission denied\n");
    }

    #[test]
    fn backoff_grows_exponentially_capped_at_ceiling() {
        assert_eq!(backoff_delay(0).as_millis(), 1_000);
        assert_eq!(backoff_delay(1).as_millis(), 2_000);
        assert_eq!(backoff_delay(2).as_millis(), 4_000);
        assert_eq!(backoff_delay(3).as_millis(), 8_000);
        assert_eq!(backoff_delay(4).as_millis(), 16_000);
        // Cap at 30s — 5+ shifts would compute past the ceiling.
        assert_eq!(backoff_delay(5).as_millis(), 30_000);
        assert_eq!(backoff_delay(10).as_millis(), 30_000);
        assert_eq!(backoff_delay(100).as_millis(), 30_000);
    }

    #[test]
    fn circuit_opens_after_max_failures_in_window() {
        let mut failures = Vec::new();
        for _ in 0..(MAX_FAILURES - 1) {
            record_failure(&mut failures);
        }
        assert!(!circuit_open(&failures));
        record_failure(&mut failures);
        assert!(circuit_open(&failures));
    }

    /// Regression test: the supervisor publishes status before any
    /// consumer subscribes (the consumer subscribes lazily from
    /// `client_for_workspace`). `watch::Sender::send` silently drops
    /// the update when `receiver_count() == 0` — so we must use
    /// `send_replace`. This test pins the assumption in case anyone
    /// "refactors" send_replace back to send.
    #[tokio::test]
    async fn send_replace_persists_without_active_receivers() {
        let (tx, rx) = watch::channel(TunnelStatus::Pending);
        drop(rx); // mimic dropping `_status_rx` after spawn() returns
        // Plain send would fail here. send_replace updates the value
        // regardless of receiver count.
        let _ = tx.send_replace(TunnelStatus::Connected { ssh_pid: 42 });
        let mut new_rx = tx.subscribe();
        assert_eq!(
            *new_rx.borrow_and_update(),
            TunnelStatus::Connected { ssh_pid: 42 },
            "subscriber that joins AFTER send_replace must see the new value"
        );
    }

    /// Counter-test that documents why we can't use plain `send`:
    /// it silently drops updates when no receiver is alive.
    #[tokio::test]
    async fn plain_send_silently_drops_without_active_receivers() {
        let (tx, rx) = watch::channel(TunnelStatus::Pending);
        drop(rx);
        let result = tx.send(TunnelStatus::Connected { ssh_pid: 42 });
        assert!(result.is_err(), "send must fail when receiver_count == 0");
        // And critically — the value did NOT update. A later
        // subscriber sees the old initial value.
        let mut new_rx = tx.subscribe();
        assert_eq!(
            *new_rx.borrow_and_update(),
            TunnelStatus::Pending,
            "plain send drops the update when no receivers are alive — \
             this is exactly the bug we fixed by switching to send_replace"
        );
    }

    #[test]
    fn open_circuit_keeps_retrying_slowly_until_connected() {
        let mut retry = RetryState::default();
        for attempt in 1..MAX_FAILURES {
            let (status, delay) = retry.on_failure();
            assert!(matches!(status, TunnelStatus::Reconnecting { attempt: a, .. } if a == attempt));
            assert_eq!(delay, backoff_delay(attempt));
        }
        let (status, delay) = retry.on_failure();
        assert_eq!(
            status,
            TunnelStatus::CircuitOpen {
                recent_failures: MAX_FAILURES
            }
        );
        assert_eq!(delay, CIRCUIT_RETRY_INTERVAL);

        // Failures aging out of the window must not drop back to fast
        // backoff while the host is still unreachable.
        retry.failures.clear();
        let (status, delay) = retry.on_failure();
        assert!(matches!(status, TunnelStatus::CircuitOpen { .. }));
        assert_eq!(delay, CIRCUIT_RETRY_INTERVAL);

        // A successful reconnect closes the circuit and resets counters.
        retry.on_connected();
        let (status, delay) = retry.on_failure();
        assert_eq!(
            status,
            TunnelStatus::Reconnecting {
                attempt: 1,
                delay_ms: backoff_delay(1).as_millis() as u64
            }
        );
        assert_eq!(delay, backoff_delay(1));
    }

    #[test]
    fn connected_without_open_circuit_keeps_failure_window() {
        let mut retry = RetryState::default();
        for _ in 0..(MAX_FAILURES - 1) {
            retry.on_failure();
            retry.on_connected();
        }
        // A tunnel that keeps connecting and dying still trips.
        let (status, _) = retry.on_failure();
        assert!(matches!(status, TunnelStatus::CircuitOpen { .. }));
    }

    #[test]
    fn old_failures_outside_window_dont_count() {
        // We can't truly time-travel in tests, but the record-failure
        // helper drops failures older than CIRCUIT_WINDOW on every
        // insert. Simulate by pre-stuffing old timestamps then
        // recording a fresh one and checking the count.
        let old = Instant::now()
            .checked_sub(CIRCUIT_WINDOW * 2)
            .unwrap_or_else(Instant::now);
        let mut failures = vec![old; (MAX_FAILURES + 5) as usize];
        record_failure(&mut failures);
        // After the record_failure call, only the one fresh failure
        // should remain (the old ones got evicted).
        assert_eq!(failures.len(), 1);
        assert!(!circuit_open(&failures));
    }
}
