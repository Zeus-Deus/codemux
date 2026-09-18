//! Headless serve-mode web-remote round-trip (issue #176, Phase 3).
//!
//! Proves the issue's core acceptance end-to-end with ZERO GUI: boot the same
//! `codemux_lib::build_headless_app()` the `codemux serve` daemon runs, bind
//! the web-remote server on loopback, then — over real HTTP/WebSocket against
//! 127.0.0.1 — pair, connect, and drive a command:
//!
//!   1. enable the server on `loopback` on an ephemeral free port (via the
//!      shared `control_enable` path the CLI/GUI use),
//!   2. mint a pairing token via `control_pair`,
//!   3. `POST /api/pair`      → session token (auto-approved: default config),
//!   4. `POST /api/ws-ticket` → one-time WS ticket,
//!   5. `GET  /ws?ticket=…`   → upgrade, send an invoke frame, and assert the
//!      id-matched `{"t":"ok","id":1,…}` response arrives.
//!
//! This exercises the web-remote WebSocket protocol contract end to end.
//!
//! CRITICAL ISOLATION (identical to `serve_headless_dispatch.rs`): a real
//! Codemux instance may be running on this machine, so `isolate_env` repoints
//! every state-dir resolver at a throwaway tempdir and defuses the boot side
//! effects BEFORE the app boots — the test never touches the real user's DB,
//! auth, scrollback, control socket, or web-remote port.
//!
//! Unix-only: `tauri::test` needs WebView2Loader.dll at process startup on
//! Windows (same gate as the other `*_ipc.rs` / dispatch tests).

#![cfg(unix)]

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tauri::Manager;
use tokio_tungstenite::tungstenite::Message;

/// Point every state-dir resolver at `tmp` and defuse boot side effects a
/// headless test should not perform. Must run BEFORE `build_headless_app()`.
/// Mirrors `serve_headless_dispatch.rs::isolate_env` verbatim.
fn isolate_env(tmp: &std::path::Path) {
    for (key, sub) in [
        ("HOME", ""),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_RUNTIME_DIR", "run"),
    ] {
        let dir = if sub.is_empty() {
            tmp.to_path_buf()
        } else {
            tmp.join(sub)
        };
        std::fs::create_dir_all(&dir).expect("create isolated state dir");
        std::env::set_var(key, &dir);
    }
    std::env::set_var("CODEMUX_DISABLE_PTY_DAEMON", "1");
    std::env::set_var("CODEMUX_API_URL", "http://127.0.0.1:1");
    std::env::set_var("SHELL", "/bin/true");
    std::env::remove_var("DISPLAY");
    std::env::remove_var("WAYLAND_DISPLAY");
}

/// Ask the OS for a currently-free loopback TCP port. There is an inherent
/// race between releasing this and the server re-binding it, so the caller
/// wraps the enable in a retry loop.
fn free_loopback_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local addr")
        .port()
}

