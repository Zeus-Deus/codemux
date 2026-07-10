//! Singleton lifecycle for the lazily-spawned OpenCode HTTP server.
//!
//! [`OpenCodeServerManager`] is the long-lived Tauri-state handle.
//! `ensure_running()` is the public entry point: first call spawns
//! the server, subsequent calls hand back a cheap clone of the
//! already-published URL + password. Spawning is gated by an async
//! `Mutex` so two concurrent first-callers don't both fire
//! `opencode serve`.
//!
//! Why a singleton? The OpenCode HTTP API exposes the user's whole
//! provider catalogue (116 providers / ~4 354 models on the dev
//! machine the live smoke ran against) and there is exactly one
//! catalogue per machine — a second server is just additional
//! background CPU + an extra port for no reason.
//!
//! # Lifetime
//!
//! The manager stays alive for the whole Tauri process. Dropping
//! it (process exit, hot-reload) drops the contained
//! [`OpenCodeServer`], which `kill_on_drop`-kills the child.
//! [`stop`](OpenCodeServerManager::stop) is the explicit teardown
//! path used by tests and the (future) settings panel "restart
//! OpenCode" button.
//!
//! # Liveness + respawn
//!
//! `ensure_running()` does NOT blindly trust the cached child: a
//! laptop sleep / crash can kill `opencode serve` while the manager
//! still holds its handle, and nothing in production ever calls
//! `stop()`. Every call with a populated cache first runs a cheap
//! authenticated HTTP probe against the cached URL (bounded by
//! [`LIVENESS_PROBE_TIMEOUT`] per attempt) with a **two-strike
//! policy**: a connect error (dead loopback port refuses instantly)
//! is an immediate dead verdict, while a timeout / other transport
//! error only condemns after one retry also fails — a server that is
//! merely wedged for a moment must not be SIGKILLed, since dropping
//! it invalidates every live session's handle at once. A confirmed
//! corpse is dropped (`kill_on_drop` reaps a wedged child) and
//! respawned under the same lock. Without this, the dead-run
//! recovery path (issue #154) dead-ends: the session rebuild would
//! POST `session_create` at a stale handle forever.
//! The respawned server gets a fresh port + password; that's fine —
//! the only sessions holding the old handle are dead ones this
//! scenario just invalidated, and OpenCode persists sessions on
//! disk, so `OpenCodeSession::start`'s readopt probe recovers the
//! conversation context on the new server.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

use super::discovery::check_opencode_availability;
use super::server::OpenCodeServer;

/// Per-attempt budget for the cached-server liveness probe in
/// [`OpenCodeServerManager::ensure_running`]. A healthy local server
/// answers in single-digit milliseconds and a dead loopback port is
/// refused instantly, so the timeout only ever elapses for a
/// wedged-but-alive child. A timeout gets a second probe attempt (see
/// [`probe_server_alive`]'s two-strike policy), so the worst-case
/// delay before recovery kicks in is twice this value — still short
/// enough that a genuinely wedged server doesn't stall a session
/// start for long.
const LIVENESS_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Cheap, owned snapshot of "where the running server lives" handed
/// to every caller of [`OpenCodeServerManager::ensure_running`].
///
/// Cloning the handle does NOT extend the server's lifetime — the
/// underlying `Arc<OpenCodeServer>` is held inside the manager. If
/// the server is stopped between handing out a handle and using it,
/// HTTP requests will fail with a connect error and the caller can
/// re-call `ensure_running` to respawn.
#[derive(Debug, Clone)]
pub struct OpenCodeServerHandle {
    pub base_url: String,
    pub server_password: String,
}

/// Singleton supervisor for the OpenCode HTTP server child.
///
/// Designed to be inserted into Tauri's managed-state map exactly
/// once at app boot. All Tauri commands that need a running server
/// pull it from `State<'_, OpenCodeServerManager>` and call
/// `ensure_running().await`.
pub struct OpenCodeServerManager {
    /// Held under an async mutex so the spawn path is serialised —
    /// a second `ensure_running()` racing with the first observes
    /// the server already populated and returns the cached handle.
    server: Mutex<Option<Arc<OpenCodeServer>>>,
}

impl OpenCodeServerManager {
    /// Build an empty manager. The server is not spawned until the
    /// first `ensure_running()` call — Codemux startup never blocks
    /// on `opencode serve`.
    pub fn new() -> Self {
        Self {
            server: Mutex::new(None),
        }
    }

