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
//! - [`account`]  — account-mode admission: verify the browser owns the same
//!   Codemux account the desktop is signed into, then mint a session.
//! - [`dispatch`] — invoke dispatch via synthesized `on_message` + channels.
//! - [`events`]   — `listen_any` fan-out hub with per-event refcounting.
//! - [`iroh`]     — parallel from-anywhere transport (default-off): the `/ws`
//!   protocol carried inside an E2E-encrypted iroh QUIC bi-stream.
//! - [`endpoints`]— reachable-endpoint enumeration (loopback/LAN/tailnet).
//! - [`assets`]   — authenticated `/api/assets` file route (`convertFileSrc`).
//! - [`proxy`]    — auth-gated browser-pane WS + HTTP proxy to loopback daemons.
//! - [`snapshot`] — authenticated `/api/snapshot` bulk state bootstrap (versioned API).
//!
//! See `docs/plans/web-remote-access.md` for the locked protocol contract.

pub mod account;
pub mod assets;
pub mod auth;
pub mod dispatch;
pub mod endpoints;
pub mod events;
pub mod iroh;
pub mod proxy;
pub mod registration;
pub mod serve;
pub mod server;
pub mod snapshot;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, Runtime};

/// Default bind port. Chosen to be memorable and unlikely to clash.
pub const DEFAULT_PORT: u16 = 4377;
/// Settings key under which the config JSON is persisted.
const CONFIG_KEY: &str = "web_remote.config";

// ── Bind scope (which interfaces the server listens on) ──────────────
//
// The default (`all`) binds `0.0.0.0` — every interface — so any LAN or
// tailnet peer can reach the server. On a hostile network that also exposes
// the port on the untrusted segment. The other two scopes narrow the
// exposure so the port simply isn't open on interfaces the user doesn't
// trust:
//   - `tailscale` — the tailnet address(es) plus loopback, so the port is
//     reachable only over the mesh VPN (and locally), never on the LAN.
//   - `loopback`  — `127.0.0.1` only; reachable only from this machine
//     (e.g. tunnelled in over SSH).

/// All interfaces (`0.0.0.0`) — the default, historical behavior.
pub const BIND_SCOPE_ALL: &str = "all";
/// Tailnet address(es) + loopback only.
pub const BIND_SCOPE_TAILSCALE: &str = "tailscale";
/// Loopback (`127.0.0.1`) only.
pub const BIND_SCOPE_LOOPBACK: &str = "loopback";

fn default_bind_scope() -> String {
    BIND_SCOPE_ALL.to_string()
}

/// Whether `scope` is one of the three recognised bind scopes.
fn is_valid_bind_scope(scope: &str) -> bool {
    matches!(
        scope,
        BIND_SCOPE_ALL | BIND_SCOPE_TAILSCALE | BIND_SCOPE_LOOPBACK
    )
}
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
    /// Which interfaces to bind: `all` (0.0.0.0) | `tailscale` | `loopback`.
    /// `#[serde(default)]` so a config persisted before this field existed
    /// deserializes to the historical `all` behavior instead of failing.
    #[serde(default = "default_bind_scope")]
    pub bind_scope: String,
    /// Master toggle for account mode (Stage A): when off, `POST
    /// /api/pair-account` refuses with 403 and a browser cannot mint a session
    /// by signing into the desktop's Codemux account. Default **off**, mirroring
    /// the base feature's default-off posture — account compromise must never
    /// reach a device that never opted in. `#[serde(default)]` so a config
    /// persisted before this field existed loads as `false`.
    #[serde(default)]
    pub account_mode_enabled: bool,
    /// Approval opt-out for account-minted sessions. Account sessions default to
    /// **pending approval** even when `require_approval` is off (a desktop-side
    /// approve click is a cheap circuit-breaker against a compromised account);
    /// setting this to `true` is the explicit "trust browsers on my account
    /// without approval" opt-out that admits them immediately. `#[serde(default)]`
    /// so a legacy config loads as `false` (approval on).
    #[serde(default)]
    pub trust_account_browsers: bool,
    /// Master toggle for the from-anywhere **iroh transport** (Stage C). When on,
    /// the desktop binds an iroh endpoint so a browser can reach it by `node_id`
    /// over an E2E-encrypted QUIC stream (see [`iroh`]). Strictly additive and
    /// **default off** — the axum `/ws` transport is unchanged and stays the
    /// default; iroh runs in parallel only while the feature is enabled *and*
    /// this flag is set. `#[serde(default)]` so a config persisted before this
    /// field existed loads as `false`.
    #[serde(default)]
    pub relay_mode_enabled: bool,
}

impl Default for WebRemoteConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: DEFAULT_PORT,
            require_approval: false,
            bind_scope: default_bind_scope(),
            account_mode_enabled: false,
            trust_account_browsers: false,
            relay_mode_enabled: false,
        }
    }
}

