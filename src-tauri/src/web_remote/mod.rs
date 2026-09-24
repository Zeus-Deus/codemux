//! Embedded web remote-access server.
//!
//! Turns the desktop app into a second frontend for its own backend: a
//! browser on another device loads the same UI bundle and drives the same
//! running instance over HTTP + WebSocket. Default-off — nothing binds
//! until the user enables it. Under that kill switch there are two independent
//! ways in: the LAN listener (an axum server on `<scope>:<port>`, default 4377)
//! and the from-anywhere iroh relay transport. Each has its own switch, and a
//! failure to start one never keeps the other down.
//!
//! Module layout:
//! - [`mod@self`] — managed state, config persistence, lifecycle, commands.
//! - [`server`]   — axum router, static assets, `/api/*`, `/ws`.
//! - [`auth`]     — pairing tokens, sessions, WS tickets, origin checks.
//! - [`connect`]  — the one-command `codemux connect` remote-access bootstrap
//!   (sign in → persist relay config → install/drive a background service).
//! - [`account`]  — account-mode admission: verify the browser owns the same
//!   Codemux account the desktop is signed into, then mint a session.
//! - [`dispatch`] — invoke dispatch via synthesized `on_message` + channels.
//! - [`compress`] — negotiated application-level WS frame compression + the
//!   per-connection outbound byte accounting, both owned by the WS writer.
//! - [`events`]   — `listen_any` fan-out hub with per-event refcounting.
//! - [`iroh`]     — parallel from-anywhere transport (default-off): the `/ws`
//!   protocol carried inside an E2E-encrypted iroh QUIC bi-stream.
//! - [`endpoints`]— reachable-endpoint enumeration (loopback/LAN/tailnet).
//! - [`assets`]   — authenticated `/api/assets` file route (`convertFileSrc`).
//! - [`proxy`]    — auth-gated browser-pane WS + HTTP proxy to loopback daemons.
//! - [`snapshot`] — authenticated `/api/snapshot` bulk state bootstrap (versioned API).

pub mod account;
pub mod assets;
pub mod auth;
pub mod compress;
pub mod connect;
pub mod dispatch;
pub mod endpoints;
pub mod events;
pub mod iroh;
mod policy;
pub(crate) mod push;
pub mod proxy;
pub mod registration;
pub mod serve;
pub mod server;
pub mod snapshot;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
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

/// Serde default for `lan_enabled`: a config persisted before the transports
/// were split keeps its listener, because back then `enabled` *was* the
/// listener. (A fresh config starts with it off — see [`WebRemoteConfig::default`].)
fn legacy_lan_enabled() -> bool {
    true
}

/// Persisted, user-controlled configuration.
///
/// `enabled` is the **kill switch** for every way in. Underneath it, each
/// transport has its own switch and runs independently of the other:
/// `lan_enabled` owns the axum `/ws` listener (LAN / tailnet / loopback), and
/// `relay_mode_enabled` owns the iroh relay transport (app.codemux.org). A
/// failure to bind one never stops the other from coming up.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebRemoteConfig {
    /// Master kill switch. Off → nothing listens and nothing registers.
    pub enabled: bool,
    /// Whether the LAN listener (axum `/ws` on `port`, bound per `bind_scope`)
    /// runs while remote access is on. Off for a fresh config, so turning remote
    /// access on never opens a network port by itself; `codemux remote enable`,
    /// `serve`, and `connect` turn it on explicitly. Legacy configs load `true`.
    #[serde(default = "legacy_lan_enabled")]
    pub lan_enabled: bool,
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
            lan_enabled: false,
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
    /// Has `config` been read back from the settings store yet?
    ///
    /// `Shared` is constructed at `Builder` time, long before a `DatabaseStore`
    /// exists, so `config` starts as [`WebRemoteConfig::default`] — a value
    /// nobody chose. Because [`persist_config`] writes the *whole* struct, any
    /// mutation made before the persisted row was loaded would silently
    /// overwrite every field the caller never touched (this is exactly how a
    /// persisted `relay_mode_enabled: true` used to be flipped back to `false`
    /// by a plain enable under `codemux serve`). Every entry point that reads or
    /// mutates the config therefore calls [`ensure_config_hydrated`] first,
    /// which flips this once.
    config_hydrated: AtomicBool,
    pub runtime: Mutex<Option<RunningServer>>,
    /// Serialises LAN-listener binds so the boot restore, the bind-retry loop,
    /// and a user-initiated enable/retry can never race two listeners onto
    /// the same port (the loser would record a bogus "address in use").
    bind_lock: tokio::sync::Mutex<()>,
    /// Why the LAN listener is not bound while remote access is enabled, or
    /// `None` when it is bound (or remote access is off). Surfaced as
    /// [`WebRemoteStatus::bind_error`] so a failed bind is never presented as
    /// "starting…" — see issue #404.
    bind_error: Mutex<Option<String>>,
    /// The background task that keeps retrying a failed LAN bind (a tailnet
    /// address that appears a few seconds after login, a port another process
    /// releases). `Some` while a retry loop is live.
    bind_retry: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
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
            config_hydrated: AtomicBool::new(false),
            runtime: Mutex::new(None),
            bind_lock: tokio::sync::Mutex::new(()),
            bind_error: Mutex::new(None),
            bind_retry: Mutex::new(None),
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

    /// Live WebSocket connections right now (raw socket count).
    ///
    /// Exposed for the background loops in `lib.rs`, which gate work on the
    /// desktop window being on screen. That gate is only sound when the
    /// desktop webview is the ONLY consumer: a minimized window with a paired
    /// browser attached still has a renderer waiting for updates, and pausing
    /// there stops the remote page rather than saving idle work.
    pub fn active_connection_count(&self) -> usize {
        self.inner.connections.active_count()
    }
}

// ── Status snapshot (command return + event payload) ────────────────

/// Live server + device state. Returned by `web_remote_status` and
/// emitted as the `web-remote-state-changed` payload.
#[derive(Debug, Clone, Serialize)]
pub struct WebRemoteStatus {
    /// Master kill switch for every way in.
    pub enabled: bool,
    /// Whether the LAN listener is bound right now.
    pub running: bool,
    /// Whether the LAN listener is wanted (its own switch under `enabled`).
    pub lan_enabled: bool,
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
    /// Why the LAN listener is not bound even though remote access and the
    /// listener's own switch are on (a port already in use, a `tailscale` scope
    /// with no tailnet address…). `None` while the listener is bound or not
    /// wanted. The backend
    /// keeps retrying in the background while this is set; the UI renders the
    /// reason (plus a Retry) instead of an open-ended "starting".
    pub bind_error: Option<String>,
    /// Whether the from-anywhere iroh endpoint is actually up right now — as
    /// opposed to `relay_mode_enabled`, which is only the persisted intent.
    /// Independent of `running`: relay mode does not need the LAN listener.
    pub relay_running: bool,
    /// The last device-registration failure (`None` when registered, not yet
    /// attempted, or relay not wanted). Covers the relay endpoint itself failing
    /// to start (recorded as "relay transport couldn't start: …"), so there is
    /// no separate relay error. Carried on the live broadcast so the Settings
    /// pane shows the actual reason the device is not listed with the account.
    pub registration_error: Option<String>,
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
    /// `public` | `loopback`), so the CLI can hint how far it reaches.
    pub endpoint_kind: String,
}

