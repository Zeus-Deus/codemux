//! The MCP protocol pieces shared by Codemux's MCP servers: message envelopes,
//! version negotiation and tool-result shapes.
//!
//! Everything here except [`stdio`] is plain data and pure functions, with no
//! Tauri handle and no async runtime, so a synchronous server such as
//! `codemux-remote mcp` can use it too. Transports own framing, authentication
//! and HTTP status mapping.
//!
//! Codemux speaks both eras of MCP. The initialize era (2024-11-05 through
//! 2025-11-25) negotiates once per connection with `initialize`. The stateless
//! era (2026-07-28) has no handshake: each request names its revision in
//! `params._meta`, and `server/discover` describes the server. [`Era::of`]
//! tells them apart per request, so one process serves both kinds of client.
//! Only `codemux mcp` and the connector serve the stateless era; the in-app
//! gateway and `codemux-remote mcp` stay initialize-era servers.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

pub mod stdio;

#[cfg(test)]
mod tests;

pub const JSONRPC_VERSION: &str = "2.0";

/// Revisions negotiated through `initialize`, newest first. 2026-07-28 is a
/// stateless revision without `initialize`, so it is never negotiated here:
/// `initialize` selects initialize-era semantics by definition.
pub const SUPPORTED_PROTOCOL_VERSIONS: [&str; 4] =
    ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
pub const LATEST_PROTOCOL_VERSION: &str = SUPPORTED_PROTOCOL_VERSIONS[0];
/// Streamable HTTP: a request without the version header is assumed to speak
/// this revision.
pub const HTTP_DEFAULT_PROTOCOL_VERSION: &str = "2025-03-26";

pub const PROTOCOL_VERSION_HEADER: &str = "mcp-protocol-version";
pub const SESSION_HEADER: &str = "mcp-session-id";

/// Serve the stateless 2026-07-28 revision beside the initialize era. `false`
/// makes every request an initialize-era one again: per-request versions are
/// ignored and `server/discover` is method-not-found (on the connector before
/// authentication), which tells dual-era clients to fall back to `initialize`.
/// Flipping it needs no test edits: tests of stateless serving skip themselves
/// while it is off, and the real-SDK tests then expect the fallback.
pub const MODERN_ERA_ENABLED: bool = true;

/// Revisions served per request, without `initialize`, newest first.
pub const MODERN_PROTOCOL_VERSIONS: [&str; 1] = ["2026-07-28"];

/// The methods of the stateless revision Codemux serves. `initialize`,
/// `notifications/initialized` and `ping` do not exist in it.
pub const MODERN_METHODS: [&str; 3] = ["server/discover", "tools/list", "tools/call"];

/// Streamable HTTP headers that mirror a stateless-era request's body.
pub const METHOD_HEADER: &str = "mcp-method";
pub const NAME_HEADER: &str = "mcp-name";

/// How long a client may reuse a `server/discover` result: it changes only
/// when Codemux is upgraded.
pub const DISCOVER_TTL_MS: u64 = 3_600_000;
/// How long a client may reuse a `tools/list` result. A list depends only on
/// the build and the caller's access level, both fixed for a `codemux mcp`
/// process and for a network grant.
pub const TOOLS_LIST_TTL_MS: u64 = 300_000;

/// `_meta` keys of the stateless revision.
pub mod meta {
    pub const PROTOCOL_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
    pub const CLIENT_INFO: &str = "io.modelcontextprotocol/clientInfo";
    pub const CLIENT_CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";
    pub const SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";
}

/// JSON-RPC error codes Codemux emits. Codemux-defined server errors stay in
/// -32000..=-32019; -32020..=-32099 are reserved for the MCP specification.
pub mod code {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;
    /// A failure of something the server depends on, such as a daemon it
    /// forwards to.
    pub const SERVER_ERROR: i64 = -32000;
    /// Stateless era, HTTP: a header disagrees with the body, or a required
    /// header is missing or malformed.
    pub const HEADER_MISMATCH: i64 = -32020;
    /// Stateless era: the request names a revision Codemux does not speak.
    pub const UNSUPPORTED_PROTOCOL_VERSION: i64 = -32022;
}

// ---------------------------------------------------------------------------
// Envelope
// ---------------------------------------------------------------------------