/// Whether an account-minted session should be admitted immediately (approved)
/// or start pending. Account sessions default to pending even when the global
/// pairing `require_approval` is off; only the explicit "trust browsers on my
/// account" opt-out (`trust_account_browsers`) admits them without a click.
/// Pure so it is directly unit-testable.
pub(crate) fn account_session_approved(cfg: &WebRemoteConfig) -> bool {
    cfg.trust_account_browsers
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

/// A running server instance. Sending `true` on `shutdown` unbinds every
/// listener it owns (one per bound address). `port` and `scope` are kept for
/// diagnostics / introspection.
pub(crate) struct RunningServer {
    #[allow(dead_code)]
    port: u16,
    #[allow(dead_code)]
    scope: String,
    /// Flips to `true` to trigger graceful shutdown on all listeners.
    shutdown: tokio::sync::watch::Sender<bool>,
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
    /// The from-anywhere iroh transport's endpoint lifecycle (default-off,
    /// gated by `relay_mode_enabled`). Shares this same `Shared` so iroh
    /// bi-streams register in the connection registry above.
    pub iroh: iroh::IrohManager,
    /// Device registration with the account control plane (default-off, started
    /// in lockstep with the iroh endpoint when relay mode is on and the desktop
    /// is signed in). Holds the periodic `lastSeenAt` refresh task + last status.
    pub registration: registration::RegistrationManager,
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
            iroh: iroh::IrohManager::default(),
            registration: registration::RegistrationManager::default(),
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
    /// Which interfaces the server binds: `all` | `tailscale` | `loopback`.
    /// Surfaced so the Settings pane can render the current access scope and
    /// so a web client sees which scope is in effect.
    pub bind_scope: String,
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
    /// Account mode (Stage A) master toggle. When on, a browser on a reachable
    /// endpoint can sign in with the desktop's Codemux account via
    /// `POST /api/pair-account` instead of pasting a pairing code.
    pub account_mode_enabled: bool,
    /// The "trust browsers on my account without approval" opt-out. When off
    /// (default), account-minted sessions start pending approval regardless of
    /// `require_approval`; when on, they connect immediately.
    pub trust_account_browsers: bool,
    /// Whether the desktop is currently signed into a Codemux account. Account
    /// mode can only verify "same account" while the desktop is signed in, so
    /// the Settings pane surfaces this to explain why account mode is inert when
    /// signed out, and the browser login screen can degrade gracefully.
    pub account_signed_in: bool,
    /// Master toggle for the from-anywhere iroh transport (Stage C). When on and
    /// the feature is enabled, the desktop is reachable by `node_id` over iroh.
    pub relay_mode_enabled: bool,
    /// The device's stable iroh `node_id` (a browser dials this), when relay
    /// mode has ever been enabled (the identity key is persisted). `None`
    /// otherwise. This is the address a later stage registers with the control
    /// plane; here it is surfaced so the desktop can display/copy it.
    pub iroh_node_id: Option<String>,
    /// Whether this desktop is currently registered with the account device
    /// registry, so a browser signed into the same account can discover it by
    /// `node_id`. Only meaningful while relay mode is on and the desktop is
    /// signed in; `false` when signed out or the registry is unreachable.
    pub device_registered: bool,
    /// The stable device id this desktop registers under, once a registration
    /// attempt has run. `None` before then.
    pub device_id: Option<String>,
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
    /// How the device was admitted: `"pair"` (pairing token) or `"account"`
    /// (signed into the desktop's Codemux account). Lets the device list tag
    /// account-minted devices distinctly from paired ones.
    pub source: String,
}

/// Result of `web_remote_create_pairing`. QR rendering is the frontend's job.
#[derive(Debug, Clone, Serialize)]
pub struct PairingInfo {
    /// Relative link a browser opens to auto-pair: `/#pair=<token>`.
    pub url_path: String,
    pub token: String,
    pub expires_at: String,
}

/// Mint a one-time pairing token and build the [`PairingInfo`] payload,
/// optionally carrying a suggested device name (used as a fallback label
/// when the connecting client sends none). The single shared token path —
/// the `web_remote_create_pairing` Tauri command and the `web_remote_pair`
/// control-socket command both go through here, so there is exactly one
/// place tokens are minted.
pub(crate) fn mint_pairing(shared: &Arc<Shared>, suggested_name: Option<String>) -> PairingInfo {
    let (token, _ttl) = shared.pairing.issue_named(suggested_name);
    let expires_at = (chrono::Utc::now()
        + chrono::Duration::seconds(auth::PAIRING_TTL.as_secs() as i64))
    .to_rfc3339();
    PairingInfo {
        url_path: format!("/#pair={token}"),
        token,
        expires_at,
    }
}

/// Result of the `web_remote_pair` control-socket command (the
/// `codemux remote pair` CLI over SSH): a freshly minted pairing token plus
/// the recommended endpoint's full pairing URL, so the terminal can print a
/// scannable link + QR without the desktop GUI ever being opened.
#[derive(Debug, Clone, Serialize)]
pub struct ControlPairing {
    /// Full URL a browser opens to auto-pair, on the recommended endpoint:
    /// `http://<host>:<port>/#pair=<token>`.
    pub pairing_url: String,
    /// The recommended endpoint's host (IP literal or MagicDNS name).
    pub host: String,
    pub port: u16,
    /// The raw pairing token (also embedded in `pairing_url`).
    pub token: String,
    pub expires_at: String,
    /// Whether the recommended endpoint is a browser secure context
    /// (loopback only). Informational for the CLI.
    pub secure: bool,
    /// The endpoint kind backing `host` (`magicdns` | `tailnet` | `lan` |
    /// `loopback`), so the CLI can hint how far it reaches.
    pub endpoint_kind: String,
}

/// Pure core of [`control_pair`], taking the shared state directly so it is
/// unit-testable without a Tauri `AppHandle`. Errors when remote access is
/// not enabled — the server must be bound for the URL to be reachable.
pub(crate) fn control_pair_from(
    shared: &Arc<Shared>,
    suggested_name: Option<String>,
) -> Result<ControlPairing, String> {
    let (enabled, port) = {
        let cfg = shared.config.lock().unwrap();
        (cfg.enabled, cfg.port)
    };
    if !enabled {
        return Err("Remote access is not enabled — enable it in Settings first".to_string());
    }
    let info = mint_pairing(shared, suggested_name);
    // Reuse the endpoint enumeration + its single `recommended` pick so the
    // CLI's URL matches what the Settings pane would surface. Fall back to
    // the first endpoint (loopback is always first) so a loopback-only host
    // still yields a usable local URL.
    let eps = endpoints::list(port);
    let chosen = eps
        .iter()
        .find(|e| e.recommended)
        .or_else(|| eps.first())
        .cloned()
        .ok_or_else(|| "No reachable endpoint found".to_string())?;
    Ok(ControlPairing {
        pairing_url: format!("{}{}", chosen.url, info.url_path),
        host: chosen.host,
        port,
        token: info.token,
        expires_at: info.expires_at,
        secure: chosen.secure,
        endpoint_kind: chosen.kind,
    })
}

/// Mint a pairing token for the same-machine control socket (the
/// `codemux remote pair` CLI over SSH). Reuses the shared [`mint_pairing`]
/// token path and the endpoint enumeration, so the terminal flow and the
/// GUI flow are the same code underneath.
pub fn control_pair<R: Runtime>(
    app: &AppHandle<R>,
    suggested_name: Option<String>,
) -> Result<ControlPairing, String> {
    let shared = app.state::<WebRemoteState>().shared();
    control_pair_from(&shared, suggested_name)
}

/// Result of the `web_remote_enable` control-socket command (the
/// `codemux remote enable` CLI over SSH): the resulting live status plus the
/// recommended reachable endpoint, so the terminal can tell the user the port,
/// bind scope, and where to point `codemux remote pair` next.
#[derive(Debug, Clone, Serialize)]
pub struct ControlEnableResult {
    /// The live server + device status after the enable (or config change).
    pub status: WebRemoteStatus,
    /// The recommended endpoint's base URL (`http://<host>:<port>`) — the same
    /// single `recommended` pick the Settings pane surfaces (MagicDNS → tailnet
    /// → local network, falling back to loopback). `None` only if no endpoint
    /// could be enumerated at all.
    pub endpoint_url: Option<String>,
    /// The recommended endpoint's host (IP literal or MagicDNS name).
    pub endpoint_host: Option<String>,
    /// The recommended endpoint kind (`magicdns` | `tailnet` | `lan` |
    /// `loopback`), so the CLI can hint how far it reaches.
    pub endpoint_kind: Option<String>,
    /// Whether the recommended endpoint is a browser secure context (loopback).
    pub endpoint_secure: bool,
    /// True when the enable was a no-op because remote access was already on and
    /// no scope/port change was requested — so the CLI can say "already running".
    pub already_running: bool,
}

/// Validate an enable request's scope and fold it (plus the port) into `cfg`,
/// flipping `enabled` on. Pure so the config side of [`control_enable`] is
/// unit-testable without an `AppHandle`. On an unknown scope it returns a clear
/// error and leaves `cfg` untouched (nothing is persisted).
fn apply_enable_request(
    cfg: &mut WebRemoteConfig,
    scope: Option<String>,
    port: Option<u16>,
) -> Result<(), String> {
    if let Some(ref s) = scope {
        if !is_valid_bind_scope(s) {
            return Err(format!("Unknown access scope: {s}"));
        }
    }
    cfg.enabled = true;
    if let Some(p) = port {
        cfg.port = p;
    }
    if let Some(s) = scope {
        cfg.bind_scope = s;
    }
    Ok(())
}

/// The recommended reachable endpoint for `port`: the single `recommended` pick
/// from the endpoint enumeration, falling back to the first entry (loopback is
/// always present) so a loopback-only host still yields a usable local URL.
/// Mirrors [`control_pair_from`]'s endpoint choice.
fn recommended_endpoint(port: u16) -> Option<endpoints::Endpoint> {
    let eps = endpoints::list(port);
    eps.iter()
        .find(|e| e.recommended)
        .or_else(|| eps.first())
        .cloned()
}

/// Enable web remote access from the same-machine control socket (the
/// `codemux remote enable` CLI over SSH). Shares the exact bind/rollback and
/// `web-remote-state-changed` emission paths the Tauri commands use:
///
/// - Off → on: fold the requested `scope`/`port` into config, then run the same
///   [`enable_core`] path (rolling `enabled` — and the scope/port — back on a
///   bind failure such as `tailscale` with no tailnet address).
/// - Already on with a `scope`/`port` flag: treat it as a config change and
///   rebind through the same [`set_config_core`] path the Settings pane uses.
/// - Already on with no flags: report it's already running with current status.
pub async fn control_enable<R: Runtime>(
    app: &AppHandle<R>,
    scope: Option<String>,
    port: Option<u16>,
) -> Result<ControlEnableResult, String> {
    let shared = app.state::<WebRemoteState>().shared();
    let already_enabled = shared.config.lock().unwrap().enabled;
    let already_bound = shared.runtime.lock().unwrap().is_some();

    let (status, already_running) = if already_enabled {
        if scope.is_some() || port.is_some() {
            // A scope/port change while running rebinds via the same path a
            // port change from the Settings pane uses (drops existing sockets).
            let status =
                set_config_core(app, &shared, port, None, scope, None, None, None).await?;
            (status, false)
        } else if already_bound {
            (build_status(app, &shared), true)
        } else {
            // The persisted switch can be on while no listener exists: most
            // notably headless serve mode deliberately leaves boot-time bind
            // restoration to its awaited startup path, and the GUI can also
            // reach this state after a prior restore failure. Treat the live
            // runtime as authoritative and actually bind instead of reporting
            // a config bit as "already running".
            (enable_core(app, &shared).await?, false)
        }
    } else {
        // Was off. Capture the last-good scope/port so a failed bind (e.g.
        // `--scope tailscale` with no tailnet) restores them rather than
        // leaving a bad scope persisted with the server off.
        let (old_port, old_scope) = {
            let cfg = shared.config.lock().unwrap();
            (cfg.port, cfg.bind_scope.clone())
        };
        {
            let mut cfg = shared.config.lock().unwrap();
            apply_enable_request(&mut cfg, scope, port)?;
        }
        match enable_core(app, &shared).await {
            Ok(status) => (status, false),
            Err(e) => {
                // `enable_core` already rolled `enabled` back to false; restore
                // the previous scope/port too and persist the last-good config.
                {
                    let mut cfg = shared.config.lock().unwrap();
                    cfg.port = old_port;
                    cfg.bind_scope = old_scope;
                }
                persist_config(app, &shared);
                return Err(e);
            }
        }
    };

    let ep = recommended_endpoint(status.port);
    Ok(ControlEnableResult {
        endpoint_url: ep.as_ref().map(|e| e.url.clone()),
        endpoint_host: ep.as_ref().map(|e| e.host.clone()),
        endpoint_kind: ep.as_ref().map(|e| e.kind.clone()),
        endpoint_secure: ep.as_ref().map(|e| e.secure).unwrap_or(false),
        status,
        already_running,
    })
}

/// Disable web remote access from the same-machine control socket (the
/// `codemux remote disable` CLI over SSH). Shares the exact [`disable_core`]
/// path the Tauri command uses: severs live sockets, tears the iroh endpoint
/// down, stops registration, and emits `web-remote-state-changed`.
pub fn control_disable<R: Runtime>(app: &AppHandle<R>) -> Result<WebRemoteStatus, String> {
    let shared = app.state::<WebRemoteState>().shared();
    disable_core(app, &shared)
}

fn build_status<R: Runtime>(app: &AppHandle<R>, shared: &Arc<Shared>) -> WebRemoteStatus {
    let cfg = shared.config.lock().unwrap().clone();
    let running = shared.runtime.lock().unwrap().is_some();
    let active_connections = shared.connections.active_count();
    let db = app.state::<crate::database::DatabaseStore>();
    let sessions: Vec<SessionView> = db
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
            source: s.source,
        })
        .collect();
    let connected_sessions = sessions.iter().filter(|s| s.connected).count();
    // Account mode can only verify "same account" while the desktop holds a
    // cached account user, so surface that state alongside the toggles.
    let account_signed_in = crate::auth::load_cached_user(&db).is_some();
    let update = shared.update.lock().unwrap().clone();
    let iroh_node_id = shared.iroh.node_id();
    let registration = shared.registration.status();
    WebRemoteStatus {
        enabled: cfg.enabled,
        running,
        port: cfg.port,
        require_approval: cfg.require_approval,
        bind_scope: cfg.bind_scope.clone(),
        active_connections,
        connected_sessions,
        sessions,
        update_available: update.available,
        update_version: update.version,
        account_mode_enabled: cfg.account_mode_enabled,
        trust_account_browsers: cfg.trust_account_browsers,
        account_signed_in,
        relay_mode_enabled: cfg.relay_mode_enabled,
        iroh_node_id,
        device_registered: registration.registered,
        device_id: registration.device_id,
    }
}

