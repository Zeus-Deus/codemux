//! Embedded web remote-access server.
//!
//! Turns the desktop app into a second frontend for its own backend: a
//! browser on another device loads the same UI bundle and drives the same
//! running instance over HTTP + WebSocket. Default-off — nothing binds
//! until the user enables it, at which point an axum server comes up on
//! `0.0.0.0:<port>` (default 4377).
//!
//! Module layout:
//! - [`mod@self`] — managed state, config persistence, lifecycle, commands.
//! - [`server`]   — axum router, static assets, `/api/*`, `/ws`.
//! - [`auth`]     — pairing tokens, sessions, WS tickets, origin checks.
//! - [`dispatch`] — invoke dispatch via synthesized `on_message` + channels.
//! - [`events`]   — `listen_any` fan-out hub with per-event refcounting.
//! - [`endpoints`]— reachable-endpoint enumeration (loopback/LAN/tailnet).
//! - [`assets`]   — authenticated `/api/assets` file route (`convertFileSrc`).
//! - [`proxy`]    — auth-gated browser-pane WS + HTTP proxy to loopback daemons.
//!
//! See `docs/plans/web-remote-access.md` for the locked protocol contract.

pub mod assets;
pub mod auth;
pub mod dispatch;
pub mod endpoints;
pub mod events;
pub mod proxy;
pub mod server;

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

/// Default bind port. Chosen to be memorable and unlikely to clash.
pub const DEFAULT_PORT: u16 = 4377;
/// Settings key under which the config JSON is persisted.
const CONFIG_KEY: &str = "web_remote.config";
/// Global event both desktop and web UIs listen to for live updates.
const STATE_CHANGED_EVENT: &str = "web-remote-state-changed";
/// Global event a paired web client raises (via [`web_remote_request_update`])
/// to ask the desktop to run its update + restart flow. Only the desktop
/// frontend's updater hook listens for it.
const UPDATE_REQUESTED_EVENT: &str = "web-remote-update-requested";

/// Persisted, user-controlled configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebRemoteConfig {
    pub enabled: bool,
    pub port: u16,
    pub require_approval: bool,
}

impl Default for WebRemoteConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: DEFAULT_PORT,
            require_approval: false,
        }
    }
}

/// Desktop-updater availability, published by the desktop frontend's updater
/// hook (via [`web_remote_publish_update_available`]) so paired web clients can
/// surface a "desktop update available" prompt. Web clients have no real
/// updater plugin, so they learn about — and request — desktop updates entirely
/// through this snapshot + the `web-remote-update-requested` event. Purely
/// informational: the actual download + restart always runs on the desktop.
#[derive(Debug, Clone, Default)]
struct UpdateAvailability {
    available: bool,
    version: Option<String>,
}

/// A running server instance. Dropping / signalling `shutdown` unbinds it.
pub(crate) struct RunningServer {
    #[allow(dead_code)]
    port: u16,
    shutdown: tokio::sync::oneshot::Sender<()>,
}

/// Everything shared across the server, its handlers, and the commands.
/// Lives behind an `Arc` inside the managed [`WebRemoteState`] and is
/// constructed before the Tauri app exists (the channel interceptor in
/// `lib.rs` needs [`Shared::channels`] at `Builder` time).
pub(crate) struct Shared {
    pub config: Mutex<WebRemoteConfig>,
    pub runtime: Mutex<Option<RunningServer>>,
    pub pairing: auth::PairingStore,
    pub tickets: auth::TicketStore,
    pub rate: auth::RateLimiter,
    pub connections: server::ConnectionRegistry,
    pub channels: Arc<dispatch::ChannelRouter>,
    pub events: events::EventHub,
    /// Last desktop-update availability the frontend updater hook published.
    update: Mutex<UpdateAvailability>,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            config: Mutex::new(WebRemoteConfig::default()),
            runtime: Mutex::new(None),
            pairing: auth::PairingStore::default(),
            tickets: auth::TicketStore::default(),
            rate: auth::RateLimiter::default(),
            connections: server::ConnectionRegistry::default(),
            channels: Arc::new(dispatch::ChannelRouter::default()),
            events: events::EventHub::default(),
            update: Mutex::new(UpdateAvailability::default()),
        }
    }
}

/// Managed Tauri state handle for the web-remote server.
#[derive(Default)]
pub struct WebRemoteState {
    inner: Arc<Shared>,
}