/// Pure core of [`control_pair`], taking the shared state directly so it is
/// unit-testable without a Tauri `AppHandle`. Errors when remote access is
/// not enabled — the server must be bound for the URL to be reachable.
pub(crate) fn control_pair_from(
    shared: &Arc<Shared>,
    suggested_name: Option<String>,
) -> Result<ControlPairing, String> {
    let (enabled, lan_enabled, port) = {
        let cfg = shared.config.lock().unwrap();
        (cfg.enabled, cfg.lan_enabled, cfg.port)
    };
    if !enabled {
        return Err("Remote access is not enabled — enable it in Settings first".to_string());
    }
    // A pairing link points at the LAN listener, so it is useless while that
    // way in is switched off (e.g. a relay-only setup).
    if !lan_enabled {
        return Err(
            "Access on your network is off — turn on \"On my network\" in Settings, or run `codemux remote enable`"
                .to_string(),
        );
    }
    // A pairing URL on a listener that failed to bind would be a dead link.
    // Say why instead (the retry loop may bring it back on its own).
    if shared.runtime.lock().unwrap().is_none() {
        if let Some(e) = shared.bind_error.lock().unwrap().clone() {
            return Err(format!(
                "The remote-access server isn't listening: {e} (it keeps retrying in the background)"
            ));
        }
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
    // Pairing refuses when remote access is off and embeds the port in the URL,
    // so it has to see the persisted config, not the pre-boot default.
    ensure_config_hydrated(app, &shared);
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
    /// → local network → public), falling back to the first enumerated entry
    /// (loopback) when nothing is reachable from off-box. `None` only if no
    /// endpoint could be enumerated at all.
    pub endpoint_url: Option<String>,
    /// The recommended endpoint's host (IP literal or MagicDNS name).
    pub endpoint_host: Option<String>,
    /// The recommended endpoint kind (`magicdns` | `tailnet` | `lan` |
    /// `public` | `loopback`), so the CLI can hint how far it reaches.
    pub endpoint_kind: Option<String>,
    /// Whether the recommended endpoint is a browser secure context (loopback).
    pub endpoint_secure: bool,
    /// True when the enable was a no-op because remote access was already on and
    /// no scope/port change was requested — so the CLI can say "already running".
    pub already_running: bool,
}

/// Validate an enable request's scope and fold it (plus the port) into `cfg`,
/// flipping `enabled` and the LAN listener (`lan_enabled`) on — `codemux remote
/// enable` / `serve` exist to open the network listener. Pure so the config
/// side of [`control_enable`] is
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
    cfg.lan_enabled = true;
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
/// - Off (or on without the LAN listener, e.g. relay-only) → on: fold the
///   requested `scope`/`port` into config, turn the kill switch and the LAN
///   listener on, then run the same [`enable_core`] path. A bind failure such
///   as `tailscale` with no tailnet address rolls the change back when relay
///   mode is off; with relay mode on the enable stands relay-only and the
///   failure is the status's `bind_error`.
/// - Already on with a `scope`/`port` flag: treat it as a config change and
///   rebind through the same [`set_config_core`] path the Settings pane uses.
/// - Already on with no flags: report it's already running with current status.
pub async fn control_enable<R: Runtime>(
    app: &AppHandle<R>,
    scope: Option<String>,
    port: Option<u16>,
) -> Result<ControlEnableResult, String> {
    let shared = app.state::<WebRemoteState>().shared();
    // Decide against the persisted config, never against the pre-boot default:
    // an enable with no flags must keep the persisted port/scope/relay, and
    // must not persist a default over them.
    ensure_config_hydrated(app, &shared);
    // "Already on" means the LAN listener is wanted too: this command exists to
    // open it, so a relay-only setup takes the off → on path below.
    let already_enabled = lan_wanted(&shared.config.lock().unwrap());
    let already_bound = shared.runtime.lock().unwrap().is_some();

    let (status, already_running) = if already_enabled {
        if scope.is_some() || port.is_some() {
            // A scope/port change while running rebinds via the same path a
            // port change from the Settings pane uses (drops existing sockets).
            let change = ConfigChange {
                port,
                bind_scope: scope,
                ..ConfigChange::default()
            };
            let changed = set_config_core(app, &shared, change).await;
            if already_bound {
                (changed?, false)
            } else {
                // Persisted-enabled but nothing bound (headless serve leaves
                // boot-time binding to its awaited startup path). `set_config_core`
                // only *rebinds* an already-running listener, so an enable with
                // flags would otherwise persist the new scope/port and leave the
                // server off. Bind through the shared path instead.
                //
                // Its bind attempt at the new scope/port may fail. With relay
                // mode on that is not fatal — `enable_core` keeps relay up and
                // reports the failure as the status's `bind_error` — so only a
                // non-bind error (an invalid scope) or relay-off fails here.
                if let Err(e) = changed {
                    let bind_failed = shared.bind_error.lock().unwrap().as_deref() == Some(e.as_str());
                    if !(bind_failed && shared.config.lock().unwrap().relay_mode_enabled) {
                        return Err(e);
                    }
                }
                (enable_core(app, &shared).await?, false)
            }
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
        // Was off. Capture the last-good config so a failed bind (e.g.
        // `--scope tailscale` with no tailnet) restores it rather than
        // leaving a bad scope persisted with the server off.
        let previous = shared.config.lock().unwrap().clone();
        {
            let mut cfg = shared.config.lock().unwrap();
            apply_enable_request(&mut cfg, scope, port)?;
        }
        match enable_core(app, &shared).await {
            Ok(status) => (status, false),
            Err(e) => {
                // `enable_core` already rolled `enabled` back to false; restore
                // the previous switches and scope/port too and persist the
                // last-good config. (Relay mode is off on this path, so nothing
                // else is running.)
                {
                    let mut cfg = shared.config.lock().unwrap();
                    cfg.enabled = previous.enabled;
                    cfg.lan_enabled = previous.lan_enabled;
                    cfg.port = previous.port;
                    cfg.bind_scope = previous.bind_scope;
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

/// Turn the from-anywhere relay transport on or off in a RUNNING instance,
/// from the same-machine control socket (`codemux connect` / `codemux connect
/// off` when a GUI or `serve` already holds the control endpoint).
///
/// Goes through the same [`set_config_core`] the Settings pane's relay switch
/// uses, so the flag is persisted and — whenever remote access is on, whether or
/// not the LAN listener is bound — the iroh endpoint plus device registration
/// start/stop in lockstep. Deliberately the
/// only way the CLI flips relay mode on a live instance: writing the setting
/// row directly would be silently overwritten the next time the instance
/// persists its in-memory config.
pub async fn control_set_relay<R: Runtime>(
    app: &AppHandle<R>,
    enabled: bool,
) -> Result<WebRemoteStatus, String> {
    let shared = app.state::<WebRemoteState>().shared();
    let change = ConfigChange {
        relay_mode_enabled: Some(enabled),
        ..ConfigChange::default()
    };
    set_config_core(app, &shared, change).await
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
    // The single funnel for every config *read* surfaced to a caller (the
    // `web_remote_status` command, the `web-remote-state-changed` payload, and
    // `serve`'s scope resolution), so none of them can see the pre-boot default
    // instead of what the user actually persisted.
    ensure_config_hydrated(app, shared);
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
        lan_enabled: cfg.lan_enabled,
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
        bind_error: if lan_wanted(&cfg) && !running {
            shared.bind_error.lock().unwrap().clone()
        } else {
            None
        },
        relay_running: shared.iroh.is_running(),
        registration_error: if registration.registered || !relay_wanted(&cfg) {
            None
        } else {
            registration.last_error
        },
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
    // Writing the live config over the settings row is only safe once the row
    // has been read back into it — otherwise fields the caller never intended
    // to change (relay mode, account mode, the port…) get stamped with the
    // in-memory default. Every mutating entry point hydrates first; this
    // assertion is the tripwire for a future one that forgets.
    debug_assert!(
        shared.config_hydrated.load(Ordering::SeqCst),
        "persist_config before ensure_config_hydrated would clobber persisted fields"
    );
    let cfg = shared.config.lock().unwrap().clone();
    let _ = save_config_to_db(&app.state::<crate::database::DatabaseStore>(), &cfg);
}

/// Read the persisted config into `shared.config` exactly once, before anything
/// reads or mutates it. Idempotent and cheap after the first call.
///
/// The GUI reaches this through [`restore_on_boot`] in the Tauri `setup` hook.
/// Anything that can run *without* that hook — headless `codemux serve`, whose
/// startup path binds the server itself — hydrates through the same door, so
/// there is no ordering in which a caller can persist a config it never loaded.
/// See [`Shared::config_hydrated`].
pub(crate) fn ensure_config_hydrated<R: Runtime>(app: &AppHandle<R>, shared: &Arc<Shared>) {
    hydrate_config_from(shared, &app.state::<crate::database::DatabaseStore>());
}

/// [`ensure_config_hydrated`] against a `DatabaseStore` directly, so the
/// invariant is unit-testable without an `AppHandle`. The load happens under
/// the config lock so a concurrent mutation can neither be lost nor observe a
/// half-hydrated config.
pub(crate) fn hydrate_config_from(shared: &Arc<Shared>, db: &crate::database::DatabaseStore) {
    let mut cfg = shared.config.lock().unwrap();
    if shared.config_hydrated.swap(true, Ordering::SeqCst) {
        return;
    }
    *cfg = load_config_from_db(db);
}

// ── Headless config access (no running instance) ────────────────────
//
// `persist_config` / `load_config` above need an `AppHandle` because that is
// how the running app reaches the settings store. `codemux connect` has no app
// — it configures a machine BEFORE anything runs — so these three take the
// `DatabaseStore` directly and go through the exact same `CONFIG_KEY` +
// serde-JSON representation the app reads back on boot (`restore_on_boot`).
// Keeping them adjacent to the app-handle pair is deliberate: a change to how
// the config is stored has to touch both or neither.

/// Read the persisted config straight from the settings store. Missing or
/// unparseable → [`WebRemoteConfig::default`], matching [`load_config`].
pub fn load_config_from_db(db: &crate::database::DatabaseStore) -> WebRemoteConfig {
    db.get_setting(CONFIG_KEY)
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default()
}

/// Write the config to the settings store. Unlike [`persist_config`] (which is
/// best-effort because the running server is the source of truth) this reports
/// failures — a headless caller has no other way to know the write was lost.
pub fn save_config_to_db(
    db: &crate::database::DatabaseStore,
    cfg: &WebRemoteConfig,
) -> Result<(), String> {
    let json = serde_json::to_string(cfg).map_err(|e| format!("serialize config: {e}"))?;
    db.set_setting(CONFIG_KEY, &json)
}

/// Load → mutate → save, returning the config as persisted. The mutation may
/// reject the request (an unknown bind scope), in which case nothing is
/// written. Used by `codemux connect` on a machine with nothing running; when
/// an instance IS running it must not be used — the live instance owns the
/// in-memory config and would re-persist over this write.
pub fn update_config_headless<F>(
    db: &crate::database::DatabaseStore,
    mutate: F,
) -> Result<WebRemoteConfig, String>
where
    F: FnOnce(&mut WebRemoteConfig) -> Result<(), String>,
{
    let mut cfg = load_config_from_db(db);
    mutate(&mut cfg)?;
    save_config_to_db(db, &cfg)?;
    Ok(cfg)
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
    // One bind at a time: the boot restore, the retry loop, and a user
    // enable/retry can all reach here concurrently.
    let _bind = shared.bind_lock.lock().await;
    // Idempotent: already bound → nothing to do.
    if shared.runtime.lock().unwrap().is_some() {
        return Ok(());
    }
    match bind_listeners(app, shared).await {
        Ok(()) => {
            *shared.bind_error.lock().unwrap() = None;
            Ok(())
        }
        Err(e) => {
            // Recorded (not just returned) so the status can say *why* nothing
            // is listening, and logged to the persistent app log — a packaged
            // app's stderr goes nowhere, which is what made #404 undiagnosable.
            // The retry loop re-fails the same way every minute while the
            // cause persists; log only when the reason changes so the rolling
            // file keeps its history.
            let previous = shared.bind_error.lock().unwrap().replace(e.clone());
            if previous.as_deref() != Some(e.as_str()) {
                log::warn!("[codemux::web_remote] LAN listener bind failed: {e}");
            }
            Err(e)
        }
    }
}

/// The bind itself: resolve the scope, bind every address, spawn the servers,
/// install the [`RunningServer`]. Only [`start_server`] calls this (under the
/// bind lock, after its idempotency check).
async fn bind_listeners<R: Runtime>(app: &AppHandle<R>, shared: &Arc<Shared>) -> Result<(), String> {
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
    // Receivers are subscribed before the runtime is installed, so a
    // `stop_lan` racing the spawns below still reaches every listener.
    let (shutdown_tx, _shutdown_rx) = tokio::sync::watch::channel(false);
    let servers: Vec<_> = listeners
        .into_iter()
        .map(|listener| (listener, shutdown_tx.subscribe()))
        .collect();

    // Remote access (or just the LAN listener) may have been turned off while
    // the bind was in flight (a background retry racing a disable). Disabling
    // is a security action, so never install a listener it has already torn
    // down. Check and install under one config-lock hold: every disable writes
    // the config before `stop_lan` takes the runtime, so either that stop sees
    // this install or this check sees the disable.
    {
        let cfg = shared.config.lock().unwrap();
        if !lan_wanted(&cfg) {
            return Err("the LAN listener was turned off".to_string());
        }
        *shared.runtime.lock().unwrap() = Some(RunningServer {
            port,
            scope,
            shutdown: shutdown_tx,
        });
    }

    for (listener, mut shutdown_rx) in servers {
        let router = server::router(app.clone());
        tokio::spawn(async move {
            let service = router.into_make_service_with_connect_info::<std::net::SocketAddr>();
            let result = axum::serve(listener, service)
                .with_graceful_shutdown(async move {
                    // Resolves when `stop_lan` flips the watch to `true`.
                    let _ = shutdown_rx.changed().await;
                })
                .await;
            if let Err(e) = result {
                log::warn!("[codemux::web_remote] server exited: {e}");
            }
        });
    }
    Ok(())
}

/// Stop the LAN listener: end any bind-retry loop, sever the sockets that
/// arrived over the listener, and clear its bind error. Relay sessions don't
/// ride this listener, so they are left alone — a phone on app.codemux.org
/// keeps working while the LAN port rebinds or is turned off.
///
/// Severing matters: `with_graceful_shutdown` only stops the listener accepting
/// *new* connections, and an already-open WebSocket would otherwise keep full
/// desktop control until it happened to reload.
fn stop_lan(shared: &Arc<Shared>) {
    stop_bind_retry(shared);
    shared.connections.close_transport(server::Transport::Lan);
    if let Some(running) = shared.runtime.lock().unwrap().take() {
        let _ = running.shutdown.send(true);
    }
    *shared.bind_error.lock().unwrap() = None;
}

/// Whether the LAN listener should be running under `cfg`.
fn lan_wanted(cfg: &WebRemoteConfig) -> bool {
    cfg.enabled && cfg.lan_enabled
}

/// [`start_server`] for callers that keep remote access on even when the bind
/// fails: on failure (and while the listener is still wanted) a background
/// retry loop takes over, so a transient cause — Tailscale coming up a few
/// seconds after login, a port another process is about to release — heals
/// without a restart.
async fn start_listener<R: Runtime>(app: &AppHandle<R>, shared: &Arc<Shared>) -> Result<(), String> {
    let result = start_server(app, shared).await;
    if result.is_err() && lan_wanted(&shared.config.lock().unwrap()) {
        ensure_bind_retry(app, shared);
    }
    result
}

/// Seconds to wait before each LAN-bind retry; the last entry repeats forever
/// (while the LAN listener stays wanted and unbound).
const BIND_RETRY_DELAYS_SECS: [u64; 5] = [2, 5, 10, 30, 60];

fn bind_retry_delay(attempt: usize) -> std::time::Duration {
    let idx = attempt.min(BIND_RETRY_DELAYS_SECS.len() - 1);
    std::time::Duration::from_secs(BIND_RETRY_DELAYS_SECS[idx])
}

/// Spawn the LAN-bind retry loop unless one is already live.
fn ensure_bind_retry<R: Runtime>(app: &AppHandle<R>, shared: &Arc<Shared>) {
    let mut slot = shared.bind_retry.lock().unwrap();
    if slot.as_ref().is_some_and(|h| !h.inner().is_finished()) {
        return;
    }
    let app = app.clone();
    let task_shared = shared.clone();
    *slot = Some(tauri::async_runtime::spawn(async move {
        bind_retry_loop(app, task_shared).await;
    }));
}

/// Abort the LAN-bind retry loop (remote access or the listener turned off).
/// Safe when idle.
fn stop_bind_retry(shared: &Arc<Shared>) {
    if let Some(handle) = shared.bind_retry.lock().unwrap().take() {
        handle.abort();
    }
}

/// Retry the LAN bind on [`BIND_RETRY_DELAYS_SECS`] until it binds or the
/// listener (or remote access) is turned off. Emits a state change on success, and on failure only
/// when the reason changed, so the UI stays live without a broadcast a minute.
async fn bind_retry_loop<R: Runtime>(app: AppHandle<R>, shared: Arc<Shared>) {
    let mut attempt = 0usize;
    loop {
        tokio::time::sleep(bind_retry_delay(attempt)).await;
        attempt += 1;
        if !lan_wanted(&shared.config.lock().unwrap()) || shared.runtime.lock().unwrap().is_some() {
            return;
        }
        let before = shared.bind_error.lock().unwrap().clone();
        match start_server(&app, &shared).await {
            Ok(()) => {
                log::info!(
                    "[codemux::web_remote] LAN listener bound after {attempt} retr{}",
                    if attempt == 1 { "y" } else { "ies" }
                );
                emit_state_changed(&app);
                return;
            }
            Err(e) => {
                if before.as_deref() != Some(e.as_str()) {
                    emit_state_changed(&app);
                }
            }
        }
    }
}

// ── Relay (iroh) transport lifecycle ────────────────────────────────
//
// The relay transport is independent of the LAN listener: it needs no port on
// this machine at all. It runs whenever remote access is on AND relay mode is
// on — never gated on the axum listener having bound, or on the listener's own
// switch (#404: a failed LAN bind used to leave relay mode silently dead, so
// the device never registered).

/// Whether the relay transport should be running under `cfg`. Deliberately
/// ignores `lan_enabled`: a relay-only setup is a supported way in.
fn relay_wanted(cfg: &WebRemoteConfig) -> bool {
    cfg.enabled && cfg.relay_mode_enabled
}

/// Bring the relay transport up if the config wants it: bind the iroh endpoint
/// (a failure is logged + surfaced as the registration error, never fatal) and
/// start the registration supervisor, which also re-tries the endpoint on its
/// own schedule. A no-op when relay is not wanted; idempotent otherwise.
async fn start_relay<R: Runtime>(app: &AppHandle<R>, shared: &Arc<Shared>) {
    if !relay_wanted(&shared.config.lock().unwrap()) {
        return;
    }
    if let Err(e) = iroh::start(app, shared).await {
        log::warn!("[codemux::web_remote] relay transport start failed: {e}");
        shared.registration.note_transport_error(&e);
    }
    // If relay was switched off while the bind was in flight, `iroh::start`
    // installed nothing and the supervisor ends on its first relay check.
    registration::start(app, shared);
}

/// Stop the relay transport: sever its sessions, then registration (its
/// supervisor may otherwise re-start the endpoint right after it is torn
/// down), then the endpoint.
fn stop_relay(shared: &Arc<Shared>) {
    shared.connections.close_transport(server::Transport::Relay);
    registration::stop(shared);
    iroh::stop(shared);
}

/// Which transports should be running for `cfg`, as `(lan, relay)`. The kill
/// switch gates both; under it each transport follows only its own switch —
/// in particular the relay never depends on the LAN listener.
fn desired_transports(cfg: &WebRemoteConfig) -> (bool, bool) {
    (lan_wanted(cfg), relay_wanted(cfg))
}

/// The start/stop seam [`reconcile_transports_with`] drives. Production is
/// [`AppTransports`]; tests substitute a scripted driver so the "one transport
/// failing never blocks the other" rule is checked without binding sockets.
/// Starts must be idempotent (already running → `Ok`).
trait TransportDriver {
    async fn start_lan(&self) -> Result<(), String>;
    fn stop_lan(&self);
    async fn start_relay(&self);
    fn stop_relay(&self);
}

struct AppTransports<'a, R: Runtime> {
    app: &'a AppHandle<R>,
    shared: &'a Arc<Shared>,
}

impl<R: Runtime> TransportDriver for AppTransports<'_, R> {
    async fn start_lan(&self) -> Result<(), String> {
        // Records `bind_error` and hands a failure to the bind-retry loop.
        start_listener(self.app, self.shared).await
    }

    fn stop_lan(&self) {
        stop_lan(self.shared);
    }

    async fn start_relay(&self) {
        // Endpoint + registration supervisor; a failure is reported as the
        // registration error and retried by the supervisor, never returned.
        start_relay(self.app, self.shared).await;
    }

    fn stop_relay(&self) {
        stop_relay(self.shared);
    }
}

/// Bring each transport to the state the config asks for, independently. A LAN
/// bind failure (port taken, no tailnet address) never keeps the relay from
/// coming up; it is returned so a caller can decide whether it is fatal (only
/// [`enable_core`] with relay mode off treats it so), and is recorded as the
/// status's `bind_error` either way.
async fn reconcile_transports_with<D: TransportDriver>(shared: &Shared, driver: &D) -> Result<(), String> {
    let (want_lan, want_relay) = desired_transports(&shared.config.lock().unwrap());
    let lan = if want_lan {
        driver.start_lan().await
    } else {
        driver.stop_lan();
        Ok(())
    };
    if want_relay {
        driver.start_relay().await;
    } else {
        driver.stop_relay();
    }
    lan
}

async fn reconcile_transports<R: Runtime>(app: &AppHandle<R>, shared: &Arc<Shared>) -> Result<(), String> {
    reconcile_transports_with(shared, &AppTransports { app, shared }).await
}

/// Load persisted config on boot and, if remote access was left on, bring each
/// wanted transport back up. Called once from the Tauri `setup` hook — which
/// the headless `codemux serve` app runs too (see `crate::build_headless_app`).
/// A bind failure (port taken) is never fatal: it is recorded as the status's
/// `bind_error`, retried in the background, and does not hold back the relay
/// transport, which needs no LAN listener.
pub fn restore_on_boot<R: Runtime>(app: &AppHandle<R>) {
    let shared = app.state::<WebRemoteState>().shared();
    ensure_config_hydrated(app, &shared);
    let enabled = shared.config.lock().unwrap().enabled;
    // `codemux serve` performs an awaited enable after the full headless app
    // has built so startup can report bind failures synchronously. Spawning
    // this normal GUI restore in parallel would race that enable and could
    // make serve print a pairing URL before any listener exists. The hydration
    // above still happened, which is what that awaited enable then merges its
    // explicit flags into.
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
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let shared = app.state::<WebRemoteState>().shared();
        // The LAN listener and the relay transport come up independently. A
        // failed bind keeps `enabled` (it is the user's intent, and the retry
        // loop will honour it once the cause clears) but is recorded in the
        // status as `bind_error`, so the UI shows the reason instead of
        // "starting…" forever.
        // (`start_server` already logged any failure.)
        let _ = reconcile_transports(&app, &shared).await;
        emit_state_changed(&app);
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
        // Same rule as every other mutate-then-persist path: load the persisted
        // row before overriding the three fields the harness owns.
        ensure_config_hydrated(&app, &shared);
        {
            let mut cfg = shared.config.lock().unwrap();
            cfg.enabled = true;
            cfg.lan_enabled = true;
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

/// Turn the kill switch on and bring up every transport that is switched on
/// underneath it — the shared body of the `web_remote_enable` Tauri command and
/// the `codemux remote enable` control command, so both persist config, start
/// the transports, and emit `web-remote-state-changed` through the exact same
/// path. Callers that want to change the scope or port first mutate
/// `shared.config`, then call this.
///
/// A failed LAN bind with relay mode **off** rolls `enabled` back and returns
/// the error — nothing would be running, so the switch must not read "on".
/// With relay mode **on**, the relay transport does not need the listener, so
/// the enable stands: relay comes up, the bind failure is reported through the
/// status's `bind_error`, and the retry loop keeps trying the listener.
async fn enable_core<R: Runtime>(app: &AppHandle<R>, shared: &Arc<Shared>) -> Result<WebRemoteStatus, String> {
    // Enabling changes exactly one field. Hydrating first is what keeps the
    // `persist_config` below from writing the in-memory default over every
    // other persisted field (notably `relay_mode_enabled`).
    ensure_config_hydrated(app, shared);
    shared.config.lock().unwrap().enabled = true;
    persist_config(app, shared);
    if let Err(e) = reconcile_transports(app, shared).await {
        if !shared.config.lock().unwrap().relay_mode_enabled {
            // Binding failed — e.g. the `tailscale` scope with no tailnet
            // address, or the port is already taken — and nothing else would
            // run. Roll the master switch back off so the UI never shows
            // "enabled but not running", persist that, and surface the reason.
            shared.config.lock().unwrap().enabled = false;
            stop_lan(shared);
            persist_config(app, shared);
            emit_state_changed(app);
            return Err(e);
        }
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
    // Same reason as `enable_core`: disable owns one field, so the rest of the
    // persisted config has to be in memory before it is written back.
    ensure_config_hydrated(app, shared);
    mark_disabled(&mut shared.config.lock().unwrap());
    persist_config(app, shared);
    // The master switch is the kill for *every* remote transport: stop both
    // (the LAN listener with its bind retry, then the relay endpoint and its
    // registration), then sever anything still connected regardless of how it
    // came in.
    stop_lan(shared);
    stop_relay(shared);
    shared.connections.close_all();
    emit_state_changed(app);
    Ok(build_status(app, shared))
}

#[tauri::command]
pub fn web_remote_disable<R: Runtime>(app: AppHandle<R>) -> Result<WebRemoteStatus, String> {
    let shared = app.state::<WebRemoteState>().shared();
    disable_core(&app, &shared)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn web_remote_set_config<R: Runtime>(
    app: AppHandle<R>,
    port: Option<u16>,
    require_approval: Option<bool>,
    bind_scope: Option<String>,
    account_mode_enabled: Option<bool>,
    trust_account_browsers: Option<bool>,
    relay_mode_enabled: Option<bool>,
    lan_enabled: Option<bool>,
) -> Result<WebRemoteStatus, String> {
    let shared = app.state::<WebRemoteState>().shared();
    let change = ConfigChange {
        port,
        require_approval,
        bind_scope,
        account_mode_enabled,
        trust_account_browsers,
        relay_mode_enabled,
        lan_enabled,
    };
    set_config_core(&app, &shared, change).await
}

/// A partial config update. Every `Some(…)` is an intentional change; every
/// `None` is left exactly as persisted.
#[derive(Debug, Clone, Default)]
pub(crate) struct ConfigChange {
    pub port: Option<u16>,
    pub require_approval: Option<bool>,
    pub bind_scope: Option<String>,
    pub account_mode_enabled: Option<bool>,
    pub trust_account_browsers: Option<bool>,
    pub relay_mode_enabled: Option<bool>,
    pub lan_enabled: Option<bool>,
}

/// Fold `change` into `cfg`, returning whether the LAN listener's address
/// (port or scope) changed. Pure so the config side is unit-testable.
fn apply_config_change(cfg: &mut WebRemoteConfig, change: ConfigChange) -> bool {
    let old_port = cfg.port;
    let old_scope = cfg.bind_scope.clone();
    if let Some(p) = change.port {
        cfg.port = p;
    }
    if let Some(r) = change.require_approval {
        cfg.require_approval = r;
    }
    if let Some(s) = change.bind_scope {
        cfg.bind_scope = s;
    }
    // Account-mode toggles never rebind the listener (they only gate the
    // `/api/pair-account` admission path).
    if let Some(a) = change.account_mode_enabled {
        cfg.account_mode_enabled = a;
    }
    if let Some(t) = change.trust_account_browsers {
        cfg.trust_account_browsers = t;
    }
    // The two transport switches start/stop their own transport only.
    if let Some(r) = change.relay_mode_enabled {
        cfg.relay_mode_enabled = r;
    }
    if let Some(l) = change.lan_enabled {
        cfg.lan_enabled = l;
    }
    cfg.port != old_port || cfg.bind_scope != old_scope
}

/// The shared body of [`web_remote_set_config`], taking the shared state
/// directly so the `codemux remote enable --scope/--port` change path can reuse
/// the exact same rebind-with-rollback semantics the Settings pane uses.
async fn set_config_core<R: Runtime>(
    app: &AppHandle<R>,
    shared: &Arc<Shared>,
    change: ConfigChange,
) -> Result<WebRemoteStatus, String> {
    // Every `Some(…)` is an intentional change; every `None` must be left
    // exactly as persisted, which requires the persisted values to be in memory
    // before the write-back.
    ensure_config_hydrated(app, shared);

    // Reject an unknown scope before mutating anything, so we never persist a
    // value the bind logic can't honour.
    if let Some(ref s) = change.bind_scope {
        if !is_valid_bind_scope(s) {
            return Err(format!("Unknown access scope: {s}"));
        }
    }

    let (old_port, old_scope, address_changed, switches_changed) = {
        let mut cfg = shared.config.lock().unwrap();
        let (old_port, old_scope) = (cfg.port, cfg.bind_scope.clone());
        let (old_lan, old_relay) = (cfg.lan_enabled, cfg.relay_mode_enabled);
        let address_changed = apply_config_change(&mut cfg, change);
        let switches_changed = cfg.lan_enabled != old_lan || cfg.relay_mode_enabled != old_relay;
        (old_port, old_scope, address_changed, switches_changed)
    };
    persist_config(app, shared);

    // A port or scope change while the LAN listener is bound requires a rebind
    // (which drops its sockets — they can't follow to a new port/interface).
    // Relay sessions are unaffected.
    let (lan_on, running) = (
        lan_wanted(&shared.config.lock().unwrap()),
        shared.runtime.lock().unwrap().is_some(),
    );
    if running && lan_on && address_changed {
        stop_lan(shared);
        if let Err(e) = start_server(app, shared).await {
            // The new binding failed (e.g. switching to `tailscale` with no
            // tailnet address). Restore the previous address and rebind to it
            // so the listener keeps running on its last-good address instead of
            // being left off, then report why the change was rejected.
            {
                let mut cfg = shared.config.lock().unwrap();
                cfg.port = old_port;
                cfg.bind_scope = old_scope;
            }
            persist_config(app, shared);
            let _ = start_listener(app, shared).await;
            emit_state_changed(app);
            return Err(e);
        }
    } else if lan_on && !running && address_changed {
        // Wanted but not listening (the previous bind failed): a port or
        // scope change is how the user fixes that, so try the new value right
        // away. There is no last-good binding to fall back to, so the new
        // value stays persisted either way; a failure is reported and left to
        // the retry loop.
        if let Err(e) = start_listener(app, shared).await {
            emit_state_changed(app);
            return Err(e);
        }
    }

    // Start or stop each transport to match its switch. The relay never waits
    // for the LAN listener (#404): it comes up whenever remote access is on.
    // Turning either on while remote access is off just persists the flag, and
    // `web_remote_enable` starts it then. A LAN listener switched on that
    // can't bind is not an error here — the status carries its `bind_error`
    // and the retry loop keeps trying.
    if switches_changed {
        let _ = reconcile_transports(app, shared).await;
    }

    emit_state_changed(app);
    Ok(build_status(app, shared))
}

/// Retry bringing remote access up right now — the Settings pane's Retry
/// button. Re-attempts the LAN bind when the listener is switched on
/// (reporting its error, if it still fails, while the background retry loop
/// carries on) and re-runs the relay transport + registration when relay mode
/// is on.
#[tauri::command]
pub async fn web_remote_retry<R: Runtime>(app: AppHandle<R>) -> Result<WebRemoteStatus, String> {
    let shared = app.state::<WebRemoteState>().shared();
    ensure_config_hydrated(&app, &shared);
    if !shared.config.lock().unwrap().enabled {
        return Err("Remote access is off".to_string());
    }
    let bind = if lan_wanted(&shared.config.lock().unwrap()) {
        start_listener(&app, &shared).await
    } else {
        Ok(())
    };
    if relay_wanted(&shared.config.lock().unwrap()) && !shared.registration.status().registered {
        // Restart registration so its first attempt runs now instead of at
        // the next backoff tick.
        registration::stop(&shared);
        start_relay(&app, &shared).await;
    }
    emit_state_changed(&app);
    bind.map(|()| build_status(&app, &shared))
}

#[tauri::command]
pub fn web_remote_create_pairing<R: Runtime>(app: AppHandle<R>) -> PairingInfo {
    let shared = app.state::<WebRemoteState>().shared();
    mint_pairing(&shared, None)
}

#[tauri::command]
pub fn web_remote_list_endpoints<R: Runtime>(app: AppHandle<R>) -> Vec<endpoints::Endpoint> {
    let shared = app.state::<WebRemoteState>().shared();
    // The enumerated URLs carry the port, so they must reflect the persisted
    // config rather than the pre-boot default.
    ensure_config_hydrated(&app, &shared);
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
            lan_enabled: true,
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
            lan_enabled: true,
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
            lan_enabled: true,
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
            bind_error: None,
            relay_running: false,
            registration_error: None,
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
            lan_enabled: true,
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
            bind_error: None,
            relay_running: false,
            registration_error: None,
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
        {
            let mut cfg = shared.config.lock().unwrap();
            cfg.enabled = true;
            cfg.lan_enabled = true;
        }

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

    #[test]
    fn control_pair_refuses_while_the_listener_failed_to_bind() {
        // Enabled but the bind failed: a pairing URL would be a dead link, so
        // refuse with the reason (#404) and mint nothing.
        let shared = Arc::new(Shared::default());
        {
            let mut cfg = shared.config.lock().unwrap();
            cfg.enabled = true;
            cfg.lan_enabled = true;
        }
        *shared.bind_error.lock().unwrap() = Some("Address already in use".to_string());
        let err = control_pair_from(&shared, None).unwrap_err();
        assert!(err.contains("isn't listening"), "clear error: {err}");
        assert!(err.contains("Address already in use"), "names the cause: {err}");
        assert_eq!(shared.pairing.live_count(), 0, "no token minted on the error path");
    }

    // ── Relay transport is independent of the LAN listener (#404) ───

    #[test]
    fn relay_is_wanted_whenever_enabled_with_relay_mode_regardless_of_the_listener() {
        let mut cfg = WebRemoteConfig {
            enabled: true,
            relay_mode_enabled: true,
            ..WebRemoteConfig::default()
        };
        // Nothing in the decision looks at whether the listener is bound.
        assert!(relay_wanted(&cfg));
        cfg.relay_mode_enabled = false;
        assert!(!relay_wanted(&cfg), "relay mode off → no relay");
        cfg.relay_mode_enabled = true;
        cfg.enabled = false;
        assert!(!relay_wanted(&cfg), "the master switch still kills every transport");
    }

    #[test]
    fn bind_retry_backs_off_then_holds_at_a_minute() {
        assert_eq!(bind_retry_delay(0), std::time::Duration::from_secs(2));
        assert_eq!(bind_retry_delay(1), std::time::Duration::from_secs(5));
        assert_eq!(bind_retry_delay(4), std::time::Duration::from_secs(60));
        assert_eq!(bind_retry_delay(1000), std::time::Duration::from_secs(60));
    }

    #[test]
    fn status_carries_bind_and_relay_health_in_snake_case() {
        let status = WebRemoteStatus {
            enabled: true,
            running: false,
            lan_enabled: true,
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
            account_signed_in: true,
            relay_mode_enabled: true,
            iroh_node_id: Some("node".to_string()),
            device_registered: false,
            device_id: None,
            bind_error: Some("No Tailscale address found".to_string()),
            relay_running: true,
            registration_error: Some("device registration returned 503".to_string()),
        };
        let v = serde_json::to_value(&status).unwrap();
        assert_eq!(v["lan_enabled"], true);
        assert_eq!(v["bind_error"], "No Tailscale address found");
        assert_eq!(v["relay_running"], true);
        assert_eq!(v["registration_error"], "device registration returned 503");
        assert!(v.get("bindError").is_none(), "no camelCase leakage");
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
            lan_enabled: true,
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
            bind_error: None,
            relay_running: false,
            registration_error: None,
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
            lan_enabled: true,
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
            bind_error: None,
            relay_running: false,
            registration_error: None,
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

    // ── Config hydration (the "enable must not clobber" invariant) ──

    /// The exact shape `codemux connect` leaves behind on a fresh box: remote
    /// access on, relay on, a non-default port.
    fn seed_connect_config(db: &crate::database::DatabaseStore) {
        update_config_headless(db, |cfg| {
            cfg.enabled = true;
            cfg.port = 4399;
            cfg.bind_scope = BIND_SCOPE_LOOPBACK.to_string();
            cfg.relay_mode_enabled = true;
            Ok(())
        })
        .expect("seed config");
    }

    #[test]
    fn enable_after_hydration_leaves_untouched_fields_alone() {
        // `Shared` is built before any DB exists, so its config starts as the
        // all-default value. Persisting THAT (as an enable does) without
        // hydrating first is what used to flip a persisted
        // `relay_mode_enabled: true` to false and reset the port to 4377.
        let db = crate::database::DatabaseStore::new_in_memory();
        seed_connect_config(&db);

        let shared = Arc::new(Shared::default());
        assert!(
            !shared.config.lock().unwrap().relay_mode_enabled,
            "precondition: the pre-boot in-memory config knows nothing"
        );

        hydrate_config_from(&shared, &db);
        // The enable-side mutation: exactly one field.
        shared.config.lock().unwrap().enabled = true;
        let live = shared.config.lock().unwrap().clone();
        save_config_to_db(&db, &live).unwrap();

        let back = load_config_from_db(&db);
        assert!(back.enabled, "the field the caller did change is written");
        assert!(
            back.relay_mode_enabled,
            "a field the caller never touched must survive the write-back"
        );
        assert_eq!(back.port, 4399, "so must the port");
        assert_eq!(back.bind_scope, BIND_SCOPE_LOOPBACK);
    }

    #[test]
    fn hydration_runs_once_and_never_overwrites_live_state() {
        // Hydration is a boot-time load, not a reload: once the running
        // instance owns the config, a later call (a status read, a second
        // enable) must not resurrect the on-disk copy over in-memory changes
        // that have not been persisted yet.
        let db = crate::database::DatabaseStore::new_in_memory();
        seed_connect_config(&db);

        let shared = Arc::new(Shared::default());
        hydrate_config_from(&shared, &db);
        assert_eq!(shared.config.lock().unwrap().port, 4399);

        shared.config.lock().unwrap().port = 5000;
        hydrate_config_from(&shared, &db);
        assert_eq!(
            shared.config.lock().unwrap().port,
            5000,
            "a second hydrate must be a no-op"
        );
    }

    #[test]
    fn hydration_of_an_unconfigured_machine_yields_the_defaults() {
        // First run: no row at all. Hydration must not invent anything — the
        // defaults are what `serve`'s first-run path then folds its flags into.
        let db = crate::database::DatabaseStore::new_in_memory();
        let shared = Arc::new(Shared::default());
        hydrate_config_from(&shared, &db);
        let cfg = shared.config.lock().unwrap().clone();
        assert!(!cfg.enabled);
        assert_eq!(cfg.port, DEFAULT_PORT);
        assert_eq!(cfg.bind_scope, BIND_SCOPE_ALL);
        assert!(!cfg.relay_mode_enabled);
    }

    // ── Transport independence (the kill switch + per-way-in switches) ──

    /// A [`TransportDriver`] that records calls and fails the LAN start on
    /// request, so the reconcile rules are checked without binding sockets or
    /// an iroh endpoint.
    #[derive(Default)]
    struct ScriptedDriver {
        lan_fails_with: Option<String>,
        calls: Mutex<Vec<&'static str>>,
    }

    impl ScriptedDriver {
        fn calls(&self) -> Vec<&'static str> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl TransportDriver for ScriptedDriver {
        async fn start_lan(&self) -> Result<(), String> {
            self.calls.lock().unwrap().push("start_lan");
            self.lan_fails_with.clone().map_or(Ok(()), Err)
        }
        fn stop_lan(&self) {
            self.calls.lock().unwrap().push("stop_lan");
        }
        async fn start_relay(&self) {
            self.calls.lock().unwrap().push("start_relay");
        }
        fn stop_relay(&self) {
            self.calls.lock().unwrap().push("stop_relay");
        }
    }

    fn shared_with(cfg: WebRemoteConfig) -> Shared {
        let shared = Shared::default();
        *shared.config.lock().unwrap() = cfg;
        shared
    }

    #[tokio::test]
    async fn relay_starts_even_when_the_lan_listener_fails_to_bind() {
        // The #404 shape: with the LAN listener unable to bind, the relay must
        // still come up, and the bind failure is handed back to the caller
        // rather than swallowed.
        let shared = shared_with(WebRemoteConfig {
            enabled: true,
            lan_enabled: true,
            relay_mode_enabled: true,
            ..WebRemoteConfig::default()
        });
        let driver = ScriptedDriver {
            lan_fails_with: Some("bind 0.0.0.0:4377: Address already in use".into()),
            ..ScriptedDriver::default()
        };

        let lan = reconcile_transports_with(&shared, &driver).await;

        assert_eq!(driver.calls(), vec!["start_lan", "start_relay"]);
        assert_eq!(lan.unwrap_err(), "bind 0.0.0.0:4377: Address already in use");
        assert!(
            shared.config.lock().unwrap().enabled,
            "reconciling never flips the kill switch off behind the user's back"
        );
    }

    #[tokio::test]
    async fn relay_only_setup_never_opens_the_lan_listener() {
        let shared = shared_with(WebRemoteConfig {
            enabled: true,
            lan_enabled: false,
            relay_mode_enabled: true,
            ..WebRemoteConfig::default()
        });
        let driver = ScriptedDriver::default();

        assert!(reconcile_transports_with(&shared, &driver).await.is_ok());

        assert_eq!(driver.calls(), vec!["stop_lan", "start_relay"]);
    }

    #[tokio::test]
    async fn the_kill_switch_stops_both_transports() {
        let shared = shared_with(WebRemoteConfig {
            enabled: false,
            lan_enabled: true,
            relay_mode_enabled: true,
            ..WebRemoteConfig::default()
        });
        let driver = ScriptedDriver::default();

        assert!(reconcile_transports_with(&shared, &driver).await.is_ok());

        assert_eq!(driver.calls(), vec!["stop_lan", "stop_relay"]);
    }

    #[test]
    fn desired_transports_follow_their_own_switch_under_the_kill_switch() {
        let cfg = |enabled, lan_enabled, relay_mode_enabled| WebRemoteConfig {
            enabled,
            lan_enabled,
            relay_mode_enabled,
            ..WebRemoteConfig::default()
        };
        assert_eq!(desired_transports(&cfg(false, true, true)), (false, false));
        assert_eq!(desired_transports(&cfg(true, true, false)), (true, false));
        assert_eq!(desired_transports(&cfg(true, false, true)), (false, true));
        assert_eq!(desired_transports(&cfg(true, true, true)), (true, true));
    }

    #[test]
    fn config_without_lan_enabled_keeps_its_listener() {
        // Before the split, `enabled` *was* the LAN listener. A config persisted
        // then must come back with the listener still on.
        let legacy = r#"{"enabled":true,"port":4377,"require_approval":false,"bind_scope":"all","relay_mode_enabled":true}"#;
        let cfg: WebRemoteConfig = serde_json::from_str(legacy).unwrap();
        assert!(cfg.lan_enabled);
        // A fresh config opens nothing on the network until asked to.
        assert!(!WebRemoteConfig::default().lan_enabled);
    }

    #[test]
    fn apply_enable_request_turns_the_lan_listener_on() {
        // `codemux remote enable` exists to open the network listener, so it
        // turns that way in back on after a relay-only setup.
        let mut cfg = WebRemoteConfig {
            lan_enabled: false,
            ..WebRemoteConfig::default()
        };
        apply_enable_request(&mut cfg, None, None).unwrap();
        assert!(cfg.enabled && cfg.lan_enabled);
    }

    #[test]
    fn control_pair_errors_when_the_lan_listener_is_off() {
        // Pairing links point at the LAN listener; in a relay-only setup there
        // is nothing for the link to reach.
        let shared = Arc::new(Shared::default());
        {
            let mut cfg = shared.config.lock().unwrap();
            cfg.enabled = true;
            cfg.lan_enabled = false;
        }
        let err = control_pair_from(&shared, None).unwrap_err();
        assert!(err.contains("On my network"), "clear error: {err}");
        assert_eq!(shared.pairing.live_count(), 0);
    }

    #[test]
    fn config_change_reports_address_changes_only() {
        let mut cfg = WebRemoteConfig::default();
        let toggles = ConfigChange {
            lan_enabled: Some(false),
            relay_mode_enabled: Some(true),
            require_approval: Some(true),
            ..ConfigChange::default()
        };
        assert!(!apply_config_change(&mut cfg, toggles), "switches never rebind");
        assert!(!cfg.lan_enabled && cfg.relay_mode_enabled && cfg.require_approval);

        let same_port = ConfigChange {
            port: Some(cfg.port),
            ..ConfigChange::default()
        };
        assert!(!apply_config_change(&mut cfg, same_port));

        let scope = ConfigChange {
            bind_scope: Some(BIND_SCOPE_LOOPBACK.into()),
            ..ConfigChange::default()
        };
        assert!(apply_config_change(&mut cfg, scope));
    }

    #[test]
    fn shared_channel_router_is_shared_by_clone() {
        let state = WebRemoteState::default();
        let a = state.channel_router();
        let b = state.shared().channels.clone();
        assert!(Arc::ptr_eq(&a, &b), "interceptor + dispatcher share one router");
    }
}
