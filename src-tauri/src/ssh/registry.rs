//! Registry of live `TunnelSupervisor` instances, keyed by
//! workspace id.
//!
//! Lifetime:
//! - Created on first push (`workspace_push_to_host`) after the rsync
//!   succeeds. The supervisor immediately spawns the SSH tunnel and
//!   the remote `codemux-remote pty-daemon`.
//! - Reused on subsequent pushes / spawns for the same workspace.
//! - Torn down on `workspace_pull_back` or `close_workspace`.
//!
//! Why a global registry rather than per-workspace owned state:
//! the supervisor needs to outlive any single request (the SSH
//! tunnel persists across HTTP-like Tauri command boundaries), so
//! some long-lived holder is required. App-level state via
//! `tauri::Manager::manage` would also work but a `OnceCell`
//! sidecar keeps the supervisor module self-contained and avoids
//! touching every consumer's state plumbing.
//!
//! Concurrency: a `tokio::sync::Mutex<HashMap<...>>` is fine here —
//! lookups are infrequent (only at push / pull / shutdown) and the
//! critical section is tiny (single HashMap op).

#![cfg(unix)]

use crate::ssh::tunnel_supervisor::TunnelSupervisor;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, OnceCell};

static REGISTRY: OnceCell<Mutex<HashMap<String, Arc<TunnelSupervisor>>>> =
    OnceCell::const_new();

async fn registry() -> &'static Mutex<HashMap<String, Arc<TunnelSupervisor>>> {
    REGISTRY
        .get_or_init(|| async { Mutex::new(HashMap::new()) })
        .await
}

/// Register a freshly-spawned supervisor under the given workspace
/// id. If an existing supervisor is registered, it's gracefully
/// shut down before the new one takes its place — protects against
/// double-pushes leaking a tunnel.
pub async fn install_supervisor(
    workspace_id: &str,
    supervisor: Arc<TunnelSupervisor>,
) {
    let map = registry().await;
    let mut guard = map.lock().await;
    if let Some(prev) = guard.insert(workspace_id.to_string(), supervisor) {
        // Run shutdown in the background so install_supervisor stays
        // snappy — the new supervisor is already in the map and live.
        tokio::spawn(async move { prev.shutdown().await });
    }
}

/// Look up a supervisor by workspace id. Returns `None` when the
/// workspace is local or was never pushed.
pub async fn get_supervisor(
    workspace_id: &str,
) -> Option<Arc<TunnelSupervisor>> {
    let map = registry().await;
    let guard = map.lock().await;
    guard.get(workspace_id).cloned()
}

/// Stop and remove the supervisor for a workspace. Called on
/// pull-back and on workspace close. Idempotent — calling on a
/// workspace that never had a supervisor is a no-op.
pub async fn shutdown_supervisor(workspace_id: &str) {
    let map = registry().await;
    let supervisor = {
        let mut guard = map.lock().await;
        guard.remove(workspace_id)
    };
    if let Some(s) = supervisor {
        s.shutdown().await;
    }
}

/// Helper for the push flow: compose a stable local socket path
/// from the workspace id. Putting all tunnels under a single dir
/// keeps cleanup easy + avoids per-call temp-file allocation. The
/// hash-truncated workspace id stays well under Darwin's 104-byte
/// sun_path limit.
pub fn local_socket_for_workspace(workspace_id: &str) -> PathBuf {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    workspace_id.hash(&mut hasher);
    let short = format!("{:x}", hasher.finish());
    let truncated = &short[..short.len().min(12)];
    std::env::temp_dir().join(format!("codemux-tunnel-{truncated}.sock"))
}

/// Compute the conventional remote socket path for a workspace's
/// tunnel. Same id-hash truncation as the local side so the two
/// match up visually in process listings, and short enough to fit
/// macOS-server sun_path limits if anyone ever runs codemux-remote
/// on a Mac.
pub fn remote_socket_for_workspace(workspace_id: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    workspace_id.hash(&mut hasher);
    let short = format!("{:x}", hasher.finish());
    let truncated = &short[..short.len().min(12)];
    format!("/tmp/codemux-ptyd-{truncated}.sock")
}

/// Per-workspace `PtyDaemonClient` cache. Local workspaces share the
/// singleton local daemon client; remote workspaces each get their
/// own client connected through their per-workspace SSH tunnel.
///
/// Keyed by workspace_id. Entries are removed when the workspace's
/// supervisor is shut down (on pull-back or close).
static WORKSPACE_CLIENTS: OnceCell<
    Mutex<HashMap<String, std::sync::Arc<crate::pty_daemon::PtyDaemonClient>>>,
> = OnceCell::const_new();

async fn workspace_clients() -> &'static Mutex<
    HashMap<String, std::sync::Arc<crate::pty_daemon::PtyDaemonClient>>,
> {
    WORKSPACE_CLIENTS
        .get_or_init(|| async { Mutex::new(HashMap::new()) })
        .await
}