/// A request id as the client sent it. MCP allows strings and integers only.
/// It serializes back unchanged: `7` stays `7` and `"7"` stays `"7"`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    Int(i64),
    Uint(u64),
    Str(String),
}

impl RequestId {
    /// `None` for anything MCP does not allow as an id: null, floats,
    /// booleans, arrays and objects.
    pub fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::String(id) => Some(Self::Str(id.clone())),
            Value::Number(number) => number
                .as_i64()
                .map(Self::Int)
                .or_else(|| number.as_u64().map(Self::Uint)),
            _ => None,
        }
    }
}

/// A request the server must answer.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub id: RequestId,
    pub method: String,
    /// Always an object; a missing or null `params` becomes `{}`. `_meta` is
    /// kept as the client sent it.
    pub params: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Notification {
    Initialized,
    Cancelled { request_id: RequestId, reason: Option<String> },
    /// Any other notification, including a malformed `notifications/cancelled`
    /// and an id-less message whose method is not a notification. It is
    /// acknowledged by silence and never executed.
    Other(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    Request(Request),
    Notification(Notification),
    /// A client's reply to a server request. Codemux sends none, so it is
    /// ignored, and it is never answered.
    Response,
}

/// Read one decoded JSON-RPC message, tolerating the quirks of existing local
/// clients: `jsonrpc` may be missing and `params` may be null. Answer an error
/// with the message's `id` when that is a valid id, and with `id: null` when
/// it cannot be read, as JSON-RPC requires.
pub fn parse(message: Value) -> Result<Incoming, RpcError> {
    let mut message = match message {
        Value::Object(message) => message,
        // MCP 2025-06-18 removed batching.
        Value::Array(_) => return Err(RpcError::invalid_request("Batch requests are not supported")),
        _ => return Err(RpcError::invalid_request("A JSON-RPC message must be an object")),
    };
    let method = match message.remove("method") {
        None => None,
        Some(Value::String(method)) => Some(method),
        Some(_) => return Err(RpcError::invalid_request("method must be a string")),
    };
    // Checked before the id so a client reply is never answered, whatever its id.
    if method.is_none() && (message.contains_key("result") || message.contains_key("error")) {
        return Ok(Incoming::Response);
    }
    let id = match message.remove("id") {
        None => None,
        Some(id) => Some(
            RequestId::from_value(&id)
                .ok_or_else(|| RpcError::invalid_request("id must be a string or an integer"))?,
        ),
    };
    let params = match message.remove("params") {
        None | Some(Value::Null) => Value::Object(Map::new()),
        Some(params @ Value::Object(_)) => params,
        Some(_) => return Err(RpcError::invalid_request("params must be an object")),
    };
    match (id, method) {
        (Some(id), Some(method)) => Ok(Incoming::Request(Request { id, method, params })),
        (None, Some(method)) => Ok(Incoming::Notification(Notification::read(method, &params))),
        (_, None) => Err(RpcError::invalid_request("A request needs a method")),
    }
}

impl Notification {
    fn read(method: String, params: &Value) -> Self {
        match method.as_str() {
            "notifications/initialized" => Self::Initialized,
            "notifications/cancelled" => match params.get("requestId").and_then(RequestId::from_value) {
                Some(request_id) => Self::Cancelled {
                    request_id,
                    reason: params.get("reason").and_then(Value::as_str).map(str::to_owned),
                },
                None => Self::Other(method),
            },
            _ => Self::Other(method),
        }
    }
}

/// One JSON-RPC reply.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Response {
    jsonrpc: &'static str,
    /// `null` for envelope errors, whose id could not be read.
    id: Option<RequestId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcError>,
}

impl Response {
    pub fn result(id: RequestId, result: Value) -> Self {
        Self { jsonrpc: JSONRPC_VERSION, id: Some(id), result: Some(result), error: None }
    }

    pub fn error(id: Option<RequestId>, error: RpcError) -> Self {
        Self { jsonrpc: JSONRPC_VERSION, id, result: None, error: Some(error) }
    }

    pub fn from_outcome(id: RequestId, outcome: Result<Value, RpcError>) -> Self {
        match outcome {
            Ok(result) => Self::result(id, result),
            Err(error) => Self::error(Some(id), error),
        }
    }