impl WebRemoteState {
    /// The channel router the `lib.rs` channel interceptor drives. Cloned
    /// out before `.manage()` so the interceptor and the dispatcher share
    /// one routing table.
    pub fn channel_router(&self) -> Arc<dispatch::ChannelRouter> {
        self.inner.channels.clone()
    }

    pub(crate) fn shared(&self) -> Arc<Shared> {
        self.inner.clone()
    }
}

// ── Status snapshot (command return + event payload) ────────────────

/// Live server + device state. Returned by `web_remote_status` and
/// emitted as the `web-remote-state-changed` payload.
#[derive(Debug, Clone, Serialize)]
pub struct WebRemoteStatus {
    pub enabled: bool,
    pub running: bool,
    pub port: u16,
    pub require_approval: bool,
    /// Number of live WebSocket connections right now (raw socket count; a
    /// single device with two tabs counts twice).
    pub active_connections: usize,
    /// Number of distinct paired devices with at least one live WebSocket.
    /// This is the "remote sessions active" signal the desktop updater keys
    /// off to defer an auto-update restart while someone is connected
    /// remotely (frontend defer policy is Stage 3b).
    pub connected_sessions: usize,
    pub sessions: Vec<SessionView>,
    /// Whether the desktop's updater found an update ready to install. Set by
    /// the desktop frontend (the web client has no updater plugin) so a paired
    /// browser can offer a "desktop update available → update & restart" prompt.
    pub update_available: bool,
    /// Version string of the available desktop update, when `update_available`.
    pub update_version: Option<String>,
}

/// A paired device as shown in the desktop management UI.
#[derive(Debug, Clone, Serialize)]
pub struct SessionView {
    pub id: String,
    pub name: Option<String>,
    pub user_agent: Option<String>,
    pub created_at: String,
    pub last_seen_at: Option<String>,
    pub approved: bool,
    /// Has at least one live WebSocket right now.
    pub connected: bool,
}

/// Result of `web_remote_create_pairing`. QR rendering is the frontend's job.
#[derive(Debug, Clone, Serialize)]
pub struct PairingInfo {
    /// Relative link a browser opens to auto-pair: `/#pair=<token>`.
    pub url_path: String,
    pub token: String,
    pub expires_at: String,
}

fn build_status(app: &AppHandle, shared: &Arc<Shared>) -> WebRemoteStatus {
    let cfg = shared.config.lock().unwrap().clone();
    let running = shared.runtime.lock().unwrap().is_some();
    let active_connections = shared.connections.active_count();
    let sessions: Vec<SessionView> = app
        .state::<crate::database::DatabaseStore>()
        .web_remote_list_sessions()
        .into_iter()
        .map(|s| SessionView {
            connected: shared.connections.session_live(&s.id),
            id: s.id,
            name: s.name,
            user_agent: s.user_agent,
            created_at: s.created_at,
            last_seen_at: s.last_seen_at,
            approved: s.approved,
        })
        .collect();
    let connected_sessions = sessions.iter().filter(|s| s.connected).count();
    let update = shared.update.lock().unwrap().clone();
    WebRemoteStatus {
        enabled: cfg.enabled,
        running,
        port: cfg.port,
        require_approval: cfg.require_approval,
        active_connections,
        connected_sessions,
        sessions,
        update_available: update.available,
        update_version: update.version,
    }
}

/// Emit the current status on the global bus so desktop + web UIs
/// live-update on every server/session state change.
pub(crate) fn emit_state_changed(app: &AppHandle) {
    let shared = app.state::<WebRemoteState>().shared();
    let status = build_status(app, &shared);
    let _ = app.emit(STATE_CHANGED_EVENT, status);
}

// ── Config persistence ──────────────────────────────────────────────

fn persist_config(app: &AppHandle, shared: &Arc<Shared>) {
    let cfg = shared.config.lock().unwrap().clone();
    if let Ok(json) = serde_json::to_string(&cfg) {
        let _ = app
            .state::<crate::database::DatabaseStore>()
            .set_setting(CONFIG_KEY, &json);
    }
}

fn load_config(app: &AppHandle) -> WebRemoteConfig {
    app.state::<crate::database::DatabaseStore>()
        .get_setting(CONFIG_KEY)
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default()
}

// ── Server lifecycle ────────────────────────────────────────────────

