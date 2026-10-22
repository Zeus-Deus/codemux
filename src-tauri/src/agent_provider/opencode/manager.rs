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
//! background CPU + an extra port for no reason. Chats share it too:
//! each registers its own `codemux_<id>` MCP server and only that chat's
//! session is granted those tools (see `server::CONFIG_CONTENT`).
//!
//! # Lifetime
//!
//! GUI quit and `codemux serve` shutdown explicitly call
//! [`shutdown`](OpenCodeServerManager::shutdown) to close admission and await
//! the owned direct child's termination/reap.
//! Tauri process exit need not drop managed state. Drop's `kill_on_drop`
//! remains only a fallback, not OS-crash or descendant cleanup.
//! [`stop`](OpenCodeServerManager::stop) awaits teardown but remains restartable.
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
//! corpse is terminated and reaped, then
//! respawned under the same lock. Without this, the dead-run
//! recovery path (issue #154) dead-ends: the session rebuild would
//! POST `session_create` at a stale handle forever.
//! The respawned server gets a fresh port + password; that's fine —
//! the only sessions holding the old handle are dead ones this
//! scenario just invalidated, and OpenCode persists sessions on
//! disk, so `OpenCodeSession::start`'s readopt probe recovers the
//! conversation context on the new server.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::Mutex;

use super::discovery::prepared_binary;
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

/// Allow the existing bounded probe, resolver, ready handshake and reaps
/// to settle, but never wait indefinitely for in-flight lifecycle work.
const SHUTDOWN_LOCK_TIMEOUT: Duration = Duration::from_secs(40);

/// Cheap, owned snapshot of "where the running server lives" handed
/// to every caller of [`OpenCodeServerManager::ensure_running`].
///
/// Cloning the handle does NOT extend the server's lifetime — the
/// underlying `OpenCodeServer` is owned by the manager. If
/// the server is stopped between handing out a handle and using it,
/// HTTP requests will fail with a connect error and the caller can
/// re-call `ensure_running` to respawn.
#[derive(Debug, Clone)]
pub struct OpenCodeServerHandle {
    pub base_url: String,
    pub server_password: String,
    pub api: super::api::OpenCodeApi,
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
    server: Mutex<Option<OpenCodeServer>>,
    /// Terminal admission fence, published before waiting for the lifecycle
    /// lock. Unlike restartable Stop, app shutdown must never admit a spawn.
    closing: AtomicBool,
    shutdown_lock_timeout: Duration,
    /// The last model catalog read from a server, with that server's API, so
    /// orchestration listings can report it without starting one.
    catalog: std::sync::Mutex<Option<(super::api::OpenCodeApi, crate::agent_provider::ProviderChatCapabilities)>>,
}

impl OpenCodeServerManager {
    /// Build an empty manager. The server is not spawned until the
    /// first `ensure_running()` call — Codemux startup never blocks
    /// on `opencode serve`.
    pub fn new() -> Self {
        Self {
            server: Mutex::new(None),
            closing: AtomicBool::new(false),
            shutdown_lock_timeout: SHUTDOWN_LOCK_TIMEOUT,
            catalog: Default::default(),
        }
    }

    pub(crate) fn remember_catalog(
        &self,
        api: super::api::OpenCodeApi,
        capabilities: crate::agent_provider::ProviderChatCapabilities,
    ) {
        *self.catalog.lock().unwrap_or_else(|p| p.into_inner()) = Some((api, capabilities));
    }

