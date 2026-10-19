//! Authenticated loopback MCP gateway for provider adapters.
//!
//! Codemux owns the real MCP child processes in [`McpRegistry`]. Providers
//! which can consume a remote MCP server (OpenCode today, future adapters
//! later) connect to this single virtual server instead of duplicating every
//! user configuration into provider-specific files.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use rand::distributions::{Alphanumeric, DistString};
use serde_json::{json, Value};

use super::registry::McpRegistry;
use super::McpConfigSource;

/// Sources OpenCode loads itself, mirroring the native-source exclusions the
/// Claude and Codex paths apply: `opencode serve` already spawns the servers
/// from its own config, so serving them back would double every tool. The
/// shared token hides them; per-chat tokens carry their own list.
///
/// Identity rides on the token, never on the request: each chat (and each
/// directory-scoped OpenCode registration) has a token that the app maps to
/// a [`GatewayCaller`]. The shared token, used by OpenCode's legacy
/// instance-global registration, has no workspace and so sees only globally
/// visible plugin tools.
pub const OPENCODE_NATIVE_SOURCES: [McpConfigSource; 2] = [
    McpConfigSource::OpenCodeUser,
    McpConfigSource::OpenCodeProject,
];

/// Connection details handed to a provider adapter. Tokens are generated in
/// this process and never written to disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpGatewayConnection {
    pub url: String,
    pub bearer_token: String,
}

/// Who a per-chat token speaks for. Codemux fixes it when the chat's session
/// starts; nothing in it comes from the agent or the model.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GatewayCaller {
    /// Workspace that workspace-scoped tools (built-in ones, workspace-scoped
    /// plugin servers) act on and are listed for.
    pub workspace_id: Option<String>,
    /// Config sources the agent already loads itself, hidden from its
    /// `tools/list` so it doesn't see every such tool twice.
    pub native_sources: Vec<McpConfigSource>,
}

/// Per-chat tokens. A chat keeps its token for the life of this process:
/// Hermes serves many chats from one process and keys ACP MCP servers by
/// name, so a chat that reopens on the same process reuses the connection it
/// registered first, token included.
#[derive(Default)]
struct Grants {
    tokens: HashMap<String, String>,
    callers: HashMap<String, GatewayCaller>,
}

/// Live gateway state retained by [`McpRegistry`]'s `OnceCell`.
pub struct McpGatewayRuntime {
    connection: McpGatewayConnection,
    grants: Arc<Mutex<Grants>>,
    abort_handle: tokio::task::AbortHandle,
}

/// Environment the `codemux mcp-bridge` stdio proxy reads its target from.
pub const BRIDGE_URL_ENV: &str = "CODEMUX_MCP_GATEWAY_URL";
pub const BRIDGE_TOKEN_ENV: &str = "CODEMUX_MCP_GATEWAY_TOKEN";

impl McpGatewayConnection {
    /// ACP `McpServerHttp` entry for this connection.
    pub fn acp_http_server(&self, name: &str) -> Value {
        json!({
            "type": "http",
            "name": name,
            "url": self.url,
            "headers": [{ "name": "Authorization", "value": format!("Bearer {}", self.bearer_token) }]
        })
    }

    /// ACP `McpServerStdio` entry that runs `program mcp-bridge`, for agents
    /// that take stdio servers only.
    pub fn acp_stdio_bridge(&self, name: &str, program: &str) -> Value {
        json!({
            "name": name,
            "command": program,
            "args": ["mcp-bridge"],
            "env": [
                { "name": BRIDGE_URL_ENV, "value": self.url },
                { "name": BRIDGE_TOKEN_ENV, "value": self.bearer_token }
            ]
        })
    }
}

impl std::fmt::Debug for McpGatewayRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpGatewayRuntime")
            .field("url", &self.connection.url)
            .finish_non_exhaustive()
    }
}

impl McpGatewayRuntime {
    pub fn connection(&self) -> McpGatewayConnection {
        self.connection.clone()
    }

    /// Connection for one consumer, keyed by `key` (a chat's thread id).
    /// Refreshes the caller each time so a reopened chat picks up its
    /// current workspace.
    pub fn chat_connection(&self, key: &str, caller: GatewayCaller) -> McpGatewayConnection {
        let mut grants = self.grants.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let token = grants
            .tokens
            .entry(key.to_string())
            .or_insert_with(new_token)
            .clone();
        grants.callers.insert(token.clone(), caller);
        McpGatewayConnection {
            url: self.connection.url.clone(),
            bearer_token: token,
        }
    }
}