/// Emit the current status on the global bus so desktop + web UIs
/// live-update on every server/session state change.
pub(crate) fn emit_state_changed<R: Runtime>(app: &AppHandle<R>) {
    let shared = app.state::<WebRemoteState>().shared();
    let status = build_status(app, &shared);
    let _ = app.emit(STATE_CHANGED_EVENT, status);
}

// ── Config persistence ──────────────────────────────────────────────

fn persist_config<R: Runtime>(app: &AppHandle<R>, shared: &Arc<Shared>) {
    let cfg = shared.config.lock().unwrap().clone();
    if let Ok(json) = serde_json::to_string(&cfg) {
        let _ = app
            .state::<crate::database::DatabaseStore>()
            .set_setting(CONFIG_KEY, &json);
    }
}

fn load_config<R: Runtime>(app: &AppHandle<R>) -> WebRemoteConfig {
    app.state::<crate::database::DatabaseStore>()
        .get_setting(CONFIG_KEY)
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default()
}

// ── Server lifecycle ────────────────────────────────────────────────

/// Resolve the socket addresses to bind for a given `scope` + `port`, taking
/// the discovered tailnet IPs as an argument so the decision is pure and
/// unit-testable. See [`bind_addrs`] for the production entry point.
///
/// - `loopback`  → `127.0.0.1:port` only.
/// - `tailscale` → every tailnet address **plus** loopback, so the port is
///   reachable over the mesh VPN and locally but not on the LAN. Errors when
///   `tailnet` is empty — it must NEVER silently fall back to all interfaces
///   (that would re-expose the port on the hostile network the scope exists
///   to hide from).
/// - `all` (and any unrecognised value) → `0.0.0.0:port` (every interface).
fn bind_addrs_from(scope: &str, port: u16, tailnet: Vec<IpAddr>) -> Result<Vec<SocketAddr>, String> {
    match scope {
        BIND_SCOPE_LOOPBACK => Ok(vec![SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)]),
        BIND_SCOPE_TAILSCALE => {
            if tailnet.is_empty() {
                return Err(
                    "No Tailscale address found — connect Tailscale or choose a different access scope"
                        .to_string(),
                );
            }
            // Keep loopback so local use (and an SSH tunnel) still works
            // alongside the tailnet address(es).
            let mut addrs = vec![SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)];
            for ip in tailnet {
                addrs.push(SocketAddr::new(ip, port));
            }
            Ok(addrs)
        }
        // `all` and any unknown value: bind every interface (historical
        // behavior). Unknown values are normalised away in `web_remote_set_config`.
        _ => Ok(vec![SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), port)]),
    }
}

