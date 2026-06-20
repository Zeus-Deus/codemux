//! Tunnel auto-reconnect supervisor.
//!
//! Wraps `TunnelHandle` with retry logic so a transient SSH failure
//! (WiFi flap, laptop sleep/wake, remote sshd restart) doesn't strand
//! a workspace's daemon. Matches the cadence superset-sh uses in
//! their `tunnel-client.ts`:
//!
//! - Exponential backoff from 1 s to 30 s
//! - Watchdog detects SSH death within ~2 s
//! - Crash circuit: 5 reconnect failures in 5 min opens the breaker
//!   and stops trying. The user has to explicitly re-push the
//!   workspace to recover (matches our local pty-daemon circuit
//!   pattern — recurring failures are environmental, not transient).
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
    /// Crash circuit tripped. The supervisor is now passive — the
    /// user must take action (re-push the workspace, fix the host).
    CircuitOpen { recent_failures: u32 },
}

/// Backoff schedule used between reconnects. Caps at 30s. The 5-min
/// window inside which `MAX_FAILURES` failures trip the breaker
/// matches the local pty-daemon circuit's policy.
const BACKOFF_FLOOR_MS: u64 = 1_000;
const BACKOFF_CEIL_MS: u64 = 30_000;
const MAX_FAILURES: u32 = 5;
const CIRCUIT_WINDOW: Duration = Duration::from_secs(300);
/// How long we wait for the tunnel socket to appear after spawning
/// SSH. The first connect can be slow (DNS, key handshake); later
/// reconnects are usually instant.
const SPAWN_TIMEOUT: Duration = Duration::from_secs(15);

/// Supervisor handle. Dropping it does NOT cancel the supervisor —
/// the tunnel keeps running until `shutdown` is called or the
/// supervisor's own task exits. Sharing the handle is fine.
pub struct TunnelSupervisor {
    inner: Arc<SupervisorInner>,
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
        let _ = tokio::spawn(async move {
            run_supervisor(
                task_inner,
                ssh_target,
                remote_socket,
                local_socket,
                remote_binary,
                shutdown_rx,
            )
            .await;
        });
        Arc::new(Self { inner })
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
        let mut guard = self.inner.current_child.lock().await;
        if let Some(mut child) = guard.take() {
            let _ = child.kill().await;
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
) {
    eprintln!(
        "[tunnel-supervisor] start: ssh_target={ssh_target} \
         local_socket={local_socket:?} remote_socket={remote_socket} \
         remote_binary={remote_binary}"
    );
    // Failure timestamps form a sliding window; we count failures in
    // the last `CIRCUIT_WINDOW` and trip the breaker when we exceed
    // the cap.
    let mut failures: Vec<Instant> = Vec::new();
    let mut attempt: u32 = 0;

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
        let _ = inner.status_tx.send_replace(TunnelStatus::Pending);

        let opts = TunnelOptions {
            ssh_target: &ssh_target,
            remote_socket: &remote_socket,
            local_socket: &local_socket,
            remote_binary: &remote_binary,
        };
        let argv = build_tunnel_argv(&opts);
        crate::trace_cloud_push!("[tunnel-supervisor] attempt {} argv: ssh {}", attempt + 1, argv.join(" "));
        let mut cmd = Command::new("ssh");
        for arg in &argv {
            cmd.arg(arg);
        }
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

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
                record_failure(&mut failures);
                if circuit_open(&failures) {
                    let _ = inner.status_tx.send_replace(TunnelStatus::CircuitOpen {
                        recent_failures: failures.len() as u32,
                    });
                    return;
                }
                attempt += 1;
                let delay = backoff_delay(attempt);
                let _ = inner.status_tx.send_replace(TunnelStatus::Reconnecting {
                    attempt,
                    delay_ms: delay.as_millis() as u64,
                });
                // Wait the delay but bail early on shutdown.
                if !sleep_or_shutdown(delay, &mut shutdown_rx).await {
                    return;
                }
                continue;
            }
        };

        // Wait for the socket to appear (or SSH to die before it does).
        let socket_ready = wait_for_socket(&local_socket, &mut child, SPAWN_TIMEOUT).await;
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
                attempt = 0; // success resets the attempt counter

                // Watchdog: wait for SSH to exit or shutdown signal.
                let exited = watch_child_until_exit(&inner, &mut shutdown_rx).await;
                if !exited {
                    // Shutdown signalled — drop everything.
                    return;
                }
                eprintln!("[tunnel-supervisor] ssh exited; will reconnect");
                record_failure(&mut failures);
            }
            Err(reason) => {
                // Capture SSH stderr verbatim so we can see WHY the
                // tunnel failed. Most useful failures (host
                // unreachable, permission denied, port-forwarding
                // refused) write to stderr before SSH exits.
                let mut stderr_dump = String::new();
                if let Some(mut err_stream) = child.stderr.take() {
                    use tokio::io::AsyncReadExt;
                    let _ = err_stream.read_to_string(&mut stderr_dump).await;
                }
                eprintln!(
                    "[tunnel-supervisor] tunnel did not come up: {reason}\n\
                     [tunnel-supervisor] ssh stderr: {}",
                    stderr_dump.trim()
                );
                let _ = child.kill().await;
                record_failure(&mut failures);
            }
        }

        if circuit_open(&failures) {
            let _ = inner.status_tx.send_replace(TunnelStatus::CircuitOpen {
                recent_failures: failures.len() as u32,
            });
            return;
        }
        attempt += 1;
        let delay = backoff_delay(attempt);
        let _ = inner.status_tx.send_replace(TunnelStatus::Reconnecting {
            attempt,
            delay_ms: delay.as_millis() as u64,
        });
        if !sleep_or_shutdown(delay, &mut shutdown_rx).await {
            return;
        }
    }
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
            let mut stderr = String::new();
            if let Some(mut err_stream) = child.stderr.take() {
                use tokio::io::AsyncReadExt;
                let _ = err_stream.read_to_string(&mut stderr).await;
            }
            return Err(format!(
                "ssh exited before tunnel came up (status={status}): {}",
                stderr.trim()
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