    /// One compact line with no embedded newline (stdio framing): JSON string
    /// escaping turns every newline in a value into `\n`.
    pub fn to_line(&self) -> String {
        serde_json::to_string(self).expect("a JSON-RPC response always serializes")
    }

    pub fn to_value(&self) -> Value {
        serde_json::to_value(self).expect("a JSON-RPC response always serializes")
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), data: None }
    }

    pub fn parse_error(detail: impl std::fmt::Display) -> Self {
        Self::new(code::PARSE_ERROR, format!("Parse error: {detail}"))
    }

    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(code::INVALID_REQUEST, message)
    }

    pub fn method_not_found(method: &str) -> Self {
        Self::new(code::METHOD_NOT_FOUND, format!("Method not found: {method}"))
    }

    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(code::INVALID_PARAMS, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(code::INTERNAL_ERROR, message)
    }

    /// -32022 with every revision Codemux speaks, so a stateless-era client
    /// retries with one of them instead of falling back to `initialize`.
    pub fn unsupported_version(requested: &str) -> Self {
        Self {
            code: code::UNSUPPORTED_PROTOCOL_VERSION,
            message: format!("Unsupported protocol version: {requested}"),
            data: Some(json!({ "supported": supported_versions(), "requested": requested })),
        }
    }

    pub fn header_mismatch(detail: impl std::fmt::Display) -> Self {
        Self::new(code::HEADER_MISMATCH, format!("Header mismatch: {detail}"))
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

// ---------------------------------------------------------------------------
// Version negotiation
// ---------------------------------------------------------------------------

/// Answer the client's `initialize` offer when Codemux speaks it, otherwise
/// the latest revision. Versions are compared exactly and need not be dates.
pub fn negotiate(requested: Option<&str>) -> &'static str {
    requested
        .and_then(|requested| SUPPORTED_PROTOCOL_VERSIONS.into_iter().find(|v| *v == requested))
        .unwrap_or(LATEST_PROTOCOL_VERSION)
}

/// Check an initialize-era Streamable HTTP request's `MCP-Protocol-Version`
/// header. Not for `initialize`, whose body carries the offer. A missing
/// header means 2025-03-26, as the spec says; any value Codemux does not speak
/// this way, including 2026-07-28, is an error the transport answers with HTTP
/// 400. Stateless-era requests are told apart first ([`Era::of_http`]).
pub fn http_version(header: Option<&str>) -> Result<&'static str, UnsupportedVersion> {
    let Some(header) = header else {
        return Ok(HTTP_DEFAULT_PROTOCOL_VERSION);
    };
    SUPPORTED_PROTOCOL_VERSIONS
        .into_iter()
        .find(|version| *version == header)
        .ok_or_else(|| UnsupportedVersion { requested: header.to_owned() })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedVersion {
    pub requested: String,
}

impl UnsupportedVersion {
    /// The JSON-RPC body for the HTTP 400. It uses -32600, not the stateless
    /// era's -32022, so a client probing with a newer revision reads it as an
    /// initialize-era server and falls back to `initialize`.
    pub fn rpc_error(&self) -> RpcError {
        RpcError::invalid_request(self.to_string())
    }
}

impl std::fmt::Display for UnsupportedVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Unsupported MCP-Protocol-Version: {} (supported: {})",
            self.requested,
            SUPPORTED_PROTOCOL_VERSIONS.join(", ")
        )
    }
}

// ---------------------------------------------------------------------------
// Initialize
// ---------------------------------------------------------------------------

/// The `clientInfo` an MCP client sent with `initialize`. Callers that turn
/// the name into an identity sanitise it themselves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientInfo {
    pub name: String,
    /// Empty when the client sent none.
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

impl ClientInfo {
    /// `None` unless the value is an object with a non-empty string `name`.
    /// Other keys (`description`, `icons`, `websiteUrl`, ...) are ignored.
    fn read(value: &Value) -> Option<Self> {
        let name = value.get("name").and_then(Value::as_str).filter(|name| !name.is_empty())?;
        let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
        Some(Self { name: name.to_owned(), version: text("version").unwrap_or_default(), title: text("title") })
    }
}

