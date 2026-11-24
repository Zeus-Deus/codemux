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

use crate::pty_daemon::{PtyDaemonClient, PtyDaemonError};
use crate::ssh::tunnel_supervisor::{TunnelStatus, TunnelSupervisor, FIRST_CONNECT_WAIT};
use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager};
use tokio::sync::{Mutex, OnceCell};

// Non-interactive SSH shells may omit ~/.local/bin from PATH.
const REMOTE_DAEMON_BINARY: &str = "$HOME/.local/bin/codemux-remote";

/// Payload for the `tunnel-status-changed` event the UI listens to so it
/// can render a "Reconnecting…" / "Connection lost" pill on remote
/// workspaces. Keyed by `workspace_id`; `status` is the supervisor's own
/// `TunnelStatus` (tagged enum: `connected` / `reconnecting` /
/// `circuit_open` / `pending`).
#[derive(Clone, serde::Serialize)]
struct TunnelStatusEvent {
    workspace_id: String,
    status: TunnelStatus,
}

/// Bridge a supervisor's status `watch` channel to a Tauri event so the
/// frontend can show tunnel health. Spawns one detached task per
/// supervisor instance; it emits the current status immediately, then on
/// every change, and **self-terminates** when the supervisor is dropped
/// (its `status_tx` closes → `changed()` errors). Because a re-push
/// installs a fresh supervisor (the old one is shut down and dropped),
/// the old forwarder exits on its own — no explicit deregistration, no
/// leak, and never two forwarders for one live supervisor.
pub fn spawn_tunnel_status_forwarder<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    workspace_id: String,
    supervisor: &Arc<TunnelSupervisor>,
) {
    let mut rx = supervisor.subscribe();
    tokio::spawn(async move {
        loop {
            let status = rx.borrow_and_update().clone();
            let _ = app.emit(
                "tunnel-status-changed",
                TunnelStatusEvent {
                    workspace_id: workspace_id.clone(),
                    status,
                },
            );
            if rx.changed().await.is_err() {
                // Supervisor dropped — stop forwarding.
                break;
            }
        }
    });
}

struct RegisteredTunnel {
    host_id: Option<i64>,
    supervisor: Arc<TunnelSupervisor>,
    client: Option<Arc<PtyDaemonClient>>,
}

impl RegisteredTunnel {
    fn matches_host(&self, host_id: i64, ssh_target: &str) -> bool {
        self.host_id == Some(host_id) && self.supervisor.ssh_target() == ssh_target
    }
}

type TunnelRoutes = HashMap<String, RegisteredTunnel>;
static REGISTRY: OnceCell<Mutex<TunnelRoutes>> = OnceCell::const_new();

async fn registry() -> &'static Mutex<TunnelRoutes> {
    REGISTRY.get_or_init(|| async { Mutex::new(HashMap::new()) }).await
}

fn workspace_host<R: tauri::Runtime>(app: &tauri::AppHandle<R>, workspace_id: &str) -> Option<Option<i64>> {
    app.state::<crate::state::AppStateStore>().with_canonical_snapshot(|snapshot| {
        snapshot.workspaces.iter()
            .find(|workspace| workspace.workspace_id.0 == workspace_id)
            .map(|workspace| workspace.host_id)
    })
}

fn ssh_target_for_host<R: tauri::Runtime>(app: &tauri::AppHandle<R>, host_id: i64) -> Option<String> {
    app.state::<crate::database::DatabaseStore>().list_hosts().into_iter()
        .find(|host| host.id == host_id)
        .map(|host| host.ssh_target)
}

fn host_changed() -> PtyDaemonError {
    PtyDaemonError::Daemon("Workspace host changed while opening the terminal tunnel".into())
}

