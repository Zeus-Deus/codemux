//! Native-only administration for inbound external-agent connections.
//! Parent integration: manage McpConnectorState, register these native commands,
//! merge router() into the existing listener and call the separate DB schema.
//! NEVER put these commands on the paired-browser invoke allowlist.
mod http;
pub mod oauth;
pub use http::router;
use oauth::protocol;

use crate::agent_control::{ControlAccess, ControlCaller, ControlError};
use crate::database::DatabaseStore;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio::sync::Semaphore;

pub const CHANGED_EVENT: &str = "agent-connector-changed";

#[derive(Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Config {
    pub enabled: bool,
    pub public_origin: Option<String>,
}

pub struct McpConnectorState {
    // Also serializes synchronous config changes, native decisions, code mint,
    // token persistence and revocation. Never held over an await.
    pub(crate) auth: Mutex<oauth::AuthState>,
    pub(crate) rate: Mutex<protocol::Rates>,
    pub(crate) concurrency: Arc<Semaphore>,
}
impl Default for McpConnectorState {
    fn default() -> Self {
        Self {
            auth: Mutex::new(oauth::AuthState::default()),
            rate: Mutex::new(protocol::Rates::default()),
            concurrency: Arc::new(Semaphore::new(16)),
        }
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
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorClient {
    pub id: String,
    pub client_name: String,
    pub callback_origin: String,
    pub access: ControlAccess,
    pub created_at: String,
    pub expires_at: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectorStatus {
    pub enabled: bool,
    pub public_origin: Option<String>,
    pub listener_running: bool,
    pub mcp_url: Option<String>,
    pub local_command: String,
    pub pending: Vec<ConnectorPending>,
    pub clients: Vec<ConnectorClient>,
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

// The port comes from the existing listener's native status, never Host or
// Forwarded. Enabling this connector does not bind/start any transport.
pub(crate) fn live<R: Runtime>(app: &AppHandle<R>) -> Result<(Config, String, u16), String> {
    let cfg = config(&app.state::<DatabaseStore>())?;
    let remote = crate::web_remote::web_remote_status(app.clone());
    if !cfg.enabled || !remote.enabled || !remote.lan_enabled || !remote.running || remote.port == 0
    {
        return Err("HTTP agent connector is disabled or its listener is stopped".into());
    }
    let issuer = cfg
        .public_origin
        .clone()
        .unwrap_or_else(|| format!("http://127.0.0.1:{}", remote.port));
    Ok((cfg, issuer, remote.port))
}

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
        .map_err(|_| ControlError::new("connector_unavailable", "Connector unavailable"))?;
    let (_, issuer, _) = live(app).map_err(|_| {
        ControlError::new(
            "connector_disabled",
            "Connector disabled or listener stopped",
        )
    })?;
    let grant = app
        .state::<DatabaseStore>()
        .mcp_grant(id, &format!("{issuer}/mcp"), now_ms())
        .map_err(|_| ControlError::new("connector_unavailable", "Grant lookup failed"))?
        .ok_or_else(|| {
            ControlError::new(
                "grant_revoked",
                "Grant is expired, revoked or bound to another resource",
            )
        })?;
    if caller.principal != format!("mcp:{id}")
        || parse_access(&grant.access).map_err(|e| ControlError::new("invalid_grant", e))?
            != caller.access
    {
        return Err(ControlError::new(
            "grant_changed",
            "Grant identity or access changed",
        ));
    }
    Ok(())
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
    let remote = crate::web_remote::web_remote_status(app.clone());
    let running = remote.enabled && remote.lan_enabled && remote.running && remote.port != 0;
    let issuer = cfg
        .public_origin
        .clone()
        .unwrap_or_else(|| format!("http://127.0.0.1:{}", remote.port));
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
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    pending.sort_by(|a, b| a.requested_at.cmp(&b.requested_at).then(a.id.cmp(&b.id)));
    let clients = db
        .mcp_list_grants(now)?
        .into_iter()
        .map(|g| {
            Ok(ConnectorClient {
                id: g.client_id,
                client_name: g.client_name,
                callback_origin: callback_origin(&g.redirect_uri),
                access: parse_access(&g.access)?,
                created_at: timestamp(g.created_at),
                expires_at: timestamp(g.expires_at),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(ConnectorStatus {
        enabled: cfg.enabled,
        public_origin: cfg.public_origin,
        listener_running: running,
        mcp_url: if cfg.enabled && running {
            Some(format!("{issuer}/mcp"))
        } else {
            None
        },
        local_command: "codemux mcp".into(),
        pending,
        clients,
    })
}

#[tauri::command]
pub fn agent_connector_set_config<R: Runtime>(
    app: AppHandle<R>,
    enabled: bool,
    public_origin: Option<String>,
) -> Result<ConnectorStatus, String> {
    let public_origin = public_origin
        .map(|o| oauth::public_origin(&o))
        .transpose()?;
    {
        let state = app.state::<McpConnectorState>();
        let mut auth = state.auth.lock().map_err(|_| "Connector unavailable")?;
        let db = app.state::<DatabaseStore>();
        let prior = config(&db)?;
        let cfg = Config {
            enabled,
            public_origin,
        };
        db.mcp_connector_set_config(
            &serde_json::to_string(&cfg).map_err(|_| "Invalid configuration")?,
        )?;
        if !enabled || prior.public_origin != cfg.public_origin {
            auth.clear();
        }
    }
    changed(&app);
    agent_connector_status(app)
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
    changed(&app);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri::{Listener, Manager};

    fn app() -> tauri::App<tauri::test::MockRuntime> {
        let app = tauri::test::mock_app();
        app.manage(crate::database::DatabaseStore::new_in_memory());
        app.manage(crate::web_remote::WebRemoteState::default());
        app.manage(McpConnectorState::default());
        app
    }

    #[test]
    fn config_does_not_bind_listener_or_return_relay_url() {
        let app = app();
        let handle = app.handle();
        let default = agent_connector_status(handle.clone()).unwrap();
        assert!(!default.enabled);
        assert!(default.mcp_url.is_none());
        let before = handle
            .state::<crate::database::DatabaseStore>()
            .get_setting("web_remote.config");
        let status =
            agent_connector_set_config(handle.clone(), true, Some("https://mcp.example".into()))
                .unwrap();
        assert!(status.enabled);
        assert!(!status.listener_running);
        assert!(status.mcp_url.is_none());
        assert_eq!(
            handle
                .state::<crate::database::DatabaseStore>()
                .get_setting("web_remote.config"),
            before
        );
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
        agent_connector_set_config(handle.clone(), true, None).unwrap();
        agent_connector_set_config(handle.clone(), false, None).unwrap();
        assert!(agent_connector_approve(
            handle.clone(),
            "old-request".into(),
            ControlAccess::FullAccess
        )
        .is_err());
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
            agent_connector_set_config(handle.clone(), true, Some("https://mcp.example".into()))
                .unwrap();
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
                    "phase": phase, "access": access,
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
        }
    }

    #[test]
    fn native_owner_can_deny_pending_authorization_while_listener_is_offline() {
        let app = app();
        let handle = app.handle();
        let status =
            agent_connector_set_config(handle.clone(), true, Some("https://mcp.example".into()))
                .unwrap();
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
    }

    #[test]
    fn native_status_expires_abandoned_registrations_without_displacing_pending_clients() {
        let app = app();
        let handle = app.handle();
        agent_connector_set_config(handle.clone(), true, Some("https://mcp.example".into()))
            .unwrap();
        let now = now_ms();
        let db = handle.state::<DatabaseStore>();
        for id in ["abandoned", "synthetic-client"] {
            db.mcp_register_client(
                crate::database::mcp_connector::NewMcpClient {
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
    }

    #[test]
    fn outside_caller_fails_closed_but_trusted_control_still_works() {
        let app = app();
        assert!(validate_caller(app.handle(), &ControlCaller::trusted()).is_ok());
        assert!(validate_caller(
            app.handle(),
            &ControlCaller::outside("missing".into(), ControlAccess::ReadOnly)
        )
        .is_err());
        for origin in [
            "https://mcp.example/path",
            "https://user@mcp.example",
            "http://not-loopback.example",
        ] {
            assert!(
                agent_connector_set_config(app.handle().clone(), true, Some(origin.into()))
                    .is_err()
            );
        }
    }
}
