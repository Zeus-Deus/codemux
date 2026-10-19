//! Minimal MCP Streamable HTTP client owned by the cross-provider registry.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::json_rpc_child::Notification;
use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::{broadcast, watch, Mutex};

use super::McpServerConfig;

const MCP_PROTOCOL_VERSION: &str = "2025-06-18";

#[derive(Clone, Debug, Default)]
struct HttpSession {
    id: Option<String>,
    generation: u64,
    needs_initialization: bool,
}

#[derive(Debug)]
pub struct HttpMcpClient {
    http: reqwest::Client,
    url: String,
    headers: HeaderMap,
    session: Mutex<HttpSession>,
    initialize_lock: Mutex<()>,
    session_changed: watch::Sender<u64>,
    next_id: AtomicU64,
    notifications: broadcast::Sender<Notification>,
}

impl HttpMcpClient {
    pub async fn connect(config: &McpServerConfig) -> Result<Self, String> {
        let headers = headers_from_config(config)?;
        let mut http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none());
        // Local MCP credentials must never travel through an environment
        // proxy. Remote servers retain the user's normal proxy settings.
        if loopback_url(&config.command) {
            http = http.no_proxy();
        }
        let client = Self {
            http: http
                .build()
                .map_err(|error| format!("build HTTP client: {error}"))?,
            url: config.command.clone(),
            headers,
            session: Mutex::new(HttpSession {
                needs_initialization: true,
                ..Default::default()
            }),
            initialize_lock: Mutex::new(()),
            session_changed: watch::channel(0).0,
            next_id: AtomicU64::new(1),
            notifications: broadcast::channel(256).0,
        };
        client.ensure_session(None).await?;
        Ok(client)
    }

    pub fn notifications(&self) -> broadcast::Receiver<Notification> {
        self.notifications.subscribe()
    }

    /// GET/SSE is optional. Only reconnect after a real SSE stream opened,
    /// except that a session-bearing 404 always requires a fresh handshake.
    pub async fn listen_notifications(&self) {
        let mut backoff = Duration::from_millis(100);
        let mut opened = false;
        let mut renewed = false;
        let mut changes = self.session_changed.subscribe();
        loop {
            // A failed renewal stays pending. Never fall back to sessionless
            // GETs (a stateful server correctly rejects those with 400).
            let needs_initialization = self.session.lock().await.needs_initialization;
            if needs_initialization {
                if self.ensure_session(None).await.is_err() {
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(30));
                    continue;
                }
            }
            changes.borrow_and_update();
            let session = self.session.lock().await.clone();
            let mut request = self
                .http
                .get(&self.url)
                .headers(self.headers.clone())
                .header(ACCEPT, "text/event-stream")
                .header("mcp-protocol-version", MCP_PROTOCOL_VERSION);
            if let Some(id) = &session.id {
                request = request.header("mcp-session-id", id);
            }
            // Only bound connection/headers, not the lifetime of a healthy SSE.
            let response = tokio::select! {
                response = tokio::time::timeout(Duration::from_secs(5), request.send()) => response.ok().and_then(Result::ok),
                _ = changes.changed() => continue,
            };
            if let Some(response) = response {
                let status = response.status();
                if matches!(status.as_u16(), 405 | 501) {
                    return;
                }
                if status == reqwest::StatusCode::NOT_FOUND {
                    // One renewal per disconnected episode; a persistent 404
                    // must not create an initialize/list polling loop.
                    if renewed || (!opened && session.id.is_none()) {
                        return;
                    }
                    renewed = true;
                    let _ = self.ensure_session(Some(&session)).await;
                } else if status.is_success()
                    && response
                        .headers()
                        .get(CONTENT_TYPE)
                        .and_then(|v| v.to_str().ok())
                        .is_some_and(|v| {
                            v.split(';')
                                .next()
                                .is_some_and(|v| v.trim().eq_ignore_ascii_case("text/event-stream"))
                        })
                {
                    opened = true;
                    renewed = false;
                    let connected_at = std::time::Instant::now();
                    // Covers changes missed while disconnected, including the
                    // initial gap between tools/list and the first GET stream.
                    self.emit_notification(&json!({"method":"notifications/tools/list_changed"}));
                    let mut stream = response.bytes_stream();
                    let mut decoder = SseDecoder::default();
                    loop {
                        let next = tokio::select! {
                            next = stream.next() => next,
                            _ = changes.changed() => break,
                        };
                        let Some(Ok(bytes)) = next else {
                            break;
                        };
                        let Ok(messages) = decoder.push(&bytes) else {
                            break;
                        };
                        for value in messages {
                            self.emit_notification(&value);
                        }
                    }
                    if connected_at.elapsed() >= Duration::from_secs(5) {
                        backoff = Duration::from_millis(100);
                    }
                } else if status.is_success() || !opened {
                    return;
                }
            } else if !opened {
                return;
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(Duration::from_secs(30));
        }
    }

    /// Coalesce expiry reports from concurrent GET/POST requests. Retain the
    /// pending state until BOTH initialize and initialized have succeeded.
    async fn ensure_session(&self, expired: Option<&HttpSession>) -> Result<(), String> {
        let _initializing = self.initialize_lock.lock().await;
        {
            let mut session = self.session.lock().await;
            if expired.is_some_and(|old| old.generation == session.generation)
                && !session.needs_initialization
            {
                session.id = None;
                session.needs_initialization = true;
                session.generation += 1;
                self.session_changed.send_replace(session.generation);
            }
            if !session.needs_initialization {
                return Ok(());
            }
        }
        let renewing = self.session.lock().await.generation > 0;
        let mut error = String::new();
        for attempt in 0..4 {
            let handshake = async {
                let message = json!({"jsonrpc":"2.0","id":self.next_id.fetch_add(1, Ordering::Relaxed),"method":"initialize","params":{"protocolVersion":MCP_PROTOCOL_VERSION,"capabilities":{},"clientInfo":{"name":"codemux","version":env!("CARGO_PKG_VERSION")}}});
                let response = self.send_post(&message, None).await?;
                let id = response
                    .headers()
                    .get("mcp-session-id")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned);
                let result = self.read_response(&message, response).await?;
                if result.get("error").is_some()
                    || result["result"]["protocolVersion"].as_str().is_none()
                {
                    return Err(
                        "initialize response omitted protocolVersion or returned an error".into(),
                    );
                }
                let message =
                    json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}});
                let response = self.send_post(&message, id.as_deref()).await?;
                let result = self.read_response(&message, response).await?;
                if let Some(error) = result.get("error") {
                    return Err(format!("initialized: {error}"));
                }
                Ok::<_, String>(id)
            };
            match tokio::time::timeout(Duration::from_secs(5), handshake).await {
                Ok(Ok(id)) => {
                    let mut session = self.session.lock().await;
                    session.id = id;
                    session.needs_initialization = false;
                    session.generation += 1;
                    self.session_changed.send_replace(session.generation);
                    drop(session);
                    if renewing {
                        self.emit_notification(
                            &json!({"method":"notifications/tools/list_changed"}),
                        );
                    }
                    return Ok(());
                }
                Ok(Err(reason)) => error = reason,
                Err(_) => error = "initialize handshake timed out".into(),
            }
            if attempt < 3 {
                tokio::time::sleep(Duration::from_millis(100 << attempt)).await;
            }
        }
        Err(format!("MCP session initialization is pending: {error}"))
    }

    fn emit_notification(&self, value: &Value) {
        if value.get("id").is_none() {
            if let Some(method) = value.get("method").and_then(Value::as_str) {
                let _ = self.notifications.send(Notification {
                    method: method.into(),
                    params: value.get("params").cloned().unwrap_or(Value::Null),
                });
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

    async fn post(&self, message: Value) -> Result<Value, String> {
        self.ensure_session(None).await?;
        let session = self.session.lock().await.clone();
        let mut response = self.send_post(&message, session.id.as_deref()).await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND && session.id.is_some() {
            self.ensure_session(Some(&session)).await?;
            let session = self.session.lock().await.clone();
            // Retry only an expired-session 404, never an ambiguous tool error.
            response = self.send_post(&message, session.id.as_deref()).await?;
        }
        self.read_response(&message, response).await
    }

    async fn send_post(
        &self,
        message: &Value,
        session_id: Option<&str>,
    ) -> Result<reqwest::Response, String> {
        let is_initialize = message.get("method").and_then(Value::as_str) == Some("initialize");
        let mut request = self
            .http
            .post(&self.url)
            .headers(self.headers.clone())
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .json(message);
        if !is_initialize {
            request = request.header("mcp-protocol-version", MCP_PROTOCOL_VERSION);
        }
        if let Some(id) = session_id {
            request = request.header("mcp-session-id", id);
        }
        request
            .send()
            .await
            .map_err(|error| format!("HTTP request failed: {error}"))
    }

    async fn read_response(
        &self,
        message: &Value,
        response: reqwest::Response,
    ) -> Result<Value, String> {
        let status = response.status();
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        if !status.is_success() {
            let mut stream = response.bytes_stream();
            let mut body = Vec::new();
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                while let Some(Ok(chunk)) = stream.next().await {
                    body.extend_from_slice(&chunk[..chunk.len().min(4096 - body.len())]);
                    if body.len() == 4096 {
                        break;
                    }
                }
            })
            .await;
            return Err(format!(
                "HTTP status {}: {}",
                status.as_u16(),
                String::from_utf8_lossy(&body)
            ));
        }
        if content_type.contains("text/event-stream") {
            let expected_id = message.get("id");
            let mut stream = response.bytes_stream();
            let mut decoder = SseDecoder::default();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|e| format!("read SSE: {e}"))?;
                for value in decoder.push(&chunk)? {
                    self.emit_notification(&value);
                    if expected_id.is_some() && value.get("id") == expected_id {
                        return Ok(value);
                    }
                }
            }
            return Err("SSE response ended before request result".into());
        }
        let body = response
            .text()
            .await
            .map_err(|e| format!("read HTTP response: {e}"))?;
        if body.trim().is_empty() {
            return Ok(Value::Null);
        }

        serde_json::from_str(&body).map_err(|error| format!("decode JSON response: {error}"))
    }
}