/// Replace both supervisor and client as a single route, including when the
/// new private local socket cannot be allocated.
fn replace_route(
    routes: &mut TunnelRoutes,
    workspace_id: &str,
    host_id: i64,
    spawn: impl FnOnce() -> Result<Arc<TunnelSupervisor>, String>,
) -> (Option<RegisteredTunnel>, Result<Arc<TunnelSupervisor>, String>) {
    let previous = routes.remove(workspace_id);
    let next = spawn().map(|supervisor| {
        routes.insert(workspace_id.to_owned(), RegisteredTunnel {
            host_id: Some(host_id),
            supervisor: supervisor.clone(),
            client: None,
        });
        supervisor
    });
    (previous, next)
}

pub(crate) async fn replace_workspace_tunnel<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    workspace_id: &str,
    host_id: i64,
    ssh_target: &str,
    cleanup: impl Future<Output = ()> + Send + 'static,
) -> Result<(), String> {
    let (previous, next) = {
        let mut routes = registry().await.lock().await;
        if workspace_host(app, workspace_id) != Some(Some(host_id)) {
            return Err(host_changed().to_string());
        }
        replace_route(&mut routes, workspace_id, host_id, || {
            if ssh_target_for_host(app, host_id).as_deref() != Some(ssh_target) {
                return Err(host_changed().to_string());
            }
            TunnelSupervisor::spawn(
                ssh_target.to_owned(), workspace_id, REMOTE_DAEMON_BINARY.to_owned(),
            )
        })
    };
    if let Ok(supervisor) = &next {
        spawn_tunnel_status_forwarder(app.clone(), workspace_id.to_owned(), supervisor);
    }
    retire_route_after(previous, cleanup).await;
    next.map(|_| ())
}

/// Retire a registered route after an explicit host assignment change.
pub(crate) async fn discard_stale_workspace_tunnel<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    workspace_id: &str,
) {
    let stale = {
        let mut routes = registry().await.lock().await;
        let selected = workspace_host(app, workspace_id).flatten();
        let current = routes.get(workspace_id).is_some_and(|route| {
            selected.is_some_and(|id| {
                ssh_target_for_host(app, id).is_some_and(|target| route.matches_host(id, &target))
            })
        });
        if current { None } else { routes.remove(workspace_id) }
    };
    if let Some(stale) = stale {
        stale.supervisor.shutdown().await;
    }
}

/// Stop and remove the tunnel and cached client for a workspace.
pub async fn shutdown_supervisor(workspace_id: &str) {
    shutdown_supervisor_after(workspace_id, std::future::ready(())).await;
}

pub(crate) async fn shutdown_supervisor_after(
    workspace_id: &str,
    cleanup: impl Future<Output = ()> + Send + 'static,
) {
    let route = registry().await.lock().await.remove(workspace_id);
    retire_route_after(route, cleanup).await;
}

async fn retire_route_after(
    route: Option<RegisteredTunnel>,
    cleanup: impl Future<Output = ()> + Send + 'static,
) {
    // Removing the entry seals it for new admissions. Finish captured close
    // requests on its still-live socket. A canceled transfer cannot strand SSH.
    let _ = tokio::spawn(async move {
        cleanup.await;
        if let Some(route) = route {
            route.supervisor.shutdown().await;
        }
    }).await;
}

/// Stop every supervisor and its ssh process on application exit.
pub async fn shutdown_all() {
    let supervisors: Vec<_> = match REGISTRY.get() {
        Some(routes) => routes.lock().await.drain().map(|(_, route)| route.supervisor).collect(),
        None => return,
    };
    futures_util::future::join_all(supervisors.iter().map(|s| s.shutdown())).await;
}

/// A tunnel to the original owner of a shell after its workspace has moved.
/// Keeps its own client so it can never use the workspace's current host.
pub(crate) struct CleanupDaemonTunnel {
    key: String,
    client: Option<Arc<PtyDaemonClient>>,
}