    /// Return a handle to the running server, spawning it on demand
    /// and **respawning it when the cached child is no longer
    /// serving** (killed by a laptop sleep / crash, or wedged past
    /// the probe budget). The liveness probe + respawn both run under
    /// the same lock, so a racing caller can never be handed the
    /// stale handle after another caller has decided it's dead — it
    /// waits, re-probes the fresh server, and gets the new handle.
    ///
    /// Failure modes (returned as a stable `Err(String)` so the
    /// Tauri command surface can forward verbatim):
    ///
    /// * `"opencode_not_installed"` — the `opencode` binary was not
    ///   found on PATH. Triggered by the discovery layer rather
    ///   than the spawn layer so the message is uniform across the
    ///   ping / list_models / future session-start commands.
    /// * `"spawn_failed: …"` / `"ready_banner_missing"` /
    ///   `"ready_timeout_after_…"` — passthrough from
    ///   [`OpenCodeServer::spawn`](super::server::OpenCodeServer::spawn).
    pub async fn ensure_running(&self) -> Result<OpenCodeServerHandle, String> {
        let mut guard = self.server.lock().await;

        if let Some(existing) = guard.as_ref() {
            let handle = handle_from(existing);
            if probe_server_alive(&handle).await {
                return Ok(handle);
            }
            // The cached child stopped serving. Drop the corpse —
            // `kill_on_drop` reaps a wedged-but-alive child — and fall
            // through to a fresh spawn while still holding the lock.
            eprintln!(
                "[codemux::opencode] cached server at {} is not responding — respawning",
                handle.base_url
            );
            *guard = None;
        }

        let binary_path = resolve_binary().await?;
        let server = OpenCodeServer::spawn(&binary_path).await?;
        let arc = Arc::new(server);
        let handle = handle_from(&arc);
        *guard = Some(arc);
        Ok(handle)
    }

    /// Tear down the running server, if any. Idempotent — calling
    /// `stop()` on an already-stopped manager is a no-op. The next
    /// `ensure_running()` call respawns from scratch.
    pub async fn stop(&self) {
        let mut guard = self.server.lock().await;
        if let Some(server) = guard.take() {
            // Drop happens implicitly when `server` goes out of
            // scope at the end of this block; `kill_on_drop(true)`
            // sends SIGKILL.
            drop(server);
        }
    }

    /// Test-only inspector. Returns `true` while a child is
    /// currently held. Public-but-`cfg(test)`-style: callers in
    /// production code should not gate behaviour on this.
    #[cfg(test)]
    pub async fn is_running_for_tests(&self) -> bool {
        self.server.lock().await.is_some()
    }
}

impl Default for OpenCodeServerManager {
    fn default() -> Self {
        Self::new()
    }
}

fn handle_from(server: &Arc<OpenCodeServer>) -> OpenCodeServerHandle {
    OpenCodeServerHandle {
        base_url: server.base_url().to_string(),
        server_password: server.server_password().to_string(),
    }
}

/// Whether the cached server is still answering HTTP.
///
/// An authenticated `GET /` bounded by [`LIVENESS_PROBE_TIMEOUT`] per
/// attempt: **any** HTTP response (including 401/404) proves a live
/// server, so only transport failures count as dead — with a
/// **two-strike policy** that distinguishes the failure kinds:
///
/// * A connect-level failure (`reqwest::Error::is_connect`) is an
///   immediate dead verdict. A dead loopback port refuses instantly
///   and deterministically — retrying would only slow the legit
///   recovery path down.
/// * A timeout (or any other transport error) gets ONE retry, and
///   only condemns when the retry also fails. A server that is merely
///   wedged for a moment (heavy turn, big serialization) must not be
///   SIGKILLed on a single slow probe: condemning a live server
///   invalidates every live session's stored handle at once — a
///   whole-server blast radius for what was just a slow moment. Worst
///   case is bounded at 2 × [`LIVENESS_PROBE_TIMEOUT`].
///
/// An HTTP probe is used instead of `Child::try_wait` deliberately:
/// `try_wait` needs `&mut Child`, which the shared `Arc<OpenCodeServer>`
/// can't give without moving the child behind a mutex (touching
/// `pid()` and `Drop`), and it can't detect a hung-but-alive server at
/// all — the probe covers both failure shapes with no refactor.
async fn probe_server_alive(handle: &OpenCodeServerHandle) -> bool {
    let Ok(client) = reqwest::Client::builder()
        .timeout(LIVENESS_PROBE_TIMEOUT)
        .build()
    else {
        return false;
    };
    let url = format!("{}/", handle.base_url.trim_end_matches('/'));
    for attempt in 0..2u8 {
        match client
            .get(&url)
            .basic_auth("opencode", Some(&handle.server_password))
            .send()
            .await
        {
            Ok(_) => return true,
            // Dead port: refused instantly, deterministically. One strike.
            Err(err) if err.is_connect() => return false,
            // Timeout / other transport hiccup: second chance. The retry
            // itself waits up to LIVENESS_PROBE_TIMEOUT again, which IS
            // the grace period for a momentarily wedged server.
            Err(err) => {
                if attempt > 0 {
                    eprintln!(
                        "[codemux::opencode] liveness probe of {} failed twice \
                         (last: {err}) — treating server as dead",
                        handle.base_url
                    );
                }
            }
        }
    }
    false
}

