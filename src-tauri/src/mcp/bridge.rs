//! `codemux mcp-bridge`: a stdio MCP server that forwards every message to
//! Codemux's loopback gateway, for ACP agents that take stdio servers only.
//!
//! The gateway URL and the chat's token arrive in the environment (see
//! [`super::gateway::McpGatewayConnection::acp_stdio_bridge`]). Each stdin line
//! is POSTed as is; JSON and SSE replies go to stdout one message per line.
//! After `initialize`, the bridge also opens the server's GET event stream when
//! it offers one, so server notifications such as `tools/list_changed` reach
//! the agent.

use std::sync::{Arc, Mutex};

use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use super::gateway::{BRIDGE_TOKEN_ENV, BRIDGE_URL_ENV};

const SESSION_HEADER: &str = "mcp-session-id";

#[derive(Clone)]
struct Upstream {
    client: reqwest::Client,
    url: String,
    authorization: HeaderValue,
    /// `Mcp-Session-Id` the server assigned at `initialize`, if any.
    session: Arc<Mutex<Option<HeaderValue>>>,
    protocol: Arc<Mutex<Option<HeaderValue>>>,
    out: mpsc::UnboundedSender<String>,
    /// In-flight requests and the event-stream listener. Each holds a sender
    /// to stdout, so they are cancelled when the agent hangs up; otherwise the
    /// writer, and with it the process, would outlive the agent.
    tasks: Arc<Mutex<Tasks>>,
    /// Runs in `initialize`'s task just before it starts the listener, so
    /// tests can force shutdown into that window.
    #[cfg(test)]
    before_listen: Arc<Mutex<Option<Box<dyn FnMut() + Send>>>>,
}

#[derive(Default)]
struct Tasks {
    running: JoinSet<()>,
    /// Set at shutdown. A task started afterwards, such as the listener an
    /// in-flight `initialize` starts once its reply is in, is never admitted.
    closed: bool,
}

pub async fn run() -> Result<(), String> {
    let var = |name: &str| {
        std::env::var(name)
            .ok()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("{name} is not set; Codemux starts this bridge for its agents"))
    };
    let url = var(BRIDGE_URL_ENV)?;
    let authorization = HeaderValue::from_str(&format!("Bearer {}", var(BRIDGE_TOKEN_ENV)?))
        .map_err(|_| format!("{BRIDGE_TOKEN_ENV} is not a valid header value"))?;
    let (out, mut lines_out) = mpsc::unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(line) = lines_out.recv().await {
            if stdout.write_all(line.as_bytes()).await.is_err()
                || stdout.write_all(b"\n").await.is_err()
                || stdout.flush().await.is_err()
            {
                break;
            }
        }
    });
    let upstream = Upstream {
        // The gateway is on loopback; an ambient HTTP(S)_PROXY must not carry
        // its requests, or the chat's bearer token, anywhere else.
        client: reqwest::Client::builder()
            .no_proxy()
            .build()
            .map_err(|error| format!("HTTP client: {error}"))?,
        url,
        authorization,
        session: Arc::default(),
        protocol: Arc::default(),
        out,
        tasks: Arc::default(),
        #[cfg(test)]
        before_listen: Arc::default(),
    };

    let mut stdin = BufReader::new(tokio::io::stdin()).lines();
    while let Some(line) = stdin
        .next_line()
        .await
        .map_err(|error| format!("stdin read error: {error}"))?
    {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(&line) {
            // Requests run concurrently, so a slow tool call doesn't hold up
            // the agent's other requests.
            Ok(message) => upstream.spawn(upstream.clone().forward(message)),
            Err(error) => upstream.send(&rpc_error(Value::Null, -32700, format!("parse error: {error}"))),
        }
    }
    // The agent closed stdin: it is done with this server.
    upstream.shutdown();
    drop(upstream);
    let _ = writer.await;
    Ok(())
}

impl Upstream {
    /// Cancel every request and the listener, and admit no new task.
    fn shutdown(&self) {
        let mut tasks = self.tasks.lock().unwrap_or_else(|p| p.into_inner());
        tasks.closed = true;
        tasks.running.abort_all();
    }

