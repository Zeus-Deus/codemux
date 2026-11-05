//! The outside-assistant connector in the headless `codemux serve` runtime:
//! a persisted-enabled connector is bound by the app's setup on its own
//! loopback port, serves OAuth metadata and personal-token MCP with web remote
//! off, and runs independently of the web-remote listener.
//!
//! A real dual-era MCP client also drives it when one is installed: the MCP
//! Python SDK (2.x) that Hermes Agent ships. `CODEMUX_TEST_MCP_PYTHON` names
//! its interpreter; otherwise Hermes's own virtualenv is used. Without either
//! that leg is skipped.
//!
//! Isolation: every state dir is repointed at a throwaway tempdir before the
//! app boots, and both listeners use free loopback ports.
//!
//! Unix-only: `tauri::test` needs WebView2Loader.dll at process startup on
//! Windows (same gate as the other headless-serve tests).

#![cfg(unix)]

use std::path::PathBuf;
use std::process::{Command, Stdio};
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

/// The SDK's interpreter, found before the test isolates `HOME`.
fn dual_era_python() -> Option<PathBuf> {
    let python = std::env::var_os("CODEMUX_TEST_MCP_PYTHON").map(PathBuf::from).or_else(|| {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".hermes/hermes-agent/.venv/bin/python"))
    })?;
    let probe = Command::new(&python)
        .args(["-I", "-c", "from mcp.client import Client; from mcp_types.version import MODERN_PROTOCOL_VERSIONS; import httpx2"])
        .env_clear()
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    probe.is_ok_and(|status| status.success()).then_some(python)
}

/// Connects once per connect mode given after the URL and token and reports
/// what each session negotiated, listed and got back.
const DUAL_ERA_CLIENT: &str = r#"
import asyncio, json, sys
import httpx2
import mcp_types
from mcp.client import Client
from mcp.client.streamable_http import streamable_http_client

async def session(url, token, mode):
    http = httpx2.AsyncClient(headers={"Authorization": "Bearer " + token}, timeout=30)
    info = mcp_types.Implementation(name="sdk-" + mode, version="2.0.0")
    async with http, Client(streamable_http_client(url, http_client=http), mode=mode, cache=None, client_info=info) as client:
        listed = await client.list_tools()
        listed_workspaces = await client.call_tool("workspace_list", {})
        refused = await client.call_tool("thread_stop", {"workspace_id": "w", "thread_id": "t", "client_request_id": "r"})
        return {
            "version": client.protocol_version,
            "server": client.server_info.name if client.server_info else None,
            "tools": sorted(tool.name for tool in listed.tools),
            "workspaces_error": bool(listed_workspaces.is_error),
            "workspaces": listed_workspaces.structured_content,
            "refused": refused.structured_content,
        }

async def main():
    url, token = sys.argv[1], sys.argv[2]
    print(json.dumps({mode: await session(url, token, mode) for mode in sys.argv[3:]}))

asyncio.run(main())
"#;

/// The SDK connects over Streamable HTTP with a personal token, once probing
/// (it goes stateless, 2026-07-28) and once with `initialize`. Both list the
/// grant's catalog and call a tool. With the stateless era switched off, the
/// probing client falls back to `initialize`.
fn a_real_dual_era_sdk_uses_the_connector(python: &std::path::Path, home: &std::path::Path, url: &str, token: &str) {
    let output = Command::new(python)
        .args(["-I", "-c", DUAL_ERA_CLIENT, url, token, "auto", "legacy"])
        .env_clear()
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .output()
        .expect("run the SDK client");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "the SDK client failed:\n{stderr}");
    let report: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("{error}: {}\n{stderr}", String::from_utf8_lossy(&output.stdout)));

    let probing = if codemux_lib::mcp::protocol::MODERN_ERA_ENABLED { "2026-07-28" } else { "2025-11-25" };
    assert_eq!(report["auto"]["version"], probing, "a dual-era client goes stateless when it can: {report}");
    assert_eq!(report["legacy"]["version"], "2025-11-25", "{report}");
    for mode in ["auto", "legacy"] {
        let session = &report[mode];
        assert_eq!(session["server"], "CodeMux", "{mode}: {session}");
        assert_eq!(session["tools"], report["auto"]["tools"], "{mode}: one catalog per grant");
        assert!(session["tools"].as_array().unwrap().contains(&json!("thread_launch")), "{mode}: {session}");
        assert_eq!(session["workspaces_error"], false, "{mode}: {session}");
        assert!(session["workspaces"]["workspaces"].is_array(), "{mode}: {session}");
        // Tool failures arrive as tool results in both eras.
        assert!(session["refused"]["error"]["code"].is_string(), "{mode}: {session}");
    }
}

#[test]
fn connector_runs_without_web_remote_and_serves_oauth_and_tokens() {
    let python = dual_era_python();
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
    match &python {
        Some(python) => a_real_dual_era_sdk_uses_the_connector(python, tmp.path(), &mcp.url, &issued.token),
        None => eprintln!("skipped the SDK leg: no MCP Python SDK with the stateless era (set CODEMUX_TEST_MCP_PYTHON)"),
    }

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
