//! Minimal MCP Streamable HTTP client owned by the cross-provider registry.

use std::sync::atomic::{AtomicU64, Ordering};

use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use serde_json::{json, Value};
use tokio::sync::Mutex;

use super::McpServerConfig;

const MCP_PROTOCOL_VERSION: &str = "2025-06-18";

#[derive(Debug)]
pub struct HttpMcpClient {
    http: reqwest::Client,
    url: String,
    headers: HeaderMap,
    session_id: Mutex<Option<String>>,
    next_id: AtomicU64,
}

impl HttpMcpClient {
    pub async fn connect(config: &McpServerConfig) -> Result<Self, String> {
        let headers = headers_from_config(config)?;
        let client = Self {
            http: reqwest::Client::builder()
                .build()
                .map_err(|error| format!("build HTTP client: {error}"))?,
            url: config.command.clone(),
            headers,
            session_id: Mutex::new(None),
            next_id: AtomicU64::new(1),
        };
        let initialized = client
            .request(
                "initialize",
                json!({
                    "protocolVersion": MCP_PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {
                        "name": "codemux",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }),
            )
            .await?;
        if initialized
            .get("protocolVersion")
            .and_then(Value::as_str)
            .is_none()
        {
            return Err("initialize response omitted protocolVersion".into());
        }
        client
            .notify("notifications/initialized", json!({}))
            .await?;
        Ok(client)
    }

    pub async fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let response = self
            .post(json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method,
                "params": params
            }))
            .await?;
        if let Some(error) = response.get("error") {
            return Err(format!("{method}: {error}"));
        }
        response
            .get("result")
            .cloned()
            .ok_or_else(|| format!("{method}: response omitted result"))
    }

    async fn notify(&self, method: &str, params: Value) -> Result<(), String> {
        let _ = self
            .post(json!({ "jsonrpc": "2.0", "method": method, "params": params }))
            .await?;
        Ok(())
    }

    async fn post(&self, message: Value) -> Result<Value, String> {
        let is_initialize = message.get("method").and_then(Value::as_str) == Some("initialize");
        let mut request = self
            .http
            .post(&self.url)
            .headers(self.headers.clone())
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .json(&message);
        if !is_initialize {
            request = request.header("mcp-protocol-version", MCP_PROTOCOL_VERSION);
        }
        if let Some(session_id) = self.session_id.lock().await.clone() {
            request = request.header("mcp-session-id", session_id);
        }
        let response = request
            .send()
            .await
            .map_err(|error| format!("HTTP request failed: {error}"))?;
        let status = response.status();
        if let Some(session_id) = response
            .headers()
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
        {
            *self.session_id.lock().await = Some(session_id.to_string());
        }
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let body = response
            .text()
            .await
            .map_err(|error| format!("read HTTP response: {error}"))?;
        if !status.is_success() {
            return Err(format!("HTTP status {}: {body}", status.as_u16()));
        }
        if body.trim().is_empty() {
            return Ok(Value::Null);
        }
        if content_type.contains("text/event-stream") {
            return parse_sse_json(&body);
        }
        serde_json::from_str(&body).map_err(|error| format!("decode JSON response: {error}"))
    }
}

fn parse_sse_json(body: &str) -> Result<Value, String> {
    for line in body.lines() {
        if let Some(data) = line.strip_prefix("data:") {
            let data = data.trim();
            if data.is_empty() {
                continue;
            }
            return serde_json::from_str(data)
                .map_err(|error| format!("decode SSE JSON response: {error}"));
        }
    }
    Err("SSE response contained no JSON data event".into())
}

fn headers_from_config(config: &McpServerConfig) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    for key in ["headers", "http_headers"] {
        if let Some(values) = config.raw.get(key).and_then(Value::as_object) {
            for (name, value) in values {
                if let Some(value) = value.as_str() {
                    insert_header(&mut headers, name, &expand_env_markers(value))?;
                }
            }
        }
    }
    if let Some(values) = config
        .raw
        .get("env_http_headers")
        .and_then(Value::as_object)
    {
        for (name, env_name) in values {
            if let Some(env_name) = env_name.as_str() {
                if let Ok(value) = std::env::var(env_name) {
                    insert_header(&mut headers, name, &value)?;
                }
            }
        }
    }
    if let Some(env_name) = config
        .raw
        .get("bearer_token_env_var")
        .and_then(Value::as_str)
    {
        if let Ok(token) = std::env::var(env_name) {
            let value = HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|error| format!("invalid bearer token header: {error}"))?;
            headers.insert(AUTHORIZATION, value);
        }
    }
    Ok(headers)
}

fn insert_header(headers: &mut HeaderMap, name: &str, value: &str) -> Result<(), String> {
    let name = HeaderName::from_bytes(name.as_bytes())
        .map_err(|error| format!("invalid HTTP header name: {error}"))?;
    let value = HeaderValue::from_str(value)
        .map_err(|error| format!("invalid value for HTTP header {name}: {error}"))?;
    headers.insert(name, value);
    Ok(())
}

fn expand_env_markers(value: &str) -> String {
    let mut output = value.to_string();
    while let Some(start) = output.find("{env:") {
        let Some(relative_end) = output[start + 5..].find('}') else {
            break;
        };
        let end = start + 5 + relative_end;
        let name = &output[start + 5..end];
        let replacement = std::env::var(name).unwrap_or_default();
        output.replace_range(start..=end, &replacement);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sse_data_event() {
        let value = parse_sse_json(
            "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n\n",
        )
        .unwrap();
        assert_eq!(value["id"], 1);
    }

    #[test]
    fn expands_opencode_environment_markers() {
        std::env::set_var("CODEMUX_MCP_HTTP_TEST", "secret");
        assert_eq!(
            expand_env_markers("Bearer {env:CODEMUX_MCP_HTTP_TEST}"),
            "Bearer secret"
        );
        std::env::remove_var("CODEMUX_MCP_HTTP_TEST");
    }
}