    fn spawn(&self, task: impl std::future::Future<Output = ()> + Send + 'static) {
        let mut tasks = self.tasks.lock().unwrap_or_else(|p| p.into_inner());
        if tasks.closed {
            return;
        }
        while tasks.running.try_join_next().is_some() {}
        tasks.running.spawn(task);
    }

    fn send(&self, message: &Value) {
        if let Some(version) = message
            .pointer("/result/protocolVersion")
            .and_then(Value::as_str)
            .and_then(|v| HeaderValue::from_str(v).ok())
        {
            *self.protocol.lock().unwrap() = Some(version);
        }
        let _ = self.out.send(message.to_string());
    }

    fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, self.authorization.clone());
        if let Some(session) = self.session.lock().unwrap_or_else(|p| p.into_inner()).clone() {
            headers.insert(SESSION_HEADER, session);
        }
        if let Some(protocol) = self.protocol.lock().unwrap().clone() {
            headers.insert("mcp-protocol-version", protocol);
        }
        headers
    }

    async fn forward(self, message: Value) {
        let id = message.get("id").cloned();
        let is_initialize = message.get("method").and_then(Value::as_str) == Some("initialize");
        let response = self
            .client
            .post(&self.url)
            .headers(self.headers())
            .header(ACCEPT, "application/json, text/event-stream")
            .json(&message)
            .send()
            .await;
        let response = match response {
            Ok(response) if response.status().is_success() => response,
            Ok(response) => {
                return self.fail(id, format!("Codemux MCP gateway returned HTTP {}", response.status()));
            }
            Err(error) => return self.fail(id, format!("Codemux MCP gateway unreachable: {error}")),
        };
        if let Some(session) = response.headers().get(SESSION_HEADER) {
            *self.session.lock().unwrap_or_else(|p| p.into_inner()) = Some(session.clone());
        }
        if is_sse(response.headers()) {
            self.relay_events(response).await;
        } else {
            match response.bytes().await {
                Ok(body) if body.is_empty() => {}
                Ok(body) => match serde_json::from_slice::<Value>(&body) {
                    Ok(reply) => self.send(&reply),
                    Err(error) => self.fail(id, format!("Codemux MCP gateway sent invalid JSON: {error}")),
                },
                Err(error) => self.fail(id, format!("Codemux MCP gateway reply failed: {error}")),
            }
        }
        if is_initialize {
            #[cfg(test)]
            if let Some(mut hook) = self.before_listen.lock().unwrap().take() {
                hook();
            }
            self.spawn(self.clone().listen());
        }
    }

    /// Answer a request the gateway couldn't; notifications get no reply.
    fn fail(&self, id: Option<Value>, message: String) {
        if let Some(id) = id {
            self.send(&rpc_error(id, -32603, message));
        }
    }

    /// Relay the server's GET event stream. A server without one answers 405
    /// and the bridge simply doesn't listen.
    async fn listen(self) {
        let response = self
            .client
            .get(&self.url)
            .headers(self.headers())
            .header(ACCEPT, "text/event-stream")
            .send()
            .await;
        if let Ok(response) = response {
            if response.status().is_success() && is_sse(response.headers()) {
                self.relay_events(response).await;
            }
        }
    }

    async fn relay_events(&self, response: reqwest::Response) {
        let mut parser = SseParser::default();
        let mut body = response.bytes_stream();
        while let Some(Ok(chunk)) = body.next().await {
            for data in parser.push(&chunk) {
                self.relay_data(&data);
            }
        }
        if let Some(data) = parser.finish() {
            self.relay_data(&data);
        }
    }

    fn relay_data(&self, data: &str) {
        // Re-serialise so a multi-line `data:` payload stays one stdout line.
        if let Ok(message) = serde_json::from_str::<Value>(data) {
            self.send(&message);
        }
    }
}

fn is_sse(headers: &HeaderMap) -> bool {
    headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/event-stream"))
}

