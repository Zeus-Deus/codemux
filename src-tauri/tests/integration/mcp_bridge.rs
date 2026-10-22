//! `codemux mcp-bridge` as a real child process against a live gateway.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use codemux_lib::mcp::gateway::{GatewayCaller, McpGatewayConnection, BRIDGE_TOKEN_ENV, BRIDGE_URL_ENV};
use codemux_lib::mcp::registry::McpRegistry;
use serde_json::{json, Value};

struct Bridge {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<String>,
}

impl Bridge {
    fn spawn(connection: &McpGatewayConnection, hostile_proxy: bool) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_codemux"));
        command
            .arg("mcp-bridge")
            .env(BRIDGE_URL_ENV, &connection.url)
            .env(BRIDGE_TOKEN_ENV, &connection.bearer_token)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        // A proxy that refuses every connection, with loopback not exempted:
        // the bridge must still reach the loopback gateway directly.
        for name in ["HTTP_PROXY", "http_proxy", "HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"] {
            if hostile_proxy {
                command.env(name, "http://127.0.0.1:9");
            } else {
                command.env_remove(name);
            }
        }
        command.env_remove("NO_PROXY").env_remove("no_proxy");
        let mut child = command.spawn().expect("spawn codemux mcp-bridge");
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Self { stdin: child.stdin.take(), child, lines }
    }

    fn send(&mut self, message: Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
    }

    fn reply(&self, id: i64) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let line = self.lines.recv_timeout(left).expect("bridge reply");
            let value: Value = serde_json::from_str(&line).unwrap();
            if value["id"] == id {
                return value;
            }
        }
    }

    fn exits_within(&mut self, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline {
            if self.child.try_wait().unwrap().is_some() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

async fn chat_connection(registry: &McpRegistry) -> McpGatewayConnection {
    registry
        .gateway_chat_connection("bridge-test", GatewayCaller::default())
        .await
        .unwrap()
}

fn initialize() -> Value {
    json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}
    }})
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bridge_reaches_the_loopback_gateway_despite_proxy_settings() {
    let registry = McpRegistry::new();
    let connection = chat_connection(&registry).await;
    let mut bridge = Bridge::spawn(&connection, true);
    bridge.send(initialize());
    let reply = tokio::task::spawn_blocking(move || {
        let reply = bridge.reply(1);
        (bridge, reply)
    })
    .await
    .unwrap()
    .1;
    assert_eq!(reply["result"]["serverInfo"]["name"], "codemux", "{reply}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bridge_exits_when_the_agent_closes_stdin() {
    let registry = McpRegistry::new();
    let connection = chat_connection(&registry).await;
    let url = connection.url.clone();
    let token = connection.bearer_token.clone();
    let exited = tokio::task::spawn_blocking(move || {
        let mut bridge = Bridge::spawn(&connection, false);
        bridge.send(initialize());
        assert!(bridge.reply(1)["result"].is_object());
        bridge.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        bridge.send(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}));
        assert!(bridge.reply(2)["result"]["tools"].is_array());
        // Give the GET event stream time to open, then hang up like an agent.
        std::thread::sleep(Duration::from_millis(500));
        drop(bridge.stdin.take());
        bridge.exits_within(Duration::from_secs(5))
    })
    .await
    .unwrap();
    assert!(exited, "bridge kept running after stdin closed");

    let health = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(&url)
        .bearer_auth(&token)
        .json(&json!({"jsonrpc": "2.0", "id": 9, "method": "ping"}))
        .send()
        .await
        .unwrap();
    assert_eq!(health.status(), 200, "gateway stays healthy");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn all_provider_one_shot_cli_uses_gateway_auth_and_never_the_control_socket() {
    let registry = McpRegistry::new();
    let connection = registry
        .gateway_chat_connection("one-shot", GatewayCaller::default())
        .await
        .unwrap();
    let call = |token: String| {
        let url = connection.url.clone();
        tokio::task::spawn_blocking(move || {
            Command::new(env!("CARGO_BIN_EXE_codemux"))
                .args(["mcp-call", "--list"])
                .env(BRIDGE_URL_ENV, url)
                .env(BRIDGE_TOKEN_ENV, token)
                .env("CODEMUX_SOCKET", "/missing/control-socket")
                .output()
                .unwrap()
        })
    };
    let output = call(connection.bearer_token.clone()).await.unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(reply["result"]["tools"].is_array());
    let output = call("wrong-token".into()).await.unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("401"));
}
