//! Minimal MCP Streamable HTTP client owned by the cross-provider registry.

use std::sync::atomic::{AtomicU64, Ordering};

use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use serde_json::{json, Value};
use tokio::sync::{Mutex, broadcast};
use futures_util::StreamExt;
use crate::json_rpc_child::Notification;

use super::McpServerConfig;

const MCP_PROTOCOL_VERSION: &str = "2025-06-18";

#[derive(Debug)]
pub struct HttpMcpClient {
    http: reqwest::Client,
    url: String,
    headers: HeaderMap,
    session_id: Mutex<Option<String>>,
    next_id: AtomicU64,
    notifications: broadcast::Sender<Notification>,
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
            notifications: broadcast::channel(256).0,
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

    pub fn notifications(&self) -> broadcast::Receiver<Notification> { self.notifications.subscribe() }

    /// A server may refuse GET (405) when it has no unsolicited events.
    pub async fn listen_notifications(&self) {
        let mut request = self.http.get(&self.url).headers(self.headers.clone()).header(ACCEPT, "text/event-stream").header("mcp-protocol-version", MCP_PROTOCOL_VERSION);
        if let Some(id) = self.session_id.lock().await.clone() { request = request.header("mcp-session-id", id); }
        let Ok(response) = request.send().await else { return; };
        if !response.status().is_success() { return; }
        let mut stream = response.bytes_stream();
        let mut decoder = SseDecoder::default();
        while let Some(Ok(bytes)) = stream.next().await {
            let Ok(messages) = decoder.push(&bytes) else { return; };
            for value in messages { self.emit_notification(&value); }
        }
    }
    fn emit_notification(&self, value: &Value) {
        if value.get("id").is_none() {
            if let Some(method) = value.get("method").and_then(Value::as_str) {
                let _ = self.notifications.send(Notification { method: method.into(), params: value.get("params").cloned().unwrap_or(Value::Null) });
            }
        }
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
        if !status.is_success() { return Err(format!("HTTP status {}", status.as_u16())); }
        if content_type.contains("text/event-stream") {
            let expected_id = message.get("id");
            let mut stream = response.bytes_stream();
            let mut decoder = SseDecoder::default();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| format!("read SSE: {e}"))?;
                for value in decoder.push(&chunk)? {
                    self.emit_notification(&value);
                    if expected_id.is_some() && value.get("id") == expected_id { return Ok(value); }
                }
            }
            return Err("SSE response ended before request result".into());
        }
        let body = response.text().await.map_err(|e| format!("read HTTP response: {e}"))?;
        if body.trim().is_empty() { return Ok(Value::Null); }

        serde_json::from_str(&body).map_err(|error| format!("decode JSON response: {error}"))
    }
}

