//! The outside-assistant connector in the headless `codemux serve` runtime:
//! a persisted-enabled connector is bound by the app's setup on its own
//! loopback port, serves OAuth metadata and personal-token MCP with web remote
//! off, and runs independently of the web-remote listener.
//!
//! Isolation: every state dir is repointed at a throwaway tempdir before the
//! app boots, and both listeners use free loopback ports.
//!
//! Unix-only: `tauri::test` needs WebView2Loader.dll at process startup on
//! Windows (same gate as the other headless-serve tests).

#![cfg(unix)]

use std::time::Duration;

use codemux_lib::agent_control::ControlAccess;
use codemux_lib::mcp_connector;
use codemux_lib::web_remote::{self, serve::{serve_startup, ServeOptions}};
use serde_json::{json, Value};

/// Point every state-dir resolver at `tmp` and defuse boot side effects a
/// headless test should not perform. Must run BEFORE the app boots.
fn isolate_env(tmp: &std::path::Path) {
    for (key, sub) in [
        ("HOME", ""),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_RUNTIME_DIR", "run"),
    ] {
        let dir = if sub.is_empty() { tmp.to_path_buf() } else { tmp.join(sub) };
        std::fs::create_dir_all(&dir).expect("create isolated state dir");
        std::env::set_var(key, &dir);
    }
    std::env::set_var("CODEMUX_DISABLE_PTY_DAEMON", "1");
    std::env::set_var("CODEMUX_API_URL", "http://127.0.0.1:1");
    std::env::remove_var("DISPLAY");
    std::env::remove_var("WAYLAND_DISPLAY");
}

fn free_loopback_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local addr")
        .port()
}

fn reachable(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(200),
    )
    .is_ok()
}

struct Mcp {
    client: reqwest::blocking::Client,
    url: String,
    token: String,
}
impl Mcp {
    fn post(&self, body: Value) -> reqwest::blocking::Response {
        self.client
            .post(&self.url)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("mcp-protocol-version", "2025-06-18")
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .expect("connector request")
    }
    fn call(&self, method: &str, params: Value) -> Value {
        let response = self.post(json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}));
        assert_eq!(response.status(), 200, "{method}");
        response.json().expect("JSON-RPC reply")
    }
}

#[test]
fn connector_runs_without_web_remote_and_serves_oauth_and_tokens() {
    let tmp = tempfile::tempdir().expect("tempdir");
    isolate_env(tmp.path());
    let port = free_loopback_port();
    {
        let db = codemux_lib::database::init_database().expect("open isolated db");
        db.mcp_connector_set_config(&format!(r#"{{"enabled":true,"port":{port}}}"#))
            .expect("seed connector config");
    }

    let app = codemux_lib::build_headless_app().expect("headless serve app should build");
    let handle = app.handle().clone();

    // Setup bound the connector; web remote stayed off.
    assert!(!web_remote::web_remote_status(handle.clone()).running);
    let status = mcp_connector::agent_connector_status(handle.clone()).expect("status");
    assert!(status.listener_running, "{:?}", status.bind_error);
    assert_eq!(status.port, port);
    let base = format!("http://127.0.0.1:{port}");
    assert_eq!(status.local_url.as_deref(), Some(format!("{base}/mcp").as_str()));

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("http client");
    let metadata: Value = client
        .get(format!("{base}/.well-known/oauth-authorization-server"))
        .send()
        .expect("metadata request")
        .json()
        .expect("metadata JSON");
    assert_eq!(metadata["issuer"], base.as_str());
    assert!(
        metadata["grant_types_supported"]
            .as_array()
            .is_some_and(|types| types.contains(&json!("refresh_token"))),
        "{metadata}"
    );

    let issued = mcp_connector::agent_connector_issue(
        handle.clone(),
        "ci-bot".into(),
        ControlAccess::Supervised,
        Some(1),
    )
    .expect("issue a personal token");
    let mcp = Mcp { client: client.clone(), url: format!("{base}/mcp"), token: issued.token.clone() };
    let init = mcp.call(
        "initialize",
        json!({"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"ci-bot","version":"1"}}),
    );
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18", "{init}");
    let tools = mcp.call("tools/list", json!({}));
    let names: Vec<_> = tools["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert!(names.contains(&"thread_launch"), "{names:?}");
    let listed = mcp.call("tools/call", json!({"name":"workspace_list","arguments":{}}));
    assert_eq!(listed["result"]["isError"], false, "{listed}");
    assert!(listed["result"]["structuredContent"]["workspaces"].is_array(), "{listed}");

    mcp_connector::agent_connector_revoke(handle.clone(), issued.client_id.clone()).expect("revoke");
    let refused = mcp.post(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
    assert_eq!(refused.status(), 401);

    // Web remote binds beside it, and each switch leaves the other alone.
    let web_port = free_loopback_port();
    let started = tauri::async_runtime::block_on(serve_startup(
        &handle,
        &ServeOptions { scope: Some("loopback".into()), port: Some(web_port), relay: false },
    ))
    .expect("web remote binds beside the connector");
    assert!(started.status.running);
    assert_ne!(web_port, port);
    assert!(mcp_connector::agent_connector_status(handle.clone()).expect("status").listener_running);
    web_remote::control_disable(&handle).expect("disable web remote");
    assert!(reachable(port), "turning web remote off leaves the connector listening");
    let metadata = client
        .get(format!("{base}/.well-known/oauth-authorization-server"))
        .send()
        .expect("metadata after web remote stopped");
    assert_eq!(metadata.status(), 200);

    tauri::async_runtime::block_on(serve_startup(
        &handle,
        &ServeOptions { scope: Some("loopback".into()), port: Some(web_port), relay: false },
    ))
    .expect("web remote binds again");
    let off = mcp_connector::agent_connector_set_config(handle.clone(), false, None, None)
        .expect("disable the connector");
    assert!(!off.listener_running);
    assert!(web_remote::web_remote_status(handle.clone()).running, "turning the connector off leaves web remote serving");
    let health = client
        .get(format!("http://127.0.0.1:{web_port}/api/health"))
        .send()
        .expect("web remote health");
    assert!(health.status().is_success());
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    while reachable(port) {
        assert!(std::time::Instant::now() < deadline, "the connector still accepts after disable");
        std::thread::sleep(Duration::from_millis(20));
    }

    let _ = web_remote::control_disable(&handle);
}