/// Production wrapper over [`bind_addrs_from`] that discovers the tailnet IPs
/// via the endpoint enumeration (interface CGNAT range + `tailscale status`).
fn bind_addrs(scope: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
    bind_addrs_from(scope, port, endpoints::tailnet_ips())
}

async fn start_server<R: Runtime>(app: &AppHandle<R>, shared: &Arc<Shared>) -> Result<(), String> {
    // Idempotent: already bound → nothing to do.
    if shared.runtime.lock().unwrap().is_some() {
        return Ok(());
    }
    let (port, scope) = {
        let cfg = shared.config.lock().unwrap();
        (cfg.port, cfg.bind_scope.clone())
    };
    // Resolve the bind scope to concrete addresses first. A `tailscale` scope
    // with no tailnet address fails HERE, before anything binds, so the
    // caller (enable / rebind) surfaces the reason and leaves the server off.
    let addrs = bind_addrs(&scope, port)?;

    // Bind every address up front. If any bind fails, the listeners already
    // bound in `listeners` drop at the `?` and unbind, so we never leak a
    // half-open server on a partial failure.
    let mut listeners = Vec::with_capacity(addrs.len());
    for addr in &addrs {
        listeners.push(server::bind(*addr).await?);
    }

    // One shutdown signal shared by every listener's graceful-shutdown future.
    let (shutdown_tx, _shutdown_rx) = tokio::sync::watch::channel(false);
    for listener in listeners {
        let router = server::router(app.clone());
        let mut shutdown_rx = shutdown_tx.subscribe();
        tokio::spawn(async move {
            let service = router.into_make_service_with_connect_info::<std::net::SocketAddr>();
            let result = axum::serve(listener, service)
                .with_graceful_shutdown(async move {
                    // Resolves when `stop_server` flips the watch to `true`.
                    let _ = shutdown_rx.changed().await;
                })
                .await;
            if let Err(e) = result {
                eprintln!("[codemux::web_remote] server exited: {e}");
            }
        });
    }

    *shared.runtime.lock().unwrap() = Some(RunningServer {
        port,
        scope,
        shutdown: shutdown_tx,
    });
    Ok(())
}

fn stop_server(shared: &Arc<Shared>) {
    // Sever every live socket up front. `with_graceful_shutdown` only stops the
    // listener accepting *new* connections; an already-open WebSocket would
    // otherwise keep working (and keep full desktop control) until it happened
    // to reload. Disabling remote access is a security action, so kick every
    // connected device immediately — the same mechanism revocation uses.
    shared.connections.close_all();
    if let Some(running) = shared.runtime.lock().unwrap().take() {
        let _ = running.shutdown.send(true);
    }
}