impl Drop for McpGatewayRuntime {
    fn drop(&mut self) {
        self.abort_handle.abort();
    }
}

#[derive(Clone)]
struct GatewayState {
    registry: McpRegistry,
    /// The shared token OpenCode's instance-global registration uses.
    bearer_token: Arc<str>,
    grants: Arc<Mutex<Grants>>,
}

fn new_token() -> String {
    Alphanumeric.sample_string(&mut rand::thread_rng(), 48)
}

/// Bind an authenticated MCP endpoint on an ephemeral loopback port.
pub async fn start_gateway(registry: McpRegistry) -> Result<McpGatewayRuntime, String> {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .map_err(|error| format!("failed to bind MCP gateway: {error}"))?;
    let address = listener
        .local_addr()
        .map_err(|error| format!("failed to read MCP gateway address: {error}"))?;
    let bearer_token = new_token();
    let grants = Arc::new(Mutex::new(Grants::default()));
    let state = GatewayState {
        registry,
        bearer_token: Arc::from(bearer_token.as_str()),
        grants: grants.clone(),
    };
    let app = router(state);
    let task = tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, app).await {
            eprintln!("[codemux::mcp] loopback gateway stopped: {error}");
        }
    });

    Ok(McpGatewayRuntime {
        connection: McpGatewayConnection {
            url: format!("http://{address}/mcp"),
            bearer_token,
        },
        grants,
        abort_handle: task.abort_handle(),
    })
}

fn router(state: GatewayState) -> Router {
    Router::new()
        .route(
            "/mcp",
            post(handle_post)
                .get(handle_events)
                .delete(method_not_allowed),
        )
        .layer(DefaultBodyLimit::max(4 * 1024 * 1024))
        .with_state(state)
}

async fn method_not_allowed() -> StatusCode {
    StatusCode::METHOD_NOT_ALLOWED
}