async fn start_server(app: &AppHandle, shared: &Arc<Shared>) -> Result<(), String> {
    // Idempotent: already bound → nothing to do.
    if shared.runtime.lock().unwrap().is_some() {
        return Ok(());
    }
    let port = shared.config.lock().unwrap().port;
    let listener = server::bind(port).await?;
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let router = server::router(app.clone());

    tokio::spawn(async move {
        let service = router.into_make_service_with_connect_info::<std::net::SocketAddr>();
        let result = axum::serve(listener, service)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .await;
        if let Err(e) = result {
            eprintln!("[codemux::web_remote] server exited: {e}");
        }
    });

    *shared.runtime.lock().unwrap() = Some(RunningServer {
        port,
        shutdown: shutdown_tx,
    });
    Ok(())
}

fn stop_server(shared: &Arc<Shared>) {
    if let Some(running) = shared.runtime.lock().unwrap().take() {
        let _ = running.shutdown.send(());
    }
}

/// Load persisted config on boot and, if the feature was left enabled,
/// re-bind the server. Called once from the Tauri `setup` hook. A bind
/// failure (port taken) is logged, not fatal — the desktop still runs.
pub fn restore_on_boot(app: &AppHandle) {
    let shared = app.state::<WebRemoteState>().shared();
    let cfg = load_config(app);
    let enabled = cfg.enabled;
    *shared.config.lock().unwrap() = cfg;
    // In end-to-end test mode the dedicated `e2e_autostart` hook owns server
    // startup (it also mints the pairing token); starting here too would race
    // it on the bind. Yield to it.
    #[cfg(debug_assertions)]
    if std::env::var("CODEMUX_WEB_REMOTE_E2E").ok().as_deref() == Some("1") {
        return;
    }
    if !enabled {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let shared = app.state::<WebRemoteState>().shared();
        match start_server(&app, &shared).await {
            Ok(()) => emit_state_changed(&app),
            Err(e) => eprintln!("[codemux::web_remote] restore-on-boot bind failed: {e}"),
        }
    });
}

// ── Dev/test end-to-end autostart affordance ────────────────────────
//
// A single, self-contained hook that an automated end-to-end harness uses to
// drive the served web client without a human clicking inside the native
// window. It is compiled **only into debug builds** (the `debug_assertions`
// gate) and, even there, stays dormant unless `CODEMUX_WEB_REMOTE_E2E=1` is
// set in the environment — so it can never affect a shipped release.
//
// When both conditions hold, on app boot it:
//   1. enables the server on `CODEMUX_WEB_REMOTE_PORT` (default [`DEFAULT_PORT`]),
//   2. mints a one-time pairing token, and
//   3. writes the full pairing URL to the file named by
//      `CODEMUX_WEB_REMOTE_E2E_PAIRING_FILE` (created 0600 so the token is
//      readable only by the launching user).
//
// This is purely a harness convenience; the real pairing/enable flow is
// unchanged and remains behind the normal desktop UI.

/// Enable the server and publish a pairing URL for an automated harness. See
/// the module-section comment above. No-op unless `CODEMUX_WEB_REMOTE_E2E=1`.
#[cfg(debug_assertions)]
pub fn e2e_autostart(app: &AppHandle) {
    if std::env::var("CODEMUX_WEB_REMOTE_E2E").ok().as_deref() != Some("1") {
        return;
    }
    let port = std::env::var("CODEMUX_WEB_REMOTE_PORT")
        .ok()
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or(DEFAULT_PORT);
    let pairing_file = std::env::var("CODEMUX_WEB_REMOTE_E2E_PAIRING_FILE").ok();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let shared = app.state::<WebRemoteState>().shared();
        {
            let mut cfg = shared.config.lock().unwrap();
            cfg.enabled = true;
            cfg.port = port;
            // Deterministic pairing for the harness: approval off.
            cfg.require_approval = false;
        }
        persist_config(&app, &shared);
        if let Err(e) = start_server(&app, &shared).await {
            eprintln!("[codemux::web_remote] e2e autostart bind failed: {e}");
            return;
        }
        let (token, _ttl) = shared.pairing.issue();
        let url = format!("http://127.0.0.1:{port}/#pair={token}");
        if let Some(path) = pairing_file {
            if let Err(e) = write_pairing_file(&path, &url) {
                eprintln!("[codemux::web_remote] e2e autostart: writing pairing file failed: {e}");
            }
        }
        emit_state_changed(&app);
        eprintln!(
            "[codemux::web_remote] e2e autostart: server bound on {port}; pairing url published"
        );
    });
}