async fn resolve_binary() -> Result<PathBuf, String> {
    // Reuse the Stage 1 discovery primitive so any future
    // refinement (e.g. honouring a `binary_path_override` setting)
    // lands in one place rather than duplicated across the spawn
    // path and the diagnostics path.
    let availability = check_opencode_availability(None).await;
    if !availability.installed {
        return Err("opencode_not_installed".into());
    }
    availability
        .binary_path
        .ok_or_else(|| "opencode_not_installed".into())
}

/// Test-only fixture: write a fake `opencode` binary into `dir` that
/// answers `--version` and otherwise prints the ready banner pointing
/// at `$FAKE_OPENCODE_URL` (a mock HTTP server the test controls),
/// then parks. Lets liveness/respawn tests drive the REAL
/// `ensure_running` spawn path — banner parse, password mint, probe —
/// while the test swaps the backing HTTP surface in and out to
/// simulate a server death. Shared with the session-level end-to-end
/// test, hence module scope rather than `mod tests`.
#[cfg(all(test, unix))]
pub(crate) fn write_fake_opencode_binary(dir: &std::path::Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join("opencode");
    // Absolute shebang + absolute `sleep`: the calling test trims PATH
    // down to the fixture dir, so nothing may rely on PATH lookups.
    std::fs::write(
        &path,
        b"#!/bin/sh\n\
if [ \"$1\" = \"--version\" ]; then echo \"9.9.9\"; exit 0; fi\n\
echo \"opencode server listening on ${FAKE_OPENCODE_URL}\"\n\
exec /bin/sleep 600\n",
    )
    .expect("write fake opencode");
    let mut perms = std::fs::metadata(&path).expect("stat").permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).expect("chmod");
    path
}

/// Test-only fixture: a minimal disposable HTTP responder whose port
/// genuinely closes on [`kill`](DisposableHttpServer::kill) — unlike
/// `mockito`, whose pooled servers keep listening after drop, which
/// makes them useless for simulating a *dead* server. Answers every
/// request with an empty `200` (any HTTP response satisfies the
/// manager's liveness probe).
#[cfg(all(test, unix))]
pub(crate) struct DisposableHttpServer {
    pub(crate) url: String,
    handle: tokio::task::JoinHandle<()>,
}

