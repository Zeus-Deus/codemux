//! Native-only administration for inbound external-agent connections.
//! router() is served on the connector's own loopback listener (listener.rs),
//! never on the web-remote listener, so web-remote's switch, bind scope and
//! relay do not reach this door.
//! NEVER put these commands on the paired-browser invoke allowlist.
mod http;
pub(crate) mod listener;
pub mod oauth;
pub use http::router;
use oauth::protocol;

use crate::agent_control::{ControlAccess, ControlCaller, ControlError, ErrorCode};
use crate::database::{
    mcp_connector::{NewPersonalToken, PERSONAL_TOKEN_DEFAULT_TTL_MS},
    DatabaseStore,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub const CHANGED_EVENT: &str = "agent-connector-changed";

/// HTTP requests in progress at once, waits excluded.
const MAX_REQUESTS: usize = 16;
/// Calls parked in a wait. They hold one of these instead of a request
/// permit, so long waits cannot starve token refresh and every other call.
const MAX_WAITS: usize = 32;
/// One client cannot take every wait slot.
const MAX_WAITS_PER_CLIENT: usize = 8;

#[derive(Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Config {
    pub enabled: bool,
    pub public_origin: Option<String>,
    /// Loopback port of the connector's own listener; omitted = DEFAULT_PORT.
    /// Not serialized when unset, so older builds can still read the row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}
impl Config {
    pub(crate) fn configured_port(&self) -> u16 {
        self.port.unwrap_or(listener::DEFAULT_PORT)
    }
}

pub struct McpConnectorState {
    // Also serializes synchronous config changes, native decisions, code mint,
    // token persistence and revocation. Never held over an await.
    pub(crate) auth: Mutex<oauth::AuthState>,
    pub(crate) rate: Mutex<protocol::Rates>,
    pub(crate) concurrency: Arc<Semaphore>,
    pub(crate) waiters: Arc<Semaphore>,
    waits_by_client: Arc<Mutex<HashMap<String, usize>>>,
    pub(crate) listener: Mutex<listener::Listener>,
    /// Bound port of the own listener, 0 while stopped. Read without locks so
    /// `live()` can run under `auth` without lock nesting.
    pub(crate) bound_port: AtomicU16,
}
impl Default for McpConnectorState {
    fn default() -> Self {
        Self {
            auth: Mutex::new(oauth::AuthState::default()),
            rate: Mutex::new(protocol::Rates::default()),
            concurrency: Arc::new(Semaphore::new(MAX_REQUESTS)),
            waiters: Arc::new(Semaphore::new(MAX_WAITS)),
            waits_by_client: Arc::default(),
            listener: Mutex::default(),
            bound_port: AtomicU16::new(0),
        }
    }
}

/// A wait slot for one parked call; dropping it frees the slot.
pub(crate) struct WaitSlot {
    _permit: OwnedSemaphorePermit,
    principal: String,
    counts: Arc<Mutex<HashMap<String, usize>>>,
}
impl Drop for WaitSlot {
    fn drop(&mut self) {
        let Ok(mut counts) = self.counts.lock() else {
            return;
        };
        if let Some(held) = counts.get_mut(&self.principal) {
            *held -= 1;
            if *held == 0 {
                counts.remove(&self.principal);
            }
        }
    }
}
impl McpConnectorState {
    /// A slot for a call that may park, or `None` when the connector or this
    /// client already holds as many waits as it may.
    pub(crate) fn admit_wait(&self, principal: &str) -> Option<WaitSlot> {
        let mut counts = self.waits_by_client.lock().ok()?;
        if counts.get(principal).copied().unwrap_or(0) >= MAX_WAITS_PER_CLIENT {
            return None;
        }
        let permit = self.waiters.clone().try_acquire_owned().ok()?;
        *counts.entry(principal.to_owned()).or_default() += 1;
        Some(WaitSlot {
            _permit: permit,
            principal: principal.to_owned(),
            counts: self.waits_by_client.clone(),
        })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorPending {
    pub id: String,
    pub client_name: String,
    pub callback_origin: String,
    pub requested_at: String,
    pub expires_at: String,
    pub phase: oauth::AuthorizationPhase,
    pub access: Option<ControlAccess>,
    /// The owner approved this client before: its sign-in lapsed or was revoked.
    pub reconnecting: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientKind {
    /// An assistant that signed in through OAuth.
    Oauth,
    /// A labelled personal token.
    Token,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorClient {
    pub id: String,
    pub client_name: String,
    /// `None` for tokens, which have no callback.
    pub callback_origin: Option<String>,
    pub access: ControlAccess,
    pub created_at: String,
    /// When access lapses if unused: a renewing sign-in's refresh expiry,
    /// otherwise the token's own expiry.
    pub expires_at: String,
    pub kind: ClientKind,
    pub label: Option<String>,
    /// The sign-in renews itself with a refresh token.
    pub renews: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorStatus {
    pub enabled: bool,
    pub public_origin: Option<String>,
    pub listener_running: bool,
    /// The bound port, else the configured one.
    pub port: u16,
    pub bind_error: Option<String>,
    /// `http://127.0.0.1:<port>/mcp` while listening: what a proxy points at.
    pub local_url: Option<String>,
    pub mcp_url: Option<String>,
    pub local_command: String,
    /// Only in `agent_connector_set_config` replies: grants an address change revoked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revoked_grants: Option<usize>,
    pub pending: Vec<ConnectorPending>,
    pub clients: Vec<ConnectorClient>,
}

/// A personal token, shown once.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssuedToken {
    pub token: String,
    pub client_id: String,
    pub label: String,
    pub access: ControlAccess,
    pub expires_at: String,
    /// `{issuer}/mcp`: the public origin's when one is saved.
    pub mcp_url: String,
    pub local_url: String,
    /// A live token for this label stopped working.
    pub replaced: bool,
}

pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
fn timestamp(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| t.to_rfc3339())
        .unwrap_or_default()
}
pub(crate) fn callback_origin(uri: &str) -> String {
    url::Url::parse(uri)
        .map(|u| u.origin().ascii_serialization())
        .unwrap_or_else(|_| "invalid callback".into())
}
pub(crate) fn access_name(access: ControlAccess) -> &'static str {
    match access {
        ControlAccess::ReadOnly => "read_only",
        ControlAccess::Supervised => "supervised",
        ControlAccess::FullAccess => "full_access",
    }
}
pub(crate) fn parse_access(value: &str) -> Result<ControlAccess, String> {
    match value {
        "read_only" => Ok(ControlAccess::ReadOnly),
        "supervised" => Ok(ControlAccess::Supervised),
        "full_access" => Ok(ControlAccess::FullAccess),
        _ => Err("Invalid grant access".into()),
    }
}
pub(crate) fn scope(access: &str) -> &'static str {
    match access {
        "supervised" => "codemux:read codemux:supervised",
        "full_access" => "codemux:read codemux:full",
        _ => "codemux:read",
    }
}
pub(crate) fn changed<R: Runtime>(app: &AppHandle<R>) {
    // The event bus also has paired browser subscribers. Never send a code,
    // credential, browser binding, or internal pending authorization tuple.
    let _ = app.emit(CHANGED_EVENT, serde_json::Value::Null);
}
/// Some caller may have lost its authority (revoked, disabled, address
/// changed, listener stopped): tell the UI, and wake every parked wait so it
/// re-checks its grant and ends instead of sleeping out its timeout.
pub(crate) fn authority_changed<R: Runtime>(app: &AppHandle<R>) {
    changed(app);
    crate::agent_control::wait::signal_all();
}

pub(crate) fn config(db: &DatabaseStore) -> Result<Config, String> {
    let cfg: Config = match db.mcp_connector_config()? {
        Some(value) => {
            serde_json::from_str(&value).map_err(|_| "Invalid connector configuration")?
        }
        None => Config::default(),
    };
    if let Some(origin) = &cfg.public_origin {
        if oauth::public_origin(origin)? != *origin {
            return Err("Invalid canonical public origin".into());
        }
    }
    Ok(cfg)
}

/// What assistants address: the public origin, else this machine's loopback port.
pub(crate) fn issuer(cfg: &Config, port: u16) -> String {
    cfg.public_origin
        .clone()
        .unwrap_or_else(|| format!("http://127.0.0.1:{port}"))
}
/// Bound port while serving, else the configured one: a stopped listener keeps
/// the issuer stable, so pending requests and grants survive an outage.
fn issuer_port<R: Runtime>(app: &AppHandle<R>, cfg: &Config) -> u16 {
    match app.state::<McpConnectorState>().bound_port.load(Ordering::SeqCst) {
        0 => cfg.configured_port(),
        bound => bound,
    }
}
fn local_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}/mcp")
}

