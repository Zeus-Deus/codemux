//! Axum HTTP server for the headless daemon.
//!
//! Routes:
//!
//! - `GET  /health`         — unauthenticated liveness probe.
//! - `GET  /tools/list`     — list of MCP tools the daemon exposes
//!                            (auth required). Used by `codemux-remote mcp`
//!                            to populate its tools/list response.
//! - `POST /tools/call`     — invoke one tool by name. Body:
//!                            `{ "name": "...", "arguments": {...} }`.
//!                            Response: `{ "ok": true, "data": ... }`
//!                            or `{ "ok": false, "error": {...} }`.
//!
//! All non-`/health` endpoints require `Authorization: Bearer <secret>`
//! matching the secret in the manifest. The middleware
//! (`auth::require_bearer`) attaches an `Identity::Local` extension
//! that the handler then forwards to the tool dispatcher.

use std::sync::Arc;

use axum::{
    extract::{Extension, State},
    middleware,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::net::TcpListener;

use super::auth::require_bearer;
use super::identity::Identity;
use super::pty::PtyManager;
use super::tools;
use super::workspace::WorkspaceStore;

/// State shared across all request handlers. `Arc` so we can clone
/// cheaply into the axum app.
pub struct DaemonState {
    pub secret: String,
    pub started_at: String,
    pub workspaces: WorkspaceStore,
    pub ptys: PtyManager,
}

pub type SharedState = Arc<DaemonState>;

pub fn router(state: SharedState) -> Router {
    // Two separate routers so /health stays unauthenticated; the
    // authed routes get the bearer middleware applied uniformly.
    let public = Router::new().route("/health", get(health));

    let authed = Router::new()
        .route("/tools/list", get(tools_list))
        .route("/tools/call", post(tools_call))
        .route_layer(middleware::from_fn_with_state(
            Arc::clone(&state),
            require_bearer,
        ));

    public.merge(authed).with_state(state)
}

/// Bind and serve until the process is killed. Returns the bound
/// address so the caller (the `serve` subcommand) can write it to
/// the manifest after binding succeeded but before we accept any
/// requests. This avoids the race where the manifest is published
/// before the listener is ready.
pub async fn serve(state: SharedState, bind_port: Option<u16>) -> Result<(), String> {
    let listener = bind_listener(bind_port).await?;
    let local_addr = listener
        .local_addr()
        .map_err(|e| format!("local_addr: {e}"))?;
    eprintln!("[codemux-remote] listening on http://{}", local_addr);
    let app = router(state);
    axum::serve(listener, app)
        .await
        .map_err(|e| format!("serve: {e}"))
}

/// Bind a TCP listener on 127.0.0.1. If `port` is `None`, asks the
/// OS for an ephemeral free port (port 0).
pub async fn bind_listener(port: Option<u16>) -> Result<TcpListener, String> {
    let port = port.unwrap_or(0);
    let addr = format!("127.0.0.1:{port}");
    TcpListener::bind(&addr)
        .await
        .map_err(|e| format!("bind {addr}: {e}"))
}

async fn health() -> Response {
    (axum::http::StatusCode::OK, Json(json!({ "ok": true }))).into_response()
}

async fn tools_list(
    State(_state): State<SharedState>,
    Extension(_identity): Extension<Identity>,
) -> Response {
    let catalog = tools::catalog();
    Json(json!({ "tools": catalog })).into_response()
}

#[derive(Debug, Deserialize)]
struct CallBody {
    name: String,
    #[serde(default)]
    arguments: Value,
}

async fn tools_call(
    State(state): State<SharedState>,
    Extension(identity): Extension<Identity>,
    Json(body): Json<CallBody>,
) -> Response {
    // Run the dispatcher on a blocking thread so a long-running tool
    // (e.g. a slow PTY spawn) doesn't pin the runtime.
    let name = body.name.clone();
    let args = body.arguments.clone();
    let started_at = state.started_at.clone();
    let result = tokio::task::spawn_blocking(move || {
        tools::dispatch(
            &name,
            &args,
            &identity,
            &state.workspaces,
            &state.ptys,
            &started_at,
        )
    })
    .await;

    match result {
        Ok(Ok(data)) => Json(json!({ "ok": true, "data": data })).into_response(),
        Ok(Err(err)) => (
            axum::http::StatusCode::from_u16(error_status_code(err.kind)).unwrap(),
            Json(json!({
                "ok": false,
                "error": { "kind": err.kind, "message": err.message }
            })),
        )
            .into_response(),
        Err(join_err) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "ok": false,
                "error": { "kind": "internal", "message": format!("dispatch panicked: {join_err}") }
            })),
        )
            .into_response(),
    }
}