#[cfg(all(test, unix))]
impl DisposableHttpServer {
    pub(crate) async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind disposable http server");
        let addr = listener.local_addr().expect("local addr");
        let handle = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = [0u8; 1024];
                    let _ = stream.read(&mut buf).await;
                    let _ = stream
                        .write_all(
                            b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                        )
                        .await;
                });
            }
        });
        Self {
            url: format!("http://{addr}"),
            handle,
        }
    }

    /// Stop serving and release the port. Awaits the accept task's
    /// cancellation so the listener fd is guaranteed closed (new
    /// connections refused) before this returns.
    pub(crate) async fn kill(self) {
        self.handle.abort();
        let _ = self.handle.await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests in this module that mutate `PATH` or rely on the live
    /// `opencode` binary are serialised under the `opencode_path`
    /// label. Without this guard the PATH-mutating test races
    /// against the live-binary tests (and the Stage 1 discovery
    /// `check_against_live_binary_when_present` smoke), producing
    /// flaky `opencode_not_installed` panics during parallel cargo
    /// runs.
    use serial_test::serial;

    #[tokio::test]
    #[serial(opencode_path)]
    async fn ensure_running_when_binary_missing_returns_not_installed() {
        // Force PATH to a guaranteed-empty location so discovery
        // returns `installed: false`. Mirror the Stage 1
        // `check_when_binary_missing_returns_not_installed`
        // pattern so the env mutation is bracketed by a restore
        // even on panic.
        let original = std::env::var("PATH").unwrap_or_default();
        let tmp = tempfile::tempdir().expect("tempdir");
        // SAFETY: serial_test serialises this against any other
        // opencode test that touches PATH, so the env mutation is
        // exclusive while it's in effect.
        unsafe {
            std::env::set_var("PATH", tmp.path());
        }

        let manager = OpenCodeServerManager::new();
        let result = manager.ensure_running().await;

        unsafe {
            std::env::set_var("PATH", original);
        }

        let err = result.expect_err("must fail");
        assert_eq!(err, "opencode_not_installed");
        assert!(
            !manager.is_running_for_tests().await,
            "manager should not have populated the server slot on failure"
        );
    }

    #[tokio::test]
    #[serial(opencode_path)]
    async fn ensure_running_live_is_idempotent() {
        // Skip when opencode isn't around. The lib-level tests
        // already exercise the not-installed path explicitly; this
        // one specifically validates "spawn once, return cached
        // handle on subsequent calls".
        if which::which("opencode").is_err() {
            eprintln!(
                "[opencode::manager] skipping live idempotency \
                 smoke — opencode not on PATH"
            );
            return;
        }

        let manager = OpenCodeServerManager::new();

        let h1 = manager
            .ensure_running()
            .await
            .expect("first ensure_running must succeed");
        assert!(h1.base_url.starts_with("http://127.0.0.1:"));
        assert!(!h1.server_password.is_empty());

        // Second call should NOT respawn — same URL, same
        // password.
        let h2 = manager
            .ensure_running()
            .await
            .expect("second ensure_running must succeed");
        assert_eq!(h1.base_url, h2.base_url);
        assert_eq!(h1.server_password, h2.server_password);

        // Stop, then a fresh ensure_running spawns again with a
        // *different* password. URL may collide if OpenCode reuses
        // the same OS-assigned port; we don't assert on URL
        // equality.
        manager.stop().await;
        assert!(!manager.is_running_for_tests().await);

        let h3 = manager
            .ensure_running()
            .await
            .expect("post-stop ensure_running must succeed");
        assert_ne!(
            h1.server_password, h3.server_password,
            "respawn must mint a fresh password"
        );

        manager.stop().await;
    }

    #[tokio::test]
    async fn stop_on_empty_manager_is_noop() {
        let manager = OpenCodeServerManager::new();
        manager.stop().await;
        assert!(!manager.is_running_for_tests().await);
        // Still no panic.
        manager.stop().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    #[serial(opencode_path)]
    async fn ensure_running_keeps_live_server_and_respawns_dead_one() {
        // The issue-#154 recovery dead-end: a laptop sleep / crash kills
        // `opencode serve` while the manager still caches its handle, and
        // nothing in production calls `stop()`. `ensure_running` must
        // detect the corpse and respawn instead of handing the stale
        // handle back forever. The fake binary banners the URL of a
        // test-controlled disposable responder, so "the child died" is
        // simulated by killing that responder (its port starts refusing
        // connections — exactly what a dead loopback child looks like to
        // the probe).
        let backend_a = DisposableHttpServer::start().await;
        let backend_a_url = backend_a.url.clone();
        let tmp = tempfile::tempdir().expect("tempdir");
        write_fake_opencode_binary(tmp.path());
        let original_path = std::env::var("PATH").unwrap_or_default();
        // SAFETY: serial_test serialises this against every other
        // opencode test that touches PATH / FAKE_OPENCODE_URL.
        unsafe {
            std::env::set_var("PATH", tmp.path());
            std::env::set_var("FAKE_OPENCODE_URL", &backend_a_url);
        }

        let manager = OpenCodeServerManager::new();
        let h1 = manager.ensure_running().await;
        // A live cached server must NOT be respawned — identical handle.
        let h2 = manager.ensure_running().await;

        // Bind the replacement backend BEFORE killing A: binding first
        // guarantees the two never share a port, so the OS can't hand
        // A's freed ephemeral port to B — which would make the probe of
        // the cached (dead) A URL hit live B and flakily skip the
        // respawn. Then kill A's HTTP surface and point the NEXT spawn
        // at B (the env var is read by the fake at spawn time).
        let backend_b = DisposableHttpServer::start().await;
        backend_a.kill().await;
        unsafe {
            std::env::set_var("FAKE_OPENCODE_URL", &backend_b.url);
        }
        let h3 = manager.ensure_running().await;

        // Restore the environment BEFORE asserting so a failure can't
        // leak the mutated PATH into later serial tests.
        unsafe {
            std::env::set_var("PATH", original_path);
            std::env::remove_var("FAKE_OPENCODE_URL");
        }
        manager.stop().await;

        let h1 = h1.expect("first ensure_running must succeed");
        let h2 = h2.expect("cached ensure_running must succeed");
        let h3 = h3.expect("post-death ensure_running must succeed");
        assert_eq!(h1.base_url, h2.base_url, "live cache must be stable");
        assert_eq!(
            h1.server_password, h2.server_password,
            "live cache must not respawn"
        );
        assert_ne!(
            h1.server_password, h3.server_password,
            "dead cache must trigger a respawn with a fresh password"
        );
        assert_eq!(
            h3.base_url, backend_b.url,
            "respawned handle must point at the new server"
        );
        backend_b.kill().await;
    }
}