/// The parts of `initialize` params Codemux reads. Every field is optional:
/// local clients send `params: {}` and still get an answer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InitializeRequest {
    pub protocol_version: Option<String>,
    /// `None` when absent or malformed.
    pub client_info: Option<ClientInfo>,
}

impl InitializeRequest {
    pub fn parse(params: &Value) -> Self {
        Self {
            protocol_version: params.get("protocolVersion").and_then(Value::as_str).map(str::to_owned),
            client_info: params.get("clientInfo").and_then(ClientInfo::read),
        }
    }

    pub fn negotiated_version(&self) -> &'static str {
        negotiate(self.protocol_version.as_deref())
    }
}

/// `serverInfo` in an `initialize` result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerInfo {
    pub name: &'static str,
    pub version: &'static str,
}

/// An `initialize` result. Each server keeps its own name, capabilities and
/// instructions; this only fixes the shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InitializeResult<'a> {
    pub protocol_version: &'static str,
    /// `None` advertises `"tools": {}`; `Some(flag)` adds `listChanged`.
    pub tools_list_changed: Option<bool>,
    pub server_info: ServerInfo,
    pub instructions: Option<&'a str>,
}

impl InitializeResult<'_> {
    pub fn to_value(&self) -> Value {
        let mut result = json!({
            "protocolVersion": self.protocol_version,
            "capabilities": capabilities(self.tools_list_changed),
            "serverInfo": self.server_info.to_value(),
        });
        if let Some(instructions) = self.instructions {
            result["instructions"] = json!(instructions);
        }
        result
    }
}

impl ServerInfo {
    fn to_value(self) -> Value {
        json!({ "name": self.name, "version": self.version })
    }
}

/// `None` advertises `"tools": {}`; `Some(flag)` adds `listChanged`.
fn capabilities(tools_list_changed: Option<bool>) -> Value {
    let tools = match tools_list_changed {
        None => json!({}),
        Some(list_changed) => json!({ "listChanged": list_changed }),
    };
    json!({ "tools": tools })
}

// ---------------------------------------------------------------------------
// Stateless era (2026-07-28)
// ---------------------------------------------------------------------------

/// Every revision Codemux speaks, newest first: what `server/discover` and an
/// unsupported-version error advertise.
pub fn supported_versions() -> Vec<&'static str> {
    MODERN_PROTOCOL_VERSIONS.into_iter().chain(SUPPORTED_PROTOCOL_VERSIONS).collect()
}

/// Whether `version` names a stateless-era revision Codemux serves.
pub fn is_modern_version(version: &str) -> bool {
    MODERN_ERA_ENABLED && MODERN_PROTOCOL_VERSIONS.contains(&version)
}

/// Which era of MCP one request speaks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Era {
    /// Served under initialize-era rules.
    Legacy,
    /// Served statelessly at the revision the request names.
    Modern(ModernRequest),
}

/// What a stateless-era request says about itself in `_meta`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModernRequest {
    pub version: &'static str,
    /// Self-reported, so good for attribution only.
    pub client_info: Option<ClientInfo>,
}

impl Era {
    /// The era of one request, from its `params._meta`:
    /// - `initialize` always selects the initialize era (the dual-era rule);
    /// - no `_meta` revision, or an initialize-era one: [`Era::Legacy`];
    /// - a stateless revision Codemux serves: [`Era::Modern`], which must
    ///   also declare its client capabilities (-32602 otherwise);
    /// - any other revision: -32022 listing the ones Codemux speaks.
    pub fn of(method: &str, params: &Value) -> Result<Self, RpcError> {
        era_of(MODERN_ERA_ENABLED, method, params)
    }

    /// [`Era::of`] for Streamable HTTP, where `MCP-Protocol-Version` (`header`)
    /// mirrors the body. A stateless request needs a matching header
    /// (-32020). A stateless header on a body without a revision is -32602,
    /// never quietly served under initialize-era rules. A header naming a
    /// revision Codemux does not speak is -32022.
    pub fn of_http(header: Option<&str>, method: &str, params: &Value) -> Result<Self, RpcError> {
        era_of_http(MODERN_ERA_ENABLED, header, method, params)
    }
}

