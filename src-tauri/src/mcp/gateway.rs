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

use sha2::{Digest, Sha256};

use super::registry::McpRegistry;
use super::runtime::McpTool;
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
#[derive(Clone, PartialEq, Eq)]
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
    /// Present tool names without `__`. Grok 1.0.52 admits a tool only when
    /// `<server>__<tool>` has exactly one `__` (`grok mcp doctor`), and
    /// registry names such as `mcp__docs__search` already contain it.
    pub flat_tool_names: bool,
    pub(crate) native: Option<super::thread_gateway::GatewayBinding>,
}

/// `name` with every run of characters outside `[A-Za-z0-9-]` folded to one
/// `_`, never leading, so it can follow `server__` without forming `___`.
fn flat_tool_name(name: &str) -> String {
    let mut flat = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' {
            flat.push(ch);
        } else if !flat.is_empty() && !flat.ends_with('_') {
            flat.push('_');
        }
    }
    flat
}

/// The names a caller sees for `tools`, in order. Flattened names that
/// collide get a hash of the original appended, so each stays unique and
/// stable while the tool list is unchanged.
fn presented_names(tools: &[McpTool], flat: bool) -> Vec<String> {
    if !flat {
        return tools.iter().map(|tool| tool.prefixed_name.clone()).collect();
    }
    let names: Vec<String> = tools.iter().map(|tool| flat_tool_name(&tool.prefixed_name)).collect();
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for name in &names {
        *counts.entry(name.as_str()).or_default() += 1;
    }
    names
        .iter()
        .zip(tools)
        .map(|(name, tool)| {
            if counts[name.as_str()] == 1 {
                return name.clone();
            }
            let digest = Sha256::digest(tool.prefixed_name.as_bytes());
            let suffix: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
            format!("{}_{suffix}", name.trim_end_matches('_'))
        })
        .collect()
}

fn caller_names(tools: &[McpTool], caller: &GatewayCaller) -> Vec<String> {
    let mut presented = tools.to_vec();
    if caller.native.is_some() {
        for tool in &mut presented {
            if tool.prefixed_name.starts_with("mcp__codemux__") {
                tool.prefixed_name = tool.name.clone();
            }
        }
    }
    presented_names(&presented, caller.flat_tool_names)
}

/// Per-chat tokens, keyed by the consumer's credential key. Reissuing a key
/// keeps its token and replaces the caller, so a provider that keeps a
/// connection open (Hermes) follows the chat's current session.
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

impl std::fmt::Debug for McpGatewayConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpGatewayConnection")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

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

    /// Connection for one consumer, keyed by `key`. Refreshes the caller
    /// each time so a reopened chat picks up its current workspace and session.
    pub fn chat_connection(&self, key: &str, caller: GatewayCaller) -> McpGatewayConnection {
        if caller
            .native
            .as_ref()
            .is_some_and(|binding| !binding.current())
        {
            // A closed session gets a credential nothing accepts.
            return McpGatewayConnection {
                url: self.connection.url.clone(),
                bearer_token: new_token(),
            };
        }
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

    /// Forget every key that starts with `prefix`; their tokens stop working.
    pub fn revoke(&self, prefix: &str) {
        let mut grants = self.grants.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let Grants { tokens, callers } = &mut *grants;
        tokens.retain(|key, token| {
            let keep = !key.starts_with(prefix);
            if !keep {
                callers.remove(token);
            }
            keep
        });
    }
}

impl Drop for McpGatewayRuntime {
    fn drop(&mut self) {
        self.abort_handle.abort();
    }
}

#[derive(Clone)]
struct GatewayState {
    calls: Arc<Mutex<HashMap<CallKey, tokio::sync::watch::Sender<bool>>>>,
    registry: McpRegistry,
    /// The shared token OpenCode's instance-global registration uses.
    bearer_token: Arc<str>,
    grants: Arc<Mutex<Grants>>,
}