/// Load persisted config on boot and, if the feature was left enabled,
/// re-bind the server. Called once from the Tauri `setup` hook. A bind
/// failure (port taken) is logged, not fatal — the desktop still runs.
pub fn restore_on_boot<R: Runtime>(app: &AppHandle<R>) {
    let shared = app.state::<WebRemoteState>().shared();
    let cfg = load_config(app);
    let enabled = cfg.enabled;
    *shared.config.lock().unwrap() = cfg;
    // `codemux serve` performs an awaited enable after the full headless app
    // has built so startup can report bind failures synchronously. Spawning
    // this normal GUI restore in parallel would race that enable and could
    // make serve print a pairing URL before any listener exists.
    if crate::app_mode(app) == crate::AppMode::ServeHeadless {
        return;
    }
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
    let relay_mode_enabled = shared.config.lock().unwrap().relay_mode_enabled;
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let shared = app.state::<WebRemoteState>().shared();
        match start_server(&app, &shared).await {
            Ok(()) => {
                // If the from-anywhere iroh transport was left enabled, bind it
                // too (best-effort; never blocks or fails the boot restore).
                if relay_mode_enabled {
                    if let Err(e) = iroh::start(&app, &shared).await {
                        eprintln!("[codemux::web_remote] restore-on-boot iroh bind failed: {e}");
                    }
                    // Re-register with the account device registry (best-effort;
                    // no-op when signed out or the registry is unreachable).
                    registration::start(&app, &shared);
                }
                emit_state_changed(&app);
            }
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
pub fn e2e_autostart<R: Runtime>(app: &AppHandle<R>) {
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
pub fn web_remote_status<R: Runtime>(app: AppHandle<R>) -> WebRemoteStatus {
    let shared = app.state::<WebRemoteState>().shared();
    build_status(&app, &shared)
}

/// Flip `enabled` on and, if binding fails, roll it back off — the shared body
/// of the `web_remote_enable` Tauri command and the `codemux remote enable`
/// control command, so both persist config, bind (with rollback), start the
/// parallel iroh transport when relay mode is on, and emit `web-remote-state-
/// changed` through the exact same path. Callers that want to change the scope
/// or port first mutate `shared.config`, then call this.
async fn enable_core<R: Runtime>(app: &AppHandle<R>, shared: &Arc<Shared>) -> Result<WebRemoteStatus, String> {
    shared.config.lock().unwrap().enabled = true;
    persist_config(app, shared);
    if let Err(e) = start_server(app, shared).await {
        // Binding failed — e.g. the `tailscale` scope with no tailnet
        // address, or the port is already taken. Roll the master switch back
        // off so the UI never shows "enabled but not running", persist that,
        // and surface the reason. The server stays off.
        shared.config.lock().unwrap().enabled = false;
        persist_config(app, shared);
        emit_state_changed(app);
        return Err(e);
    }
    // Bring the parallel iroh transport up too if relay mode was left enabled.
    // A failure here is logged but never fails the enable — iroh is strictly
    // additive over the primary `/ws` transport, which is already bound.
    if shared.config.lock().unwrap().relay_mode_enabled {
        if let Err(e) = iroh::start(app, shared).await {
            eprintln!("[codemux::web_remote] iroh transport enable failed: {e}");
        }
        // Register this device with the account control plane so an account
        // browser can discover it (best-effort; skips when signed out).
        registration::start(app, shared);
    }
    emit_state_changed(app);
    Ok(build_status(app, shared))
}

#[tauri::command]
pub async fn web_remote_enable<R: Runtime>(app: AppHandle<R>) -> Result<WebRemoteStatus, String> {
    let shared = app.state::<WebRemoteState>().shared();
    enable_core(&app, &shared).await
}

/// Flip `enabled` off in the config. The pure config-side of disable, shared by
/// the Tauri command and the control socket so both persist the same state.
fn mark_disabled(cfg: &mut WebRemoteConfig) {
    cfg.enabled = false;
}

/// Turn every remote transport off — the shared body of the `web_remote_disable`
/// Tauri command and the `codemux remote disable` control command.
fn disable_core<R: Runtime>(app: &AppHandle<R>, shared: &Arc<Shared>) -> Result<WebRemoteStatus, String> {
    mark_disabled(&mut shared.config.lock().unwrap());
    persist_config(app, shared);
    // The master switch is the kill for *every* remote transport: `stop_server`
    // severs all live sockets (iroh sessions included, via the shared registry's
    // `close_all`), then tear the iroh endpoint down so it stops accepting and
    // stop refreshing this device's registration.
    stop_server(shared);
    iroh::stop(shared);
    registration::stop(shared);
    emit_state_changed(app);
    Ok(build_status(app, shared))
}

#[tauri::command]
pub fn web_remote_disable<R: Runtime>(app: AppHandle<R>) -> Result<WebRemoteStatus, String> {
    let shared = app.state::<WebRemoteState>().shared();
    disable_core(&app, &shared)
}

#[tauri::command]
pub async fn web_remote_set_config<R: Runtime>(
    app: AppHandle<R>,
    port: Option<u16>,
    require_approval: Option<bool>,
    bind_scope: Option<String>,
    account_mode_enabled: Option<bool>,
    trust_account_browsers: Option<bool>,
    relay_mode_enabled: Option<bool>,
) -> Result<WebRemoteStatus, String> {
    let shared = app.state::<WebRemoteState>().shared();
    set_config_core(
        &app,
        &shared,
        port,
        require_approval,
        bind_scope,
        account_mode_enabled,
        trust_account_browsers,
        relay_mode_enabled,
    )
    .await
}

/// The shared body of [`web_remote_set_config`], taking the shared state
/// directly so the `codemux remote enable --scope/--port` change path can reuse
/// the exact same rebind-with-rollback semantics the Settings pane uses.
async fn set_config_core<R: Runtime>(
    app: &AppHandle<R>,
    shared: &Arc<Shared>,
    port: Option<u16>,
    require_approval: Option<bool>,
    bind_scope: Option<String>,
    account_mode_enabled: Option<bool>,
    trust_account_browsers: Option<bool>,
    relay_mode_enabled: Option<bool>,
) -> Result<WebRemoteStatus, String> {
    // Reject an unknown scope before mutating anything, so we never persist a
    // value the bind logic can't honour.
    if let Some(ref s) = bind_scope {
        if !is_valid_bind_scope(s) {
            return Err(format!("Unknown access scope: {s}"));
        }
    }

    let (old_port, old_scope, port_changed, scope_changed, relay_target, relay_changed) = {
        let mut cfg = shared.config.lock().unwrap();
        let old_port = cfg.port;
        let old_scope = cfg.bind_scope.clone();
        let old_relay = cfg.relay_mode_enabled;
        if let Some(p) = port {
            cfg.port = p;
        }
        if let Some(r) = require_approval {
            cfg.require_approval = r;
        }
        if let Some(s) = bind_scope {
            cfg.bind_scope = s;
        }
        // Account-mode toggles never rebind the listener (they only gate the
        // `/api/pair-account` admission path), so they're applied here and
        // persisted without touching the running server.
        if let Some(a) = account_mode_enabled {
            cfg.account_mode_enabled = a;
        }
        if let Some(t) = trust_account_browsers {
            cfg.trust_account_browsers = t;
        }
        // The iroh transport toggle is a start/stop of a *parallel* endpoint,
        // never a rebind of the axum listener — applied below, after persist.
        if let Some(r) = relay_mode_enabled {
            cfg.relay_mode_enabled = r;
        }
        (
            old_port,
            old_scope.clone(),
            cfg.port != old_port,
            cfg.bind_scope != old_scope,
            cfg.relay_mode_enabled,
            cfg.relay_mode_enabled != old_relay,
        )
    };
    persist_config(app, shared);

    // A port or scope change while running requires a rebind (the same
    // stop→start path a port change already used, which also drops existing
    // connections — they can't follow to a new port/interface).
    let running = shared.runtime.lock().unwrap().is_some();
    if running && (port_changed || scope_changed) {
        stop_server(shared);
        if let Err(e) = start_server(app, shared).await {
            // The new binding failed (e.g. switching to `tailscale` with no
            // tailnet address). Restore the previous config and rebind to it
            // so the server keeps running on its last-good address instead of
            // being left off, then report why the change was rejected.
            {
                let mut cfg = shared.config.lock().unwrap();
                cfg.port = old_port;
                cfg.bind_scope = old_scope;
            }
            persist_config(app, shared);
            let _ = start_server(app, shared).await;
            emit_state_changed(app);
            return Err(e);
        }
    }

    // Apply an iroh relay-mode toggle. It only *runs* while the feature itself
    // is bound (`running`); turning it on while the server is off just persists
    // the flag, and `web_remote_enable` starts the endpoint then. An iroh start
    // failure is logged, never fatal — the primary `/ws` transport is unaffected.
    if relay_changed {
        if relay_target {
            if running {
                if let Err(e) = iroh::start(app, shared).await {
                    eprintln!("[codemux::web_remote] iroh transport enable failed: {e}");
                }
                // Start device registration in lockstep with the endpoint.
                registration::start(app, shared);
            }
        } else {
            iroh::stop(shared);
            registration::stop(shared);
        }
    }

    emit_state_changed(app);
    Ok(build_status(app, shared))
}

#[tauri::command]
pub fn web_remote_create_pairing<R: Runtime>(app: AppHandle<R>) -> PairingInfo {
    let shared = app.state::<WebRemoteState>().shared();
    mint_pairing(&shared, None)
}

#[tauri::command]
pub fn web_remote_list_endpoints<R: Runtime>(app: AppHandle<R>) -> Vec<endpoints::Endpoint> {
    let shared = app.state::<WebRemoteState>().shared();
    let (port, relay_mode_enabled) = {
        let cfg = shared.config.lock().unwrap();
        (cfg.port, cfg.relay_mode_enabled)
    };
    let mut list = endpoints::list(port);
    // Surface the from-anywhere iroh endpoint (host = the device's node_id) only
    // when relay mode is on and a stable identity exists. Appended last so it
    // never displaces the HTTP endpoints' "recommended" hint.
    if relay_mode_enabled {
        if let Some(node_id) = shared.iroh.node_id() {
            list.push(endpoints::iroh_endpoint(&node_id));
        }
    }
    list
}

/// The device's stable iroh `node_id` (its iroh `EndpointId`) — the address a
/// browser dials over the from-anywhere transport. Exposed so a later stage can
/// register it with the control plane and the desktop can display/copy it.
/// `None` until relay mode has been enabled at least once (the identity key is
/// generated + persisted on first enable).
#[tauri::command]
pub fn web_remote_iroh_node_id<R: Runtime>(app: AppHandle<R>) -> Option<String> {
    app.state::<WebRemoteState>().shared().iroh.node_id()
}

/// This desktop's account-device-registry registration status: whether it is
/// currently registered (discoverable by an account browser), the stable device
/// id it registers under, the `node_id` last registered, the last successful
/// registration time, and the last error (for diagnostics). Surfaced so the
/// Settings pane can show a "device registered" indicator distinct from the raw
/// `node_id`. Registration is best-effort and only runs while relay mode is on
/// and the desktop is signed in.
#[tauri::command]
pub fn web_remote_registration_status<R: Runtime>(app: AppHandle<R>) -> registration::RegistrationStatus {
    app.state::<WebRemoteState>().shared().registration.status()
}

#[tauri::command]
pub fn web_remote_list_sessions<R: Runtime>(app: AppHandle<R>) -> Vec<SessionView> {
    let shared = app.state::<WebRemoteState>().shared();
    build_status(&app, &shared).sessions
}

#[tauri::command]
pub fn web_remote_revoke_session<R: Runtime>(app: AppHandle<R>, session_id: String) -> Result<WebRemoteStatus, String> {
    let shared = app.state::<WebRemoteState>().shared();
    app.state::<crate::database::DatabaseStore>()
        .web_remote_revoke_session(&session_id)?;
    // Drop any live sockets belonging to the revoked device immediately.
    shared.connections.close_session(&session_id);
    emit_state_changed(&app);
    Ok(build_status(&app, &shared))
}

#[tauri::command]
pub fn web_remote_approve_session<R: Runtime>(
    app: AppHandle<R>,
    session_id: String,
) -> Result<WebRemoteStatus, String> {
    let shared = app.state::<WebRemoteState>().shared();
    app.state::<crate::database::DatabaseStore>()
        .web_remote_set_session_approved(&session_id, true)?;
    emit_state_changed(&app);
    Ok(build_status(&app, &shared))
}

#[tauri::command]
pub fn web_remote_reject_session<R: Runtime>(
    app: AppHandle<R>,
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
pub fn web_remote_publish_update_available<R: Runtime>(
    app: AppHandle<R>,
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
pub fn web_remote_request_update<R: Runtime>(app: AppHandle<R>) {
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
        assert_eq!(cfg.bind_scope, BIND_SCOPE_ALL, "default scope is all interfaces");
    }

    #[test]
    fn config_roundtrips_through_json() {
        let cfg = WebRemoteConfig {
            enabled: true,
            port: 5000,
            require_approval: true,
            bind_scope: BIND_SCOPE_TAILSCALE.to_string(),
            account_mode_enabled: false,
            trust_account_browsers: false,
            relay_mode_enabled: false,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: WebRemoteConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.enabled, cfg.enabled);
        assert_eq!(back.port, cfg.port);
        assert_eq!(back.require_approval, cfg.require_approval);
        assert_eq!(back.bind_scope, cfg.bind_scope);
    }

    #[test]
    fn config_without_bind_scope_deserializes_to_all() {
        // A config persisted before `bind_scope` existed must load as the
        // historical all-interfaces behavior, not fail — `#[serde(default)]`.
        let legacy = r#"{"enabled":true,"port":4377,"require_approval":false}"#;
        let cfg: WebRemoteConfig = serde_json::from_str(legacy).unwrap();
        assert_eq!(cfg.bind_scope, BIND_SCOPE_ALL);
        assert!(cfg.enabled);
    }

    #[test]
    fn config_account_mode_defaults_off_and_approval_on() {
        // Account mode is default-off (must be explicitly opted into) and
        // account browsers are NOT trusted by default (approval on), mirroring
        // the design's default-off / approval-on-by-default posture.
        let cfg = WebRemoteConfig::default();
        assert!(!cfg.account_mode_enabled, "account mode default off");
        assert!(
            !cfg.trust_account_browsers,
            "account browsers not trusted by default (approval on)"
        );
    }

    #[test]
    fn config_without_account_fields_deserializes_to_defaults() {
        // A config persisted before account mode existed must load with account
        // mode off and approval on, never fail — `#[serde(default)]`.
        let legacy = r#"{"enabled":true,"port":4377,"require_approval":false,"bind_scope":"all"}"#;
        let cfg: WebRemoteConfig = serde_json::from_str(legacy).unwrap();
        assert!(!cfg.account_mode_enabled);
        assert!(!cfg.trust_account_browsers);
        // The iroh relay transport is likewise off for a config predating it.
        assert!(!cfg.relay_mode_enabled, "legacy config loads relay mode off");
    }

    #[test]
    fn account_config_roundtrips_through_json() {
        let cfg = WebRemoteConfig {
            enabled: true,
            port: 4377,
            require_approval: false,
            bind_scope: BIND_SCOPE_ALL.to_string(),
            account_mode_enabled: true,
            trust_account_browsers: true,
            relay_mode_enabled: true,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: WebRemoteConfig = serde_json::from_str(&json).unwrap();
        assert!(back.account_mode_enabled);
        assert!(back.relay_mode_enabled, "relay mode round-trips through JSON");
        assert!(back.trust_account_browsers);
    }

    #[test]
    fn account_session_approval_follows_trust_opt_out() {
        // Pending by default (approval on for account sessions), even when the
        // pairing-path `require_approval` is off.
        let mut cfg = WebRemoteConfig {
            require_approval: false,
            ..WebRemoteConfig::default()
        };
        assert!(
            !account_session_approved(&cfg),
            "account session pends by default despite require_approval=off"
        );
        // The explicit opt-out admits account browsers immediately.
        cfg.trust_account_browsers = true;
        assert!(account_session_approved(&cfg));
    }

    #[test]
    fn status_carries_account_fields_in_snake_case() {
        let status = WebRemoteStatus {
            enabled: true,
            running: true,
            port: DEFAULT_PORT,
            require_approval: false,
            bind_scope: BIND_SCOPE_ALL.to_string(),
            active_connections: 0,
            connected_sessions: 0,
            sessions: vec![],
            update_available: false,
            update_version: None,
            account_mode_enabled: true,
            trust_account_browsers: false,
            account_signed_in: true,
            relay_mode_enabled: false,
            iroh_node_id: None,
            device_registered: false,
            device_id: None,
        };
        let v = serde_json::to_value(&status).unwrap();
        assert_eq!(v["account_mode_enabled"], true);
        assert_eq!(v["trust_account_browsers"], false);
        assert_eq!(v["account_signed_in"], true);
        assert!(v.get("accountModeEnabled").is_none(), "no camelCase leakage");
    }

    #[test]
    fn status_carries_bind_scope_in_snake_case() {
        let status = WebRemoteStatus {
            enabled: true,
            running: true,
            port: DEFAULT_PORT,
            require_approval: false,
            bind_scope: BIND_SCOPE_TAILSCALE.to_string(),
            active_connections: 0,
            connected_sessions: 0,
            sessions: vec![],
            update_available: false,
            update_version: None,
            account_mode_enabled: false,
            trust_account_browsers: false,
            account_signed_in: false,
            relay_mode_enabled: false,
            iroh_node_id: None,
            device_registered: false,
            device_id: None,
        };
        let v = serde_json::to_value(&status).unwrap();
        assert_eq!(v["bind_scope"], "tailscale");
        assert!(v.get("bindScope").is_none(), "no camelCase leakage");
    }

    // ── Bind-scope address resolution ───────────────────────────────

    #[test]
    fn bind_scope_all_binds_every_interface() {
        let addrs = bind_addrs_from(BIND_SCOPE_ALL, 4377, vec![]).unwrap();
        assert_eq!(addrs, vec![SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 4377)]);
    }

    #[test]
    fn bind_scope_unknown_value_defaults_to_all() {
        // `bind_addrs_from` treats an unrecognised scope as all-interfaces so
        // it can never leave the server unbindable; `web_remote_set_config`
        // rejects unknown values before they are ever persisted.
        let addrs = bind_addrs_from("wat", 4377, vec![]).unwrap();
        assert_eq!(addrs, vec![SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 4377)]);
    }

    #[test]
    fn bind_scope_loopback_binds_loopback_only() {
        let addrs = bind_addrs_from(BIND_SCOPE_LOOPBACK, 4377, vec![]).unwrap();
        assert_eq!(addrs, vec![SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 4377)]);
    }

    #[test]
    fn bind_scope_tailscale_without_address_fails_and_never_falls_back() {
        // The whole point of the scope is to NOT expose the port when there's
        // no tailnet — a silent fall-back to 0.0.0.0 would defeat it.
        let err = bind_addrs_from(BIND_SCOPE_TAILSCALE, 4377, vec![]).unwrap_err();
        assert!(err.contains("No Tailscale address found"), "clear error: {err}");
    }

    #[test]
    fn bind_scope_tailscale_binds_tailnet_plus_loopback() {
        let tailnet: IpAddr = "100.101.102.103".parse().unwrap();
        let addrs = bind_addrs_from(BIND_SCOPE_TAILSCALE, 4377, vec![tailnet]).unwrap();
        // Loopback is kept so local use / an SSH tunnel still works.
        assert!(addrs.contains(&SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 4377)));
        assert!(addrs.contains(&SocketAddr::new(tailnet, 4377)));
        assert_eq!(addrs.len(), 2, "exactly loopback + the one tailnet address");
    }

    // ── Control-socket pairing (codemux remote pair over SSH) ───────

    #[test]
    fn control_pair_errors_when_disabled() {
        // Default config is disabled — pairing over the control socket must
        // refuse with a clear, actionable message and mint no token.
        let shared = Arc::new(Shared::default());
        let err = control_pair_from(&shared, None).unwrap_err();
        assert!(
            err.contains("Remote access is not enabled"),
            "clear error: {err}"
        );
        assert_eq!(shared.pairing.live_count(), 0, "no token minted on the error path");
    }

    #[test]
    fn control_pair_when_enabled_mints_a_usable_single_use_token() {
        let shared = Arc::new(Shared::default());
        shared.config.lock().unwrap().enabled = true;

        let res = control_pair_from(&shared, Some("Phone".to_string())).unwrap();
        // The URL is the recommended endpoint's origin + the pairing fragment;
        // loopback is always present so we always get a usable http:// URL.
        assert!(res.pairing_url.starts_with("http://"), "url: {}", res.pairing_url);
        assert!(
            res.pairing_url.ends_with(&format!("/#pair={}", res.token)),
            "url embeds the token: {}",
            res.pairing_url
        );
        assert_eq!(res.port, DEFAULT_PORT);

        // The token pairs via the SAME path `/api/pair` uses (PairingStore),
        // and is single-use: the first consume succeeds, the second fails.
        let first = shared.pairing.consume_named(&res.token);
        assert!(first.consumed, "minted token pairs successfully");
        assert_eq!(
            first.suggested_name.as_deref(),
            Some("Phone"),
            "the --name label rides along as the fallback device name"
        );
        assert!(
            !shared.pairing.consume(&res.token),
            "token is single-use — a second pair attempt fails"
        );
    }

    // ── Control-socket enable/disable (codemux remote enable/disable) ───

    #[test]
    fn apply_enable_request_flips_on_and_persists_scope_and_port() {
        // The config side of `control_enable`'s off→on path: enable is set and
        // the requested scope/port are folded in.
        let mut cfg = WebRemoteConfig::default();
        assert!(!cfg.enabled);
        apply_enable_request(
            &mut cfg,
            Some(BIND_SCOPE_LOOPBACK.to_string()),
            Some(5000),
        )
        .unwrap();
        assert!(cfg.enabled, "enable flips the master switch on");
        assert_eq!(cfg.bind_scope, BIND_SCOPE_LOOPBACK);
        assert_eq!(cfg.port, 5000);
    }

    #[test]
    fn apply_enable_request_without_flags_keeps_current_scope_and_port() {
        // `codemux remote enable` with no flags enables on the existing config.
        let mut cfg = WebRemoteConfig {
            port: 4400,
            bind_scope: BIND_SCOPE_TAILSCALE.to_string(),
            ..WebRemoteConfig::default()
        };
        apply_enable_request(&mut cfg, None, None).unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.port, 4400, "port unchanged when no --port given");
        assert_eq!(cfg.bind_scope, BIND_SCOPE_TAILSCALE, "scope unchanged");
    }

    #[test]
    fn apply_enable_request_rejects_unknown_scope_and_leaves_config_untouched() {
        let mut cfg = WebRemoteConfig::default();
        let err = apply_enable_request(&mut cfg, Some("wat".to_string()), None).unwrap_err();
        assert!(err.contains("Unknown access scope"), "clear error: {err}");
        // Nothing was mutated — the switch stays off, so nothing is persisted.
        assert!(!cfg.enabled, "an invalid request never flips enable on");
        assert_eq!(cfg.bind_scope, BIND_SCOPE_ALL);
    }

    #[test]
    fn enable_requesting_tailscale_without_tailnet_would_fail_to_bind() {
        // `apply_enable_request` accepts the scope (it's a valid value), but the
        // actual enable binds through `bind_addrs`, which fails with the clear
        // no-fallback error when there is no tailnet address — so a
        // `codemux remote enable --scope tailscale` on a host off the mesh
        // surfaces that reason and the control layer restores the last-good scope.
        let mut cfg = WebRemoteConfig::default();
        apply_enable_request(&mut cfg, Some(BIND_SCOPE_TAILSCALE.to_string()), None).unwrap();
        assert_eq!(cfg.bind_scope, BIND_SCOPE_TAILSCALE);
        let err = bind_addrs_from(&cfg.bind_scope, cfg.port, vec![]).unwrap_err();
        assert!(
            err.contains("No Tailscale address found"),
            "enable propagates the bind error: {err}"
        );
    }

    #[test]
    fn mark_disabled_turns_the_switch_off() {
        // The config side of `control_disable`: flip enable off, which
        // `disable_core` then persists and acts on (severing sockets).
        let mut cfg = WebRemoteConfig {
            enabled: true,
            ..WebRemoteConfig::default()
        };
        mark_disabled(&mut cfg);
        assert!(!cfg.enabled, "disable clears the master switch");
    }

    #[test]
    fn enable_scope_strings_parse_as_valid_bind_scopes() {
        // The three CLI `--scope` values the control layer accepts.
        assert!(is_valid_bind_scope(BIND_SCOPE_ALL));
        assert!(is_valid_bind_scope(BIND_SCOPE_TAILSCALE));
        assert!(is_valid_bind_scope(BIND_SCOPE_LOOPBACK));
        assert!(!is_valid_bind_scope("wan"));
        assert!(!is_valid_bind_scope(""));
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
            bind_scope: BIND_SCOPE_ALL.to_string(),
            active_connections: 3,
            connected_sessions: 2,
            sessions: vec![],
            update_available: false,
            update_version: None,
            account_mode_enabled: false,
            trust_account_browsers: false,
            account_signed_in: false,
            relay_mode_enabled: false,
            iroh_node_id: None,
            device_registered: false,
            device_id: None,
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
            bind_scope: BIND_SCOPE_ALL.to_string(),
            active_connections: 0,
            connected_sessions: 0,
            sessions: vec![],
            update_available: true,
            update_version: Some("1.2.3".to_string()),
            account_mode_enabled: false,
            trust_account_browsers: false,
            account_signed_in: false,
            relay_mode_enabled: false,
            iroh_node_id: None,
            device_registered: false,
            device_id: None,
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
            source: "pair".to_string(),
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