fn envelope_version(params: &Value) -> Option<&Value> {
    params.get("_meta")?.get(meta::PROTOCOL_VERSION)
}

fn missing_meta(key: &str) -> RpcError {
    RpcError::invalid_params(format!("Missing required _meta field {key}"))
}

fn era_of(enabled: bool, method: &str, params: &Value) -> Result<Era, RpcError> {
    if !enabled || method == "initialize" {
        return Ok(Era::Legacy);
    }
    let Some(version) = envelope_version(params) else {
        return Ok(Era::Legacy);
    };
    let Some(version) = version.as_str() else {
        return Err(RpcError::invalid_params(format!("_meta field {} must be a string", meta::PROTOCOL_VERSION)));
    };
    if SUPPORTED_PROTOCOL_VERSIONS.contains(&version) {
        return Ok(Era::Legacy);
    }
    let Some(version) = MODERN_PROTOCOL_VERSIONS.into_iter().find(|modern| *modern == version) else {
        return Err(RpcError::unsupported_version(version));
    };
    let fields = &params["_meta"];
    if !fields.get(meta::CLIENT_CAPABILITIES).is_some_and(Value::is_object) {
        return Err(missing_meta(meta::CLIENT_CAPABILITIES));
    }
    let client_info = fields.get(meta::CLIENT_INFO).and_then(ClientInfo::read);
    Ok(Era::Modern(ModernRequest { version, client_info }))
}

fn era_of_http(enabled: bool, header: Option<&str>, method: &str, params: &Value) -> Result<Era, RpcError> {
    if !enabled || method == "initialize" {
        return Ok(Era::Legacy);
    }
    let differs = |header: &str, body: &str| {
        RpcError::header_mismatch(format_args!(
            "MCP-Protocol-Version header value '{header}' does not match body value '{body}'"
        ))
    };
    match era_of(enabled, method, params)? {
        Era::Modern(request) => match header {
            Some(header) if header == request.version => Ok(Era::Modern(request)),
            Some(header) => Err(differs(header, request.version)),
            None => Err(RpcError::header_mismatch("missing required MCP-Protocol-Version header")),
        },
        Era::Legacy => match header {
            Some(header) if MODERN_PROTOCOL_VERSIONS.contains(&header) => {
                Err(match envelope_version(params).and_then(Value::as_str) {
                    Some(body) => differs(header, body),
                    None => missing_meta(meta::PROTOCOL_VERSION),
                })
            }
            Some(header) if !SUPPORTED_PROTOCOL_VERSIONS.contains(&header) => Err(RpcError::unsupported_version(header)),
            _ => Ok(Era::Legacy),
        },
    }
}

/// Check a stateless-era HTTP request's `Mcp-Method` and `Mcp-Name` headers
/// against its body: a missing or different one is -32020. `Mcp-Name` may
/// carry its value as `=?base64?…?=`. Requests without a name (`tools/list`,
/// `server/discover`) need only `Mcp-Method`.
pub fn check_standard_headers(
    method_header: Option<&str>,
    name_header: Option<&str>,
    method: &str,
    params: &Value,
) -> Result<(), RpcError> {
    match method_header {
        Some(header) if header == method => {}
        Some(header) => {
            return Err(RpcError::header_mismatch(format_args!(
                "Mcp-Method header value '{header}' does not match body value '{method}'"
            )))
        }
        None => return Err(RpcError::header_mismatch("missing required Mcp-Method header")),
    }
    let source = match method {
        "tools/call" | "prompts/get" => "name",
        "resources/read" => "uri",
        _ => return Ok(()),
    };
    let body = params.get(source).and_then(Value::as_str);
    let header = name_header.map(decode_header_value).transpose()?;
    match (header.as_deref(), body) {
        (Some(header), Some(body)) if header == body => Ok(()),
        (Some(header), Some(body)) => Err(RpcError::header_mismatch(format_args!(
            "Mcp-Name header value '{header}' does not match body value '{body}'"
        ))),
        (None, Some(_)) => Err(RpcError::header_mismatch("missing required Mcp-Name header")),
        (Some(_), None) => Err(RpcError::header_mismatch(format_args!("Mcp-Name header without a params.{source}"))),
        // The method's own parameter check reports the missing name.
        (None, None) => Ok(()),
    }
}

