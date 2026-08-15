//! Authenticated loopback MCP gateway for provider adapters.
//!
//! Codemux owns the real MCP child processes in [`McpRegistry`]. Providers
//! which can consume a remote MCP server (OpenCode today, future adapters
//! later) connect to this single virtual server instead of duplicating every
//! user configuration into provider-specific files.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use rand::distributions::{Alphanumeric, DistString};
use serde_json::{json, Value};

use super::registry::McpRegistry;
use super::McpConfigSource;

/// Sources the gateway hides from `tools/list`, mirroring the native-source
/// exclusions the Claude and Codex paths apply. OpenCode is the gateway's only
/// consumer today and `opencode serve` already spawns the servers from its own
/// config, so serving them back would double every tool. If another provider
/// ever attaches, the gateway will need per-consumer identity (a token or path
/// per consumer) instead of this fixed list.
const GATEWAY_NATIVE_SOURCES: [McpConfigSource; 2] =
    [McpConfigSource::OpenCodeUser, McpConfigSource::OpenCodeProject];

/// Connection details handed to a provider adapter. The token is generated
/// once per Codemux process and never written to disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpGatewayConnection {
    pub url: String,
    pub bearer_token: String,
}

/// Live gateway state retained by [`McpRegistry`]'s `OnceCell`.
pub struct McpGatewayRuntime {
    connection: McpGatewayConnection,
    abort_handle: tokio::task::AbortHandle,
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
}

impl Drop for McpGatewayRuntime {
    fn drop(&mut self) {
        self.abort_handle.abort();
    }
}

#[derive(Clone)]
struct GatewayState {
    registry: McpRegistry,
    bearer_token: Arc<str>,
}

/// Bind an authenticated MCP endpoint on an ephemeral loopback port.
pub async fn start_gateway(registry: McpRegistry) -> Result<McpGatewayRuntime, String> {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .map_err(|error| format!("failed to bind MCP gateway: {error}"))?;
    let address = listener
        .local_addr()
        .map_err(|error| format!("failed to read MCP gateway address: {error}"))?;
    let bearer_token = Alphanumeric.sample_string(&mut rand::thread_rng(), 48);
    let state = GatewayState {
        registry,
        bearer_token: Arc::from(bearer_token.as_str()),
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
        abort_handle: task.abort_handle(),
    })
}

fn router(state: GatewayState) -> Router {
    Router::new()
        .route(
            "/mcp",
            post(handle_post)
                .get(method_not_allowed)
                .delete(method_not_allowed),
        )
        .layer(DefaultBodyLimit::max(4 * 1024 * 1024))
        .with_state(state)
}

async fn method_not_allowed() -> StatusCode {
    StatusCode::METHOD_NOT_ALLOWED
}

async fn handle_post(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !origin_allowed(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !authenticated(&headers, &state.bearer_token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }

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
                    "capabilities": { "tools": { "listChanged": false } },
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
            let tools = state
                .registry
                .list_all_tools_excluding_sources(&GATEWAY_NATIVE_SOURCES)
                .await;
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
            match state.registry.dispatch_tool_call(name, arguments).await {
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

fn authenticated(headers: &HeaderMap, bearer_token: &str) -> bool {
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == format!("Bearer {bearer_token}"))
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
            false
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
        assert_eq!(names, vec!["mcp__shared__tool"], "{value}");
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
}