fn rpc_error(id: Value, code: i64, message: String) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// Incremental `text/event-stream` parser yielding each event's `data`.
#[derive(Default)]
struct SseParser {
    pending: Vec<u8>,
    data: Option<String>,
}

impl SseParser {
    fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.pending.extend_from_slice(chunk);
        let mut events = Vec::new();
        while let Some(end) = self.pending.iter().position(|&b| b == b'\n') {
            let raw: Vec<u8> = self.pending.drain(..=end).collect();
            let line = String::from_utf8_lossy(&raw);
            let line = line.trim_end_matches(['\n', '\r']);
            if line.is_empty() {
                events.extend(self.data.take());
            } else if let Some(value) = line.strip_prefix("data:") {
                let value = value.strip_prefix(' ').unwrap_or(value);
                match &mut self.data {
                    Some(data) => {
                        data.push('\n');
                        data.push_str(value);
                    }
                    None => self.data = Some(value.to_string()),
                }
            }
        }
        events
    }

    fn finish(&mut self) -> Option<String> {
        self.data.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn relays_requests_to_the_gateway_with_the_chat_token() {
        let registry = crate::mcp::registry::McpRegistry::new();
        registry
            .insert_running_server_for_test("fake", vec![crate::mcp::McpConfigSource::CodemuxUser])
            .await;
        let connection = registry
            .gateway_chat_connection("thread", Default::default())
            .await
            .unwrap();
        let (out, mut replies) = mpsc::unbounded_channel();
        let upstream = |token: &str| Upstream {
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
            url: connection.url.clone(),
            authorization: HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
            session: Arc::default(),
            protocol: Arc::default(),
            out: out.clone(),
            tasks: Arc::default(),
            before_listen: Arc::default(),
        };

        let chat = upstream(&connection.bearer_token);
        chat.clone()
            .forward(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}))
            .await;
        let reply: Value = serde_json::from_str(&replies.recv().await.unwrap()).unwrap();
        assert_eq!(reply["result"]["serverInfo"]["name"], "codemux");
        chat.clone()
            .forward(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await;
        chat.forward(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})).await;
        let reply: Value = serde_json::from_str(&replies.recv().await.unwrap()).unwrap();
        assert_eq!(reply["id"], 2, "a notification gets no reply: {reply}");
        assert_eq!(reply["result"]["tools"][0]["name"], "mcp__fake__tool");

        upstream("wrong")
            .forward(json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list"}))
            .await;
        let reply: Value = serde_json::from_str(&replies.recv().await.unwrap()).unwrap();
        assert_eq!(reply["id"], 3);
        assert!(reply["error"]["message"].as_str().unwrap().contains("401"), "{reply}");
    }

    /// The agent hangs up while `initialize`'s reply is already in and its
    /// task is about to start the event-stream listener. Shutdown must still
    /// release every stdout sender, or the writer, and the process, never end.
    #[tokio::test(flavor = "multi_thread", worker_threads = 3)]
    async fn shutdown_racing_initialize_lets_the_writer_finish() {
        let registry = crate::mcp::registry::McpRegistry::new();
        let connection = registry
            .gateway_chat_connection("thread", Default::default())
            .await
            .unwrap();
        let (out, mut replies) = mpsc::unbounded_channel();
        let upstream = Upstream {
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
            url: connection.url.clone(),
            authorization: HeaderValue::from_str(&format!("Bearer {}", connection.bearer_token)).unwrap(),
            session: Arc::default(),
            protocol: Arc::default(),
            out,
            tasks: Arc::default(),
            before_listen: Arc::default(),
        };
        let (reached_tx, reached_rx) = std::sync::mpsc::channel::<()>();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        *upstream.before_listen.lock().unwrap() = Some(Box::new(move || {
            reached_tx.send(()).unwrap();
            release_rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        }));
        upstream.spawn(
            upstream
                .clone()
                .forward(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}})),
        );
        tokio::task::spawn_blocking(move || reached_rx.recv_timeout(std::time::Duration::from_secs(10)))
            .await
            .unwrap()
            .expect("initialize reached the listener spawn");

        upstream.shutdown();
        release_tx.send(()).unwrap();
        drop(upstream);
        let drained = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while replies.recv().await.is_some() {}
        })
        .await;
        assert!(drained.is_ok(), "a listener started after shutdown kept stdout open");
    }

    #[test]
    fn sse_events_survive_split_chunks_and_multiline_data() {
        let mut parser = SseParser::default();
        assert!(parser.push(b"event: message\ndata: {\"a\":").is_empty());
        assert_eq!(parser.push(b"1}\n\n: comment\ndata: x\r\ndata: y\n\n"), vec!["{\"a\":1}", "x\ny"]);
        // A multi-byte character split across chunks is reassembled.
        let text = "data: é\n\n".as_bytes();
        assert!(parser.push(&text[..7]).is_empty());
        assert_eq!(parser.push(&text[7..]), vec!["é"]);
        parser.push(b"data: tail");
        assert_eq!(parser.push(b"\n"), Vec::<String>::new());
        assert_eq!(parser.finish().as_deref(), Some("tail"));
    }
}