pub(super) fn loopback_url(value: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(value) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.trim_end_matches('.');
    host == "localhost"
        || host.ends_with(".localhost")
        || host
            .trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

#[derive(Default)]
struct SseDecoder {
    buffer: Vec<u8>,
    data: Vec<String>,
}
impl SseDecoder {
    fn push(&mut self, chunk: &[u8]) -> Result<Vec<Value>, String> {
        self.buffer.extend_from_slice(chunk);
        if self.buffer.len() + self.data.iter().map(String::len).sum::<usize>() > 16 * 1024 * 1024 {
            return Err("SSE event exceeds 16 MiB".into());
        }
        let mut out = Vec::new();
        while let Some(end) = self.buffer.iter().position(|b| *b == b'\n') {
            let line: Vec<_> = self.buffer.drain(..=end).collect();
            let line = std::str::from_utf8(&line)
                .map_err(|e| e.to_string())?
                .trim_end_matches(['\r', '\n']);
            if line.is_empty() {
                if !self.data.is_empty() {
                    let data = self.data.join("\n");
                    self.data.clear();
                    out.push(
                        serde_json::from_str(&data).map_err(|e| format!("decode SSE JSON: {e}"))?,
                    );
                }
            } else if let Some(data) = line.strip_prefix("data:") {
                self.data
                    .push(data.strip_prefix(' ').unwrap_or(data).into());
            }
        }
        Ok(out)
    }
}
#[cfg(test)]
fn parse_sse_json(body: &str) -> Result<Value, String> {
    SseDecoder::default()
        .push(body.as_bytes())?
        .into_iter()
        .next()
        .ok_or("SSE response contained no JSON data event".into())
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
    use crate::mcp::registry::McpRegistry;
    use axum::{
        extract::State,
        response::sse::{Event, Sse},
        routing::post,
        Json, Router,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[derive(Clone)]
    struct Fixture {
        ready: Arc<AtomicBool>,
        connected: Arc<AtomicBool>,
        changes: broadcast::Sender<()>,
    }
    async fn handle(
        State(state): State<Fixture>,
        Json(request): Json<Value>,
    ) -> axum::response::Response {
        use axum::response::IntoResponse;
        let Some(id) = request.get("id") else {
            return axum::http::StatusCode::ACCEPTED.into_response();
        };
        let result = match request["method"].as_str().unwrap() {
            "initialize" => {
                json!({"protocolVersion":"2025-06-18","capabilities":{"tools":{"listChanged":true}},"serverInfo":{"name":"fixture","version":"1"}})
            }
            "tools/list" if !state.ready.load(Ordering::SeqCst) => json!({"tools":[]}),
            "tools/list" if request["params"]["cursor"].is_null() => {
                json!({"tools":[{"name":"first","inputSchema":{"type":"object"}}],"nextCursor":"second"})
            }
            "tools/list" => json!({"tools":[{"name":"second","inputSchema":{"type":"object"}}]}),
            "tools/call" => {
                json!({"content":[{"type":"image","mimeType":"image/jpeg","data":"/9j/intact"}]})
            }
            _ => json!({}),
        };
        Json(json!({"jsonrpc":"2.0","id":id,"result":result})).into_response()
    }
    async fn events(
        State(state): State<Fixture>,
    ) -> Sse<impl futures_core::Stream<Item = Result<Event, std::convert::Infallible>>> {
        let rx = state.changes.subscribe();
        state.connected.store(true, Ordering::SeqCst);
        Sse::new(futures_util::stream::unfold(rx, |mut rx| async {
            rx.recv().await.ok()?;
            Some((
                Ok(Event::default()
                    .data(r#"{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}"#)),
                rx,
            ))
        }))
    }
    #[tokio::test]
    async fn plugin_mcp_http_redirects_do_not_forward_custom_credentials() {
        use axum::{extract::State, response::IntoResponse};
        let received = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let destination = Router::new()
            .route(
                "/",
                post(
                    |State(count): State<Arc<std::sync::atomic::AtomicUsize>>| async move {
                        count.fetch_add(1, Ordering::SeqCst);
                        "destination"
                    },
                ),
            )
            .with_state(received.clone());
        let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let location = format!("http://{}/", socket.local_addr().unwrap());
        let target = tokio::spawn(async move {
            axum::serve(socket, destination).await.unwrap();
        });
        let redirect = Router::new().route(
            "/",
            post(move || {
                let location = location.clone();
                async move {
                    (
                        axum::http::StatusCode::TEMPORARY_REDIRECT,
                        [("location", location)],
                        "fixture redirect",
                    )
                        .into_response()
                }
            }),
        );
        let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", socket.local_addr().unwrap());
        let source = tokio::spawn(async move {
            axum::serve(socket, redirect).await.unwrap();
        });
        // Establish the pre-fix reqwest behaviour with synthetic credentials.
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .post(&url)
            .header("x-api-key", "fixture-only")
            .send()
            .await
            .unwrap();
        assert_eq!(received.load(Ordering::SeqCst), 1);
        let config = McpServerConfig {
            id: "redirect".into(),
            name: "redirect".into(),
            sources: vec![],
            command: url,
            args: vec![],
            env: Default::default(),
            disabled: false,
            transport: super::super::McpTransport::Http,
            raw: json!({"headers":{"x-api-key":"fixture-only"}}),
        };
        let error = HttpMcpClient::connect(&config).await.unwrap_err();
        assert!(
            error.contains("307") && error.contains("fixture redirect"),
            "{error}"
        );
        assert_eq!(
            received.load(Ordering::SeqCst),
            1,
            "redirect forwarded custom credentials"
        );
        source.abort();
        target.abort();
    }
    #[tokio::test]
    async fn plugin_mcp_http_without_an_event_stream_stops_without_reconciliation() {
        for status in [404, 405, 501, 503, 200] {
            let fixture = Fixture {
                ready: Arc::new(AtomicBool::new(false)),
                connected: Arc::new(AtomicBool::new(false)),
                changes: broadcast::channel(8).0,
            };
            let requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let count = requests.clone();
            let app = Router::new()
                .route(
                    "/mcp",
                    post(handle).get(move || {
                        let count = count.clone();
                        async move {
                            count.fetch_add(1, Ordering::SeqCst);
                            (
                                axum::http::StatusCode::from_u16(status).unwrap(),
                                [("content-type", "application/json")],
                                "{}",
                            )
                        }
                    }),
                )
                .with_state(fixture);
            let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let config = McpServerConfig {
                id: "no-events".into(),
                name: "No events".into(),
                sources: vec![],
                command: format!("http://{}/mcp", socket.local_addr().unwrap()),
                args: vec![],
                env: Default::default(),
                disabled: false,
                transport: super::super::McpTransport::Http,
                raw: json!({}),
            };
            let server = tokio::spawn(async move {
                axum::serve(socket, app).await.unwrap();
            });
            let client = HttpMcpClient::connect(&config).await.unwrap();
            let mut notifications = client.notifications();
            let stopped = tokio::time::timeout(
                std::time::Duration::from_millis(450),
                client.listen_notifications(),
            )
            .await;
            server.abort();
            assert!(
                stopped.is_ok(),
                "GET {status} kept retrying ({} requests)",
                requests.load(Ordering::SeqCst)
            );
            assert_eq!(requests.load(Ordering::SeqCst), 1, "GET {status}");
            assert!(
                notifications.try_recv().is_err(),
                "GET {status} reconciled without SSE"
            );
        }
    }

    #[tokio::test]
    async fn plugin_mcp_http_failed_session_renewal_retries_the_handshake() {
        use axum::{
            http::{HeaderMap, StatusCode},
            response::IntoResponse,
        };
        let log = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let requests = log.clone();
        let initializes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = initializes.clone();
        let app = Router::new().route("/mcp", post(move |headers: HeaderMap, Json(request): Json<Value>| {
            let log = requests.clone(); let count = count.clone();
            async move {
                let session = headers.get("mcp-session-id").and_then(|v|v.to_str().ok()).unwrap_or("none");
                let method = request["method"].as_str().unwrap();
                log.lock().unwrap().push(format!("{method}:{session}"));
                if method == "initialize" {
                    let attempt = count.fetch_add(1, Ordering::SeqCst);
                    if attempt == 1 { return StatusCode::SERVICE_UNAVAILABLE.into_response(); }
                    return ([("mcp-session-id", if attempt == 0 {"old"} else {"new"})], Json(json!({"jsonrpc":"2.0","id":request["id"],"result":{"protocolVersion":"2025-06-18","capabilities":{}}}))).into_response();
                }
                if request.get("id").is_none() { return StatusCode::ACCEPTED.into_response(); }
                Json(json!({"jsonrpc":"2.0","id":request["id"],"result":{"tools":[]}})).into_response()
            }
        }).get({ let log=log.clone(); move |headers:HeaderMap| { let log=log.clone(); async move {
            let session=headers.get("mcp-session-id").and_then(|v|v.to_str().ok()).unwrap_or("none");
            log.lock().unwrap().push(format!("GET:{session}"));
            match session {
                "old" => StatusCode::NOT_FOUND.into_response(),
                "new" => ([("content-type","text/event-stream")], axum::body::Body::from_stream(futures_util::stream::pending::<Result<axum::body::Bytes,std::io::Error>>())).into_response(),
                _ => StatusCode::BAD_REQUEST.into_response(),
            }
        }}}));
        let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config = McpServerConfig {
            id: "renew".into(),
            name: "renew".into(),
            sources: vec![],
            command: format!("http://{}/mcp", socket.local_addr().unwrap()),
            args: vec![],
            env: Default::default(),
            disabled: false,
            transport: super::super::McpTransport::Http,
            raw: json!({}),
        };
        let server = tokio::spawn(async move {
            axum::serve(socket, app).await.unwrap();
        });
        let client = Arc::new(HttpMcpClient::connect(&config).await.unwrap());
        let listening = client.clone();
        let listener = tokio::spawn(async move {
            listening.listen_notifications().await;
        });
        let recovered = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if log.lock().unwrap().iter().any(|s| s == "GET:new") {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await;
        listener.abort();
        server.abort();
        assert!(
            recovered.is_ok(),
            "recovery requests: {:?}",
            log.lock().unwrap()
        );
        let actual = log.lock().unwrap().clone();
        assert_eq!(
            actual,
            vec![
                "initialize:none",
                "notifications/initialized:old",
                "GET:old",
                "initialize:none",
                "initialize:none",
                "notifications/initialized:new",
                "GET:new"
            ]
        );
    }

    #[tokio::test]
    async fn plugin_mcp_http_stops_after_unsupported_or_persistent_expiry() {
        use axum::{
            http::{HeaderMap, StatusCode},
            response::IntoResponse,
        };
        // Even after a valid stream, 405/501 permanently disable GET. An
        // expired session gets one renewal, never an initialize/list loop.
        for status in [404, 405, 501] {
            let initializes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let gets = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let count = initializes.clone();
            let requests = gets.clone();
            let app = Router::new().route("/mcp", post(move |headers:HeaderMap, Json(message):Json<Value>| {
                let count=count.clone(); async move {
                    if message["method"] == "initialize" {
                        assert!(headers.get("mcp-session-id").is_none());
                        let generation=count.fetch_add(1, Ordering::SeqCst);
                        return ([("mcp-session-id",generation.to_string())],Json(json!({"jsonrpc":"2.0","id":message["id"],"result":{"protocolVersion":"2025-06-18"}}))).into_response();
                    }
                    StatusCode::ACCEPTED.into_response()
                }
            }).get(move || { let requests=requests.clone(); async move {
                if requests.fetch_add(1,Ordering::SeqCst)==0 {
                    ([("content-type","text/event-stream")],": opened\n\n").into_response()
                } else { StatusCode::from_u16(status).unwrap().into_response() }
            }}));
            let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let config = McpServerConfig {
                id: "expiry".into(),
                name: "expiry".into(),
                sources: vec![],
                command: format!("http://{}/mcp", socket.local_addr().unwrap()),
                args: vec![],
                env: Default::default(),
                disabled: false,
                transport: super::super::McpTransport::Http,
                raw: json!({}),
            };
            let server = tokio::spawn(async move {
                axum::serve(socket, app).await.unwrap();
            });
            let client = HttpMcpClient::connect(&config).await.unwrap();
            let stopped =
                tokio::time::timeout(Duration::from_secs(2), client.listen_notifications()).await;
            server.abort();
            assert!(stopped.is_ok(), "GET {status} kept retrying");
            assert_eq!(
                gets.load(Ordering::SeqCst),
                if status == 404 { 3 } else { 2 }
            );
            assert_eq!(
                initializes.load(Ordering::SeqCst),
                if status == 404 { 2 } else { 1 }
            );
        }
    }

    #[tokio::test]
    async fn plugin_mcp_http_post_expiry_renews_and_retries_once() {
        use axum::{
            http::{HeaderMap, StatusCode},
            response::IntoResponse,
        };
        for fail_initialized in [false, true] {
            let log = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
            let requests = log.clone();
            let initializes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let count = initializes.clone();
            let app=Router::new().route("/mcp",post(move |headers:HeaderMap,Json(message):Json<Value>| {
                let log=requests.clone();let count=count.clone();async move {
                    let session=headers.get("mcp-session-id").and_then(|v|v.to_str().ok()).unwrap_or("none");
                    let method=message["method"].as_str().unwrap();
                    log.lock().unwrap().push(format!("{method}:{session}"));
                    if method=="initialize" {
                        assert_eq!(session,"none");
                        let generation=count.fetch_add(1,Ordering::SeqCst);
                        return ([("mcp-session-id",generation.to_string())],Json(json!({"jsonrpc":"2.0","id":message["id"],"result":{"protocolVersion":"2025-06-18"}}))).into_response();
                    }
                    if method=="notifications/initialized" {
                        return if fail_initialized && session=="1" {StatusCode::SERVICE_UNAVAILABLE} else {StatusCode::ACCEPTED}.into_response();
                    }
                    if session=="none" {return StatusCode::BAD_REQUEST.into_response();}
                    if session=="0" {return StatusCode::NOT_FOUND.into_response();}
                    Json(json!({"jsonrpc":"2.0","id":message["id"],"result":{"tools":[]}})).into_response()
                }
            }));
            let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let config = McpServerConfig {
                id: "post-expiry".into(),
                name: "post-expiry".into(),
                sources: vec![],
                command: format!("http://{}/mcp", socket.local_addr().unwrap()),
                args: vec![],
                env: Default::default(),
                disabled: false,
                transport: super::super::McpTransport::Http,
                raw: json!({}),
            };
            let server = tokio::spawn(async move {
                axum::serve(socket, app).await.unwrap();
            });
            let client = HttpMcpClient::connect(&config).await.unwrap();
            let mut notifications = client.notifications();
            assert_eq!(
                client.request("tools/list", json!({})).await.unwrap(),
                json!({"tools":[]})
            );
            assert_eq!(
                notifications.try_recv().unwrap().method,
                "notifications/tools/list_changed"
            );
            let mut expected = vec![
                "initialize:none",
                "notifications/initialized:0",
                "tools/list:0",
                "initialize:none",
                "notifications/initialized:1",
            ];
            if fail_initialized {
                expected.extend([
                    "initialize:none",
                    "notifications/initialized:2",
                    "tools/list:2",
                ]);
            } else {
                expected.push("tools/list:1");
            }
            assert_eq!(*log.lock().unwrap(), expected);
            server.abort();
        }
    }

    #[tokio::test]
    async fn plugin_mcp_http_reconnects_and_reconciles() {
        for reconnect_status in [503, 502, 200] {
            let fixture = Fixture {
                ready: Arc::new(AtomicBool::new(false)),
                connected: Arc::new(AtomicBool::new(false)),
                changes: broadcast::channel(8).0,
            };
            let connections = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let count = connections.clone();
            let ready = fixture.ready.clone();
            let fail_list = Arc::new(AtomicBool::new(true));
            let app = Router::new()
                .route(
                    "/mcp",
                    post(
                        move |State(fixture): State<Fixture>, Json(request): Json<Value>| {
                            let fail_list = fail_list.clone();
                            async move {
                                use axum::response::IntoResponse;
                                if request["method"] == "tools/list"
                                    && fixture.ready.load(Ordering::SeqCst)
                                    && fail_list.swap(false, Ordering::SeqCst)
                                {
                                    return (
                                        axum::http::StatusCode::SERVICE_UNAVAILABLE,
                                        "fixture restarting",
                                    )
                                        .into_response();
                                }
                                handle(State(fixture), Json(request)).await
                            }
                        },
                    )
                    .get(move || {
                        let count = count.clone();
                        let ready = ready.clone();
                        async move {
                            use axum::response::IntoResponse;
                            let connection = count.fetch_add(1, Ordering::SeqCst);
                            if connection <= 1 {
                                (
                                    axum::http::StatusCode::from_u16(if connection == 0 {
                                        200
                                    } else {
                                        reconnect_status
                                    })
                                    .unwrap(),
                                    [("content-type", "text/event-stream")],
                                    ": first stream ended\n\n",
                                )
                                    .into_response()
                            } else {
                                ready.store(true, Ordering::SeqCst);
                                // No list_changed event: the reconnect itself must reconcile.
                                ([("content-type", "text/event-stream")], ": reconnected\n\n")
                                    .into_response()
                            }
                        }
                    }),
                )
                .with_state(fixture);
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}/mcp", listener.local_addr().unwrap());
            let task = tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            let registry = crate::mcp::registry::McpRegistry::new();
            registry.register_plugin_server::<tauri::Wry>(None, "demo", "http".into(), serde_json::from_value(json!({"id":"http","name":"HTTP fixture","toolPrefix":"httpfixture","scope":"all","transport":"http","url":url})).unwrap()).await.unwrap();
            let observed = tokio::time::timeout(std::time::Duration::from_secs(4), async {
                while registry
                    .list_tools_for_server("plugin:demo:http")
                    .await
                    .len()
                    != 2
                {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await;
            registry.shutdown_all().await;
            task.abort();
            assert!(
                observed.is_ok(),
                "no reconciliation after status {reconnect_status}; GET count {}",
                connections.load(Ordering::SeqCst)
            );
        }
    }
    #[tokio::test]
    async fn plugin_mcp_http_pagination_and_live_notification() {
        let fixture = Fixture {
            ready: Arc::new(AtomicBool::new(false)),
            connected: Arc::new(AtomicBool::new(false)),
            changes: broadcast::channel(8).0,
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/mcp", post(handle).get(events))
            .with_state(fixture.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let registry = McpRegistry::new();
        registry.register_plugin_server::<tauri::Wry>(None, "demo", "http".into(), serde_json::from_value(json!({"id":"http","name":"HTTP fixture","toolPrefix":"httpfixture","scope":"all","transport":"http","url":url,"headers":{}})).unwrap()).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while !fixture.connected.load(Ordering::SeqCst) {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        fixture.ready.store(true, Ordering::SeqCst);
        fixture.changes.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while registry
                .list_tools_for_server("plugin:demo:http")
                .await
                .len()
                != 2
            {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let result = registry
            .dispatch_tool_call("httpfixture__second", json!({}), None)
            .await
            .unwrap();
        assert_eq!(result["content"][0]["data"], "/9j/intact");
        registry.clear_plugin_servers::<tauri::Wry>(None).await;
        task.abort();
    }
    #[tokio::test]
    async fn plugin_mcp_http_backoff_resets_after_healthy_stream() {
        use axum::response::IntoResponse;
        use futures_util::StreamExt;
        let fixture = Fixture {
            ready: Arc::new(AtomicBool::new(false)),
            connected: Arc::new(AtomicBool::new(false)),
            changes: broadcast::channel(8).0,
        };
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (events, mut times) = tokio::sync::mpsc::unbounded_channel();
        let app = Router::new()
            .route(
                "/mcp",
                post(handle).get(move || {
                    let count = count.clone();
                    let events = events.clone();
                    async move {
                        match count.fetch_add(1, Ordering::SeqCst) {
                            0 => (
                                [("content-type", "text/event-stream")],
                                ": initial stream ended\n\n",
                            )
                                .into_response(),
                            1..=3 => axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response(),
                            4 => {
                                let stream = futures_util::stream::iter([Ok::<
                                    _,
                                    std::convert::Infallible,
                                >(
                                    axum::body::Bytes::from(": connected\n\n"),
                                )])
                                .chain(
                                    futures_util::stream::once(async move {
                                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                                        let _ = events.send(std::time::Instant::now());
                                        Ok(axum::body::Bytes::from(": healthy stream ended\n\n"))
                                    }),
                                );
                                let mut response = axum::response::Response::new(
                                    axum::body::Body::from_stream(stream),
                                );
                                response
                                    .headers_mut()
                                    .insert("content-type", "text/event-stream".parse().unwrap());
                                response
                            }
                            _ => {
                                let _ = events.send(std::time::Instant::now());
                                axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response()
                            }
                        }
                    }
                }),
            )
            .with_state(fixture);
        let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config = McpServerConfig {
            id: "backoff".into(),
            name: "backoff".into(),
            sources: vec![],
            command: format!("http://{}/mcp", socket.local_addr().unwrap()),
            args: vec![],
            env: Default::default(),
            disabled: false,
            transport: super::super::McpTransport::Http,
            raw: json!({}),
        };
        let server = tokio::spawn(async move {
            axum::serve(socket, app).await.unwrap();
        });
        let client = HttpMcpClient::connect(&config).await.unwrap();
        let listener = tokio::spawn(async move {
            client.listen_notifications().await;
        });
        let result = tokio::time::timeout(std::time::Duration::from_secs(10), async {
            let ended = times.recv().await.unwrap();
            times.recv().await.unwrap().duration_since(ended)
        })
        .await;
        listener.abort();
        server.abort();
        let elapsed = result.unwrap();
        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "healthy reconnect retained stale backoff: {elapsed:?}"
        );
    }

    #[test]
    fn plugin_mcp_sse_decodes_split_utf8_and_multiline_frames() {
        let mut decoder = SseDecoder::default();
        let data = "event: message\r\ndata: {\"text\":\"画像\"}\r\n\r\n".as_bytes();
        let mut values = Vec::new();
        for byte in data {
            values.extend(decoder.push(&[*byte]).unwrap());
        }
        assert_eq!(values, vec![json!({"text":"画像"})]);
    }

    #[test]
    fn plugin_mcp_loopback_host_classification() {
        for url in [
            "http://127.0.0.1/mcp",
            "https://127.9.8.7/mcp",
            "http://[::1]/mcp",
            "http://localhost/mcp",
            "http://localhost./mcp",
            "http://app.localhost/mcp",
        ] {
            assert!(loopback_url(url), "{url}");
        }
        for url in [
            "https://example.com/mcp",
            "http://localhost.example/mcp",
            "http://192.168.1.1/mcp",
        ] {
            assert!(!loopback_url(url), "{url}");
        }
    }

    #[tokio::test]
    async fn plugin_mcp_http_proxy_is_bypassed_only_for_loopback() {
        // Environment proxy discovery is process-global. Run alone in a child
        // test process instead of changing the other tests' environment.
        if std::env::var_os("CODEMUX_TEST_MCP_PROXY_CHILD").is_none() {
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "mcp::http_client::live_tests::plugin_mcp_http_proxy_is_bypassed_only_for_loopback", "--test-threads=1", "--nocapture"])
                .env("CODEMUX_TEST_MCP_PROXY_CHILD", "1")
                .status().unwrap();
            assert!(status.success());
            return;
        }
        use axum::{
            body::Bytes,
            extract::State,
            http::{HeaderMap, StatusCode},
            response::IntoResponse,
            Json, Router,
        };
        use std::sync::{Arc, Mutex};
        async fn peer(
            State(auth): State<Arc<Mutex<Vec<String>>>>,
            headers: HeaderMap,
            body: Bytes,
        ) -> axum::response::Response {
            auth.lock().unwrap().push(
                headers
                    .get("authorization")
                    .and_then(|h| h.to_str().ok())
                    .unwrap_or("")
                    .into(),
            );
            if body.is_empty() {
                return ([("content-type", "text/event-stream")], "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}\n\n").into_response();
            }
            let request: Value = serde_json::from_slice(&body).unwrap();
            if request.get("id").is_none() {
                return StatusCode::ACCEPTED.into_response();
            }
            let result = if request["method"] == "initialize" {
                json!({"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"proxy-fixture","version":"1"}})
            } else {
                json!({"tools":[]})
            };
            Json(json!({"jsonrpc":"2.0","id":request["id"],"result":result})).into_response()
        }
        async fn server(auth: Arc<Mutex<Vec<String>>>) -> (String, tokio::task::JoinHandle<()>) {
            let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", socket.local_addr().unwrap());
            let app = Router::new().fallback(peer).with_state(auth);
            let task = tokio::spawn(async move {
                axum::serve(socket, app).await.unwrap();
            });
            (url, task)
        }
        let proxied = Arc::new(Mutex::new(Vec::new()));
        let local = Arc::new(Mutex::new(Vec::new()));
        let (proxy, proxy_task) = server(proxied.clone()).await;
        let (target, target_task) = server(local.clone()).await;
        for key in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
            "http_proxy",
            "https_proxy",
            "all_proxy",
        ] {
            std::env::set_var(key, &proxy);
        }
        for key in ["NO_PROXY", "no_proxy"] {
            std::env::remove_var(key);
        }

        // Confirm the original default-client behavior in this same harness.
        reqwest::Client::new()
            .get(format!("{target}/mcp"))
            .header("authorization", "Bearer loopback-test")
            .send()
            .await
            .unwrap();
        assert_eq!(*proxied.lock().unwrap(), vec!["Bearer loopback-test"]);
        proxied.lock().unwrap().clear();

        let config = |url: String, token: &str| McpServerConfig {
            id: "proxy-fixture".into(),
            name: "Proxy fixture".into(),
            sources: vec![super::super::McpConfigSource::Plugin],
            command: url,
            args: vec![],
            env: Default::default(),
            disabled: false,
            transport: super::super::McpTransport::Http,
            raw: json!({"headers":{"Authorization":format!("Bearer {token}")}}),
        };
        let client = HttpMcpClient::connect(&config(format!("{target}/mcp"), "loopback-test"))
            .await
            .unwrap();
        client.request("tools/list", json!({})).await.unwrap();
        let mut notifications = client.notifications();
        tokio::select! {
            _ = client.listen_notifications() => panic!("notification listener ended"),
            event = notifications.recv() => assert_eq!(event.unwrap().method, "notifications/tools/list_changed"),
            _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => panic!("GET did not reconcile"),
        }

        assert!(
            proxied.lock().unwrap().is_empty(),
            "loopback credentials reached proxy"
        );
        assert_eq!(local.lock().unwrap().len(), 4);
        assert!(local
            .lock()
            .unwrap()
            .iter()
            .all(|auth| auth == "Bearer loopback-test"));

        HttpMcpClient::connect(&config(
            "http://remote-mcp.invalid/mcp".into(),
            "remote-test",
        ))
        .await
        .unwrap();
        assert_eq!(
            *proxied.lock().unwrap(),
            vec!["Bearer remote-test", "Bearer remote-test"]
        );
        proxy_task.abort();
        target_task.abort();
    }
}
