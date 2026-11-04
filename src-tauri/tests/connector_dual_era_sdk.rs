//! A real dual-era MCP client against the connector: the MCP Python SDK (2.x)
//! that Hermes Agent ships connects over Streamable HTTP with a personal
//! token, once probing (it goes stateless, 2026-07-28) and once with
//! `initialize`. Both list the grant's catalog and call a tool.
//!
//! `CODEMUX_TEST_MCP_PYTHON` names the SDK's interpreter; otherwise Hermes's
//! own virtualenv is used when it is installed. Without either the test is
//! skipped.
//!
//! Isolation: every state dir is repointed at a throwaway tempdir before the
//! headless app boots, and the connector listens on a free loopback port.
//! Unix-only, like the other headless-serve tests.

#![cfg(unix)]

use std::path::PathBuf;
use std::process::{Command, Stdio};

use codemux_lib::agent_control::ControlAccess;
use codemux_lib::mcp_connector;
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
const CLIENT: &str = r#"
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

#[test]
fn a_real_dual_era_sdk_uses_the_connector_in_both_eras() {
    let Some(python) = dual_era_python() else {
        eprintln!("skipped: no MCP Python SDK with the stateless era (set CODEMUX_TEST_MCP_PYTHON)");
        return;
    };
    let tmp = tempfile::tempdir().expect("tempdir");
    isolate_env(tmp.path());
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    {
        let db = codemux_lib::database::init_database().expect("open isolated db");
        db.mcp_connector_set_config(&format!(r#"{{"enabled":true,"port":{port}}}"#))
            .expect("seed connector config");
    }
    let app = codemux_lib::build_headless_app().expect("headless serve app should build");
    let handle = app.handle().clone();
    let status = mcp_connector::agent_connector_status(handle.clone()).expect("status");
    assert!(status.listener_running, "{:?}", status.bind_error);
    let issued = mcp_connector::agent_connector_issue(handle.clone(), "sdk-bot".into(), ControlAccess::Supervised, Some(1))
        .expect("issue a personal token");

    let output = Command::new(&python)
        .args(["-I", "-c", CLIENT])
        .arg(format!("http://127.0.0.1:{port}/mcp"))
        .arg(&issued.token)
        .args(["auto", "legacy"])
        .env_clear()
        .env("HOME", tmp.path())
        .env("PATH", "/usr/bin:/bin")
        .output()
        .expect("run the SDK client");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "the SDK client failed:\n{stderr}");
    let report: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("{error}: {}\n{stderr}", String::from_utf8_lossy(&output.stdout)));

    assert_eq!(report["auto"]["version"], "2026-07-28", "a dual-era client goes stateless: {report}");
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

    mcp_connector::agent_connector_revoke(handle.clone(), issued.client_id).expect("revoke");
    let _ = mcp_connector::agent_connector_set_config(handle, false, None, None);
}
