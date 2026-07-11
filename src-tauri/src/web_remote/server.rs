//! Axum HTTP + WebSocket server for web remote access.
//!
//! Bound on `0.0.0.0:<port>` only while the feature is enabled. Serves
//! the frontend bundle from Tauri's embedded assets (falling back to the
//! on-disk `dist/` in dev) and exposes the `/api/*` control surface plus
//! the `/ws` transport the browser shim drives.
//!
//! HTTP surface (see `docs/plans/web-remote-access.md`):
//!
//! - `GET  /*`             — static bundle (public; all state is behind auth).
//! - `GET  /api/health`    — public version probe.
//! - `POST /api/pair`      — trade a one-time token for a session + cookie.
//! - `POST /api/ws-ticket` — trade a session for a 30s single-use WS ticket.
//! - `GET  /api/snapshot`  — authed bulk state bootstrap (versioned API).
//! - `GET  /ws?ticket=…`   — upgrade to the WebSocket transport.
//!
//! Origin is checked on every state-touching request; pairing is
//! rate-limited per IP.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        ConnectInfo, Query, State,
    },
    http::{header, HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{any, get, post},
    Json, Router,
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, watch};

use tauri::{AppHandle, Manager};

use super::auth;
use super::{Shared, WebRemoteState};

/// Cookie name for the HttpOnly session token (asset/`<img>` GET auth).
pub const SESSION_COOKIE: &str = "cmux_web_session";
/// Keepalive ping cadence.
const PING_INTERVAL_SECS: u64 = 30;

/// Writer handle for a connection: every frame producer (invoke
/// responses, channel router, event hub) pushes onto this.
pub type OutboundTx = mpsc::UnboundedSender<Message>;

/// Ordered dispatch lane for invokes whose *arrival order* is load-bearing.
/// Carries `(id, cmd, args)` and is drained by a single per-connection task,
/// so the commands it carries are applied in exactly the order the client
/// sent them. See [`ORDERED_INVOKE_CMDS`].
type OrderedInvokeTx = mpsc::UnboundedSender<(u64, String, Value)>;

/// Commands that MUST be applied in client-send order. Terminal input
/// (`write_to_pty`) carries keystrokes: dispatching each on its own task
/// (as every other invoke is) lets two writes from one client race and
/// scramble the bytes reaching the PTY — the desktop's in-process IPC never
/// reorders them, and the web transport must match that. These are routed
/// through the per-connection [`OrderedInvokeTx`] instead.
fn is_ordered_invoke(cmd: &str) -> bool {
    matches!(cmd, "write_to_pty")
}

// ── Live connection registry ────────────────────────────────────────

struct ConnInfo {
    session_id: String,
    out: OutboundTx,
    /// Set `true` to force this socket closed (revocation / keepalive death).
    close: watch::Sender<bool>,
}

/// Tracks every open WebSocket so a revoked session's sockets can be
/// dropped immediately and the UI can show live-connection state.
#[derive(Default)]
pub struct ConnectionRegistry {
    conns: Mutex<HashMap<u64, ConnInfo>>,
    next: AtomicU64,
}

impl ConnectionRegistry {
    fn register(&self, session_id: &str, out: OutboundTx, close: watch::Sender<bool>) -> u64 {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        self.conns.lock().unwrap().insert(
            id,
            ConnInfo {
                session_id: session_id.to_string(),
                out,
                close,
            },
        );
        id
    }

    fn unregister(&self, id: u64) {
        self.conns.lock().unwrap().remove(&id);
    }

    /// Number of live sockets.
    pub fn active_count(&self) -> usize {
        self.conns.lock().unwrap().len()
    }

    /// Whether a session currently has at least one live socket.
    pub fn session_live(&self, session_id: &str) -> bool {
        self.conns
            .lock()
            .unwrap()
            .values()
            .any(|c| c.session_id == session_id)
    }

    /// Force every socket for a session closed. Returns the count closed.
    /// Sends a Close frame to the client and trips the connection's close
    /// signal so its read loop unwinds and releases its channels/events.
    pub fn close_session(&self, session_id: &str) -> usize {
        let conns = self.conns.lock().unwrap();
        let mut closed = 0;
        for c in conns.values().filter(|c| c.session_id == session_id) {
            let _ = c.out.send(Message::Close(None));
            let _ = c.close.send(true);
            closed += 1;
        }
        closed
    }