/// Serving: enabled and the own listener bound. The port is the bound one,
/// never from Host or Forwarded. Used by HTTP preflight and approval.
pub(crate) fn live<R: Runtime>(app: &AppHandle<R>) -> Result<(Config, String, u16), String> {
    let cfg = config(&app.state::<DatabaseStore>())?;
    let port = app.state::<McpConnectorState>().bound_port.load(Ordering::SeqCst);
    if !cfg.enabled || port == 0 {
        return Err("HTTP agent connector is disabled or its listener is stopped".into());
    }
    let issuer = issuer(&cfg, port);
    Ok((cfg, issuer, port))
}

/// Re-checks an outside caller's grant. Needs the switch and a live grant,
/// not a bound listener: queued turns and background work admitted under a
/// grant keep running through a listener outage.
pub fn validate_caller<R: Runtime>(
    app: &AppHandle<R>,
    caller: &ControlCaller,
) -> Result<(), ControlError> {
    let Some(id) = &caller.grant_id else {
        return Ok(());
    };
    let state = app.state::<McpConnectorState>();
    let _gate = state
        .auth
        .lock()
        .map_err(|_| ControlError::new(ErrorCode::ConnectorUnavailable, "Connector unavailable"))?;
    let cfg = config(&app.state::<DatabaseStore>())
        .ok()
        .filter(|cfg| cfg.enabled)
        .ok_or_else(|| ControlError::new(ErrorCode::ConnectorDisabled, "Connector disabled"))?;
    let issuer = issuer(&cfg, issuer_port(app, &cfg));
    let grant = app
        .state::<DatabaseStore>()
        .mcp_grant(id, &format!("{issuer}/mcp"), now_ms())
        .map_err(|_| ControlError::new(ErrorCode::ConnectorUnavailable, "Grant lookup failed"))?
        .ok_or_else(|| {
            ControlError::new(
                ErrorCode::GrantRevoked,
                "Grant is expired, revoked or bound to another resource",
            )
        })?;
    if caller.principal != format!("mcp-client:{}", grant.client_id)
        || parse_access(&grant.access).map_err(|e| ControlError::new(ErrorCode::InvalidGrant, e))?
            != caller.access
    {
        return Err(ControlError::new(
            ErrorCode::GrantChanged,
            "Grant identity or access changed",
        ));
    }
    Ok(())
}