/// Poll `/api/health` until the listener accepts (the bind→serve step spawns a
/// task, so the socket is ready a beat after `control_enable` returns).
fn wait_healthy(client: &reqwest::blocking::Client, base: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(resp) = client.get(format!("{base}/api/health")).send() {
            if resp.status().is_success() {
                return;
            }
        }
        if std::time::Instant::now() > deadline {
            panic!("web-remote server at {base} never became healthy");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn serve_web_remote_pair_connect_and_invoke_roundtrip() {
    let tmp = tempfile::tempdir().expect("tempdir");
    isolate_env(tmp.path());

    // Boot the full backend headless — same entry point `codemux serve` uses.
    let app = codemux_lib::build_headless_app().expect("headless serve app should build");
    let handle = app.handle().clone();

    // ── 1. Enable on loopback on a free ephemeral port (bind-probe retry). ──
    let mut bound_port = 0u16;
    for _ in 0..12 {
        let port = free_loopback_port();
        match tauri::async_runtime::block_on(codemux_lib::web_remote::control_enable(
            &handle,
            Some("loopback".to_string()),
            Some(port),
        )) {
            Ok(result) => {
                assert!(
                    result.status.running,
                    "server should report running after a successful enable"
                );
                assert_eq!(result.status.bind_scope, "loopback");
                bound_port = result.status.port;
                break;
            }
            // Port raced away between probe and bind — try another.
            Err(_) => continue,
        }
    }
    assert!(
        bound_port != 0,
        "should have bound the web-remote server on a free loopback port"
    );

    // ── 2. Mint a one-time pairing token via the shared control path. ──
    let pairing = codemux_lib::web_remote::control_pair(&handle, None)
        .expect("control_pair should mint a token when the server is enabled");
    let token = pairing.token;
    assert!(!token.is_empty(), "pairing token should be non-empty");

    let base = format!("http://127.0.0.1:{bound_port}");
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("http client");
    wait_healthy(&client, &base);

    // ── 3. POST /api/pair → session (auto-approved; default require_approval=false). ──
    let pair_resp = client
        .post(format!("{base}/api/pair"))
        .json(&json!({ "token": token }))
        .send()
        .expect("pair request");
    assert!(
        pair_resp.status().is_success(),
        "POST /api/pair should succeed, got {}",
        pair_resp.status()
    );
    let pair_body: Value = pair_resp.json().expect("pair response json");
    assert_eq!(
        pair_body["approved"],
        json!(true),
        "default config auto-approves the paired device"
    );
    let session_token = pair_body["session_token"]
        .as_str()
        .expect("pair response carries a session_token")
        .to_string();

    // ── 4. POST /api/ws-ticket (Bearer) → one-time WS ticket. ──
    let ticket_resp = client
        .post(format!("{base}/api/ws-ticket"))
        .header("Authorization", format!("Bearer {session_token}"))
        .send()
        .expect("ws-ticket request");
    assert!(
        ticket_resp.status().is_success(),
        "POST /api/ws-ticket should succeed, got {}",
        ticket_resp.status()
    );
    let ticket_body: Value = ticket_resp.json().expect("ws-ticket response json");
    let ticket = ticket_body["ticket"]
        .as_str()
        .expect("ws-ticket response carries a ticket")
        .to_string();

    let second_ticket: Value = client
        .post(format!("{base}/api/ws-ticket"))
        .header("Authorization", format!("Bearer {session_token}"))
        .send()
        .unwrap()
        .json()
        .unwrap();
    let second_url = format!(
        "ws://127.0.0.1:{bound_port}/ws?ticket={}",
        second_ticket["ticket"].as_str().unwrap()
    );
    let state = handle.state::<codemux_lib::state::AppStateStore>();
    let phone = state
        .create_empty_workspace_at_path(tmp.path().join("phone"))
        .0;
    let tablet = state
        .create_empty_workspace_at_path(tmp.path().join("tablet"))
        .0;
    let desktop = state
        .create_empty_workspace_at_path(tmp.path().join("desktop"))
        .0;
    let cwd = tmp.path().display().to_string();
    let observability = handle.state::<codemux_lib::observability::ObservabilityStore>();
    let mut flags = observability.feature_flags();
    flags.enable_agent_chat = true;
    observability.set_feature_flags(flags);

    // ── 5. GET /ws?ticket=… → upgrade, invoke get_app_state, expect ok frame. ──
    let ws_url = format!("ws://127.0.0.1:{bound_port}/ws?ticket={ticket}");
    tauri::async_runtime::block_on(async move {
        let (mut socket, _resp) = tokio_tungstenite::connect_async(&ws_url)
            .await
            .expect("WS upgrade should succeed with a valid ticket");

        // C→S invoke frame per the WS protocol contract.
        socket
            .send(Message::Text(
                json!({ "t": "invoke", "id": 1, "cmd": "get_app_state", "args": {} })
                    .to_string()
                    .into(),
            ))
            .await
            .expect("send invoke frame");

        // Read frames until the id-matched ok arrives (the server may interleave
        // event/other frames; responses can arrive out of order by design).
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let frame = tokio::time::timeout(remaining, socket.next())
                .await
                .expect("an ok frame should arrive before the timeout")
                .expect("WS stream should stay open")
                .expect("WS frame should read cleanly");

            if let Message::Text(text) = frame {
                let v: Value = serde_json::from_str(text.as_str()).expect("valid JSON frame");
                if v["t"] == json!("err") && v["id"] == json!(1) {
                    panic!("get_app_state returned an err frame: {v}");
                }
                if v["t"] == json!("ok") && v["id"] == json!(1) {
                    assert!(
                        v.get("data").and_then(|d| d.get("workspaces")).is_some(),
                        "ok frame should carry the app-state snapshot: {v}"
                    );
                    break;
                }
            }
        }

        for (index, cmd) in [
            "bootstrap_session",
            "get_feature_flags",
            "db_get_all_settings",
            "get_presets",
            "get_home_dir",
        ]
        .into_iter()
        .enumerate()
        {
            let response = remote_invoke(&mut socket, 100 + index as u64, cmd, json!({})).await;
            assert_eq!(response["t"], "ok", "remote bootstrap {cmd}: {response}");
        }

        // Push uses the authenticated socket's session, never a client-supplied owner.
        let config = remote_invoke(&mut socket, 200, "web_push_config", json!({})).await;
        assert_eq!(config["t"], "ok", "{config}");
        let public_key = config["data"]["public_key"].as_str().expect("VAPID public key");
        assert_eq!(public_key.len(), 87);
        let invalid = remote_invoke(&mut socket, 201, "web_push_subscribe", json!({
            "subscription": {"endpoint":"https://127.0.0.1/private", "keys":{"p256dh":public_key,"auth":"AAAAAAAAAAAAAAAAAAAAAA"}},
            "categories":{"attention":true,"complete":true,"failure":true}, "host":"test-desktop"
        })).await;
        assert_eq!(invalid["t"], "err", "private endpoints must be rejected");
        let subscribed = remote_invoke(&mut socket, 202, "web_push_subscribe", json!({
            "subscription": {"endpoint":"https://web.push.apple.com/test-not-sent", "keys":{"p256dh":public_key,"auth":"AAAAAAAAAAAAAAAAAAAAAA"}},
            "categories":{"attention":true,"complete":false,"failure":true}, "host":"test-desktop"
        })).await;
        assert_eq!(subscribed["t"], "ok", "{subscribed}");
        let status = remote_invoke(&mut socket, 203, "web_push_status", json!({})).await;
        assert_eq!(status["data"]["categories"]["complete"], false);
        assert!(status["data"].get("private_key").is_none());
        assert_eq!(remote_invoke(&mut socket, 204, "web_push_unsubscribe", json!({})).await["t"], "ok");
        assert_eq!(remote_invoke(&mut socket, 205, "web_push_status", json!({})).await["data"], json!({}));

        let (mut second, _) = tokio_tungstenite::connect_async(&second_url).await.unwrap();
        assert_eq!(
            remote_invoke(
                &mut socket,
                2,
                "touch_workspace",
                json!({"workspaceId": phone, "__remoteClientId": "phone-client"})
            )
            .await["t"],
            "ok"
        );
        assert_eq!(
            remote_invoke(
                &mut second,
                2,
                "touch_workspace",
                json!({"workspaceId": tablet, "__remoteClientId": "tablet-client"})
            )
            .await["t"],
            "ok"
        );
        let snapshot = remote_invoke(&mut socket, 3, "get_app_state", json!({})).await;
        assert_eq!(snapshot["data"]["active_workspace_id"], desktop);

        // Workspace, pane and binding arrive in a single state mutation. The
        // bridge must override a client's attempt to select the desktop too.
        let created = remote_invoke(
            &mut socket,
            4,
            "materialize_chat_workspace",
            json!({
                "cwd": cwd, "skipSetup": true, "select": true,
                "initialChat": {"provider": "claude", "thread_id": "remote-first-turn"}
            }),
        )
        .await;
        assert_eq!(created["t"], "ok", "{created}");
        let workspace_id = created["data"]["workspace_id"].as_str().unwrap();
        let snapshot = remote_invoke(&mut second, 3, "get_app_state", json!({})).await;
        assert_eq!(snapshot["data"]["active_workspace_id"], desktop);
        let workspace = snapshot["data"]["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|w| w["workspace_id"] == workspace_id)
            .unwrap();
        assert_eq!(
            workspace["surfaces"][0]["root"]["thread_id"],
            "remote-first-turn"
        );
        let pane_id = workspace["surfaces"][0]["root"]["pane_id"]
            .as_str()
            .unwrap();
        assert_eq!(
            remote_invoke(
                &mut socket,
                5,
                "activate_pane",
                json!({"paneId": pane_id, "select": true})
            )
            .await["t"],
            "ok"
        );
        assert_eq!(
            handle
                .state::<codemux_lib::state::AppStateStore>()
                .active_workspace_id(),
            desktop
        );

        for (index, cmd) in [
            "activate_workspace",
            "dev_agent_chat_spawn_test_pane",
            "quit_app",
            "remove_worktree",
            "future_command",
        ]
        .into_iter()
        .enumerate()
        {
            let denied = remote_invoke(
                &mut socket,
                10 + index as u64,
                cmd,
                json!({"workspaceId": phone}),
            )
            .await;
            assert_eq!(denied["t"], "err", "{cmd}: {denied}");
            assert!(denied["error"].as_str().unwrap().contains("not allowed"));
        }
        let denied = remote_invoke(
            &mut socket,
            20,
            "close_workspace_with_worktree",
            json!({"workspaceId": workspace_id, "deleteWorktree": true}),
        )
        .await;
        assert_eq!(denied["t"], "err");
        assert_eq!(
            handle
                .state::<codemux_lib::state::AppStateStore>()
                .active_workspace_id(),
            desktop
        );

        let archived = remote_invoke(&mut socket, 206, "archive_workspace", json!({"workspaceId":workspace_id})).await;
        assert_eq!(archived["t"], "ok", "{archived}");
        let restored = remote_invoke(&mut socket, 207, "unarchive_workspace", json!({"archiveId":archived["data"],"select":true})).await;
        assert_eq!(restored["t"], "ok", "{restored}");
        let workspace_id = restored["data"].as_str().unwrap();
        assert_eq!(handle.state::<codemux_lib::state::AppStateStore>().active_workspace_id(), desktop, "restoring from phone must not navigate the desktop");

        let closed = remote_invoke(
            &mut socket,
            22,
            "close_workspace",
            json!({"workspaceId": workspace_id, "forceDelete": false}),
        )
        .await;
        assert_eq!(closed["t"], "ok", "{closed}");
        assert_eq!(
            handle
                .state::<codemux_lib::state::AppStateStore>()
                .active_workspace_id(),
            desktop
        );

        // The desktop still owns the shared id, including subsequent switches.
        handle
            .state::<codemux_lib::state::AppStateStore>()
            .activate_workspace(&tablet);
        assert_eq!(
            remote_invoke(
                &mut socket,
                21,
                "touch_workspace",
                json!({"workspaceId": phone})
            )
            .await["t"],
            "ok"
        );
        let snapshot = remote_invoke(&mut second, 4, "get_app_state", json!({})).await;
        assert_eq!(snapshot["data"]["active_workspace_id"], tablet);
        socket.close(None).await.unwrap();
        second.close(None).await.unwrap();
    });

    // Keep the backend alive for the whole test.
    drop(app);
}

async fn remote_invoke(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    id: u64,
    cmd: &str,
    args: Value,
) -> Value {
    socket
        .send(Message::Text(
            json!({"t":"invoke", "id":id, "cmd":cmd, "args":args})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let frame = socket.next().await.unwrap().unwrap();
            if let Message::Text(text) = frame {
                let frame: Value = serde_json::from_str(&text).unwrap();
                if frame["id"] == id && (frame["t"] == "ok" || frame["t"] == "err") {
                    return frame;
                }
            }
        }
    })
    .await
    .expect("remote invoke response")
}