    /// Force *every* live socket closed. Used when the whole server is
    /// disabled (or rebinding to a new port) so a device that is already
    /// connected loses control immediately — turning the master switch off is
    /// a security action, and graceful shutdown alone would let existing
    /// WebSockets linger indefinitely (they never close on their own). Same
    /// mechanism as [`close_session`]: frame a Close and trip each close signal.
    pub fn close_all(&self) -> usize {
        let conns = self.conns.lock().unwrap();
        for c in conns.values() {
            let _ = c.out.send(Message::Close(None));
            let _ = c.close.send(true);
        }
        conns.len()
    }
}

// ── Router / lifecycle ──────────────────────────────────────────────

/// Bind the public listener. `0.0.0.0` so LAN/tailnet peers can reach it.
pub async fn bind(port: u16) -> Result<TcpListener, String> {
    let addr = format!("0.0.0.0:{port}");
    TcpListener::bind(&addr)
        .await
        .map_err(|e| format!("bind {addr}: {e}"))
}

/// Build the axum router. State is the `AppHandle`, from which handlers
/// reach both the managed [`WebRemoteState`] and the `DatabaseStore`.
pub fn router(app: AppHandle) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/pair", post(pair))
        .route("/api/ws-ticket", post(ws_ticket))
        // Authenticated one-shot state bootstrap (versioned API surface).
        .route("/api/snapshot", get(super::snapshot::serve))
        // Authenticated file streamer backing the shim's `convertFileSrc`.
        .route("/api/assets", get(super::assets::serve))
        // Browser-pane proxy to the loopback agent-browser daemons.
        .route("/proxy/browser/:port/ws", get(super::proxy::ws_proxy))
        .route("/proxy/browser/:port/api/*rest", any(super::proxy::http_forward))
        .route("/ws", get(ws_upgrade))
        .fallback(static_asset)
        .with_state(app)
}

// ── HTTP handlers ───────────────────────────────────────────────────

async fn health(State(app): State<AppHandle>) -> Response {
    let version = app.package_info().version.to_string();
    Json(json!({ "ok": true, "version": version })).into_response()
}

#[derive(Deserialize)]
struct PairBody {
    token: String,
    #[serde(default)]
    device_name: Option<String>,
}

