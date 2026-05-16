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