impl CleanupDaemonTunnel {
    pub(crate) async fn start(workspace_id: &str, ssh_target: &str) -> Result<Self, PtyDaemonError> {
        let key = format!("terminal-cleanup:{}", uuid::Uuid::new_v4());
        let mut supervisors = registry().await.lock().await;
        // Register without an await after spawning so cancellation cannot
        // leave a live tunnel outside the app's shutdown registry.
        let supervisor = TunnelSupervisor::spawn(
            ssh_target.to_string(),
            workspace_id,
            REMOTE_DAEMON_BINARY.to_string(),
        ).map_err(PtyDaemonError::Daemon)?;
        supervisors.insert(key.clone(), RegisteredTunnel { host_id: None, supervisor, client: None });
        Ok(Self { key, client: None })
    }

    pub(crate) async fn client(
        &mut self,
        workspace_id: &str,
    ) -> Result<Arc<PtyDaemonClient>, PtyDaemonError> {
        if let Some(client) = &self.client {
            if !client.is_closed() {
                return Ok(client.clone());
            }
        }
        let supervisor = registry().await.lock().await.get(&self.key)
            .map(|route| route.supervisor.clone())
            .ok_or_else(|| PtyDaemonError::Daemon("terminal cleanup tunnel shut down".into()))?;
        let client = connect_tunnel_daemon(workspace_id, &supervisor).await?;
        self.client = Some(client.clone());
        Ok(client)
    }

    pub(crate) async fn shutdown(self) {
        shutdown_supervisor(&self.key).await;
    }
}

/// Resolve the `PtyDaemonClient` for a workspace given its host
/// assignment.
///
/// - `host_id = None`: returns the singleton live local daemon client,
///   reconnecting after its reader observes EOF.
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
pub async fn client_for_workspace<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    workspace_id: &str,
    host_id: Option<i64>,
) -> Result<Arc<PtyDaemonClient>, PtyDaemonError> {
    if workspace_host(app, workspace_id) != Some(host_id) {
        return Err(host_changed());
    }
    let Some(host_id) = host_id else {
        let client = crate::pty_daemon::ensure_daemon().await?;
        return if workspace_host(app, workspace_id) == Some(None) {
            Ok(client)
        } else {
            Err(host_changed())
        };
    };

    let (previous, supervisor, created, ssh_target) = {
        let mut routes = registry().await.lock().await;
        if workspace_host(app, workspace_id) != Some(Some(host_id)) {
            return Err(host_changed());
        }
        let Some(ssh_target) = ssh_target_for_host(app, host_id) else {
            drop(routes);
            discard_stale_workspace_tunnel(app, workspace_id).await;
            return Err(PtyDaemonError::Daemon(format!(
                "Workspace's host {host_id} is no longer in the local hosts list"
            )));
        };
        if let Some(route) = routes.get(workspace_id).filter(|route| route.matches_host(host_id, &ssh_target)) {
            if let Some(client) = route.client.as_ref().filter(|client| !client.is_closed()) {
                return Ok(client.clone());
            }
            (None, Ok(route.supervisor.clone()), false, ssh_target)
        } else {
            let (previous, next) = replace_route(&mut routes, workspace_id, host_id, || {
                TunnelSupervisor::spawn(
                    ssh_target.clone(), workspace_id, REMOTE_DAEMON_BINARY.to_owned(),
                )
            });
            (previous, next, true, ssh_target)
        }
    };
    if let Some(previous) = previous {
        previous.supervisor.shutdown().await;
    }
    let supervisor = supervisor.map_err(PtyDaemonError::Daemon)?;
    if created {
        spawn_tunnel_status_forwarder(app.clone(), workspace_id.to_owned(), &supervisor);
    }
    let client = connect_tunnel_daemon(workspace_id, &supervisor).await?;

    // A transfer can complete while the SSH connection is in flight. Only
    // the still-current route can publish a client back into the registry.
    let mut routes = registry().await.lock().await;
    if workspace_host(app, workspace_id) != Some(Some(host_id))
        || ssh_target_for_host(app, host_id).as_deref() != Some(ssh_target.as_str())
    {
        return Err(host_changed());
    }
    let current = routes.get_mut(workspace_id).filter(|route| Arc::ptr_eq(&route.supervisor, &supervisor))
        .ok_or_else(host_changed)?;
    current.client = Some(client.clone());
    Ok(client)
}