async fn pair(
    State(app): State<AppHandle>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<PairBody>,
) -> Response {
    if !auth::origin_ok(&headers) {
        return error(StatusCode::FORBIDDEN, "origin_mismatch");
    }
    let shared = app.state::<WebRemoteState>().shared();

    if !shared.rate.check_and_record(peer.ip()) {
        return error(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    }
    if !shared.pairing.consume(&body.token) {
        return error(StatusCode::UNAUTHORIZED, "invalid_or_expired_token");
    }

    let require_approval = shared.config.lock().unwrap().require_approval;
    let approved = !require_approval;
    let session_id = uuid::Uuid::new_v4().to_string();
    let session_token = auth::random_token();
    let token_hash = auth::sha256_hex(&session_token);
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let name = body.device_name.filter(|s| !s.trim().is_empty());

    let insert = app.state::<crate::database::DatabaseStore>().web_remote_insert_session(
        &session_id,
        name.as_deref(),
        user_agent.as_deref(),
        &token_hash,
        approved,
    );
    if let Err(e) = insert {
        return error(StatusCode::INTERNAL_SERVER_ERROR, &e);
    }

    super::emit_state_changed(&app);

    // HttpOnly so JS can't read it; SameSite=Strict so a cross-site page
    // can't drive authed asset GETs with the stored cookie.
    let cookie = format!("{SESSION_COOKIE}={session_token}; HttpOnly; SameSite=Strict; Path=/");
    let mut out_headers = HeaderMap::new();
    if let Ok(v) = cookie.parse() {
        out_headers.insert(header::SET_COOKIE, v);
    }
    (
        StatusCode::OK,
        out_headers,
        Json(json!({
            "session_id": session_id,
            "session_token": session_token,
            "approved": approved,
        })),
    )
        .into_response()
}

async fn ws_ticket(State(app): State<AppHandle>, headers: HeaderMap) -> Response {
    if !auth::origin_ok(&headers) {
        return error(StatusCode::FORBIDDEN, "origin_mismatch");
    }
    let token = match extract_session_token(&headers) {
        Some(t) => t,
        None => return error(StatusCode::UNAUTHORIZED, "missing_credentials"),
    };

    let session = match auth::authenticate(&app.state::<crate::database::DatabaseStore>(), &token) {
        Some(s) => s,
        None => return error(StatusCode::UNAUTHORIZED, "invalid_session"),
    };
    // Pending (unapproved) devices get no tickets until approved.
    if !session.approved {
        return error(StatusCode::FORBIDDEN, "pending_approval");
    }
    app.state::<crate::database::DatabaseStore>()
        .web_remote_touch_session(&session.id);

    let ticket = app.state::<WebRemoteState>().shared().tickets.issue(&session.id);
    Json(json!({ "ticket": ticket })).into_response()
}

async fn ws_upgrade(
    State(app): State<AppHandle>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !auth::origin_ok(&headers) {
        return (StatusCode::FORBIDDEN, "origin mismatch").into_response();
    }
    let ticket = match params.get("ticket") {
        Some(t) => t.clone(),
        None => return (StatusCode::BAD_REQUEST, "missing ticket").into_response(),
    };

    let shared = app.state::<WebRemoteState>().shared();
    let session_id = match shared.tickets.consume(&ticket) {
        Some(id) => id,
        None => return (StatusCode::UNAUTHORIZED, "invalid or expired ticket").into_response(),
    };

    // Re-validate at upgrade time: the session may have been revoked (or
    // its approval withdrawn) in the 30s the ticket was valid.
    let still_valid = app
        .state::<crate::database::DatabaseStore>()
        .web_remote_get_session(&session_id)
        .map(|s| !s.revoked && s.approved)
        .unwrap_or(false);
    if !still_valid {
        return (StatusCode::FORBIDDEN, "session not active").into_response();
    }

    let app_for_socket = app.clone();
    ws.on_upgrade(move |socket| handle_socket(app_for_socket, session_id, socket))
}

/// Serve a frontend asset. Missing route-looking paths fall back to
/// `index.html` for client-side routing.
async fn static_asset(State(app): State<AppHandle>, uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let rel = if path.is_empty() { "index.html" } else { path };

    if let Some((bytes, mime, csp)) = resolve_asset(&app, rel) {
        return asset_response(bytes, &mime, csp);
    }
    // SPA fallback: a path with no file extension is a client route.
    if !rel.contains('.') {
        if let Some((bytes, mime, csp)) = resolve_asset(&app, "index.html") {
            return asset_response(bytes, &mime, csp);
        }
    }
    (StatusCode::NOT_FOUND, "not found").into_response()
}

/// Resolve `rel` (no leading slash) to `(bytes, mime, csp)` from the
/// embedded bundle, falling back to the on-disk `dist/` in dev builds.
fn resolve_asset(app: &AppHandle, rel: &str) -> Option<(Vec<u8>, String, Option<String>)> {
    if let Some(asset) = app.asset_resolver().get(rel.to_string()) {
        return Some((asset.bytes, asset.mime_type, asset.csp_header));
    }
    #[cfg(debug_assertions)]
    {
        if let Some((bytes, mime)) = dev_dist_fallback(rel) {
            return Some((bytes, mime, None));
        }
    }
    None
}

fn asset_response(bytes: Vec<u8>, mime: &str, csp: Option<String>) -> Response {
    let mut headers = HeaderMap::new();
    if let Ok(v) = mime.parse() {
        headers.insert(header::CONTENT_TYPE, v);
    }
    if let Some(csp) = csp {
        if let Ok(v) = csp.parse() {
            headers.insert(header::CONTENT_SECURITY_POLICY, v);
        }
    }
    (StatusCode::OK, headers, bytes).into_response()
}

/// Dev-only: read the source-tree `dist/` so `npm run tauri:dev` (which
/// does not embed assets) can still serve the bundle. Compiled out of
/// release builds, where `asset_resolver` returns the embedded copy.
#[cfg(debug_assertions)]
fn dev_dist_fallback(rel: &str) -> Option<(Vec<u8>, String)> {
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../dist");
    let base = base.canonicalize().ok()?;
    let full = base.join(rel).canonicalize().ok()?;
    // Refuse to escape the dist root.
    if !full.starts_with(&base) {
        return None;
    }
    let bytes = std::fs::read(&full).ok()?;
    let mime = mime_for(&full);
    Some((bytes, mime))
}

#[cfg(debug_assertions)]
fn mime_for(path: &std::path::Path) -> String {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("wasm") => "application/wasm",
        _ => "application/octet-stream",
    }
    .to_string()
}

