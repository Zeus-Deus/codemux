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

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Mutex;

use super::discovery::check_opencode_availability;
use super::server::OpenCodeServer;

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

    /// Return a handle to the running server, spawning it on demand.
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
            return Ok(handle_from(existing));
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
}