/// Resolve the `PtyDaemonClient` for a workspace given its host
/// assignment.
///
/// - `host_id = None`: returns the singleton local daemon client.
///   Cheap on every call thanks to its OnceCell.
/// - `host_id = Some(id)`: returns the per-workspace client
///   connected through the workspace's SSH tunnel. If no tunnel
///   exists yet (e.g. the workspace was restored from a snapshot
///   after an app restart and nobody has interacted with it
///   since), one is spawned lazily using the stored host's
///   ssh_target.
///
/// Waits up to `tunnel_wait` for a freshly-spawned tunnel to
/// become reachable. Returns a clean `PtyDaemonError::Daemon` on
/// any failure (missing host record, tunnel didn't come up,
/// supervisor circuit-broken) — callers fall back to the
/// in-process spawn path so the user still gets a working
/// terminal.
pub async fn client_for_workspace(
    app: &tauri::AppHandle,
    workspace_id: &str,
    host_id: Option<i64>,
) -> Result<
    std::sync::Arc<crate::pty_daemon::PtyDaemonClient>,
    crate::pty_daemon::PtyDaemonError,
> {
    use crate::pty_daemon::{ensure_daemon, PtyDaemonClient, PtyDaemonError};
    use std::time::{Duration, Instant};
    use tauri::Manager;

    // Local fast path.
    let Some(host_id) = host_id else {
        return ensure_daemon().await;
    };

    // Per-workspace cache.
    {
        let map = workspace_clients().await;
        let guard = map.lock().await;
        if let Some(client) = guard.get(workspace_id) {
            return Ok(client.clone());
        }
    }

    // Ensure a supervisor exists for this workspace. Lazy-create
    // for restored workspaces that had host_id persisted but no
    // active tunnel.
    let supervisor = match get_supervisor(workspace_id).await {
        Some(s) => s,
        None => {
            let db = app.state::<crate::database::DatabaseStore>();
            let host = db
                .list_hosts()
                .into_iter()
                .find(|h| h.id == host_id)
                .ok_or_else(|| {
                    PtyDaemonError::Daemon(format!(
                        "Workspace's host {host_id} is no longer in the local hosts list"
                    ))
                })?;
            let local_socket = local_socket_for_workspace(workspace_id);
            let remote_socket = remote_socket_for_workspace(workspace_id);
            let s = crate::ssh::TunnelSupervisor::spawn(
                host.ssh_target.clone(),
                remote_socket,
                local_socket,
                "codemux-remote".to_string(),
            );
            install_supervisor(workspace_id, s.clone()).await;
            s
        }
    };

    // Wait for the tunnel to become Connected (or fail loudly).
    let tunnel_wait = Duration::from_secs(15);
    let mut rx = supervisor.subscribe();
    let deadline = Instant::now() + tunnel_wait;
    loop {
        let status = rx.borrow().clone();
        use crate::ssh::TunnelStatus;
        match status {
            TunnelStatus::Connected { .. } => break,
            TunnelStatus::CircuitOpen { recent_failures } => {
                return Err(PtyDaemonError::Daemon(format!(
                    "tunnel circuit breaker open ({recent_failures} recent \
                     failures); push the workspace again to retry"
                )));
            }
            _ => {}
        }
        if Instant::now() >= deadline {
            return Err(PtyDaemonError::Daemon(format!(
                "tunnel for workspace {workspace_id} did not come up within {:?}",
                tunnel_wait
            )));
        }
        // Wait for next status change (with a short timeout so we
        // re-check the deadline periodically).
        let _ = tokio::time::timeout(
            Duration::from_millis(500),
            rx.changed(),
        )
        .await;
    }

    // Connect a fresh client to the tunneled local socket. The
    // client is already Arc-wrapped by its constructor — share
    // across all panes in this workspace via the cache.
    let client_arc = PtyDaemonClient::connect(supervisor.local_socket()).await?;
    {
        let map = workspace_clients().await;
        let mut guard = map.lock().await;
        guard.insert(workspace_id.to_string(), client_arc.clone());
    }
    Ok(client_arc)
}

/// Forget the cached client for a workspace. Called on pull-back
/// (workspace goes back to local) and on close, before the
/// supervisor itself shuts down.
pub async fn forget_workspace_client(workspace_id: &str) {
    let map = workspace_clients().await;
    let mut guard = map.lock().await;
    guard.remove(workspace_id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_socket_for_workspace_is_deterministic() {
        let a = local_socket_for_workspace("workspace-42");
        let b = local_socket_for_workspace("workspace-42");
        assert_eq!(a, b, "same workspace id must yield same socket path");
    }

    #[test]
    fn local_socket_for_workspace_distinguishes_workspaces() {
        let a = local_socket_for_workspace("workspace-42");
        let b = local_socket_for_workspace("workspace-43");
        assert_ne!(a, b, "different workspace ids must yield different paths");
    }

    #[test]
    fn local_socket_path_fits_sun_path_limit() {
        // Darwin's sun_path is 104 bytes; we should stay well under
        // that even when the system tempdir is longish (e.g.
        // /var/folders/... on macOS).
        let path = local_socket_for_workspace("workspace-with-long-name");
        let len = path.to_string_lossy().len();
        assert!(
            len < 100,
            "socket path is {len} bytes, must stay under 104 for Darwin: {}",
            path.display()
        );
    }
}