/// Bind the persisted connector at startup (GUI and serve).
pub fn restore_on_boot<R: Runtime>(app: &AppHandle<R>) {
    match config(&app.state::<DatabaseStore>()) {
        Ok(cfg) => {
            listener::reconcile(app, &cfg);
            if cfg.enabled {
                changed(app);
            }
        }
        Err(e) => log::warn!("[codemux::mcp_connector] not restoring the connector: {e}"),
    }
}

pub(crate) struct ConfigUpdate {
    pub enabled: bool,
    pub public_origin: Option<String>,
    /// `None` keeps the configured port.
    pub port: Option<u16>,
}

/// Save the switch, origin and port, then bring the listener to match.
/// Disabling pauses: pending sign-ins end, grants stay. A new address (origin,
/// or port without an origin) also revokes every grant bound to the old one.
pub(crate) fn apply_config<R: Runtime>(
    app: &AppHandle<R>,
    update: ConfigUpdate,
) -> Result<ConnectorStatus, String> {
    let public_origin = update
        .public_origin
        .map(|o| oauth::public_origin(&o))
        .transpose()?;
    // Tests bind an ephemeral port with 0.
    if cfg!(not(test)) && update.port == Some(0) {
        return Err("Choose a port between 1 and 65535".into());
    }
    let (cfg, revoked) = {
        let state = app.state::<McpConnectorState>();
        let mut auth = state.auth.lock().map_err(|_| "Connector unavailable")?;
        let db = app.state::<DatabaseStore>();
        let prior = config(&db)?;
        let cfg = Config {
            enabled: update.enabled,
            port: update.port.or(prior.port),
            public_origin,
        };
        db.mcp_connector_set_config(
            &serde_json::to_string(&cfg).map_err(|_| "Invalid configuration")?,
        )?;
        let new_issuer = issuer(&cfg, cfg.configured_port());
        let moved = issuer(&prior, prior.configured_port()) != new_issuer;
        if !cfg.enabled || moved {
            auth.clear();
        }
        let revoked = if moved {
            let now = now_ms();
            // Count only grants that still had authority.
            db.mcp_prune_clients(now, &auth.protected_clients(now))?;
            db.mcp_revoke_grants_not_bound_to(&format!("{new_issuer}/mcp"))?
        } else {
            0
        };
        (cfg, revoked)
    };
    listener::reconcile(app, &cfg);
    authority_changed(app);
    let mut status = agent_connector_status(app.clone())?;
    status.revoked_grants = Some(revoked);
    Ok(status)
}