async fn connect_tunnel_daemon(
    workspace_id: &str,
    supervisor: &TunnelSupervisor,
) -> Result<Arc<PtyDaemonClient>, PtyDaemonError> {
    // Wait for the tunnel to become Connected (or fail loudly).
    // Connected fires after SSH binds the local forward and its authenticated
    // remote command confirms that the daemon socket is ready.
    let tunnel_wait = FIRST_CONNECT_WAIT;
    let mut rx = supervisor.subscribe();
    // Mark the initial subscribed value as seen so `rx.changed()`
    // wakes ONLY on subsequent updates. Without this, on a freshly
    // subscribed receiver, changed() can resolve immediately on
    // the current value, throwing off the wait timing. We then
    // do an explicit borrow_and_update at the top of every
    // iteration to pick up changes between awaits.
    let initial_status = rx.borrow_and_update().clone();
    let deadline = Instant::now() + tunnel_wait;
    crate::trace_cloud_push!(
        "[client_for_workspace:{workspace_id}] waiting for tunnel local-socket bind \
         (initial supervisor status: {initial_status:?})"
    );
    let mut iter: u32 = 0;
    loop {
        iter += 1;
        let status = rx.borrow_and_update().clone();
        // Log every iteration for the first few + every ~5s after
        // so the log doesn't drown but we see the polling alive.
        if iter <= 3 || iter % 10 == 0 {
            crate::trace_cloud_push!(
                "[client_for_workspace:{workspace_id}] poll iter={iter} status={status:?}"
            );
        }
        match status {
            TunnelStatus::Connected { ssh_pid } => {
                crate::trace_cloud_push!(
                    "[client_for_workspace:{workspace_id}] tunnel local-socket bound, ssh_pid={ssh_pid}"
                );
                break;
            }
            TunnelStatus::CircuitOpen { recent_failures } => {
                // The supervisor keeps retrying in the background; a later
                // call succeeds once it reports Connected again.
                return Err(PtyDaemonError::Daemon(format!(
                    "tunnel to the host is down ({recent_failures} recent \
                     failures); retrying automatically every minute"
                )));
            }
            _ => {}
        }
        if Instant::now() >= deadline {
            return Err(PtyDaemonError::Daemon(format!(
                "tunnel for workspace {workspace_id} did not come up within {:?} \
                 (check Settings → Hosts → Test connection)",
                tunnel_wait
            )));
        }
        // Wait for next status change (with a short timeout so we
        // re-check the deadline periodically).
        let _ = tokio::time::timeout(Duration::from_millis(500), rx.changed()).await;
    }

    // Connect + Hello with retry. SSH -L can have the local socket
    // bound before the remote daemon is ready to accept (esp. for
    // a cold-start of a multi-hundred-MB debug binary, or any
    // first-run after a fresh install). Connect attempts during
    // that gap fail silently with EOF or "connection refused".
    // Retry every 500ms for up to 20s.
    //
    // Without this, the first spawn after a fresh push would
    // ~always fail because the daemon takes 1-5s to come up but
    // we'd try to connect immediately.
    let connect_deadline = Instant::now() + Duration::from_secs(20);
    // The compiler can't see that this is read on the timeout
    // branch (it sees per-iteration overwrites without a read in
    // between for the happy path) — silence the warning.
    #[allow(unused_assignments)]
    let mut last_err: Option<String> = None;
    loop {
        if Instant::now() >= connect_deadline {
            return Err(PtyDaemonError::Daemon(format!(
                "tunnel up but remote daemon never responded after 20s \
                 (last error: {}). The remote codemux-remote binary may have \
                 failed to start — try Test connection in Settings → Hosts.",
                last_err.unwrap_or_else(|| "unknown".into())
            )));
        }
        let remaining = connect_deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(
            remaining,
            PtyDaemonClient::connect(supervisor.local_socket()),
        )
        .await
        {
            Ok(Ok(c)) => {
                // Connection accepted — but verify the daemon is
                // actually responsive by doing a Hello round-trip.
                // SSH -L's connection-refused → succeed-then-EOF
                // semantics means a successful connect() doesn't
                // prove the remote side is healthy.
                let remaining = connect_deadline.saturating_duration_since(Instant::now());
                match tokio::time::timeout(remaining, c.hello_with_host_plugin()).await {
                    Ok(Ok(((pid, version, _), host_claude_plugin))) => {
                        c.set_ssh_target(supervisor.ssh_target());
                        eprintln!(
                            "[client_for_workspace:{workspace_id}] daemon reached: \
                             pid={pid} version={version}"
                        );
                        // A daemon that kept running across an upgrade is
                        // reused, so its new shells keep its old behaviour.
                        if !host_claude_plugin {
                            log::warn!(
                                "[codemux::ssh] the host's terminal daemon (codemux-remote {version}, pid {pid}) \
                                 has no Claude Code plugin (it predates per-terminal Codemux tools, or could not \
                                 write the plugin): Claude Code in its new terminals gets no Codemux MCP server \
                                 until that daemon restarts (the host reboots, or about an hour after its last \
                                 Codemux terminal closes)"
                            );
                        }
                        return Ok(c);
                    }
                    Ok(Err(error)) => {
                        last_err = Some(format!("hello: {error}"));
                    }
                    Err(_) => last_err = Some("hello: overall 20s deadline elapsed".into()),
                }
            }
            Ok(Err(error)) => {
                last_err = Some(format!("connect: {error}"));
            }
            Err(_) => last_err = Some("connect: overall 20s deadline elapsed".into()),
        }
        if Instant::now() >= connect_deadline {
            return Err(PtyDaemonError::Daemon(format!(
                "tunnel up but remote daemon never responded after 20s \
                 (last error: {}). The remote codemux-remote binary may have \
                 failed to start — try Test connection in Settings → Hosts.",
                last_err.unwrap_or_else(|| "unknown".into())
            )));
        }
        let remaining = connect_deadline.saturating_duration_since(Instant::now());
        tokio::time::sleep(Duration::from_millis(500).min(remaining)).await;
    }
}