/// Write `url` to `path` with owner-only (0600) permissions on Unix.
#[cfg(debug_assertions)]
fn write_pairing_file(path: &str, url: &str) -> std::io::Result<()> {
    std::fs::write(path, format!("{url}\n"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

// ── Tauri commands ──────────────────────────────────────────────────

#[tauri::command]
pub fn web_remote_status(app: AppHandle) -> WebRemoteStatus {
    let shared = app.state::<WebRemoteState>().shared();
    build_status(&app, &shared)
}

#[tauri::command]
pub async fn web_remote_enable(app: AppHandle) -> Result<WebRemoteStatus, String> {
    let shared = app.state::<WebRemoteState>().shared();
    shared.config.lock().unwrap().enabled = true;
    persist_config(&app, &shared);
    start_server(&app, &shared).await?;
    emit_state_changed(&app);
    Ok(build_status(&app, &shared))
}

#[tauri::command]
pub fn web_remote_disable(app: AppHandle) -> Result<WebRemoteStatus, String> {
    let shared = app.state::<WebRemoteState>().shared();
    shared.config.lock().unwrap().enabled = false;
    persist_config(&app, &shared);
    stop_server(&shared);
    emit_state_changed(&app);
    Ok(build_status(&app, &shared))
}

#[tauri::command]
pub async fn web_remote_set_config(
    app: AppHandle,
    port: Option<u16>,
    require_approval: Option<bool>,
) -> Result<WebRemoteStatus, String> {
    let shared = app.state::<WebRemoteState>().shared();
    let port_changed = {
        let mut cfg = shared.config.lock().unwrap();
        let old_port = cfg.port;
        if let Some(p) = port {
            cfg.port = p;
        }
        if let Some(r) = require_approval {
            cfg.require_approval = r;
        }
        cfg.port != old_port
    };
    persist_config(&app, &shared);

    // A port change while running requires a rebind.
    let running = shared.runtime.lock().unwrap().is_some();
    if running && port_changed {
        stop_server(&shared);
        start_server(&app, &shared).await?;
    }
    emit_state_changed(&app);
    Ok(build_status(&app, &shared))
}

#[tauri::command]
pub fn web_remote_create_pairing(app: AppHandle) -> PairingInfo {
    let shared = app.state::<WebRemoteState>().shared();
    let (token, _ttl) = shared.pairing.issue();
    let expires_at = (chrono::Utc::now()
        + chrono::Duration::seconds(auth::PAIRING_TTL.as_secs() as i64))
    .to_rfc3339();
    PairingInfo {
        url_path: format!("/#pair={token}"),
        token,
        expires_at,
    }
}

#[tauri::command]
pub fn web_remote_list_endpoints(app: AppHandle) -> Vec<endpoints::Endpoint> {
    let shared = app.state::<WebRemoteState>().shared();
    let port = shared.config.lock().unwrap().port;
    endpoints::list(port)
}

#[tauri::command]
pub fn web_remote_list_sessions(app: AppHandle) -> Vec<SessionView> {
    let shared = app.state::<WebRemoteState>().shared();
    build_status(&app, &shared).sessions
}

#[tauri::command]
pub fn web_remote_revoke_session(app: AppHandle, session_id: String) -> Result<WebRemoteStatus, String> {
    let shared = app.state::<WebRemoteState>().shared();
    app.state::<crate::database::DatabaseStore>()
        .web_remote_revoke_session(&session_id)?;
    // Drop any live sockets belonging to the revoked device immediately.
    shared.connections.close_session(&session_id);
    emit_state_changed(&app);
    Ok(build_status(&app, &shared))
}

#[tauri::command]
pub fn web_remote_approve_session(
    app: AppHandle,
    session_id: String,
) -> Result<WebRemoteStatus, String> {
    let shared = app.state::<WebRemoteState>().shared();
    app.state::<crate::database::DatabaseStore>()
        .web_remote_set_session_approved(&session_id, true)?;
    emit_state_changed(&app);
    Ok(build_status(&app, &shared))
}

#[tauri::command]
pub fn web_remote_reject_session(
    app: AppHandle,
    session_id: String,
) -> Result<WebRemoteStatus, String> {
    let shared = app.state::<WebRemoteState>().shared();
    // Close live sockets first, then erase the pending row entirely.
    shared.connections.close_session(&session_id);
    app.state::<crate::database::DatabaseStore>()
        .web_remote_delete_session(&session_id)?;
    emit_state_changed(&app);
    Ok(build_status(&app, &shared))
}

// ── Update-while-remote bridge (Stage 3b) ───────────────────────────
//
// Desktop-update state lives in the desktop frontend's `useUpdateChecker`
// hook. These two commands are the seam that lets paired web clients — which
// have no updater plugin — see and drive it:
//
// - `web_remote_publish_update_available`: the DESKTOP updater hook pushes its
//   availability here; it rides out on `web-remote-state-changed` (and the
//   `web_remote_status` snapshot) so web clients live-update and late joiners
//   still see it.
// - `web_remote_request_update`: a WEB client calls it to ask the desktop to
//   run its standard download + restart flow (the desktop confirmation UX
//   still applies). The PTY daemon keeps agents alive across the restart; the
//   web client reconnects via the shim's backoff loop.

/// Publish the desktop updater's availability so paired web clients can offer a
/// "desktop update available" prompt. Called by the desktop frontend only.
#[tauri::command]
pub fn web_remote_publish_update_available(
    app: AppHandle,
    available: bool,
    version: Option<String>,
) {
    let shared = app.state::<WebRemoteState>().shared();
    {
        let mut update = shared.update.lock().unwrap();
        update.available = available;
        update.version = if available { version } else { None };
    }
    emit_state_changed(&app);
}

/// Ask the desktop to run its update + restart flow. Emits the global
/// `web-remote-update-requested` event the desktop updater hook listens for.
#[tauri::command]
pub fn web_remote_request_update(app: AppHandle) {
    let _ = app.emit(UPDATE_REQUESTED_EVENT, ());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_default_is_off_on_4377() {
        let cfg = WebRemoteConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.port, DEFAULT_PORT);
        assert!(!cfg.require_approval);
    }

    #[test]
    fn config_roundtrips_through_json() {
        let cfg = WebRemoteConfig {
            enabled: true,
            port: 5000,
            require_approval: true,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: WebRemoteConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.enabled, cfg.enabled);
        assert_eq!(back.port, cfg.port);
        assert_eq!(back.require_approval, cfg.require_approval);
    }

    #[test]
    fn status_exposes_connection_counts_in_snake_case() {
        // The desktop updater's defer-while-remote policy (Stage 3b) reads
        // both counts off this payload, so the wire contract must carry them.
        let status = WebRemoteStatus {
            enabled: true,
            running: true,
            port: DEFAULT_PORT,
            require_approval: false,
            active_connections: 3,
            connected_sessions: 2,
            sessions: vec![],
            update_available: false,
            update_version: None,
        };
        let v = serde_json::to_value(&status).unwrap();
        assert_eq!(v["active_connections"], 3);
        assert_eq!(v["connected_sessions"], 2);
        // No camelCase leakage.
        assert!(v.get("connectedSessions").is_none());
    }

    #[test]
    fn status_carries_desktop_update_availability_in_snake_case() {
        // The web client keys its "desktop update available" prompt off these
        // fields, so the wire contract must carry them in snake_case.
        let status = WebRemoteStatus {
            enabled: true,
            running: true,
            port: DEFAULT_PORT,
            require_approval: false,
            active_connections: 0,
            connected_sessions: 0,
            sessions: vec![],
            update_available: true,
            update_version: Some("1.2.3".to_string()),
        };
        let v = serde_json::to_value(&status).unwrap();
        assert_eq!(v["update_available"], true);
        assert_eq!(v["update_version"], "1.2.3");
        assert!(v.get("updateAvailable").is_none(), "no camelCase leakage");
    }

    #[test]
    fn publish_update_clears_version_when_unavailable() {
        // `available=false` must drop any stale version so a web client never
        // shows a phantom update after the desktop's update state clears.
        let mut update = UpdateAvailability {
            available: true,
            version: Some("9.9.9".to_string()),
        };
        // Mirror the command body's write path.
        update.available = false;
        update.version = if update.available {
            Some("9.9.9".to_string())
        } else {
            None
        };
        assert!(!update.available);
        assert_eq!(update.version, None);
    }

    #[test]
    fn connected_sessions_counts_only_live_devices() {
        let mk = |id: &str, connected: bool| SessionView {
            id: id.to_string(),
            name: None,
            user_agent: None,
            created_at: String::new(),
            last_seen_at: None,
            approved: true,
            connected,
        };
        let sessions = vec![mk("a", true), mk("b", false), mk("c", true)];
        let connected = sessions.iter().filter(|s| s.connected).count();
        assert_eq!(connected, 2, "only sessions with a live socket count");
    }

    #[test]
    fn shared_channel_router_is_shared_by_clone() {
        let state = WebRemoteState::default();
        let a = state.channel_router();
        let b = state.shared().channels.clone();
        assert!(Arc::ptr_eq(&a, &b), "interceptor + dispatcher share one router");
    }
}