/// A mirrored header value, decoding the `=?base64?…?=` form clients use for
/// values that are not plain ASCII.
fn decode_header_value(value: &str) -> Result<String, RpcError> {
    use base64::Engine;
    let Some(encoded) = value.strip_prefix("=?base64?").and_then(|rest| rest.strip_suffix("?=")) else {
        return Ok(value.to_owned());
    };
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .ok_or_else(|| RpcError::header_mismatch("Mcp-Name header is not valid Base64"))
}

/// Mark a stateless-era result as final: `resultType: "complete"` and the
/// server's identity in `_meta`. Fields the result already has are kept.
pub fn complete_result(mut result: Value, server: ServerInfo) -> Value {
    if let Value::Object(fields) = &mut result {
        fields.entry("resultType").or_insert_with(|| json!("complete"));
        if let Value::Object(stamp) = fields.entry("_meta").or_insert_with(|| json!({})) {
            stamp.entry(meta::SERVER_INFO).or_insert_with(|| server.to_value());
        }
    }
    result
}

/// Who may keep a cached result (`cacheScope`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheScope {
    /// The same for every caller, so shared caches may keep it.
    Public,
    /// Only for the caller's own authorization.
    Private,
}

/// Add the cache hints the stateless era requires on `tools/list` and
/// `server/discover` results.
pub fn cacheable(mut result: Value, ttl_ms: u64, scope: CacheScope) -> Value {
    if let Value::Object(fields) = &mut result {
        let scope = match scope {
            CacheScope::Public => "public",
            CacheScope::Private => "private",
        };
        fields.insert("ttlMs".into(), json!(ttl_ms));
        fields.insert("cacheScope".into(), json!(scope));
    }
    result
}

/// A `server/discover` result: every revision Codemux speaks (both eras), its
/// capabilities, identity and instructions, ready to send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscoverResult<'a> {
    pub tools_list_changed: Option<bool>,
    pub server_info: ServerInfo,
    pub instructions: Option<&'a str>,
}

impl DiscoverResult<'_> {
    pub fn to_value(&self) -> Value {
        let mut result = json!({
            "supportedVersions": supported_versions(),
            "capabilities": capabilities(self.tools_list_changed),
        });
        if let Some(instructions) = self.instructions {
            result["instructions"] = json!(instructions);
        }
        // Nothing in it depends on who asks.
        cacheable(complete_result(result, self.server_info), DISCOVER_TTL_MS, CacheScope::Public)
    }
}

// ---------------------------------------------------------------------------
// Tool calls and results
// ---------------------------------------------------------------------------

/// `tools/call` params.
#[derive(Debug, Clone, PartialEq)]
pub struct CallToolParams {
    pub name: String,
    /// Always an object; missing or null becomes `{}`.
    pub arguments: Value,
    /// `_meta` when it is an object; anything else is ignored.
    pub meta: Option<Value>,
}

impl CallToolParams {
    pub fn parse(params: &Value) -> Result<Self, RpcError> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| RpcError::invalid_params("tools/call requires params.name"))?;
        let arguments = match params.get("arguments") {
            None | Some(Value::Null) => Value::Object(Map::new()),
            Some(arguments @ Value::Object(_)) => arguments.clone(),
            Some(_) => return Err(RpcError::invalid_params("tools/call params.arguments must be an object")),
        };
        let meta = params.get("_meta").filter(|meta| meta.is_object()).cloned();
        Ok(Self { name: name.to_owned(), arguments, meta })
    }
}

/// A successful text tool result: `{"content":[text]}`, with no `isError` key.
pub fn text_result(text: impl Into<String>) -> Value {
    json!({ "content": [text_content(text.into())] })
}

/// A failed text tool result: `{"content":[text],"isError":true}`. The model
/// sees it; it is not a JSON-RPC error.
pub fn error_result(text: impl Into<String>) -> Value {
    text_with_status(text, true)
}

/// A text tool result that always states `isError`, for servers whose clients
/// read the flag on every result.
pub fn text_with_status(text: impl Into<String>, is_error: bool) -> Value {
    json!({ "content": [text_content(text.into())], "isError": is_error })
}

fn text_content(text: String) -> Value {
    json!({ "type": "text", "text": text })
}