async fn handle_events(State(state): State<GatewayState>, headers: HeaderMap) -> Response {
    if !origin_allowed(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if authenticate(&headers, &state).is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let rx = state.registry.subscribe_status();
    let stream = futures_util::stream::unfold(rx, |mut rx| async {
        loop {
            match rx.recv().await {
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    return Some((
                        Ok::<_, std::convert::Infallible>(Event::default().event("message").data(
                            r#"{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}"#,
                        )),
                        rx,
                    ))
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

async fn handle_post(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !origin_allowed(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(caller) = authenticate(&headers, &state) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let workspace_id = caller.workspace_id.as_deref();

    let message: Value = match serde_json::from_slice(&body) {
        Ok(value) => value,
        Err(error) => {
            return rpc_error(Value::Null, -32700, format!("parse error: {error}")).into_response();
        }
    };
    let id = message.get("id").cloned();
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let params = message.get("params").cloned().unwrap_or_else(|| json!({}));

    // MCP notifications have no id and are acknowledged at the HTTP layer.
    if id.is_none() {
        return StatusCode::ACCEPTED.into_response();
    }
    let id = id.unwrap_or(Value::Null);

    match method {
        "initialize" => {
            let requested = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or("2025-11-25");
            let version = supported_protocol_version(requested);
            rpc_result(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": true } },
                    "serverInfo": {
                        "name": "codemux",
                        "title": "Codemux MCP Gateway",
                        "version": env!("CARGO_PKG_VERSION")
                    },
                    "instructions": "Tools configured in Codemux and shared with this agent session."
                }),
            )
            .into_response()
        }
        "ping" => rpc_result(id, json!({})).into_response(),
        "tools/list" => {
            let mut tools = state
                .registry
                .list_tools_for_workspace(&caller.native_sources, workspace_id)
                .await;
            tools.extend(super::registry::plugin_bridge_tools());
            let data: Vec<Value> = tools
                .into_iter()
                .map(|tool| {
                    json!({
                        "name": tool.prefixed_name,
                        "description": tool.description,
                        "inputSchema": tool.input_schema,
                    })
                })
                .collect();
            rpc_result(id, json!({ "tools": data })).into_response()
        }
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return rpc_error(id, -32602, "tools/call requires params.name").into_response();
            };
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let bridge = state
                .registry
                .call_plugin_bridge(name, arguments.clone(), workspace_id, &[])
                .await;
            let result = match bridge {
                Some(result) => result,
                None => {
                    state
                        .registry
                        .dispatch_tool_call(name, arguments, workspace_id)
                        .await
                }
            };
            match result {
                Ok(result) => rpc_result(id, result).into_response(),
                Err(message) => rpc_result(
                    id,
                    json!({
                        "content": [{ "type": "text", "text": message }],
                        "isError": true
                    }),
                )
                .into_response(),
            }
        }
        _ => rpc_error(id, -32601, format!("method not found: {method}")).into_response(),
    }
}

fn supported_protocol_version(requested: &str) -> &str {
    match requested {
        "2024-11-05" | "2025-03-26" | "2025-06-18" | "2025-11-25" | "2026-07-28" => requested,
        _ => "2025-11-25",
    }
}

/// The caller a request's bearer token speaks for.
fn authenticate(headers: &HeaderMap, state: &GatewayState) -> Option<GatewayCaller> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")?;
    if token == &*state.bearer_token {
        return Some(GatewayCaller {
            workspace_id: None,
            native_sources: OPENCODE_NATIVE_SOURCES.to_vec(),
        });
    }
    let grants = state.grants.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    grants.callers.get(token).cloned()
}

/// MCP's HTTP transport requires Origin validation to prevent DNS rebinding.
/// Native MCP clients normally omit Origin; loopback browser origins are the
/// only explicit values accepted.
fn origin_allowed(headers: &HeaderMap) -> bool {
    let Some(origin) = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    else {
        return true;
    };
    origin
        .strip_prefix("http://127.0.0.1")
        .or_else(|| origin.strip_prefix("http://localhost"))
        .is_some_and(|suffix| suffix.is_empty() || suffix.starts_with(':'))
}

fn rpc_result(id: Value, result: Value) -> Json<Value> {
    Json(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

fn rpc_error(id: Value, code: i64, message: impl Into<String>) -> Json<Value> {
    Json(json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message.into() }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn state() -> GatewayState {
        GatewayState {
            registry: McpRegistry::new(),
            bearer_token: Arc::from("test-token"),
            grants: Arc::default(),
        }
    }

    fn request(body: Value) -> Request<Body> {
        Request::post("/mcp")
            .header("authorization", "Bearer test-token")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    #[tokio::test]
    async fn initialize_negotiates_and_advertises_tools() {
        let response = router(state())
            .oneshot(request(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {},
                    "clientInfo": { "name": "test", "version": "1" }
                }
            })))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["result"]["protocolVersion"], "2025-11-25");
        assert_eq!(
            value["result"]["capabilities"]["tools"]["listChanged"],
            true
        );
    }

    #[tokio::test]
    async fn tools_list_hides_opencode_native_servers() {
        let registry = McpRegistry::new();
        registry
            .insert_running_server_for_test("shared", vec![McpConfigSource::CodemuxUser])
            .await;
        registry
            .insert_running_server_for_test("oc-user", vec![McpConfigSource::OpenCodeUser])
            .await;
        registry
            .insert_running_server_for_test("oc-project", vec![McpConfigSource::OpenCodeProject])
            .await;
        let state = GatewayState {
            registry,
            bearer_token: Arc::from("test-token"),
            grants: Arc::default(),
        };

        let response = router(state)
            .oneshot(request(json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/list",
                "params": {}
            })))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        let names: Vec<&str> = value["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            vec![
                "mcp__shared__tool",
                "codemux_list_plugin_tools",
                "codemux_call_plugin_tool"
            ],
            "{value}"
        );
    }

    fn tool_names(value: &Value) -> Vec<&str> {
        value["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .filter(|name| name.starts_with("mcp__"))
            .collect()
    }

    async fn tools_list_with_token(state: GatewayState, token: &str) -> (StatusCode, Value) {
        let response = router(state)
            .oneshot(
                Request::post("/mcp")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::from(
                        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    #[tokio::test]
    async fn chat_tokens_hide_only_their_own_native_sources() {
        let registry = McpRegistry::new();
        registry
            .insert_running_server_for_test("cursor-user", vec![McpConfigSource::CursorUser])
            .await;
        registry
            .insert_running_server_for_test("oc-user", vec![McpConfigSource::OpenCodeUser])
            .await;
        let runtime = start_gateway(registry.clone()).await.unwrap();
        let state = GatewayState {
            registry,
            bearer_token: Arc::from(runtime.connection().bearer_token.as_str()),
            grants: runtime.grants.clone(),
        };
        let cursor = runtime.chat_connection(
            "thread-cursor",
            GatewayCaller {
                workspace_id: Some("ws-1".into()),
                native_sources: vec![McpConfigSource::CursorUser],
            },
        );
        let hermes = runtime.chat_connection("thread-hermes", GatewayCaller::default());
        assert_ne!(cursor.bearer_token, hermes.bearer_token);
        assert_ne!(cursor.bearer_token, runtime.connection().bearer_token);
        assert_eq!(cursor.url, runtime.connection().url);

        let (_, value) = tools_list_with_token(state.clone(), &cursor.bearer_token).await;
        assert_eq!(tool_names(&value), vec!["mcp__oc-user__tool"], "{value}");
        let (_, value) = tools_list_with_token(state.clone(), &hermes.bearer_token).await;
        let mut names = tool_names(&value);
        names.sort();
        assert_eq!(names, vec!["mcp__cursor-user__tool", "mcp__oc-user__tool"]);
        let (_, value) =
            tools_list_with_token(state.clone(), &runtime.connection().bearer_token).await;
        assert_eq!(tool_names(&value), vec!["mcp__cursor-user__tool"]);
        let (status, _) = tools_list_with_token(state, "not-a-grant").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_chat_keeps_its_token_and_takes_its_latest_caller() {
        let runtime = start_gateway(McpRegistry::new()).await.unwrap();
        let first = runtime.chat_connection("thread", GatewayCaller::default());
        let caller = GatewayCaller {
            workspace_id: Some("ws-2".into()),
            native_sources: vec![McpConfigSource::ClaudeUser],
        };
        let second = runtime.chat_connection("thread", caller.clone());
        assert_eq!(first, second);
        let grants = runtime.grants.lock().unwrap();
        assert_eq!(grants.callers.get(&second.bearer_token), Some(&caller));
    }

    #[test]
    fn acp_entries_carry_the_chat_token() {
        let connection = McpGatewayConnection {
            url: "http://127.0.0.1:1/mcp".into(),
            bearer_token: "tok".into(),
        };
        assert_eq!(
            connection.acp_http_server("codemux"),
            json!({
                "type": "http", "name": "codemux", "url": "http://127.0.0.1:1/mcp",
                "headers": [{ "name": "Authorization", "value": "Bearer tok" }]
            })
        );
        assert_eq!(
            connection.acp_stdio_bridge("codemux", "/bin/codemux"),
            json!({
                "name": "codemux", "command": "/bin/codemux", "args": ["mcp-bridge"],
                "env": [
                    { "name": BRIDGE_URL_ENV, "value": "http://127.0.0.1:1/mcp" },
                    { "name": BRIDGE_TOKEN_ENV, "value": "tok" }
                ]
            })
        );
    }

    #[tokio::test]
    async fn a_chat_token_scopes_plugin_servers_to_its_workspace() {
        let registry = McpRegistry::new();
        let server: super::super::registry::PluginMcpServer = serde_json::from_value(json!({
            "id": "fixture",
            "name": "Fixture",
            "toolPrefix": "fixture",
            "scope": { "workspaceIds": ["ws-a"] },
            "transport": "stdio",
            "command": "python3",
            "args": [format!("{}/tests/fixtures/plugin_mcp.py", env!("CARGO_MANIFEST_DIR"))],
        }))
        .unwrap();
        registry
            .register_plugin_server::<tauri::Wry>(None, "demo", "one".into(), server)
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while registry.list_tools_for_workspace(&[], Some("ws-a")).await.is_empty() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("fixture tools load");
        let runtime = start_gateway(registry.clone()).await.unwrap();
        let state = GatewayState {
            registry,
            bearer_token: Arc::from(runtime.connection().bearer_token.as_str()),
            grants: runtime.grants.clone(),
        };
        let chat = |key: &str, workspace: &str| {
            runtime
                .chat_connection(
                    key,
                    GatewayCaller { workspace_id: Some(workspace.into()), native_sources: vec![] },
                )
                .bearer_token
        };
        let (_, value) = tools_list_with_token(state.clone(), &chat("in", "ws-a")).await;
        assert!(value["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|tool| tool["name"] == "fixture__echo"), "{value}");
        let outside = chat("out", "ws-b");
        let (_, value) = tools_list_with_token(state.clone(), &outside).await;
        assert!(!value.to_string().contains("fixture__"), "{value}");
        let response = router(state)
            .oneshot(
                Request::post("/mcp")
                    .header("authorization", format!("Bearer {outside}"))
                    .body(Body::from(
                        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                               "params": {"name": "fixture__echo", "arguments": {}}})
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["result"]["isError"], true, "{value}");
    }

    #[tokio::test]
    async fn rejects_missing_auth_and_foreign_origins() {
        let no_auth = Request::post("/mcp").body(Body::from("{}")).unwrap();
        let response = router(state()).oneshot(no_auth).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let foreign = Request::post("/mcp")
            .header("authorization", "Bearer test-token")
            .header("origin", "https://attacker.example")
            .body(Body::from("{}"))
            .unwrap();
        let response = router(state()).oneshot(foreign).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    #[tokio::test]
    async fn plugin_mcp_gateway_never_exposes_plugin_only_scope() {
        let registry = super::super::registry::test_support::plugin_fixture("plugin").await;
        let app = router(GatewayState {
            registry: registry.clone(),
            bearer_token: Arc::from("test-token"),
            grants: Arc::default(),
        });
        let response = app
            .clone()
            .oneshot(request(
                json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}),
            ))
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert!(value["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .all(|tool| !tool["name"].as_str().unwrap().starts_with("fixture__")));
        for params in [
            json!({"name":"fixture__new_tool","arguments":{}}),
            json!({"name":"codemux_call_plugin_tool","arguments":{"name":"fixture__new_tool","arguments":{}}}),
        ] {
            let response = app
                .clone()
                .oneshot(request(
                    json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":params}),
                ))
                .await
                .unwrap();
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            let value: Value = serde_json::from_slice(&body).unwrap();
            assert!(value.get("error").is_some() || value["result"]["isError"] == true);
        }
        registry.shutdown_all().await;
    }

    #[tokio::test]
    async fn plugin_mcp_gateway_preserves_structured_results_for_native_and_bridge() {
        let registry = super::super::registry::test_support::plugin_fixture("all").await;
        let expected = registry
            .plugin_call("plugin:test-plugin:fixture", "image", json!({}))
            .await
            .unwrap();
        let app = router(GatewayState {
            registry: registry.clone(),
            bearer_token: Arc::from("test-token"),
            grants: Arc::default(),
        });
        for params in [
            json!({"name":"fixture__image","arguments":{}}),
            json!({"name":"codemux_call_plugin_tool","arguments":{"name":"fixture__image","arguments":{}}}),
        ] {
            let response = app
                .clone()
                .oneshot(request(
                    json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":params}),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            let value: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(value["result"], expected);
            assert_eq!(value.to_string().matches("STRUCTURED-581").count(), 1);
        }
        registry.shutdown_all().await;
    }

    #[tokio::test]
    async fn plugin_mcp_gateway_ignores_untrusted_workspace_query() {
        let registry = super::super::registry::test_support::plugin_fixture(
            r#"{"workspaceIds":["other-workspace"]}"#,
        )
        .await;
        let app = router(GatewayState {
            registry: registry.clone(),
            bearer_token: Arc::from("test-token"),
            grants: Arc::default(),
        });
        for method in ["tools/list", "tools/call"] {
            let mut req = request(
                json!({"jsonrpc":"2.0","id":1,"method":method,"params":{"name":"fixture__new_tool","arguments":{}}}),
            );
            *req.uri_mut() = "/mcp?workspaceId=other-workspace".parse().unwrap();
            let response = app.clone().oneshot(req).await.unwrap();
            let value: Value = serde_json::from_slice(
                &axum::body::to_bytes(response.into_body(), usize::MAX)
                    .await
                    .unwrap(),
            )
            .unwrap();
            if method == "tools/list" {
                assert!(
                    value["result"]["tools"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .all(|tool| !tool["name"].as_str().unwrap().starts_with("fixture__")),
                    "{value}"
                );
            } else {
                assert!(
                    value.get("error").is_some() || value["result"]["isError"] == true,
                    "{value}"
                );
            }
        }
        registry.shutdown_all().await;
    }

    #[tokio::test]
    async fn plugin_mcp_gateway_emits_list_changed() {
        use futures_util::StreamExt;
        let state = state();
        let registry = state.registry.clone();
        let response = router(state)
            .oneshot(
                Request::get("/mcp")
                    .header("authorization", "Bearer test-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let mut stream = response.into_body().into_data_stream();
        registry.notify_host_tools_changed();
        let frame = tokio::time::timeout(std::time::Duration::from_secs(1), stream.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(std::str::from_utf8(&frame)
            .unwrap()
            .contains("notifications/tools/list_changed"));
    }
}
