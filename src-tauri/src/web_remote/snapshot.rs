//! Authenticated bulk-snapshot endpoint: `GET /api/snapshot`.
//!
//! After pairing (or a reconnect) the web client would otherwise render blank
//! until a full round-trip completes: `POST /api/ws-ticket` → `GET /ws` upgrade
//! → React mount → the app's own `get_app_state` invoke over the socket → the
//! reply. This endpoint collapses that: the client fetches the whole initial
//! state over one authenticated HTTP GET, fired **in parallel** with the WS
//! handshake, so the UI can paint real state as soon as it mounts instead of
//! waiting for the post-mount invoke to make its own round-trip.
//!
//! ## A versioned API, not a web-only hack
//!
//! This is deliberately shaped as the first piece of a small, versioned API
//! for future non-web clients (a native mobile client is the obvious next
//! consumer). The payload is a stable envelope:
//!
//! ```json
//! { "api_version": 1, "app_state": <AppStateSnapshot>, "status": <WebRemoteStatus> }
//! ```
//!
//! - `app_state` is byte-for-byte the value the `get_app_state` command
//!   returns — it is produced by the *same* [`crate::state::AppStateStore::snapshot`]
//!   call, so there is no second serialization path to keep in sync.
//! - `status` is the same [`super::WebRemoteStatus`] the `web_remote_status`
//!   command and the `web-remote-state-changed` event carry, built by the same
//!   [`super::build_status`] helper.
//! - `api_version` lets a native client detect a breaking envelope change; bump
//!   [`API_VERSION`] whenever the shape changes incompatibly.
//!
//! ## Freshness & auth
//!
//! The response is marked `Cache-Control: no-store` — it is a live state
//! snapshot and must never be served from a cache. Admission is the same gate
//! the other authenticated `/api/*` routes use ([`super::server::require_session`]):
//! same-origin, an approved non-revoked session (bearer or HttpOnly cookie).

use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use super::WebRemoteState;

/// Version of the JSON bulk-snapshot envelope. Bump on any breaking change to
/// the `{ api_version, app_state, status }` shape so a versioned client (e.g.
/// a future native mobile app) can detect and adapt to it.
pub const API_VERSION: u32 = 1;

/// `GET /api/snapshot` — authenticated one-shot state bootstrap.
pub async fn serve(State(app): State<AppHandle>, headers: HeaderMap) -> Response {
    if let Err(resp) = super::server::require_session(&app, &headers) {
        return resp;
    }

    // The *same* snapshot the `get_app_state` command returns — one code path,
    // no duplicated serialization.
    let app_state = app.state::<crate::state::AppStateStore>().snapshot();
    let shared = app.state::<WebRemoteState>().shared();
    let status = super::build_status(&app, &shared);
    let body = snapshot_json(&app_state, &status);

    (
        StatusCode::OK,
        // A live state snapshot: never cache it, always re-fetch fresh.
        [(header::CACHE_CONTROL, "no-store")],
        Json(body),
    )
        .into_response()
}

/// Build the versioned snapshot envelope. Kept separate from [`serve`] so the
/// payload contract can be unit-tested without a running HTTP stack, and so the
/// route and the tests share one shape.
fn snapshot_json(app_state: &crate::state::AppStateSnapshot, status: &super::WebRemoteStatus) -> Value {
    json!({
        "api_version": API_VERSION,
        "app_state": app_state,
        "status": status,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::auth::{random_token, sha256_hex};
    use super::super::server::{authorize_headers, GateError};
    use axum::http::header;

    #[test]
    fn api_version_is_one() {
        assert_eq!(API_VERSION, 1);
    }

    // ── Endpoint auth ───────────────────────────────────────────────
    //
    // `serve`'s first line is `require_session` == `authorize_headers` +
    // `gate_error_response`. These assert the admission outcome for each
    // required scenario; the GateError → HTTP status mapping (MissingCreds =>
    // 401, Origin => 403) is proven by `server::tests::gate_errors_map_to_
    // expected_http_status`.

    fn insert_session(db: &crate::database::DatabaseStore, approved: bool) -> String {
        let token = random_token();
        db.web_remote_insert_session("snap-sess", Some("Phone"), None, &sha256_hex(&token), approved)
            .unwrap();
        token
    }

    #[test]
    fn snapshot_requires_authentication() {
        // No bearer and no session cookie → 401 (missing_credentials).
        let db = crate::database::init_test_database();
        insert_session(&db, true);
        assert_eq!(
            authorize_headers(&db, &HeaderMap::new()).unwrap_err(),
            GateError::MissingCreds
        );
    }

    #[test]
    fn snapshot_rejects_cross_origin() {
        // A valid bearer riding a cross-site Origin → 403 before any state is
        // read, so another site can't exfiltrate the snapshot with a stored cookie.
        let db = crate::database::init_test_database();
        let token = insert_session(&db, true);
        let mut h = HeaderMap::new();
        h.insert(header::AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
        h.insert(header::ORIGIN, "http://evil.example".parse().unwrap());
        h.insert(header::HOST, "192.168.1.5:4377".parse().unwrap());
        assert_eq!(authorize_headers(&db, &h).unwrap_err(), GateError::Origin);
    }

    #[test]
    fn snapshot_admits_an_approved_session() {
        // An approved, non-revoked session with a valid bearer passes admission
        // → the handler proceeds to build the 200 payload.
        let db = crate::database::init_test_database();
        let token = insert_session(&db, true);
        let mut h = HeaderMap::new();
        h.insert(header::AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
        let session = authorize_headers(&db, &h).expect("approved session is admitted");
        assert!(session.approved);
    }

    #[test]
    fn envelope_carries_app_state_status_and_version() {
        // `app_state` must be the real `AppStateSnapshot` (the same value
        // `get_app_state` returns), so build it from the same store path.
        let app_state = crate::state::AppStateStore::default().snapshot();
        let status = super::super::WebRemoteStatus {
            enabled: true,
            running: true,
            port: super::super::DEFAULT_PORT,
            require_approval: false,
            bind_scope: super::super::BIND_SCOPE_ALL.to_string(),
            active_connections: 1,
            connected_sessions: 1,
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

        let body = snapshot_json(&app_state, &status);

        // Versioned envelope.
        assert_eq!(body["api_version"], 1);

        // `app_state` is the serialized AppStateSnapshot — spot-check its
        // stable shape so a field rename can't silently change the contract.
        assert!(body["app_state"].is_object());
        assert!(
            body["app_state"].get("schema_version").is_some(),
            "app_state carries the AppStateSnapshot fields"
        );
        assert!(body["app_state"].get("workspaces").is_some());
        assert!(body["app_state"].get("active_workspace_id").is_some());

        // `status` is the WebRemoteStatus payload (snake_case wire contract).
        assert_eq!(body["status"]["enabled"], true);
        assert_eq!(body["status"]["port"], super::super::DEFAULT_PORT);
        assert_eq!(body["status"]["active_connections"], 1);
        assert!(
            body["status"].get("connectedSessions").is_none(),
            "no camelCase leakage from the status payload"
        );
    }
}
