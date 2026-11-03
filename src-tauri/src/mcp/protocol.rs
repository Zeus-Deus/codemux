//! The MCP protocol pieces shared by Codemux's MCP servers: message envelopes,
//! version negotiation and tool-result shapes.
//!
//! Everything here except [`stdio`] is plain data and pure functions, with no
//! Tauri handle and no async runtime, so a synchronous server such as
//! `codemux-remote mcp` can use it too. Transports own framing, authentication
//! and HTTP status mapping.
//!
//! Codemux speaks the initialize era of MCP (newest 2025-11-25). Requests keep
//! their `params._meta` untouched, so a later stateless-era path can read the
//! per-request protocol version from it without a new envelope type.

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
/// clients: `jsonrpc` may be missing and `params` may be null. An error is
/// answered with `id: null`, as JSON-RPC requires when the request is invalid.
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

/// Check a Streamable HTTP request's `MCP-Protocol-Version` header. Not for
/// `initialize`, whose body carries the offer. A missing header means
/// 2025-03-26, as the spec says; any value Codemux does not speak, including
/// 2026-07-28, is an error the transport answers with HTTP 400.
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
        let tools = match self.tools_list_changed {
            None => json!({}),
            Some(list_changed) => json!({ "listChanged": list_changed }),
        };
        let mut result = json!({
            "protocolVersion": self.protocol_version,
            "capabilities": { "tools": tools },
            "serverInfo": { "name": self.server_info.name, "version": self.server_info.version },
        });
        if let Some(instructions) = self.instructions {
            result["instructions"] = json!(instructions);
        }
        result
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