    /// The last catalog read, without starting a server.
    pub(crate) fn cached_catalog(
        &self,
    ) -> Option<(super::api::OpenCodeApi, crate::agent_provider::ProviderChatCapabilities)> {
        self.catalog.lock().unwrap_or_else(|p| p.into_inner()).clone()
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
    /// * Prepared-executable resolution errors (missing install, invalid
    ///   authoritative override, or unsupported npm layout) are forwarded
    ///   verbatim, without a best-effort `--version` probe.
    /// * `"spawn_failed: …"` / `"ready_banner_missing"` /
    ///   `"ready_timeout_after_…"` — passthrough from
    ///   [`OpenCodeServer::spawn`](super::server::OpenCodeServer::spawn).
    pub async fn ensure_running(&self) -> Result<OpenCodeServerHandle, String> {
        self.check_open()?;
        let mut guard = self.server.lock().await;
        self.check_open()?;

        if let Some(existing) = guard.as_ref() {
            let handle = handle_from(existing);
            let alive = probe_server_alive(&handle).await;
            self.check_open()?;
            if alive {
                return Ok(handle);
            }
            // Keep direct-child ownership until its reap completes, even
            // when terminal shutdown races a dead-cache recovery.
            eprintln!(
                "[codemux::opencode] cached server at {} is not responding — respawning",
                handle.base_url
            );
            if let Some(existing) = guard.as_mut() {
                existing.shutdown().await?;
            }
            *guard = None;
        }

        let binary_path = resolve_binary().await?;
        self.check_open()?;
        let server = OpenCodeServer::spawn(&binary_path).await?;
        if let Err(error) = self.check_open() {
            // Shutdown may have begun while the ready banner was pending.
            // Retain the ready child's owner before awaiting cleanup: a
            // canceled/failed reap must remain available to shutdown joiners.
            *guard = Some(server);
            if let Some(server) = guard.as_mut() {
                server.shutdown().await?;
            }
            *guard = None;
            return Err(error);
        }
        let handle = handle_from(&server);
        *guard = Some(server);
        Ok(handle)
    }

    /// Tear down the running server, if any. Idempotent — calling
    /// `stop()` on an already-stopped manager is a no-op. The next
    /// `ensure_running()` call respawns from scratch unless terminal shutdown
    /// has begun. Termination errors are logged, with Drop as the fallback.
    pub async fn stop(&self) {
        let mut guard = self.server.lock().await;
        if let Some(server) = guard.as_mut() {
            if let Err(error) = server.shutdown().await {
                eprintln!("[codemux::opencode] stop incomplete: {error}");
            }
        }
        *guard = None;
    }

    /// Permanently close admission, terminate and reap the direct child.
    /// Concurrent callers join teardown under the same lifecycle lock;
    /// a canceled/failed waiter leaves the child owned for a later retry.
    pub async fn shutdown(&self) -> Result<(), String> {
        self.closing.store(true, Ordering::SeqCst);
        let mut guard = tokio::time::timeout(self.shutdown_lock_timeout, self.server.lock())
            .await
            .map_err(|_| "opencode_shutdown_lock_timeout".to_string())?;
        if let Some(server) = guard.as_mut() {
            server.shutdown().await?;
        }
        *guard = None;
        Ok(())
    }

    fn check_open(&self) -> Result<(), String> {
        if self.closing.load(Ordering::SeqCst) {
            Err("opencode_manager_shutdown".into())
        } else {
            Ok(())
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

fn handle_from(server: &OpenCodeServer) -> OpenCodeServerHandle {
    OpenCodeServerHandle {
        base_url: server.base_url().to_string(),
        api: server.api(),
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
/// a process-exit check cannot detect a hung-but-alive server. The probe
/// covers both failure shapes.
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
    // Spawn admission needs the selected prepared path, not a best-effort
    // availability/version probe. Preserve authoritative resolver diagnostics.
    prepared_binary().await
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

    struct EnvGuard(Vec<(&'static str, Option<std::ffi::OsString>)>);
    impl EnvGuard {
        fn new(values: &[(&'static str, Option<&std::ffi::OsStr>)]) -> Self {
            let guard = Self(
                values
                    .iter()
                    .map(|(key, _)| (*key, std::env::var_os(key)))
                    .collect(),
            );
            for (key, value) in values {
                unsafe {
                    match value {
                        Some(value) => std::env::set_var(key, value),
                        None => std::env::remove_var(key),
                    }
                }
            }
            guard
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in &self.0 {
                unsafe {
                    match value {
                        Some(value) => std::env::set_var(key, value),
                        None => std::env::remove_var(key),
                    }
                }
            }
        }
    }

    #[tokio::test]
    #[serial(opencode_path)]
    async fn resolve_binary_preserves_authoritative_override_diagnostics() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("missing.exe");
        let launcher = tmp.path().join("opencode.cmd");
        std::fs::write(&launcher, "@echo off\ninvalid selected legacy install\n").unwrap();
        let legacy_root = tmp.path().join("legacy");
        let package = legacy_root.join("node_modules/opencode-ai");
        std::fs::create_dir_all(package.join("bin")).unwrap();
        std::fs::write(
            package.join("bin/opencode"),
            "inert Node launcher; never executed",
        )
        .unwrap();
        std::fs::write(package.join("package.json"), r#"{"name":"opencode-ai","version":"1.14.31","bin":{"opencode":"./bin/opencode"},"optionalDependencies":{"opencode-windows-x64":"1.14.31","opencode-windows-x64-baseline":"1.14.31","opencode-windows-arm64":"1.14.31"}}"#).unwrap();
        let legacy = legacy_root.join("opencode.cmd");
        std::fs::write(
            &legacy,
            "@echo off\n\"%dp0%/node_modules/opencode-ai/bin/opencode\" %*\n",
        )
        .unwrap();
        let _path = EnvGuard::new(&[("PATH", Some(tmp.path().as_os_str()))]);
        for (path, expected) in [
            (PathBuf::from("relative/opencode.exe"), "OpenCode requires an absolute prepared executable path"),
            (missing, "The prepared OpenCode executable is unavailable"),
            (launcher, "Cannot resolve this OpenCode npm launcher to an adjacent prepared native executable"),
            (legacy, "Cannot resolve this legacy OpenCode npm launcher to a same-version prepared Windows optional dependency; check its node_modules or set CODEMUX_OPENCODE_BINARY to the prepared native executable"),
        ] {
            let _override = EnvGuard::new(&[("CODEMUX_OPENCODE_BINARY", Some(path.as_os_str()))]);
            let manager = OpenCodeServerManager::new();
            assert_eq!(manager.ensure_running().await.unwrap_err(), expected);
            assert!(!manager.is_running_for_tests().await);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    #[serial(opencode_path)]
    async fn resolve_binary_never_probes_version_or_falls_back_to_path() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let binary = tmp.path().join("opencode");
        let marker = tmp.path().join("executed");
        std::fs::write(
            &binary,
            format!("#!/bin/sh\nprintf touched > '{}'\n", marker.display()),
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _env = EnvGuard::new(&[
            ("PATH", Some(tmp.path().as_os_str())),
            ("CODEMUX_OPENCODE_BINARY", Some(binary.as_os_str())),
        ]);
        assert_eq!(
            resolve_binary().await.unwrap(),
            binary.canonicalize().unwrap()
        );
        assert!(
            !marker.exists(),
            "ordinary spawn resolution must not run --version"
        );
        let missing = tmp.path().join("missing.exe");
        let _bad = EnvGuard::new(&[("CODEMUX_OPENCODE_BINARY", Some(missing.as_os_str()))]);
        assert_eq!(
            resolve_binary().await.unwrap_err(),
            "The prepared OpenCode executable is unavailable"
        );
        assert!(
            !marker.exists(),
            "bad override must not run the valid PATH binary"
        );
    }

    #[tokio::test]
    #[serial(opencode_path)]
    async fn ensure_running_when_binary_missing_preserves_resolver_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let _env = EnvGuard::new(&[
            ("PATH", Some(tmp.path().as_os_str())),
            ("CODEMUX_OPENCODE_BINARY", None),
        ]);
        let manager = OpenCodeServerManager::new();
        let err = manager.ensure_running().await.expect_err("must fail");
        assert_eq!(err, "Install a supported OpenCode CLI or set CODEMUX_OPENCODE_BINARY to its prepared executable");
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

    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[serial(opencode_path)]
    async fn stop_reaps_owned_child_before_return_and_remains_restartable() {
        let backend = DisposableHttpServer::start().await;
        let tmp = tempfile::tempdir().unwrap();
        let binary = write_fake_opencode_binary(tmp.path());
        let _env = EnvGuard::new(&[
            ("CODEMUX_OPENCODE_BINARY", Some(binary.as_os_str())),
            (
                "FAKE_OPENCODE_URL",
                Some(std::ffi::OsStr::new(&backend.url)),
            ),
        ]);
        let manager = OpenCodeServerManager::new();
        let first = manager.ensure_running().await.unwrap();
        let pid = manager.server.lock().await.as_ref().unwrap().pid().unwrap();
        manager.stop().await;
        // No sleep or polling: completion of Stop must mean the owned child
        // was waited, not merely sent a kill signal for later runtime cleanup.
        assert!(
            !std::path::Path::new(&format!("/proc/{pid}")).exists(),
            "Stop returned before its owned child was reaped"
        );
        assert!(!manager.is_running_for_tests().await);
        let second = manager.ensure_running().await.unwrap();
        assert_ne!(first.server_password, second.server_password);
        manager.stop().await;
        backend.kill().await;
    }

    #[tokio::test]
    #[serial(opencode_path)]
    async fn terminal_shutdown_denies_later_admission() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("not-installed");
        let _env = EnvGuard::new(&[("CODEMUX_OPENCODE_BINARY", Some(missing.as_os_str()))]);
        let manager = OpenCodeServerManager::new();
        manager.shutdown().await.unwrap();
        manager.shutdown().await.unwrap();
        manager.stop().await;
        assert_eq!(
            manager.ensure_running().await.unwrap_err(),
            "opencode_manager_shutdown"
        );
        assert!(!manager.is_running_for_tests().await);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[serial(opencode_path)]
    async fn concurrent_terminal_shutdowns_wait_for_owned_reap() {
        let backend = DisposableHttpServer::start().await;
        let tmp = tempfile::tempdir().unwrap();
        let binary = write_fake_opencode_binary(tmp.path());
        let _env = EnvGuard::new(&[
            ("CODEMUX_OPENCODE_BINARY", Some(binary.as_os_str())),
            (
                "FAKE_OPENCODE_URL",
                Some(std::ffi::OsStr::new(&backend.url)),
            ),
        ]);
        let manager = OpenCodeServerManager::new();
        manager.ensure_running().await.unwrap();
        let pid = manager.server.lock().await.as_ref().unwrap().pid().unwrap();
        let joiner = || async {
            manager.shutdown().await.unwrap();
            assert!(
                !std::path::Path::new(&format!("/proc/{pid}")).exists(),
                "Every shutdown caller must await the owned child's reap"
            );
        };
        let first = joiner();
        let second = joiner();
        tokio::pin!(first, second);
        if futures_util::poll!(first.as_mut()).is_pending() {
            // The first caller still owns teardown. Even an already-signaled
            // child cannot let a second caller skip its owner's awaited reap.
            assert!(futures_util::poll!(second.as_mut()).is_pending());
            tokio::join!(first, second);
        } else {
            second.await;
        }
        assert!(!manager.is_running_for_tests().await);
        backend.kill().await;
    }

    #[tokio::test]
    #[serial(opencode_path)]
    async fn terminal_shutdown_denies_startup_already_queued_on_lifecycle_lock() {
        let manager = OpenCodeServerManager::new();
        let guard = manager.server.lock().await;
        let start = manager.ensure_running();
        let shutdown = manager.shutdown();
        tokio::pin!(start, shutdown);
        assert!(futures_util::poll!(start.as_mut()).is_pending());
        assert!(futures_util::poll!(shutdown.as_mut()).is_pending());
        // A fresh request must reject without waiting behind lifecycle work.
        assert_eq!(
            manager.ensure_running().await.unwrap_err(),
            "opencode_manager_shutdown"
        );
        drop(guard);
        assert_eq!(start.await.unwrap_err(), "opencode_manager_shutdown");
        shutdown.await.unwrap();
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[serial(opencode_path)]
    async fn terminal_shutdown_denies_racing_cached_handle_publication() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        tokio::time::timeout(Duration::from_secs(5), async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let tmp = tempfile::tempdir().unwrap();
            let binary = write_fake_opencode_binary(tmp.path());
            let _env = EnvGuard::new(&[
                ("CODEMUX_OPENCODE_BINARY", Some(binary.as_os_str())),
                ("FAKE_OPENCODE_URL", Some(std::ffi::OsStr::new(&url))),
            ]);
            let manager = OpenCodeServerManager::new();
            manager.ensure_running().await.unwrap();
            let pid = manager.server.lock().await.as_ref().unwrap().pid().unwrap();
            let cached = manager.ensure_running();
            tokio::pin!(cached);
            let (mut peer, _) = tokio::select! {
                result = &mut cached => panic!("Probe completed before HTTP reply: {result:?}"),
                accepted = listener.accept() => accepted.unwrap(),
            };
            tokio::select! {
                result = &mut cached => panic!("Probe completed before fixture read: {result:?}"),
                _ = async {
                    let mut request = Vec::new();
                    while !request.ends_with(b"\r\n\r\n") {
                        request.push(peer.read_u8().await.unwrap());
                    }
                    assert!(request.starts_with(b"GET / HTTP/1.1\r\n"));
                } => {},
            }
            let shutdown = manager.shutdown();
            tokio::pin!(shutdown);
            assert!(futures_util::poll!(shutdown.as_mut()).is_pending());
            peer.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                .await
                .unwrap();
            let result = cached.await;
            shutdown.await.unwrap();
            assert_eq!(
                result.unwrap_err(),
                "opencode_manager_shutdown",
                "A cached probe must recheck terminal admission before returning its handle"
            );
            assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
        })
        .await
        .expect("Racing cached probe/shutdown must settle");
    }

    #[cfg(unix)]
    fn fixture_fifo(path: &std::path::Path) -> std::fs::File {
        use std::os::unix::{ffi::OsStrExt, fs::OpenOptionsExt};
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: a valid owned path; mkfifo only creates this fixture's FIFO.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
            .unwrap()
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[serial(opencode_path)]
    async fn terminal_shutdown_denies_racing_startup_publication() {
        use std::{
            io::{Read, Write},
            os::unix::fs::PermissionsExt,
        };
        tokio::time::timeout(Duration::from_secs(5), async {
            let tmp = tempfile::tempdir().unwrap();
            let started_path = tmp.path().join("started");
            let ready_path = tmp.path().join("ready");
            let started = tokio::io::unix::AsyncFd::new(fixture_fifo(&started_path)).unwrap();
            let mut ready = fixture_fifo(&ready_path);
            let binary = tmp.path().join("opencode");
            // The direct child blocks before its ready banner, without any
            // helper descendants or sleeps. FIFO acknowledgement owns ordering.
            std::fs::write(&binary, b"#!/bin/sh\nprintf '%s\\n' \"$$\" > \"$FAKE_OPENCODE_STARTED\"\nread ready < \"$FAKE_OPENCODE_READY\"\necho 'opencode server listening on http://127.0.0.1:1'\nexec /bin/sleep 600\n").unwrap();
            std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
            let _env = EnvGuard::new(&[
                ("CODEMUX_OPENCODE_BINARY", Some(binary.as_os_str())),
                ("FAKE_OPENCODE_STARTED", Some(started_path.as_os_str())),
                ("FAKE_OPENCODE_READY", Some(ready_path.as_os_str())),
            ]);
            let manager = OpenCodeServerManager::new();
            let start = manager.ensure_running();
            tokio::pin!(start);
            let read_pid = async {
                let mut bytes = [0; 32];
                loop {
                    let mut readable = started.readable().await.unwrap();
                    match readable.try_io(|fd| fd.get_ref().read(&mut bytes)) {
                        Ok(Ok(count)) => break std::str::from_utf8(&bytes[..count]).unwrap().trim().parse::<u32>().unwrap(),
                        Ok(Err(error)) => panic!("fixture PID read: {error}"),
                        Err(_) => continue,
                    }
                }
            };
            let pid = tokio::select! {
                result = &mut start => panic!("Startup completed before fixture release: {result:?}"),
                pid = read_pid => pid,
            };
            let shutdown = manager.shutdown();
            tokio::pin!(shutdown);
            assert!(futures_util::poll!(shutdown.as_mut()).is_pending());
            ready.write_all(b"go\n").unwrap();
            let result = start.await;
            shutdown.await.unwrap();
            assert_eq!(result.unwrap_err(), "opencode_manager_shutdown", "An in-flight spawn must not publish after terminal shutdown");
            assert!(!manager.is_running_for_tests().await);
            assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
        }).await.expect("Racing startup/shutdown must settle");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[serial(opencode_path)]
    async fn cancelled_rejected_startup_retains_child_for_terminal_shutdown() {
        use std::{
            io::{Read, Write},
            os::unix::fs::PermissionsExt,
            sync::Arc,
        };
        // Failure-safe cleanup of only the direct fixture child. Do not mask
        // the ownership/reap observations made before this guard drops.
        struct FixtureChild(u32);
        impl Drop for FixtureChild {
            fn drop(&mut self) {
                unsafe {
                    if libc::waitpid(self.0 as i32, std::ptr::null_mut(), libc::WNOHANG) == 0 {
                        libc::kill(self.0 as i32, libc::SIGKILL);
                        libc::waitpid(self.0 as i32, std::ptr::null_mut(), 0);
                    }
                }
            }
        }
        tokio::time::timeout(Duration::from_secs(5), async {
            let tmp = tempfile::tempdir().unwrap();
            let started_path = tmp.path().join("started");
            let ready_path = tmp.path().join("ready");
            let started = tokio::io::unix::AsyncFd::new(fixture_fifo(&started_path)).unwrap();
            let mut ready = fixture_fifo(&ready_path);
            let binary = tmp.path().join("opencode");
            std::fs::write(&binary, b"#!/bin/sh\nprintf '%s\\n' \"$$\" > \"$FAKE_OPENCODE_STARTED\"\nread ready < \"$FAKE_OPENCODE_READY\"\necho 'opencode server listening on http://127.0.0.1:1'\nexec /bin/sleep 600\n").unwrap();
            std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
            let _env = EnvGuard::new(&[
                ("CODEMUX_OPENCODE_BINARY", Some(binary.as_os_str())),
                ("FAKE_OPENCODE_STARTED", Some(started_path.as_os_str())),
                ("FAKE_OPENCODE_READY", Some(ready_path.as_os_str())),
            ]);
            let manager = OpenCodeServerManager::new();
            let entered = Arc::new(tokio::sync::Notify::new());
            let release = Arc::new(tokio::sync::Notify::new());
            let mut start = Box::pin(super::super::server::SHUTDOWN_GATE.scope(
                (entered.clone(), release),
                manager.ensure_running(),
            ));
            let read_pid = async {
                let mut bytes = [0; 32];
                loop {
                    let mut readable = started.readable().await.unwrap();
                    match readable.try_io(|fd| fd.get_ref().read(&mut bytes)) {
                        Ok(Ok(count)) => break std::str::from_utf8(&bytes[..count]).unwrap().trim().parse::<u32>().unwrap(),
                        Ok(Err(error)) => panic!("fixture PID read: {error}"),
                        Err(_) => continue,
                    }
                }
            };
            let pid = tokio::select! {
                result = &mut start => panic!("Startup completed before ready release: {result:?}"),
                pid = read_pid => pid,
            };
            let _child = FixtureChild(pid);
            let mut shutdown = Box::pin(manager.shutdown());
            assert!(futures_util::poll!(shutdown.as_mut()).is_pending());
            ready.write_all(b"go\n").unwrap();
            tokio::select! {
                result = &mut start => panic!("Rejected cleanup bypassed scheduling gate: {result:?}"),
                _ = entered.notified() => {},
            }
            // Cancel the actual owned async future while production server
            // shutdown is pending, not just a borrowed pinned reference.
            drop(start);
            // Cancel the queued lock waiter as well so ownership can be
            // inspected before a fresh shutdown retry acquires the slot.
            drop(shutdown);
            let retained_pid = manager.server.lock().await.as_ref().and_then(OpenCodeServer::pid);
            let result = manager.shutdown().await;
            let reaped = !std::path::Path::new(&format!("/proc/{pid}")).exists();
            eprintln!("rejected-startup cancellation: owned_pid={pid}, retained_pid={retained_pid:?}, shutdown={result:?}, reaped={reaped}");
            assert_eq!(retained_pid, Some(pid), "Canceled rejected-publication cleanup must retain the ready child's owner; shutdown must not succeed on an empty slot");
            result.unwrap();
            assert!(reaped, "Terminal shutdown must await the retained direct child's reap");
            assert!(!manager.is_running_for_tests().await);
            assert_eq!(manager.ensure_running().await.unwrap_err(), "opencode_manager_shutdown");
        }).await.expect("Canceled rejected startup/shutdown must settle");
    }

    #[tokio::test]
    async fn terminal_shutdown_bounds_wait_for_inflight_lifecycle_work() {
        let mut manager = OpenCodeServerManager::new();
        manager.shutdown_lock_timeout = Duration::ZERO;
        let guard = manager.server.lock().await;
        let result = tokio::time::timeout(Duration::from_millis(100), manager.shutdown()).await;
        drop(guard);
        assert_eq!(
            result
                .expect("Shutdown must bound its own lifecycle lock wait")
                .unwrap_err(),
            "opencode_shutdown_lock_timeout"
        );
        // A timed-out shutdown is still terminal, and another caller can
        // retry the owned teardown once the lifecycle lock is available.
        manager.shutdown().await.unwrap();
        assert_eq!(
            manager.ensure_running().await.unwrap_err(),
            "opencode_manager_shutdown"
        );
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