// ── WebSocket lifecycle ─────────────────────────────────────────────

async fn handle_socket(app: AppHandle, session_id: String, socket: WebSocket) {
    let shared = app.state::<WebRemoteState>().shared();
    app.state::<crate::database::DatabaseStore>()
        .web_remote_touch_session(&session_id);

    let (mut sink, mut stream) = socket.split();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Message>();
    let (close_tx, mut close_rx) = watch::channel(false);
    let conn_id = shared
        .connections
        .register(&session_id, out_tx.clone(), close_tx.clone());
    super::emit_state_changed(&app);

    // Writer: the single owner of the sink; every producer feeds `out_tx`.
    let writer = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            let is_close = matches!(msg, Message::Close(_));
            if sink.send(msg).await.is_err() {
                break;
            }
            if is_close {
                break;
            }
        }
        let _ = sink.close().await;
    });

    // Ordered dispatch lane: terminal input (`write_to_pty`) must reach the
    // PTY in the exact order the client sent it. Every other invoke keeps
    // dispatching on its own task (out-of-order responses are fine), but the
    // ordered commands are drained one-at-a-time by this single task, in
    // arrival order, so a burst of keystrokes can never be scrambled by the
    // task-per-invoke concurrency below.
    let (ordered_tx, mut ordered_rx) = mpsc::unbounded_channel::<(u64, String, Value)>();
    let ordered_task = {
        let app = app.clone();
        let router = shared.channels.clone();
        let out = out_tx.clone();
        tokio::spawn(async move {
            while let Some((id, cmd, args)) = ordered_rx.recv().await {
                super::dispatch::dispatch_invoke(&app, &router, conn_id, &out, id, cmd, args).await;
            }
        })
    };

    // Keepalive: ping every 30s; two unanswered pings closes the socket.
    let awaiting_pongs = Arc::new(AtomicU32::new(0));
    let pinger = {
        let out = out_tx.clone();
        let awaiting = awaiting_pongs.clone();
        let close = close_tx.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(PING_INTERVAL_SECS));
            ticker.tick().await; // consume the immediate first tick
            loop {
                ticker.tick().await;
                if awaiting.load(Ordering::SeqCst) >= 2 {
                    let _ = close.send(true);
                    break;
                }
                awaiting.fetch_add(1, Ordering::SeqCst);
                if out.send(Message::Ping(Vec::new())).is_err() {
                    break;
                }
            }
        })
    };

    // Reader: dispatch client frames until close/error/revocation.
    loop {
        tokio::select! {
            changed = close_rx.changed() => {
                if changed.is_err() || *close_rx.borrow() {
                    break;
                }
            }
            incoming = stream.next() => {
                match incoming {
                    Some(Ok(Message::Text(txt))) => {
                        handle_text_frame(&app, &shared, conn_id, &out_tx, &ordered_tx, &txt);
                    }
                    Some(Ok(Message::Pong(_))) => {
                        awaiting_pongs.store(0, Ordering::SeqCst);
                    }
                    Some(Ok(Message::Ping(_))) | Some(Ok(Message::Binary(_))) => {}
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(_)) => break,
                }
            }
        }
    }

    // Teardown: channels + event subscriptions die with the socket.
    shared.connections.unregister(conn_id);
    shared.channels.remove_conn(conn_id);
    shared.events.remove_conn(&app, conn_id);
    pinger.abort();
    ordered_task.abort();
    let _ = close_tx.send(true);
    drop(out_tx);
    writer.abort();
    super::emit_state_changed(&app);
}