#[tauri::command]
pub fn agent_connector_status<R: Runtime>(app: AppHandle<R>) -> Result<ConnectorStatus, String> {
    let state = app.state::<McpConnectorState>();
    let mut auth = state.auth.lock().map_err(|_| "Connector unavailable")?;
    let db = app.state::<DatabaseStore>();
    let cfg = config(&db)?;
    let now = now_ms();
    auth.prune(now);
    if !cfg.enabled {
        auth.clear();
    }
    let bound = state.bound_port.load(Ordering::SeqCst);
    let running = bound != 0;
    let port = issuer_port(&app, &cfg);
    let issuer = issuer(&cfg, port);
    let bind_error = state
        .listener
        .lock()
        .map_err(|_| "Connector unavailable")?
        .bind_error
        .clone();
    auth.retain_issuer(&issuer);
    db.mcp_prune_clients(now, &auth.protected_clients(now))?;
    let mut pending: Vec<_> = auth
        .pending
        .iter()
        .map(|(id, p)| {
            let (phase, access) = p.consent();
            Ok(ConnectorPending {
                id: id.clone(),
                client_name: p.authorization.client_name.clone(),
                callback_origin: callback_origin(&p.authorization.redirect_uri),
                requested_at: timestamp(p.requested_at),
                expires_at: timestamp(p.expires_at),
                phase,
                access: access.map(parse_access).transpose()?,
                reconnecting: db
                    .mcp_client_approved_at(&p.authorization.client_id)?
                    .is_some(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    pending.sort_by(|a, b| a.requested_at.cmp(&b.requested_at).then(a.id.cmp(&b.id)));
    let clients = db
        .mcp_list_grants(now)?
        .into_iter()
        .map(|g| {
            let kind = if g.label.is_some() {
                ClientKind::Token
            } else {
                ClientKind::Oauth
            };
            Ok(ConnectorClient {
                id: g.client_id,
                client_name: g.client_name,
                callback_origin: (kind == ClientKind::Oauth)
                    .then(|| callback_origin(&g.redirect_uri)),
                access: parse_access(&g.access)?,
                created_at: timestamp(g.created_at),
                expires_at: timestamp(g.expires_at.max(g.refresh_expires_at.unwrap_or(0))),
                kind,
                renews: g.refresh_expires_at.is_some(),
                label: g.label,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(ConnectorStatus {
        enabled: cfg.enabled,
        public_origin: cfg.public_origin,
        listener_running: running,
        port,
        bind_error,
        local_url: running.then(|| local_url(bound)),
        mcp_url: (cfg.enabled && running).then(|| format!("{issuer}/mcp")),
        local_command: "codemux mcp".into(),
        revoked_grants: None,
        pending,
        clients,
    })
}

/// `port` is optional: omitted keeps the configured port.
#[tauri::command]
pub fn agent_connector_set_config<R: Runtime>(
    app: AppHandle<R>,
    enabled: bool,
    public_origin: Option<String>,
    port: Option<u16>,
) -> Result<ConnectorStatus, String> {
    apply_config(
        &app,
        ConfigUpdate {
            enabled,
            public_origin,
            port,
        },
    )
}

/// Issue a personal token for a headless bot, or rotate the label's token.
/// Needs the switch on, not a bound listener: the issuer comes from config.
#[tauri::command]
pub fn agent_connector_issue<R: Runtime>(
    app: AppHandle<R>,
    label: String,
    access: ControlAccess,
    expires_in_hours: Option<u32>,
) -> Result<IssuedToken, String> {
    let lifetime = expires_in_hours
        .map(|hours| i64::from(hours) * 3_600_000)
        .unwrap_or(PERSONAL_TOKEN_DEFAULT_TTL_MS);
    let issued = {
        let state = app.state::<McpConnectorState>();
        let mut auth = state.auth.lock().map_err(|_| "Connector unavailable")?;
        let db = app.state::<DatabaseStore>();
        let cfg = config(&db)?;
        if !cfg.enabled {
            return Err("Turn the connector on first: codemux connector enable".into());
        }
        let port = issuer_port(&app, &cfg);
        let mcp_url = format!("{}/mcp", issuer(&cfg, port));
        let token = oauth::access_token();
        let now = now_ms();
        let minted = db.mcp_issue_personal_token(NewPersonalToken {
            label,
            access: access_name(access).into(),
            resource: mcp_url.clone(),
            token_hash: oauth::credential_hash(&token),
            now,
            expires_at: now + lifetime,
            protected_clients: auth.protected_clients(now),
        })?;
        // The store keeps a label's first spelling; show that one.
        let grant = db
            .mcp_list_grants(now)?
            .into_iter()
            .find(|g| g.id == minted.grant_id)
            .ok_or("Token persistence failed")?;
        IssuedToken {
            token,
            client_id: minted.client_id,
            label: grant.label.unwrap_or(grant.client_name),
            access,
            expires_at: timestamp(grant.expires_at),
            mcp_url,
            local_url: local_url(port),
            replaced: minted.replaced,
        }
    };
    authority_changed(&app);
    Ok(issued)
}

#[tauri::command]
pub fn agent_connector_approve<R: Runtime>(
    app: AppHandle<R>,
    request_id: String,
    access: ControlAccess,
) -> Result<(), String> {
    {
        let state = app.state::<McpConnectorState>();
        let mut auth = state.auth.lock().map_err(|_| "Connector unavailable")?;
        let (_, issuer, _) = live(&app)?;
        auth.retain_issuer(&issuer);
        auth.decide(&request_id, Some(access_name(access).into()), now_ms())?;
    }
    changed(&app);
    Ok(())
}
#[tauri::command]
pub fn agent_connector_deny<R: Runtime>(
    app: AppHandle<R>,
    request_id: String,
) -> Result<(), String> {
    {
        let state = app.state::<McpConnectorState>();
        let mut auth = state.auth.lock().map_err(|_| "Connector unavailable")?;
        // Native rejection only reduces authority; it does not need a listener.
        auth.decide(&request_id, None, now_ms())?;
    }
    changed(&app);
    Ok(())
}
#[tauri::command]
pub fn agent_connector_revoke<R: Runtime>(
    app: AppHandle<R>,
    client_id: String,
) -> Result<(), String> {
    {
        let state = app.state::<McpConnectorState>();
        let mut auth = state.auth.lock().map_err(|_| "Connector unavailable")?;
        app.state::<DatabaseStore>().mcp_revoke_client(&client_id)?;
        auth.revoke_client(&client_id);
    }
    authority_changed(&app);
    Ok(())
}
/// The incident switch: every OAuth sign-in and personal token stops working,
/// and approvals not yet exchanged for a token are withdrawn.
#[tauri::command]
pub fn agent_connector_revoke_all<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    {
        let state = app.state::<McpConnectorState>();
        let mut auth = state.auth.lock().map_err(|_| "Connector unavailable")?;
        app.state::<DatabaseStore>().mcp_revoke_all()?;
        auth.withdraw_approvals();
    }
    authority_changed(&app);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::mcp_connector::{NewMcpClient, NewMcpGrant, PERSONAL_CLIENT_PREFIX};
    use std::time::{Duration, Instant};
    use tauri::{Listener, Manager};

    fn app() -> tauri::App<tauri::test::MockRuntime> {
        let app = tauri::test::mock_app();
        app.manage(crate::database::DatabaseStore::new_in_memory());
        app.manage(crate::web_remote::WebRemoteState::default());
        app.manage(McpConnectorState::default());
        app
    }
    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }
    fn set(
        handle: &AppHandle<tauri::test::MockRuntime>,
        enabled: bool,
        origin: Option<&str>,
        port: Option<u16>,
    ) -> Result<ConnectorStatus, String> {
        agent_connector_set_config(handle.clone(), enabled, origin.map(str::to_owned), port)
    }
    fn reachable(port: u16) -> bool {
        std::net::TcpStream::connect_timeout(
            &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
            Duration::from_millis(200),
        )
        .is_ok()
    }
    /// A client with an owner-approved grant bound to `resource`, as `token()` mints it.
    fn approved_client(
        handle: &AppHandle<tauri::test::MockRuntime>,
        client_id: &str,
        resource: &str,
    ) -> ControlCaller {
        let now = now_ms();
        let db = handle.state::<DatabaseStore>();
        db.mcp_register_client(
            NewMcpClient {
                id: client_id.into(),
                name: "Synthetic outside agent".into(),
                redirects: vec!["https://client.example/callback".into()],
                created_at: now,
            },
            now,
        )
        .unwrap();
        let grant_id = format!("{client_id}-grant");
        db.mcp_mint_grant(NewMcpGrant {
            id: grant_id.clone(),
            client_id: client_id.into(),
            token_hash: oauth::credential_hash(&oauth::access_token()),
            redirect_uri: "https://client.example/callback".into(),
            resource: resource.into(),
            access: "supervised".into(),
            created_at: now,
            expires_at: now + 60_000,
        })
        .unwrap();
        ControlCaller::outside(client_id.into(), grant_id, ControlAccess::Supervised)
    }
    fn begin(
        handle: &AppHandle<tauri::test::MockRuntime>,
        client_id: &str,
        issuer: &str,
    ) -> String {
        let mut authorization = pending_authorization();
        authorization.client_id = client_id.into();
        authorization.resource = format!("{issuer}/mcp");
        authorization.issuer = issuer.into();
        handle
            .state::<McpConnectorState>()
            .auth
            .lock()
            .unwrap()
            .begin(authorization, now_ms())
            .unwrap()
            .0
    }

    #[test]
    fn connector_binds_its_own_loopback_listener_and_never_touches_web_remote() {
        let app = app();
        let handle = app.handle();
        let default = agent_connector_status(handle.clone()).unwrap();
        assert!(!default.enabled && !default.listener_running);
        assert!(default.mcp_url.is_none() && default.local_url.is_none());
        assert!(default.bind_error.is_none());
        let before = handle
            .state::<DatabaseStore>()
            .get_setting("web_remote.config");
        let port = free_port();
        let status = set(handle, true, None, Some(port)).unwrap();
        let local = format!("http://127.0.0.1:{port}/mcp");
        assert!(status.enabled && status.listener_running);
        assert_eq!(status.port, port);
        assert_eq!(status.local_url.as_deref(), Some(local.as_str()));
        assert_eq!(status.mcp_url.as_deref(), Some(local.as_str()));
        assert!(status.bind_error.is_none());
        assert!(reachable(port), "the connector must listen on 127.0.0.1:{port}");
        // A public origin names the proxy's address; the listener stays local.
        let status = set(handle, true, Some("https://mcp.example"), None).unwrap();
        assert_eq!(status.mcp_url.as_deref(), Some("https://mcp.example/mcp"));
        assert_eq!(status.local_url.as_deref(), Some(local.as_str()));
        assert_eq!(
            handle
                .state::<DatabaseStore>()
                .get_setting("web_remote.config"),
            before,
            "the connector never writes web-remote's configuration"
        );
        assert!(!crate::web_remote::web_remote_status(handle.clone()).running);
        let status = set(handle, false, Some("https://mcp.example"), None).unwrap();
        assert!(!status.listener_running);
        assert!(status.local_url.is_none() && status.mcp_url.is_none());
        let deadline = Instant::now() + Duration::from_secs(1);
        while reachable(port) {
            assert!(Instant::now() < deadline, "the listener still accepts after disable");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn connector_bind_failure_is_reported_without_falling_back_to_another_port() {
        let app = app();
        let handle = app.handle();
        let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = occupied.local_addr().unwrap().port();
        let status = set(handle, true, None, Some(port)).unwrap();
        assert!(status.enabled && !status.listener_running);
        assert_eq!(status.port, port, "the issuer port never moves to a free one");
        let error = status.bind_error.unwrap();
        assert!(error.starts_with(&format!("bind 127.0.0.1:{port}: ")), "{error}");
        assert!(status.local_url.is_none() && status.mcp_url.is_none());
        let json = serde_json::to_value(agent_connector_status(handle.clone()).unwrap()).unwrap();
        assert_eq!(json["bindError"], error.as_str());
        assert!(json["localUrl"].is_null());
        // Saving the same settings again is the retry.
        drop(occupied);
        let status = set(handle, true, None, None).unwrap();
        assert!(status.listener_running && status.bind_error.is_none());
        assert_eq!(status.port, port);
        set(handle, false, None, None).unwrap();
        assert!(agent_connector_status(handle.clone()).unwrap().bind_error.is_none());
    }

    #[test]
    fn issuer_change_revokes_grants_and_clears_pending() {
        for change in ["origin", "port"] {
            let app = app();
            let handle = app.handle();
            let port = free_port();
            set(handle, true, None, Some(port)).unwrap();
            let issuer = format!("http://127.0.0.1:{port}");
            let caller = approved_client(handle, "bound-client", &format!("{issuer}/mcp"));
            begin(handle, "bound-client", &issuer);
            // Saving the same address keeps everything.
            let same = set(handle, true, None, None).unwrap();
            assert_eq!(same.revoked_grants, Some(0), "{change}");
            assert_eq!(same.pending.len(), 1, "{change}");
            assert!(validate_caller(handle, &caller).is_ok(), "{change}");
            let moved = match change {
                "origin" => set(handle, true, Some("https://mcp.example"), None),
                _ => set(handle, true, None, Some(free_port())),
            }
            .unwrap();
            assert_eq!(moved.revoked_grants, Some(1), "{change}");
            assert!(moved.pending.is_empty(), "{change}: pending sign-ins name the old address");
            assert!(moved.clients.is_empty(), "{change}");
            assert_eq!(
                validate_caller(handle, &caller).unwrap_err().code,
                ErrorCode::GrantRevoked,
                "{change}"
            );
            // A plain status read is not a set_config reply.
            let status = serde_json::to_value(agent_connector_status(handle.clone()).unwrap()).unwrap();
            assert!(status.get("revokedGrants").is_none(), "{change}");
            set(handle, false, None, None).unwrap();
        }
    }

    #[test]
    fn disabling_pauses_connections_and_enabling_resumes_them() {
        let app = app();
        let handle = app.handle();
        let port = free_port();
        set(handle, true, None, Some(port)).unwrap();
        let caller = approved_client(handle, "paused-client", &format!("http://127.0.0.1:{port}/mcp"));
        let paused = set(handle, false, None, None).unwrap();
        assert_eq!(paused.revoked_grants, Some(0));
        assert_eq!(paused.clients.len(), 1, "disabling keeps grants");
        assert_eq!(
            validate_caller(handle, &caller).unwrap_err().code,
            ErrorCode::ConnectorDisabled
        );
        set(handle, true, None, None).unwrap();
        assert!(validate_caller(handle, &caller).is_ok());
        set(handle, false, None, None).unwrap();
    }

    #[test]
    fn revoke_all_revokes_oauth_grants_and_personal_tokens() {
        let app = app();
        let handle = app.handle();
        let port = free_port();
        set(handle, true, None, Some(port)).unwrap();
        let issuer = format!("http://127.0.0.1:{port}");
        let resource = format!("{issuer}/mcp");
        let oauth_caller = approved_client(handle, "oauth-client", &resource);
        let token = agent_connector_issue(handle.clone(), "bot".into(), ControlAccess::Supervised, None)
            .unwrap();
        let db = handle.state::<DatabaseStore>();
        let token_grant = db
            .mcp_grant_by_hash(&oauth::credential_hash(&token.token), &resource, now_ms())
            .unwrap()
            .unwrap();
        let token_caller = ControlCaller::outside(
            token.client_id.clone(),
            token_grant.id,
            ControlAccess::Supervised,
        );
        assert!(validate_caller(handle, &token_caller).is_ok());
        let approved = begin(handle, "oauth-client", &issuer);
        agent_connector_approve(handle.clone(), approved.clone(), ControlAccess::FullAccess).unwrap();
        let awaiting = begin(handle, "oauth-client", &issuer);
        agent_connector_revoke_all(handle.clone()).unwrap();
        let status = agent_connector_status(handle.clone()).unwrap();
        assert!(status.clients.is_empty());
        for caller in [&oauth_caller, &token_caller] {
            assert_eq!(validate_caller(handle, caller).unwrap_err().code, ErrorCode::GrantRevoked);
        }
        assert!(db
            .mcp_grant_by_hash(&oauth::credential_hash(&token.token), &resource, now_ms())
            .unwrap()
            .is_none());
        let pending: Vec<_> = status.pending.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(pending, [awaiting.as_str()], "an approval not yet exchanged is withdrawn");
        assert!(status.enabled, "revoke all does not turn the connector off");
        set(handle, false, None, None).unwrap();
    }

    #[test]
    fn pending_requests_from_previously_approved_clients_are_marked_reconnecting() {
        let app = app();
        let handle = app.handle();
        let port = free_port();
        set(handle, true, None, Some(port)).unwrap();
        let issuer = format!("http://127.0.0.1:{port}");
        approved_client(handle, "returning-client", &format!("{issuer}/mcp"));
        agent_connector_revoke(handle.clone(), "returning-client".into()).unwrap();
        let now = now_ms();
        handle
            .state::<DatabaseStore>()
            .mcp_register_client(
                NewMcpClient {
                    id: "new-client".into(),
                    name: "Synthetic outside agent".into(),
                    redirects: vec!["https://client.example/callback".into()],
                    created_at: now,
                },
                now,
            )
            .unwrap();
        let returning = begin(handle, "returning-client", &issuer);
        let fresh = begin(handle, "new-client", &issuer);
        let status = agent_connector_status(handle.clone()).unwrap();
        let reconnecting = |id: &str| status.pending.iter().find(|p| p.id == id).unwrap().reconnecting;
        assert!(reconnecting(&returning));
        assert!(!reconnecting(&fresh));
        set(handle, false, None, None).unwrap();
    }

    #[test]
    fn issue_requires_an_enabled_connector_but_not_a_bound_listener() {
        let app = app();
        let handle = app.handle();
        let off = agent_connector_issue(handle.clone(), "ci-bot".into(), ControlAccess::Supervised, None);
        assert_eq!(off.err().as_deref(), Some("Turn the connector on first: codemux connector enable"));
        let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = occupied.local_addr().unwrap().port();
        assert!(!set(handle, true, None, Some(port)).unwrap().listener_running);
        let issued =
            agent_connector_issue(handle.clone(), "ci-bot".into(), ControlAccess::Supervised, Some(1)).unwrap();
        let url = format!("http://127.0.0.1:{port}/mcp");
        assert!(issued.token.starts_with(oauth::ACCESS_PREFIX));
        assert!(issued.client_id.starts_with(PERSONAL_CLIENT_PREFIX));
        assert_eq!((issued.label.as_str(), issued.access), ("ci-bot", ControlAccess::Supervised));
        assert_eq!((issued.mcp_url.as_str(), issued.local_url.as_str()), (url.as_str(), url.as_str()));
        assert!(!issued.replaced);
        let expires = chrono::DateTime::parse_from_rfc3339(&issued.expires_at).unwrap().timestamp_millis();
        assert!((expires - now_ms() - 3_600_000).abs() < 60_000, "{}", issued.expires_at);
        let db = handle.state::<DatabaseStore>();
        assert!(db
            .mcp_grant_by_hash(&oauth::credential_hash(&issued.token), &url, now_ms())
            .unwrap()
            .is_some());
        // Reissuing a label rotates only its token and keeps its first spelling.
        let again =
            agent_connector_issue(handle.clone(), " CI-bot ".into(), ControlAccess::ReadOnly, None).unwrap();
        assert!(again.replaced);
        assert_eq!((again.client_id.as_str(), again.label.as_str()), (issued.client_id.as_str(), "ci-bot"));
        assert!(db
            .mcp_grant_by_hash(&oauth::credential_hash(&issued.token), &url, now_ms())
            .unwrap()
            .is_none());
        let status = serde_json::to_value(agent_connector_status(handle.clone()).unwrap()).unwrap();
        assert_eq!(
            status["clients"][0],
            serde_json::json!({
                "id": issued.client_id, "clientName": "ci-bot", "callbackOrigin": null,
                "access": "read_only", "createdAt": status["clients"][0]["createdAt"],
                "expiresAt": status["clients"][0]["expiresAt"], "kind": "token",
                "label": "ci-bot", "renews": false,
            })
        );
        let text = status.to_string();
        assert!(!text.contains(&issued.token) && !text.contains(&again.token), "a token is shown only once");
        let issued = serde_json::to_value(&again).unwrap();
        let keys: std::collections::BTreeSet<_> = issued.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            ["access", "clientId", "expiresAt", "label", "localUrl", "mcpUrl", "replaced", "token"].into()
        );
        assert!(agent_connector_issue(handle.clone(), "-bad".into(), ControlAccess::ReadOnly, None).is_err());
        assert!(agent_connector_issue(handle.clone(), "bot".into(), ControlAccess::ReadOnly, Some(0)).is_err());
        assert!(agent_connector_issue(handle.clone(), "bot".into(), ControlAccess::ReadOnly, Some(365 * 24 + 1)).is_err());
        set(handle, false, None, None).unwrap();
    }

    #[test]
    fn wait_slots_are_capped_per_client_and_freed_on_drop() {
        let state = McpConnectorState::default();
        let mut first: Vec<_> = (0..MAX_WAITS_PER_CLIENT)
            .map(|_| state.admit_wait("mcp-client:a").unwrap())
            .collect();
        assert!(state.admit_wait("mcp-client:a").is_none(), "one client holds at most its share");
        let others: Vec<_> = (0..MAX_WAITS - MAX_WAITS_PER_CLIENT)
            .map(|i| state.admit_wait(&format!("mcp-client:{}", i % 3)).unwrap())
            .collect();
        assert!(state.admit_wait("mcp-client:fresh").is_none(), "the total stays bounded");
        drop(others);
        first.pop();
        assert!(state.admit_wait("mcp-client:a").is_some(), "a dropped slot is free again");
        assert!(state.admit_wait("mcp-client:fresh").is_some());
    }

    #[test]
    fn events_are_null_and_disabled_approvals_cannot_resurrect_requests() {
        let app = app();
        let handle = app.handle();
        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured = events.clone();
        handle.listen_any("agent-connector-changed", move |e| {
            captured.lock().unwrap().push(e.payload().to_string());
        });
        set(handle, true, None, None).unwrap();
        set(handle, false, None, None).unwrap();
        assert!(agent_connector_approve(
            handle.clone(),
            "old-request".into(),
            ControlAccess::FullAccess
        )
        .is_err());
        assert!(!events.lock().unwrap().is_empty());
        assert!(events.lock().unwrap().iter().all(|e| e == "null"));
        let status = serde_json::to_value(agent_connector_status(handle.clone()).unwrap()).unwrap();
        assert!(status.get("publicOrigin").is_some());
        for key in ["token", "code", "nonce", "tokenHash"] {
            assert!(status.get(key).is_none());
        }
    }

    fn pending_authorization() -> oauth::Authorization {
        oauth::Authorization {
            client_id: "synthetic-client".into(),
            client_name: "Synthetic outside agent".into(),
            redirect_uri: "https://client.example/callback?private=query".into(),
            challenge: "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM".into(),
            resource: "https://mcp.example/mcp".into(),
            issuer: "https://mcp.example".into(),
            state: Some("synthetic-client-state".into()),
        }
    }

    #[test]
    fn native_pending_status_exposes_only_phase_and_selected_access_for_every_decision() {
        for (decision, phase) in [
            (None, "denied"),
            (Some("read_only"), "approved_awaiting_client"),
            (Some("supervised"), "approved_awaiting_client"),
            (Some("full_access"), "approved_awaiting_client"),
        ] {
            let app = app();
            let handle = app.handle();
            set(handle, true, Some("https://mcp.example"), None).unwrap();
            let now = now_ms();
            let (id, _, _) = handle
                .state::<McpConnectorState>()
                .auth
                .lock()
                .unwrap()
                .begin(pending_authorization(), now)
                .unwrap();
            let expected = |phase: &str, access: Option<&str>| {
                serde_json::json!({
                    "id": id, "clientName": "Synthetic outside agent",
                    "callbackOrigin": "https://client.example", "requestedAt": timestamp(now),
                    "expiresAt": timestamp(now + oauth::PENDING_TTL_MS),
                    "phase": phase, "access": access, "reconnecting": false,
                })
            };
            let status =
                serde_json::to_value(agent_connector_status(handle.clone()).unwrap()).unwrap();
            assert_eq!(status["pending"][0], expected("awaiting_approval", None));
            handle
                .state::<McpConnectorState>()
                .auth
                .lock()
                .unwrap()
                .decide(&id, decision.map(str::to_owned), now)
                .unwrap();
            let status =
                serde_json::to_value(agent_connector_status(handle.clone()).unwrap()).unwrap();
            assert_eq!(status["pending"][0], expected(phase, decision));
            assert_eq!(status["clients"], serde_json::json!([]));
            set(handle, false, None, None).unwrap();
        }
    }

    #[test]
    fn native_owner_can_deny_pending_authorization_while_listener_is_offline() {
        let app = app();
        let handle = app.handle();
        // Offline: another program holds the configured port.
        let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = occupied.local_addr().unwrap().port();
        let status = set(handle, true, Some("https://mcp.example"), Some(port)).unwrap();
        assert!(status.enabled && !status.listener_running);
        let (id, nonce, cookie) = handle
            .state::<McpConnectorState>()
            .auth
            .lock()
            .unwrap()
            .begin(pending_authorization(), now_ms())
            .unwrap();
        let denied = agent_connector_deny(handle.clone(), id.clone());
        assert_eq!(
            denied,
            Ok(()),
            "Native owner denial must not require an online listener"
        );
        let status = serde_json::to_value(agent_connector_status(handle.clone()).unwrap()).unwrap();
        assert_eq!(status["pending"][0]["phase"], "denied");
        assert!(status["pending"][0].get("access").is_some());
        assert!(status["pending"][0]["access"].is_null());
        assert_eq!(status["clients"], serde_json::json!([]));
        assert!(agent_connector_deny(handle.clone(), id.clone()).is_err());
        let redirect = handle
            .state::<McpConnectorState>()
            .auth
            .lock()
            .unwrap()
            .complete(&id, &nonce, &cookie, now_ms())
            .unwrap()
            .unwrap();
        assert!(redirect.contains("error=access_denied"));
        assert!(!redirect.contains("code="));
        assert!(agent_connector_status(handle.clone())
            .unwrap()
            .pending
            .is_empty());
        drop(occupied);
    }

    #[test]
    fn native_status_expires_abandoned_registrations_without_displacing_pending_clients() {
        let app = app();
        let handle = app.handle();
        set(handle, true, Some("https://mcp.example"), None).unwrap();
        let now = now_ms();
        let db = handle.state::<DatabaseStore>();
        for id in ["abandoned", "synthetic-client"] {
            db.mcp_register_client(
                NewMcpClient {
                    id: id.into(),
                    name: "Untrusted label".into(),
                    redirects: vec!["https://client.example/callback".into()],
                    created_at: now - 600_000,
                },
                now - 600_000,
            )
            .unwrap();
        }
        let (id, _, _) = handle
            .state::<McpConnectorState>()
            .auth
            .lock()
            .unwrap()
            .begin(pending_authorization(), now)
            .unwrap();
        assert_eq!(
            agent_connector_status(handle.clone())
                .unwrap()
                .pending
                .len(),
            1
        );
        assert!(db.mcp_client("abandoned").unwrap().is_none(), "Native status must clean short-lived abandoned registrations, not retain them seven days");
        assert!(db.mcp_client("synthetic-client").unwrap().is_some());
        handle
            .state::<McpConnectorState>()
            .auth
            .lock()
            .unwrap()
            .pending
            .get_mut(&id)
            .unwrap()
            .expires_at = now;
        assert!(agent_connector_status(handle.clone())
            .unwrap()
            .pending
            .is_empty());
        assert!(db.mcp_client("synthetic-client").unwrap().is_none());
        set(handle, false, None, None).unwrap();
    }

    #[test]
    fn outside_caller_fails_closed_but_trusted_control_still_works() {
        let app = app();
        assert!(validate_caller(app.handle(), &ControlCaller::trusted()).is_ok());
        assert!(validate_caller(
            app.handle(),
            &ControlCaller::outside("missing-client".into(), "missing".into(), ControlAccess::ReadOnly)
        )
        .is_err());
        for origin in [
            "https://mcp.example/path",
            "https://user@mcp.example",
            "http://not-loopback.example",
        ] {
            assert!(set(app.handle(), true, Some(origin), None).is_err());
        }
        assert!(!agent_connector_status(app.handle().clone()).unwrap().enabled);
    }
}