/// Credential, `Mcp-Session-Id` and JSON-RPC id of an in-flight call: ids
/// are unique only within one MCP session.
type CallKey = (String, String, String);

fn call_key(headers: &HeaderMap, request: &Value) -> CallKey {
    let header = |name| {
        headers
            .get(name)
            .and_then(|h| h.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    (
        header(axum::http::header::AUTHORIZATION.as_str()),
        header(SESSION_HEADER),
        request.to_string(),
    )
}

const SESSION_HEADER: &str = "mcp-session-id";

struct CallLease {
    calls: Arc<Mutex<HashMap<CallKey, tokio::sync::watch::Sender<bool>>>>,
    key: CallKey,
}
impl Drop for CallLease {
    fn drop(&mut self) {
        self.calls.lock().unwrap().remove(&self.key);
    }
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
        calls: Default::default(),
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
    if let Some(binding) = &caller.native {
        binding.session.mark_contacted();
    }
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
        if method == "notifications/cancelled" {
            if let Some(request) = params.get("requestId") {
                if let Some(cancel) = state.calls.lock().unwrap().get(&call_key(&headers, request)) {
                    let _ = cancel.send(true);
                }
            }
        }
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
            let session = uuid::Uuid::new_v4().simple().to_string();
            let mut response = rpc_result(
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
            .into_response();
            if let Ok(value) = session.parse() {
                response.headers_mut().insert(SESSION_HEADER, value);
            }
            response
        }
        "ping" => rpc_result(id, json!({})).into_response(),
        "tools/list" => {
            let tools = caller_tools(&state, &caller).await;
            let names = caller_names(&tools, &caller);
            let data: Vec<Value> = tools
                .into_iter()
                .zip(names)
                .map(|(tool, name)| {
                    json!({
                        "name": name,
                        "description": tool.description,
                        "inputSchema": tool.input_schema,
                    })
                })
                .collect();
            rpc_result(id, json!({ "tools": data })).into_response()
        }
        "tools/call" => {
            let Some(requested) = params.get("name").and_then(Value::as_str) else {
                return rpc_error(id, -32602, "tools/call requires params.name").into_response();
            };
            // Listing and dispatch share source exclusions and native policy.
            let tools = caller_tools(&state, &caller).await;
            let resolved = caller_names(&tools, &caller)
                    .into_iter()
                    .zip(tools)
                    .find(|(name, _)| name == requested)
                    .map(|(_, tool)| tool.prefixed_name);
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let key = call_key(&headers, &id);
            if let Some(binding) = &caller.native {
                if let Err(message) = calling_session_matches(binding, &params) {
                    return rpc_result(
                        id,
                        json!({
                            "content": [{ "type": "text", "text": message }],
                            "isError": true
                        }),
                    )
                    .into_response();
                }
            }
            let (cancel, mut cancelled) = tokio::sync::watch::channel(false);
            {
                let mut calls = state.calls.lock().unwrap();
                if calls.contains_key(&key) {
                    return rpc_error(id, -32600, "request ID is already in flight")
                        .into_response();
                }
                calls.insert(key.clone(), cancel);
            }
            let _lease = CallLease {
                calls: state.calls.clone(),
                key: key.clone(),
            };
            let invocation = async {
                let result = if let Some(name) = resolved {
                    if let Some(binding) = &caller.native {
                        let actor = binding.session.actor();
                        match actor.permit.wait_ready().await {
                            Err(error) => Err(error),
                            Ok(()) if !binding.current() => {
                                Err("stale_caller: gateway credential was revoked".into())
                            }
                            Ok(()) => match state
                .registry
                .call_plugin_bridge(&name, arguments.clone(), workspace_id, &[])
                .await
                            {
                                Some(result) => result,
                                None => {
                                    state
                                        .registry
                                        .dispatch_native_tool_call_with_id(
                                            &name,
                                            arguments,
                                            actor,
                                            Some(id.to_string()),
                                        )
                                        .await
                                }
                            },
                        }
                    } else {
                        match state
                            .registry
                            .call_plugin_bridge(&name, arguments.clone(), workspace_id, &[])
                            .await
                        {
                Some(result) => result,
                None => {
                    state
                        .registry
                        .dispatch_tool_call(&name, arguments, workspace_id)
                        .await
                }
            }
                    }
                } else {
                    Err("provider_control_denied: tool is unavailable to this chat".into())
                };
                result
            };
            let result = tokio::select! {
                biased;
                _=cancelled.changed()=>Err("request_cancelled: tool invocation ended; accepted work remains readable".into()),
                result=invocation=>result,
            };
            state.calls.lock().unwrap().remove(&key);
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

/// OpenCode 2 names the calling session in `_meta`. Every OpenCode chat shares
/// one server, and OpenCode's own permission rules (which a user's agent
/// configuration can widen) are the only thing hiding one chat's tools from
/// another. So an OpenCode call must name the session its token is bound to:
/// a missing name, another chat's session or a native subagent's child
/// session is refused. Other providers run one process per chat; a name they
/// send is still checked.
fn calling_session_matches(
    binding: &super::thread_gateway::GatewayBinding,
    params: &Value,
) -> Result<(), &'static str> {
    let named = params
        .pointer("/_meta/ai.opencode~1sessionID")
        .and_then(Value::as_str);
    let bound = binding.session.session_id();
    match named {
        Some(session) if !bound.is_empty() && session == bound => Ok(()),
        Some(_) => Err("provider_control_denied: Codemux tools act only for the chat's own session, not another chat or its native subagents"),
        None if binding.session.caller().provider == crate::agent_provider::ProviderKind::OpenCode => {
            Err("provider_control_denied: OpenCode did not name the calling session; OpenCode 2 or newer is required for agent collaboration")
        }
        None => Ok(()),
    }
}

/// Every tool the caller may list or call: its workspace's registry tools
/// minus the sources its agent loads itself, plus the plugin bridge tools.
async fn caller_tools(state: &GatewayState, caller: &GatewayCaller) -> Vec<McpTool> {
    let mut tools = state
        .registry
        .list_tools_for_workspace(&caller.native_sources, caller.workspace_id.as_deref())
        .await;
    if let Some(binding) = &caller.native {
        let host = binding.session.caller();
        tools.retain(|tool| super::registry::native_tool_visible(tool, &host));
        tools.extend(state.registry.host_tools(&host));
    }
    tools.extend(super::registry::plugin_bridge_tools());
    let mut seen = std::collections::HashSet::new();
    tools.retain(|tool| seen.insert(tool.prefixed_name.clone()));
    tools
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
            flat_tool_names: false,
            native: None,
        });
    }
    let grants = state.grants.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    grants.callers.get(token)
        .filter(|caller| {
            caller
                .native
                .as_ref()
                .is_none_or(|binding| binding.current())
        }).cloned()
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
            calls: Default::default(),
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
            calls: Default::default(),
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
            calls: Default::default(),
        };
        let cursor = runtime.chat_connection(
            "thread-cursor",
            GatewayCaller {
                workspace_id: Some("ws-1".into()),
                native_sources: vec![McpConfigSource::CursorUser],
                flat_tool_names: false,
                native: None,
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
            flat_tool_names: false,
            native: None,
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
            calls: Default::default(),
        };
        let chat = |key: &str, workspace: &str| {
            runtime
                .chat_connection(
                    key,
                    GatewayCaller { workspace_id: Some(workspace.into()), native_sources: vec![], flat_tool_names: false,
                        native: None,
                    },
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

    fn tool(prefixed_name: &str) -> McpTool {
        McpTool {
            name: "raw".into(),
            prefixed_name: prefixed_name.into(),
            description: None,
            input_schema: json!({}),
            server_id: "s".into(),
        }
    }

    #[test]
    fn flat_names_have_no_double_underscore_or_leading_underscore() {
        assert_eq!(flat_tool_name("mcp__fakeuser__fake_echo"), "mcp_fakeuser_fake_echo");
        assert_eq!(flat_tool_name("unity__manage_gameobject"), "unity_manage_gameobject");
        assert_eq!(flat_tool_name("__a.b:c___d-e"), "a_b_c_d-e");
        assert_eq!(flat_tool_name("codemux_list_plugin_tools"), "codemux_list_plugin_tools");
    }

    #[test]
    fn colliding_flat_names_stay_unique() {
        let tools = [tool("mcp__a__b_c"), tool("mcp__a_b__c"), tool("mcp__z__y")];
        let names = presented_names(&tools, true);
        assert_eq!(names[2], "mcp_z_y");
        assert_ne!(names[0], names[1]);
        assert!(names[0].starts_with("mcp_a_b_c_") && names[1].starts_with("mcp_a_b_c_"), "{names:?}");
        assert!(names.iter().all(|name| !name.contains("__")), "{names:?}");
        assert_eq!(names, presented_names(&tools, true), "stable across calls");
        assert_eq!(presented_names(&tools, false)[0], "mcp__a__b_c");
    }

    #[tokio::test]
    async fn a_flat_caller_lists_and_calls_by_flat_name() {
        let registry = McpRegistry::new();
        registry
            .insert_running_server_for_test("shared", vec![McpConfigSource::CodemuxUser])
            .await;
        let runtime = start_gateway(registry.clone()).await.unwrap();
        let state = GatewayState {
            registry,
            bearer_token: Arc::from(runtime.connection().bearer_token.as_str()),
            grants: runtime.grants.clone(),
            calls: Default::default(),
        };
        let token = runtime
            .chat_connection(
                "grok-thread",
                GatewayCaller { flat_tool_names: true, ..Default::default() },
            )
            .bearer_token;
        let (_, value) = tools_list_with_token(state.clone(), &token).await;
        let names: Vec<&str> = value["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            vec!["mcp_shared_tool", "codemux_list_plugin_tools", "codemux_call_plugin_tool"]
        );
        let response = router(state)
            .oneshot(
                Request::post("/mcp")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::from(
                        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
                               "params": {"name": "mcp_shared_tool", "arguments": {}}})
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        // Resolved to the registry tool: the test server has no transport,
        // where an unresolved name would be an unknown tool.
        let text = value["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("'shared' has no live transport"), "{value}");
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
            calls: Default::default(),
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
            calls: Default::default(),
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
            calls: Default::default(),
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

#[cfg(test)]
mod all_provider_tests {
    use super::*;
    use crate::{
        agent_provider::ProviderKind,
        mcp::{
            registry::{HostCaller, HostTools},
            runtime::prefix_tool_name,
            thread_gateway::{GatewayBinding, GatewaySession},
        },
    };
    struct Host;
    impl HostTools for Host {
        fn tools_for(&self, _: &HostCaller) -> Vec<McpTool> {
            vec![McpTool {
                name: "orchestrator_capabilities".into(),
                prefixed_name: prefix_tool_name("codemux", "orchestrator_capabilities"),
                description: None,
                input_schema: json!({"type":"object"}),
                server_id: "codemux".into(),
            }]
        }
        fn call(&self, _: &HostCaller, _: &str, _: &Value) -> Option<Value> {
            None
        }
    }
    #[tokio::test]
    async fn all_provider_gateway_identity_is_session_bound_survives_stop_and_rejects_hidden_tools() {
        let registry = McpRegistry::new();
        registry.set_host_tools(Arc::new(Host));
        registry.set_native_delegation_handler(Arc::new(|actor,_,args,_|Box::pin(async move {
            actor.permit.identity()?.verify(&actor.thread_id)?;
            Ok(json!({"thread":actor.thread_id,"session":actor.session_id,"untrusted":args["thread_id"]}))
        })));
        let gateway = start_gateway(registry).await.unwrap();
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        for (index, kind) in [
            ProviderKind::OpenCode,
            ProviderKind::Cursor,
            ProviderKind::Grok,
            ProviderKind::Hermes,
            ProviderKind::Acp,
        ]
        .into_iter()
        .enumerate()
        {
            let first = GatewaySession::new(
                &format!("all-provider-{index}-a"),
                kind,
                Some("same-workspace".into()),
                Some("default".into()),
            );
            first.bind_session("session-a");
            first.begin("turn-a", None).unwrap();
            let second = GatewaySession::new(
                &format!("all-provider-{index}-b"),
                kind,
                Some("same-workspace".into()),
                Some("default".into()),
            );
            second.bind_session("session-b");
            second.begin("turn-b", None).unwrap();
            let connection = |session: &Arc<GatewaySession>| {
                gateway.chat_connection(
                    &session.caller().thread_id,
                    GatewayCaller {
                        native: Some(GatewayBinding::new(session.clone())),
                        ..Default::default()
                    },
                )
            };
            let a = connection(&first);
            let b = connection(&second);
            assert_ne!(a.bearer_token, b.bearer_token);
            let listed: Value = client
                .post(&a.url)
                .bearer_auth(&a.bearer_token)
                .json(&json!({"id":1,"method":"tools/list"}))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            assert!(
                listed["result"]["tools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|t| t["name"] == "orchestrator_capabilities"),
                "{listed}"
            );
            // Each call names its chat's session, as OpenCode 2 does.
            let sessions = [(a.bearer_token.clone(), "session-a"), (b.bearer_token.clone(), "session-b")];
            let invoke = |connection: McpGatewayConnection, name: &'static str, args: Value| {
                let client = client.clone();
                let session = sessions
                    .iter()
                    .find(|(token, _)| *token == connection.bearer_token)
                    .map_or("replacement", |(_, session)| *session);
                async move {
                    client.post(connection.url).bearer_auth(connection.bearer_token).json(&json!({"id":2,"method":"tools/call","params":{"name":name,"arguments":args,"_meta":{"ai.opencode/sessionID":session}}})).send().await.unwrap()
                }
            };
            let reply: Value = invoke(
                a.clone(),
                "orchestrator_capabilities",
                json!({"thread_id":second.caller().thread_id}),
            )
            .await
            .json()
            .await
            .unwrap();
            let text = reply["result"]["content"][0]["text"].as_str().unwrap();
            let body: Value = serde_json::from_str(text).unwrap();
            assert_eq!(body["thread"], first.caller().thread_id);
            assert_eq!(body["session"], "session-a");
            let hidden: Value = invoke(a.clone(), "thread_respond", json!({}))
                .await
                .json()
                .await
                .unwrap();
            assert_eq!(hidden["result"]["isError"], true);
            // Stop keeps the credential; the turn fence refuses its calls
            // until the next turn begins, so nothing is reissued or reloaded.
            first.authority.invalidate();
            let stopped: Value = invoke(a.clone(), "orchestrator_capabilities", json!({}))
                .await
                .json()
                .await
                .unwrap();
            assert_eq!(stopped["result"]["isError"], true, "{stopped}");
            assert!(stopped.to_string().contains("revoked"), "{stopped}");
            assert_eq!(connection(&first).bearer_token, a.bearer_token);
            first.begin("after-stop", None).unwrap();
            assert_eq!(
                invoke(a.clone(), "orchestrator_capabilities", json!({}))
                    .await
                    .status(),
                StatusCode::OK
            );
            assert_eq!(
                invoke(b, "orchestrator_capabilities", json!({}))
                    .await
                    .status(),
                StatusCode::OK
            );
            // Replacing the session revokes its credential.
            let replacement = GatewaySession::new(
                &first.caller().thread_id,
                kind,
                Some("same-workspace".into()),
                Some("default".into()),
            );
            replacement.bind_session("replacement");
            replacement.begin("new-run", None).unwrap();
            assert_eq!(
                invoke(a, "orchestrator_capabilities", json!({}))
                    .await
                    .status(),
                StatusCode::UNAUTHORIZED
            );
            let new = gateway.chat_connection(
                &format!("replacement-{index}"),
                GatewayCaller {
                    native: Some(GatewayBinding::new(replacement.clone())),
                    ..Default::default()
                },
            );
            assert_eq!(
                invoke(new, "orchestrator_capabilities", json!({}))
                    .await
                    .status(),
                StatusCode::OK
            );
        }
    }
}

#[cfg(test)]
mod all_provider_cancel_tests {
    use super::*;
    use crate::{
        agent_provider::ProviderKind,
        mcp::{
            registry::{HostCaller, HostTools},
            runtime::prefix_tool_name,
            thread_gateway::{GatewayBinding, GatewaySession},
        },
    };
    struct Host;
    impl HostTools for Host {
        fn tools_for(&self, _: &HostCaller) -> Vec<McpTool> {
            vec![McpTool {
                name: "thread_wait".into(),
                prefixed_name: prefix_tool_name("codemux", "thread_wait"),
                description: None,
                input_schema: json!({}),
                server_id: "codemux".into(),
            }]
        }
        fn call(&self, _: &HostCaller, _: &str, _: &Value) -> Option<Value> {
            None
        }
    }
    #[tokio::test]
    async fn all_provider_cancellation_is_bound_to_the_calling_credential() {
        let registry = McpRegistry::new();
        registry.set_host_tools(Arc::new(Host));
        let entered = Arc::new(tokio::sync::Notify::new());
        let signal = entered.clone();
        registry.set_native_delegation_handler(Arc::new(move |_, _, _, _| {
            let signal = signal.clone();
            Box::pin(async move {
                signal.notify_one();
                std::future::pending().await
            })
        }));
        let gateway = start_gateway(registry).await.unwrap();
        let session = GatewaySession::new(
            "cancel-native",
            ProviderKind::Cursor,
            None,
            Some("agent".into()),
        );
        session.bind_session("s");
        session.begin("turn", None).unwrap();
        let connection = gateway.chat_connection(
            "cancel-native",
            GatewayCaller {
                native: Some(GatewayBinding::new(session)),
                ..Default::default()
            },
        );
        let other = gateway.chat_connection("other", Default::default());
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let call = {
            let client = client.clone();
            let connection = connection.clone();
            tokio::spawn(async move {
                client.post(connection.url).bearer_auth(connection.bearer_token).json(&json!({"id":19,"method":"tools/call","params":{"name":"thread_wait","arguments":{}}})).send().await.unwrap().json::<Value>().await.unwrap()
            })
        };
        entered.notified().await;
        let cancel = json!({"method":"notifications/cancelled","params":{"requestId":19}});
        client
            .post(&other.url)
            .bearer_auth(other.bearer_token)
            .json(&cancel)
            .send()
            .await
            .unwrap();
        assert!(
            !call.is_finished(),
            "another credential cannot cancel this call"
        );
        client
            .post(&connection.url)
            .bearer_auth(connection.bearer_token)
            .json(&cancel)
            .send()
            .await
            .unwrap();
        let reply = tokio::time::timeout(std::time::Duration::from_secs(2), call)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reply["result"]["isError"], true);
        assert!(reply.to_string().contains("request_cancelled"));
    }
}

#[cfg(test)]
mod session_scope_tests {
    use super::*;
    use crate::{
        agent_provider::ProviderKind,
        mcp::{
            registry::{HostCaller, HostTools},
            runtime::prefix_tool_name,
            thread_gateway::{GatewayBinding, GatewaySession},
        },
    };
    struct Host;
    impl HostTools for Host {
        fn tools_for(&self, _: &HostCaller) -> Vec<McpTool> {
            vec![McpTool {
                name: "thread_wait".into(),
                prefixed_name: prefix_tool_name("codemux", "thread_wait"),
                description: None,
                input_schema: json!({}),
                server_id: "codemux".into(),
            }]
        }
        fn call(&self, _: &HostCaller, _: &str, _: &Value) -> Option<Value> {
            None
        }
    }

    async fn fixture(
        provider: ProviderKind,
        block: bool,
    ) -> (McpGatewayRuntime, McpGatewayConnection, Arc<GatewaySession>, Arc<tokio::sync::Notify>) {
        let registry = McpRegistry::new();
        registry.set_host_tools(Arc::new(Host));
        let entered = Arc::new(tokio::sync::Notify::new());
        let signal = entered.clone();
        registry.set_native_delegation_handler(Arc::new(move |actor, _, _, _| {
            let signal = signal.clone();
            Box::pin(async move {
                signal.notify_one();
                if block {
                    std::future::pending::<()>().await;
                }
                Ok(json!({"session": actor.session_id}))
            })
        }));
        let gateway = start_gateway(registry).await.unwrap();
        let session = GatewaySession::new(
            &format!("scope-{}", uuid::Uuid::new_v4()),
            provider,
            None,
            Some("default".into()),
        );
        session.bind_session("ses_parent");
        session.begin("turn", None).unwrap();
        let connection = gateway.chat_connection(
            "scope",
            GatewayCaller {
                native: Some(GatewayBinding::new(session.clone())),
                ..Default::default()
            },
        );
        (gateway, connection, session, entered)
    }

    fn call(
        client: &reqwest::Client,
        connection: &McpGatewayConnection,
        mcp_session: Option<&str>,
        body: Value,
    ) -> reqwest::RequestBuilder {
        let request = client
            .post(&connection.url)
            .bearer_auth(&connection.bearer_token)
            .json(&body);
        match mcp_session {
            Some(id) => request.header(SESSION_HEADER, id),
            None => request,
        }
    }

    #[tokio::test]
    async fn request_ids_are_scoped_to_their_mcp_session() {
        let (_gateway, connection, session, entered) = fixture(ProviderKind::Hermes, true).await;
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let initialized = call(&client, &connection, None, json!({"id":0,"method":"initialize","params":{}}))
            .send()
            .await
            .unwrap();
        assert!(initialized.headers().get(SESSION_HEADER).is_some(), "initialize issues an MCP session");
        assert!(session.contacted(), "any authenticated request counts as contact");
        let wait = json!({"id":7,"method":"tools/call","params":{"name":"thread_wait","arguments":{}}});
        let first = tokio::spawn(call(&client, &connection, Some("one"), wait.clone()).send());
        entered.notified().await;
        // The same id in another MCP session sharing the token is not a duplicate.
        let second = tokio::spawn(call(&client, &connection, Some("two"), wait.clone()).send());
        entered.notified().await;
        let duplicate: Value = call(&client, &connection, Some("one"), wait)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(duplicate["error"]["code"], -32600, "{duplicate}");
        // Cancellation names its own session's request only.
        let cancel = json!({"method":"notifications/cancelled","params":{"requestId":7}});
        call(&client, &connection, Some("two"), cancel).send().await.unwrap();
        let reply: Value = tokio::time::timeout(std::time::Duration::from_secs(2), second)
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert!(reply.to_string().contains("request_cancelled"), "{reply}");
        assert!(!first.is_finished());
        first.abort();
    }

    #[tokio::test]
    async fn opencode_child_sessions_cannot_borrow_the_chat_credential() {
        let (_gateway, connection, _session, _entered) = fixture(ProviderKind::OpenCode, false).await;
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let invoke = |session: &str| {
            call(&client, &connection, None, json!({"id":1,"method":"tools/call","params":{
                "name":"thread_wait","arguments":{},"_meta":{"ai.opencode/sessionID":session}}}))
            .send()
        };
        let child: Value = invoke("ses_child").await.unwrap().json().await.unwrap();
        assert_eq!(child["result"]["isError"], true, "{child}");
        assert!(child.to_string().contains("native subagents"), "{child}");
        let own: Value = invoke("ses_parent").await.unwrap().json().await.unwrap();
        assert_ne!(own["result"]["isError"], true, "{own}");
        assert!(own.to_string().contains("ses_parent"), "{own}");
    }

    /// A user's OpenCode agent config (`agent.build.permission={"*":"allow"}`)
    /// overrides the server-wide `codemux_*` deny, so chat A is offered chat
    /// B's server and OpenCode sends B's token. The gateway alone must refuse.
    #[tokio::test]
    async fn opencode_calls_must_name_the_session_their_token_is_bound_to() {
        let (gateway, chat_a, _a, _entered) = fixture(ProviderKind::OpenCode, false).await;
        let b = GatewaySession::new("scope-b", ProviderKind::OpenCode, None, Some("default".into()));
        b.bind_session("ses_b");
        b.begin("turn-b", None).unwrap();
        let chat_b = gateway.chat_connection(
            "scope-b",
            GatewayCaller { native: Some(GatewayBinding::new(b)), ..Default::default() },
        );
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let invoke = |connection: &McpGatewayConnection, meta: Value| {
            call(&client, connection, None, json!({"id":1,"method":"tools/call","params":{
                "name":"thread_wait","arguments":{},"_meta":meta}}))
            .send()
        };
        let denied = |reply: &Value| reply["result"]["isError"] == true
            && reply.to_string().contains("provider_control_denied");
        // A's session presenting B's credential.
        let borrowed: Value = invoke(&chat_b, json!({"ai.opencode/sessionID":"ses_parent"}))
            .await.unwrap().json().await.unwrap();
        assert!(denied(&borrowed), "{borrowed}");
        // OpenCode 1.x sends only a progress token: never trusted.
        for meta in [json!({"progressToken":2}), json!(null)] {
            let unnamed: Value = invoke(&chat_b, meta).await.unwrap().json().await.unwrap();
            assert!(denied(&unnamed), "{unnamed}");
            assert!(unnamed.to_string().contains("OpenCode 2 or newer is required"), "{unnamed}");
        }
        // Each chat still reaches its own tools.
        let own_b: Value = invoke(&chat_b, json!({"ai.opencode/sessionID":"ses_b"}))
            .await.unwrap().json().await.unwrap();
        assert_ne!(own_b["result"]["isError"], true, "{own_b}");
        assert!(own_b.to_string().contains("ses_b"), "{own_b}");
        let own_a: Value = invoke(&chat_a, json!({"ai.opencode/sessionID":"ses_parent"}))
            .await.unwrap().json().await.unwrap();
        assert_ne!(own_a["result"]["isError"], true, "{own_a}");
    }

    #[tokio::test]
    async fn an_unbound_opencode_credential_acts_for_no_session() {
        let (gateway, _connection, _session, _entered) = fixture(ProviderKind::OpenCode, false).await;
        let unbound = GatewaySession::new("scope-unbound", ProviderKind::OpenCode, None, None);
        unbound.begin("turn", None).unwrap();
        let connection = gateway.chat_connection(
            "scope-unbound",
            GatewayCaller { native: Some(GatewayBinding::new(unbound)), ..Default::default() },
        );
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let reply: Value = call(&client, &connection, None, json!({"id":1,"method":"tools/call","params":{
            "name":"thread_wait","arguments":{},"_meta":{"ai.opencode/sessionID":""}}}))
            .send().await.unwrap().json().await.unwrap();
        assert_eq!(reply["result"]["isError"], true, "{reply}");
    }

    #[tokio::test]
    async fn one_process_providers_need_no_session_name() {
        let (_gateway, connection, _session, _entered) = fixture(ProviderKind::Hermes, false).await;
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let reply: Value = call(&client, &connection, None, json!({"id":1,"method":"tools/call","params":{
            "name":"thread_wait","arguments":{}}}))
            .send().await.unwrap().json().await.unwrap();
        assert_ne!(reply["result"]["isError"], true, "{reply}");
    }
}