/// Forget the cached client for a workspace. Called on pull-back
/// (workspace goes back to local) and on close, before the
/// supervisor itself shuts down.
pub async fn forget_workspace_client(workspace_id: &str) {
    if let Some(route) = registry().await.lock().await.get_mut(workspace_id) {
        route.client = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn host_change_discards_previous_route_even_when_socket_allocation_fails() {
        // An option-like target is rejected before the supervisor invokes
        // discovery; the test never connects to an SSH host.
        let old = TunnelSupervisor::spawn(
            "-old-fixture".into(),
            "workspace",
            REMOTE_DAEMON_BINARY.into(),
        )
        .unwrap();
        let mut routes = TunnelRoutes::new();
        let (_, installed) = replace_route(&mut routes, "workspace", 1, || Ok(old.clone()));
        assert!(installed.is_ok());
        assert!(routes["workspace"].matches_host(1, "-old-fixture"));
        assert!(!routes["workspace"].matches_host(2, "-old-fixture"));
        assert!(!routes["workspace"].matches_host(1, "-other-fixture"));

        let (retired, replacement) = replace_route(&mut routes, "workspace", 2, || {
            Err("fixture socket allocation failed".into())
        });
        assert!(replacement.is_err());
        assert!(!routes.contains_key("workspace"));
        let retired = retired.unwrap();
        assert!(Arc::ptr_eq(&retired.supervisor, &old));
        retired.supervisor.shutdown().await;
    }
}