/// Route one text frame from the client per the WS protocol contract.
fn handle_text_frame(
    app: &AppHandle,
    shared: &Arc<Shared>,
    conn_id: u64,
    out: &OutboundTx,
    ordered: &OrderedInvokeTx,
    txt: &str,
) {
    let value: Value = match serde_json::from_str(txt) {
        Ok(v) => v,
        Err(_) => return,
    };
    match value.get("t").and_then(|v| v.as_str()).unwrap_or("") {
        "invoke" => {
            let id = value.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
            let cmd = value
                .get("cmd")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let args = value
                .get("args")
                .cloned()
                .unwrap_or_else(|| Value::Object(Default::default()));
            if cmd.is_empty() {
                let _ = out.send(Message::Text(
                    json!({ "t": "err", "id": id, "error": "missing cmd" }).to_string(),
                ));
                return;
            }
            // Ordered commands (terminal input) go through the per-connection
            // serial lane so their client-send order is preserved; the send
            // is synchronous here, inside the in-order reader loop, so frames
            // enqueue in arrival order.
            if is_ordered_invoke(&cmd) {
                let _ = ordered.send((id, cmd, args));
                return;
            }
            // Task per invoke → out-of-order responses are fine (id-matched).
            let app = app.clone();
            let router = shared.channels.clone();
            let out = out.clone();
            tokio::spawn(async move {
                super::dispatch::dispatch_invoke(&app, &router, conn_id, &out, id, cmd, args).await;
            });
        }
        "listen" => {
            if let Some(event) = value.get("event").and_then(|v| v.as_str()) {
                shared.events.subscribe(app, conn_id, event, out.clone());
            }
        }
        "unlisten" => {
            if let Some(event) = value.get("event").and_then(|v| v.as_str()) {
                shared.events.unsubscribe(app, conn_id, event);
            }
        }
        _ => {}
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

fn extract_session_token(headers: &HeaderMap) -> Option<String> {
    if let Some(auth) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        if let Some(tok) = auth.strip_prefix("Bearer ") {
            let t = tok.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    // Cookie fallback: authed asset / `<img>` GETs carry the HttpOnly cookie.
    let cookie_header = headers.get(header::COOKIE).and_then(|v| v.to_str().ok())?;
    let prefix = format!("{SESSION_COOKIE}=");
    for part in cookie_header.split(';') {
        if let Some(val) = part.trim().strip_prefix(&prefix) {
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    None
}

fn error(status: StatusCode, code: &str) -> Response {
    (status, Json(json!({ "error": code }))).into_response()
}

// ── Shared session gate ─────────────────────────────────────────────
//
// The authenticated `/api/*` and `/proxy/*` routes (assets, browser-pane
// proxy) all need the same admission decision the WS-ticket handler makes:
// same-origin, a valid non-revoked session (bearer or HttpOnly cookie), and
// approval granted. [`authorize_headers`] is the pure decision (unit-tested
// against a `DatabaseStore`); [`require_session`] maps it onto the HTTP error
// responses used across those routes.

/// Why a request failed the session gate. Maps 1:1 to an HTTP response.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum GateError {
    /// `Origin` present and cross-site — a page on another site is riding a
    /// stored cookie.
    Origin,
    /// No bearer token and no session cookie.
    MissingCreds,
    /// Token/cookie present but no live session matches it.
    InvalidSession,
    /// Session exists but is still pending approval.
    Pending,
}

/// Resolve request headers to an approved session, or the reason it was
/// rejected. Pure over `db` + `headers` so it is directly unit-testable.
pub(super) fn authorize_headers(
    db: &crate::database::DatabaseStore,
    headers: &HeaderMap,
) -> Result<auth::AuthedSession, GateError> {
    if !auth::origin_ok(headers) {
        return Err(GateError::Origin);
    }
    let token = extract_session_token(headers).ok_or(GateError::MissingCreds)?;
    let session = auth::authenticate(db, &token).ok_or(GateError::InvalidSession)?;
    if !session.approved {
        return Err(GateError::Pending);
    }
    Ok(session)
}

fn gate_error_response(err: GateError) -> Response {
    match err {
        GateError::Origin => error(StatusCode::FORBIDDEN, "origin_mismatch"),
        GateError::MissingCreds => error(StatusCode::UNAUTHORIZED, "missing_credentials"),
        GateError::InvalidSession => error(StatusCode::UNAUTHORIZED, "invalid_session"),
        GateError::Pending => error(StatusCode::FORBIDDEN, "pending_approval"),
    }
}

/// Admission gate for the authenticated asset + proxy routes. Returns the
/// approved session on success, or a ready-to-return error `Response`.
pub(super) fn require_session(
    app: &AppHandle,
    headers: &HeaderMap,
) -> Result<auth::AuthedSession, Response> {
    authorize_headers(&app.state::<crate::database::DatabaseStore>(), headers)
        .map_err(gate_error_response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[test]
    fn connection_registry_close_session_signals_and_frames() {
        let reg = ConnectionRegistry::default();
        let (out_tx, mut out_rx) = mpsc::unbounded_channel();
        let (close_tx, close_rx) = watch::channel(false);
        let id = reg.register("sess-1", out_tx, close_tx);

        assert_eq!(reg.active_count(), 1);
        assert!(reg.session_live("sess-1"));
        assert!(!reg.session_live("other"));

        // Revoking the session must both frame + signal the socket.
        let closed = reg.close_session("sess-1");
        assert_eq!(closed, 1);
        assert!(matches!(out_rx.try_recv(), Ok(Message::Close(_))));
        assert!(*close_rx.borrow(), "close signal must be tripped");

        reg.unregister(id);
        assert_eq!(reg.active_count(), 0);
        assert!(!reg.session_live("sess-1"));
    }

    #[test]
    fn close_session_only_targets_matching_session() {
        let reg = ConnectionRegistry::default();
        let (tx_a, mut rx_a) = mpsc::unbounded_channel();
        let (ctx_a, _cra) = watch::channel(false);
        let (tx_b, mut rx_b) = mpsc::unbounded_channel();
        let (ctx_b, crb) = watch::channel(false);
        reg.register("A", tx_a, ctx_a);
        reg.register("B", tx_b, ctx_b);

        assert_eq!(reg.close_session("A"), 1);
        assert!(matches!(rx_a.try_recv(), Ok(Message::Close(_))));
        // B is untouched.
        assert!(rx_b.try_recv().is_err());
        assert!(!*crb.borrow());
    }

    #[test]
    fn close_all_severs_every_connection() {
        // Disabling the server (or rebinding) must kick *every* connected
        // device, regardless of session — an already-open socket keeps full
        // control until it is closed, so graceful listener shutdown is not
        // enough on its own.
        let reg = ConnectionRegistry::default();
        let (tx_a, mut rx_a) = mpsc::unbounded_channel();
        let (ctx_a, cra) = watch::channel(false);
        let (tx_b, mut rx_b) = mpsc::unbounded_channel();
        let (ctx_b, crb) = watch::channel(false);
        reg.register("A", tx_a, ctx_a);
        reg.register("B", tx_b, ctx_b);

        assert_eq!(reg.close_all(), 2);
        // Both sockets get a Close frame and have their close signal tripped.
        assert!(matches!(rx_a.try_recv(), Ok(Message::Close(_))));
        assert!(matches!(rx_b.try_recv(), Ok(Message::Close(_))));
        assert!(*cra.borrow(), "A's close signal tripped");
        assert!(*crb.borrow(), "B's close signal tripped");
    }

    // ── Session gate (backs the assets + proxy route auth) ──────────

    fn insert_session(db: &crate::database::DatabaseStore, approved: bool) -> String {
        let token = auth::random_token();
        let hash = auth::sha256_hex(&token);
        db.web_remote_insert_session("gate-sess", Some("Phone"), None, &hash, approved)
            .unwrap();
        token
    }

    #[test]
    fn gate_errors_map_to_expected_http_status() {
        // The asset + proxy routes rely on these exact codes: 401 when no/bad
        // credentials, 403 for cross-origin or pending-approval.
        assert_eq!(
            gate_error_response(GateError::MissingCreds).status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            gate_error_response(GateError::InvalidSession).status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            gate_error_response(GateError::Origin).status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            gate_error_response(GateError::Pending).status(),
            StatusCode::FORBIDDEN
        );
    }

    #[test]
    fn authorize_rejects_when_no_credentials() {
        let db = crate::database::init_test_database();
        insert_session(&db, true);
        // No Authorization / Cookie header → 401 missing_credentials.
        let err = authorize_headers(&db, &HeaderMap::new()).unwrap_err();
        assert_eq!(err, GateError::MissingCreds);
    }

    #[test]
    fn authorize_accepts_bearer_and_cookie() {
        let db = crate::database::init_test_database();
        let token = insert_session(&db, true);

        // Bearer path (the shim's `fetch` uses `Authorization: Bearer`).
        let mut bearer = HeaderMap::new();
        bearer.insert(header::AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
        let s = authorize_headers(&db, &bearer).expect("bearer authenticates");
        assert_eq!(s.id, "gate-sess");

        // Cookie path (`<img src>` / same-origin GETs carry the HttpOnly cookie).
        let mut cookie = HeaderMap::new();
        cookie.insert(
            header::COOKIE,
            format!("{SESSION_COOKIE}={token}").parse().unwrap(),
        );
        let s = authorize_headers(&db, &cookie).expect("cookie authenticates");
        assert_eq!(s.id, "gate-sess");
    }

    #[test]
    fn authorize_rejects_unknown_token() {
        let db = crate::database::init_test_database();
        insert_session(&db, true);
        let mut h = HeaderMap::new();
        h.insert(header::AUTHORIZATION, "Bearer deadbeef".parse().unwrap());
        assert_eq!(authorize_headers(&db, &h).unwrap_err(), GateError::InvalidSession);
    }

    #[test]
    fn authorize_rejects_pending_and_cross_origin() {
        let db = crate::database::init_test_database();
        let token = insert_session(&db, false); // pending approval

        // Valid token but unapproved → pending.
        let mut h = HeaderMap::new();
        h.insert(header::AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
        assert_eq!(authorize_headers(&db, &h).unwrap_err(), GateError::Pending);

        // Cross-site Origin is rejected before credentials are even consulted.
        let mut x = HeaderMap::new();
        x.insert(header::AUTHORIZATION, format!("Bearer {token}").parse().unwrap());
        x.insert(header::ORIGIN, "http://evil.example".parse().unwrap());
        x.insert(header::HOST, "192.168.1.5:4377".parse().unwrap());
        assert_eq!(authorize_headers(&db, &x).unwrap_err(), GateError::Origin);
    }

    #[test]
    fn extract_token_prefers_bearer_then_cookie() {
        let mut h = HeaderMap::new();
        h.insert(header::AUTHORIZATION, "Bearer abc123".parse().unwrap());
        assert_eq!(extract_session_token(&h).as_deref(), Some("abc123"));

        let mut h = HeaderMap::new();
        h.insert(
            header::COOKIE,
            format!("foo=bar; {SESSION_COOKIE}=zzz; baz=qux")
                .parse()
                .unwrap(),
        );
        assert_eq!(extract_session_token(&h).as_deref(), Some("zzz"));

        assert_eq!(extract_session_token(&HeaderMap::new()), None);
    }

    #[test]
    fn terminal_input_is_routed_to_the_ordered_lane() {
        // `write_to_pty` carries keystrokes whose order is load-bearing, so it
        // must take the per-connection serial lane, not the task-per-invoke
        // path that lets writes race and scramble bytes reaching the PTY.
        assert!(is_ordered_invoke("write_to_pty"));

        // Everything else stays on the concurrent path (out-of-order responses
        // are fine and head-of-line blocking is avoided). Spot-check a few,
        // including PTY commands whose ordering is not correctness-critical.
        assert!(!is_ordered_invoke("get_app_state"));
        assert!(!is_ordered_invoke("resize_pty"));
        assert!(!is_ordered_invoke("attach_pty_output"));
        assert!(!is_ordered_invoke(""));
    }

    #[tokio::test]
    async fn ordered_lane_preserves_client_send_order() {
        // The lane is a single-consumer channel drained in arrival order, so a
        // burst of keystroke frames enqueued in send-order is delivered to the
        // dispatcher in that same order — the property the fix relies on.
        let (tx, mut rx) = mpsc::unbounded_channel::<(u64, String, Value)>();
        let seq = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
        for (i, ch) in seq.chars().enumerate() {
            tx.send((i as u64, "write_to_pty".into(), json!({ "data": ch.to_string() })))
                .unwrap();
        }
        drop(tx);

        let mut got = String::new();
        while let Some((_, _, args)) = rx.recv().await {
            got.push_str(args["data"].as_str().unwrap());
        }
        assert_eq!(got, seq, "ordered lane must preserve client send order");
    }
}