#[derive(Default)]
struct SseDecoder { buffer: Vec<u8>, data: Vec<String> }
impl SseDecoder {
    fn push(&mut self, chunk: &[u8]) -> Result<Vec<Value>, String> {
        self.buffer.extend_from_slice(chunk);
        if self.buffer.len() + self.data.iter().map(String::len).sum::<usize>() > 16 * 1024 * 1024 { return Err("SSE event exceeds 16 MiB".into()); }
        let mut out = Vec::new();
        while let Some(end) = self.buffer.iter().position(|b| *b == b'\n') {
            let line: Vec<_> = self.buffer.drain(..=end).collect();
            let line = std::str::from_utf8(&line).map_err(|e| e.to_string())?.trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                if !self.data.is_empty() {
                    let data = self.data.join("\n"); self.data.clear();
                    out.push(serde_json::from_str(&data).map_err(|e| format!("decode SSE JSON: {e}"))?);
                }
            } else if let Some(data) = line.strip_prefix("data:") { self.data.push(data.strip_prefix(' ').unwrap_or(data).into()); }
        }
        Ok(out)
    }
}
#[cfg(test)]
fn parse_sse_json(body: &str) -> Result<Value, String> {
    SseDecoder::default().push(body.as_bytes())?.into_iter().next().ok_or("SSE response contained no JSON data event".into())
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

#[cfg(test)]
mod live_tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use axum::{Router, Json, extract::State, routing::post, response::sse::{Sse, Event}};
    use crate::mcp::registry::McpRegistry;

    #[derive(Clone)]
    struct Fixture { ready: Arc<AtomicBool>, connected: Arc<AtomicBool>, changes: broadcast::Sender<()> }
    async fn handle(State(state): State<Fixture>, Json(request): Json<Value>) -> axum::response::Response {
        use axum::response::IntoResponse;
        let Some(id) = request.get("id") else { return axum::http::StatusCode::ACCEPTED.into_response(); };
        let result = match request["method"].as_str().unwrap() {
            "initialize" => json!({"protocolVersion":"2025-06-18","capabilities":{"tools":{"listChanged":true}},"serverInfo":{"name":"fixture","version":"1"}}),
            "tools/list" if !state.ready.load(Ordering::SeqCst) => json!({"tools":[]}),
            "tools/list" if request["params"]["cursor"].is_null() => json!({"tools":[{"name":"first","inputSchema":{"type":"object"}}],"nextCursor":"second"}),
            "tools/list" => json!({"tools":[{"name":"second","inputSchema":{"type":"object"}}]}),
            "tools/call" => json!({"content":[{"type":"image","mimeType":"image/jpeg","data":"/9j/intact"}]}),
            _ => json!({}),
        };
        Json(json!({"jsonrpc":"2.0","id":id,"result":result})).into_response()
    }
    async fn events(State(state): State<Fixture>) -> Sse<impl futures_core::Stream<Item=Result<Event, std::convert::Infallible>>> {
        let rx = state.changes.subscribe();
        state.connected.store(true, Ordering::SeqCst);
        Sse::new(futures_util::stream::unfold(rx, |mut rx| async {
            rx.recv().await.ok()?;
            Some((Ok(Event::default().data(r#"{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}"#)), rx))
        }))
    }
    #[tokio::test]
    async fn plugin_mcp_http_pagination_and_live_notification() {
        let fixture = Fixture { ready: Arc::new(AtomicBool::new(false)), connected: Arc::new(AtomicBool::new(false)), changes: broadcast::channel(8).0 };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let app = Router::new().route("/mcp", post(handle).get(events)).with_state(fixture.clone());
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); });
        let registry = McpRegistry::new();
        registry.register_plugin_server::<tauri::Wry>(None, "demo", "http".into(), serde_json::from_value(json!({"id":"http","name":"HTTP fixture","toolPrefix":"httpfixture","scope":"all","transport":"http","url":url,"headers":{}})).unwrap()).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !fixture.connected.load(Ordering::SeqCst) { tokio::time::sleep(std::time::Duration::from_millis(10)).await; }
        }).await.unwrap();
        fixture.ready.store(true, Ordering::SeqCst);
        fixture.changes.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while registry.list_tools_for_server("plugin:demo:http").await.len() != 2 { tokio::time::sleep(std::time::Duration::from_millis(10)).await; }
        }).await.unwrap();
        let result = registry.dispatch_tool_call("httpfixture__second", json!({}), None).await.unwrap();
        assert_eq!(result["content"][0]["data"], "/9j/intact");
        registry.clear_plugin_servers::<tauri::Wry>(None).await;
        task.abort();
    }
    #[test]
    fn plugin_mcp_sse_decodes_split_utf8_and_multiline_frames() {
        let mut decoder = SseDecoder::default();
        let data = "event: message\r\ndata: {\"text\":\"画像\"}\r\n\r\n".as_bytes();
        let mut values = Vec::new();
        for byte in data { values.extend(decoder.push(&[*byte]).unwrap()); }
        assert_eq!(values, vec![json!({"text":"画像"})]);
    }
}