/// One-shot fallback for agents that accept MCP injection but hide its tools.
/// This is an ordinary authenticated gateway client, never a control-socket client.
pub async fn call(
    tool: Option<String>,
    arguments: Option<String>,
    connection_file: Option<std::path::PathBuf>,
    list: bool,
) -> Result<(), String> {
    let connection: super::gateway::McpGatewayConnection = match connection_file {
        Some(path) => {
            let file = std::fs::File::open(path)
                .map_err(|e| format!("Chat MCP connection is unavailable: {e}"))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if file
                    .metadata()
                    .map_err(|e| e.to_string())?
                    .permissions()
                    .mode()
                    & 0o077
                    != 0
                {
                    return Err("Chat MCP connection must be private to its owner".into());
                }
            }
            serde_json::from_reader(file).map_err(|_| "Invalid chat MCP connection".to_string())?
        }
        None => super::gateway::McpGatewayConnection {
            url: std::env::var(BRIDGE_URL_ENV)
                .map_err(|_| "No authenticated chat MCP connection".to_string())?,
            bearer_token: std::env::var(BRIDGE_TOKEN_ENV)
                .map_err(|_| "No authenticated chat MCP connection".to_string())?,
        },
    };
    let url = reqwest::Url::parse(&connection.url).map_err(|_| "Invalid chat MCP endpoint")?;
    if url.scheme() != "http"
        || !matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
        || url.path() != "/mcp"
    {
        return Err("Chat MCP endpoint must be the local gateway".into());
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .map_err(|e| e.to_string())?;
    let request_id = uuid::Uuid::new_v4().to_string();
    let message = if list {
        json!({"jsonrpc":"2.0","id":request_id,"method":"tools/list","params":{}})
    } else {
        let name = tool.ok_or("Pass a tool name, or --list")?;
        let name = name.strip_prefix("mcp__codemux__").unwrap_or(&name);
        let args: Value = serde_json::from_str(arguments.as_deref().unwrap_or("{}"))
            .map_err(|e| format!("Invalid arguments JSON: {e}"))?;
        json!({"jsonrpc":"2.0","id":request_id,"method":"tools/call","params":{"name":name,"arguments":args}})
    };
    let reply = client
        .post(url)
        .bearer_auth(connection.bearer_token)
        .json(&message)
        .send()
        .await
        .map_err(|_| "Chat MCP gateway is unavailable")?;
    if !reply.status().is_success() {
        return Err(format!("Chat MCP gateway returned HTTP {}", reply.status()));
    }
    let reply: Value = reply
        .json()
        .await
        .map_err(|_| "Invalid chat MCP gateway reply")?;
    println!(
        "{}",
        serde_json::to_string(&reply).map_err(|e| e.to_string())?
    );
    if reply.get("error").is_some() || reply["result"]["isError"] == true {
        return Err("Chat MCP call was refused; inspect the JSON result".into());
    }
    Ok(())
}