fn error_status_code(kind: &str) -> u16 {
    match kind {
        "invalid_input" => 400,
        "not_found" => 404,
        _ => 500,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn build_state(dir: &TempDir, secret: &str) -> SharedState {
        let db = super::super::workspace::WorkspaceStore::open(
            &dir.path().join("codemux.db"),
            "test-host".into(),
            dir.path().join("workspaces"),
        )
        .unwrap();
        Arc::new(DaemonState {
            secret: secret.into(),
            started_at: chrono::Utc::now().to_rfc3339(),
            workspaces: db,
            ptys: PtyManager::new(),
        })
    }

    /// Boot the router on a real ephemeral port and return the URL.
    /// Tests then use `reqwest::Client` against it.
    async fn boot(state: SharedState) -> String {
        let listener = bind_listener(None).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = router(state);
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn health_is_unauthenticated() {
        let dir = TempDir::new().unwrap();
        let state = build_state(&dir, "secret-abc");
        let url = boot(state).await;

        let client = reqwest::Client::new();
        let res = client.get(format!("{url}/health")).send().await.unwrap();
        assert_eq!(res.status(), 200);
        let body: Value = res.json().await.unwrap();
        assert_eq!(body, json!({ "ok": true }));
    }

    #[tokio::test]
    async fn missing_auth_returns_401() {
        let dir = TempDir::new().unwrap();
        let state = build_state(&dir, "secret-abc");
        let url = boot(state).await;
        let client = reqwest::Client::new();
        let res = client.get(format!("{url}/tools/list")).send().await.unwrap();
        assert_eq!(res.status(), 401);
    }

    #[tokio::test]
    async fn wrong_auth_returns_401() {
        let dir = TempDir::new().unwrap();
        let state = build_state(&dir, "secret-abc");
        let url = boot(state).await;
        let client = reqwest::Client::new();
        let res = client
            .get(format!("{url}/tools/list"))
            .header("Authorization", "Bearer wrong-secret")
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 401);
    }

    #[tokio::test]
    async fn tools_list_returns_catalog() {
        let dir = TempDir::new().unwrap();
        let state = build_state(&dir, "secret-abc");
        let url = boot(state).await;
        let client = reqwest::Client::new();
        let res = client
            .get(format!("{url}/tools/list"))
            .header("Authorization", "Bearer secret-abc")
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        let body: Value = res.json().await.unwrap();
        let tools = body["tools"].as_array().expect("tools array");
        assert!(tools.len() >= 10, "expected many tools, got {}", tools.len());
        let names: Vec<_> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert!(names.contains(&"workspace_create"));
        assert!(names.contains(&"terminal_write"));
        assert!(names.contains(&"app_status"));
    }

    #[tokio::test]
    async fn workspace_create_list_roundtrip_over_http() {
        let dir = TempDir::new().unwrap();
        let state = build_state(&dir, "tok");
        let url = boot(state).await;
        let client = reqwest::Client::new();

        let create = client
            .post(format!("{url}/tools/call"))
            .header("Authorization", "Bearer tok")
            .json(&json!({
                "name": "workspace_create",
                "arguments": { "path": "/tmp/my-repo", "name": "demo" }
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(create.status(), 200);
        let create_body: Value = create.json().await.unwrap();
        assert_eq!(create_body["ok"], json!(true));
        let id = create_body["data"]["workspace"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        let list = client
            .post(format!("{url}/tools/call"))
            .header("Authorization", "Bearer tok")
            .json(&json!({ "name": "workspace_list", "arguments": {} }))
            .send()
            .await
            .unwrap();
        let list_body: Value = list.json().await.unwrap();
        let listed = list_body["data"]["workspaces"].as_array().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0]["id"], json!(id));
    }

    #[tokio::test]
    async fn invalid_input_returns_400() {
        let dir = TempDir::new().unwrap();
        let state = build_state(&dir, "tok");
        let url = boot(state).await;
        let res = reqwest::Client::new()
            .post(format!("{url}/tools/call"))
            .header("Authorization", "Bearer tok")
            .json(&json!({ "name": "workspace_create", "arguments": { "name": "x" } })) // missing path
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 400);
    }

    #[tokio::test]
    async fn unknown_tool_returns_404() {
        let dir = TempDir::new().unwrap();
        let state = build_state(&dir, "tok");
        let url = boot(state).await;
        let res = reqwest::Client::new()
            .post(format!("{url}/tools/call"))
            .header("Authorization", "Bearer tok")
            .json(&json!({ "name": "no_such_tool", "arguments": {} }))
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 404);
    }
}
