//! Relay mode must come up — and register the device — even when the LAN
//! listener cannot bind (issue #404).
//!
//! The iroh relay transport needs no port on this machine, yet it used to be
//! started only inside the LAN listener's success path. With the listener's
//! bind failing (a taken port, a `tailscale` scope before the tailnet is up),
//! relay mode silently never started: no identity key, no `node_id`, no
//! `POST /api/devices`, and the hosted client showed "No devices yet" forever
//! while every surface reported success.
//!
//! What this drives, through the production `codemux serve` startup path
//! against a settings row seeded `enabled + relay on` with the port already
//! occupied:
//!   1. startup succeeds relay-only: the listener is down with a recorded
//!      `bind_error`, the relay endpoint is up,
//!   2. the device registers with the (mocked) control plane under the live
//!      relay `node_id`,
//!   3. pairing refuses with the bind reason instead of minting a dead link,
//!      and re-enabling with another taken `--port` stays relay-only (the bind
//!      failure is the status's `bind_error`, not a failed enable),
//!   4. once the port frees up, the background retry binds the listener
//!      without a restart and the error clears,
//!   5. with relay mode OFF, the same bind failure still rolls the enable back
//!      (nothing would be running, so the switch must not read "on").
//!
//! Isolation matches the other headless-serve tests: every state-dir resolver
//! points into a tempdir before the app boots, scope is `loopback`, and
//! `CODEMUX_API_URL` points at a local mock — never the real API.
//!
//! Unix-only: `tauri::test` needs WebView2Loader.dll at process startup on
//! Windows (same gate as the other headless-serve tests).

#![cfg(unix)]

use std::time::{Duration, Instant};

use codemux_lib::web_remote;
use codemux_lib::web_remote::serve::{serve_startup, ServeOptions};

fn isolate_env(tmp: &std::path::Path, api_url: &str) {
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
    std::env::set_var("CODEMUX_API_URL", api_url);
    std::env::remove_var("DISPLAY");
    std::env::remove_var("WAYLAND_DISPLAY");
}

fn wait_for<T>(what: &str, timeout: Duration, mut probe: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(v) = probe() {
            return v;
        }
        if Instant::now() > deadline {
            panic!("timed out waiting for {what}");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn healthy(port: u16) -> bool {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .expect("http client")
        .get(format!("http://127.0.0.1:{port}/api/health"))
        .send()
        .is_ok_and(|r| r.status().is_success())
}

#[test]
fn relay_registers_while_the_lan_listener_cannot_bind() {
    // A local stand-in for the account control plane.
    let mut api = mockito::Server::new();
    let devices = api
        .mock("POST", "/api/devices")
        .match_header("authorization", "Bearer test-bearer")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body("{}")
        .expect_at_least(1)
        .create();

    let tmp = tempfile::tempdir().expect("tempdir");
    isolate_env(tmp.path(), &api.url());

    // Occupy the port the listener is configured for.
    let blocker = std::net::TcpListener::bind("127.0.0.1:0").expect("bind blocker");
    let port = blocker.local_addr().expect("blocker addr").port();

    {
        let db = codemux_lib::database::init_database().expect("open isolated db");
        web_remote::update_config_headless(&db, |cfg| {
            cfg.enabled = true;
            cfg.port = port;
            cfg.bind_scope = web_remote::BIND_SCOPE_LOOPBACK.to_string();
            cfg.relay_mode_enabled = true;
            Ok(())
        })
        .expect("seed config");
        let user = codemux_lib::auth::AuthUser {
            id: "user-1".to_string(),
            email: "desktop@example.com".to_string(),
            name: None,
            image: None,
        };
        codemux_lib::auth::save_auth(&db, "test-bearer", "2099-01-01T00:00:00Z", Some(&user))
            .expect("seed signed-in desktop");
    }

    let app = codemux_lib::build_headless_app().expect("headless serve app should build");
    let handle = app.handle().clone();

    // ── 1. Startup survives the failed bind because relay mode is on. ──
    let result = tauri::async_runtime::block_on(serve_startup(
        &handle,
        &ServeOptions {
            scope: None,
            port: None,
            relay: false,
        },
    ))
    .expect("relay mode must keep serve up when only the LAN bind fails");
    assert!(result.status.enabled, "remote access stays on");
    assert!(
        !result.status.running,
        "the listener cannot be up — its port is taken"
    );
    let bind_error = result
        .status
        .bind_error
        .clone()
        .expect("a failed bind must be reported, not left as 'starting'");
    assert!(!bind_error.is_empty());
    assert!(
        result.status.relay_running,
        "the relay endpoint came up without the listener"
    );
    let node_id = result
        .status
        .iroh_node_id
        .clone()
        .expect("relay identity exists");

    // ── 2. The device registers under the live node id. ──
    let registration = wait_for("device registration", Duration::from_secs(20), || {
        let s = web_remote::web_remote_registration_status(handle.clone());
        s.registered.then_some(s)
    });
    assert_eq!(registration.node_id.as_deref(), Some(node_id.as_str()));
    devices.assert();
    let status = web_remote::web_remote_status(handle.clone());
    assert!(status.device_registered);
    assert_eq!(status.registration_error, None);

    // ── 3. No dead pairing links while nothing listens. ──
    let err = web_remote::control_pair(&handle, None).expect_err("pairing must refuse");
    assert!(err.contains("isn't listening"), "names the cause: {err}");

    // ── 3b. Re-enabling at another taken port stays relay-only, not an error
    //        (`codemux serve --relay --port <taken>` / `codemux connect --port`). ──
    let blocker2 = std::net::TcpListener::bind("127.0.0.1:0").expect("bind second blocker");
    let port2 = blocker2.local_addr().expect("second blocker addr").port();
    let enabled = tauri::async_runtime::block_on(web_remote::control_enable(
        &handle,
        None,
        Some(port2),
    ))
    .expect("with relay on, a taken --port must not fail the enable");
    assert_eq!(enabled.status.port, port2, "the new port is kept");
    assert!(!enabled.status.running, "the new port is taken too");
    assert!(
        enabled.status.bind_error.is_some(),
        "the bind failure is reported as the status's bind_error"
    );
    assert!(enabled.status.relay_running, "relay stays up");
    let port = port2;
    drop(blocker);
    let blocker = blocker2;

    // ── 4. The port frees up → the retry loop binds without a restart. ──
    drop(blocker);
    wait_for(
        "the listener retry to bind",
        Duration::from_secs(30),
        || {
            web_remote::web_remote_status(handle.clone())
                .running
                .then_some(())
        },
    );
    assert!(healthy(port), "the recovered listener serves /api/health");
    let status = web_remote::web_remote_status(handle.clone());
    assert_eq!(status.bind_error, None, "the error clears once bound");
    assert!(status.relay_running, "relay was never interrupted");

    // ── 5. Relay OFF: the same failure rolls the enable back. ──
    web_remote::control_disable(&handle).expect("disable");
    let status = web_remote::web_remote_status(handle.clone());
    assert!(!status.relay_running && !status.running && status.bind_error.is_none());
    tauri::async_runtime::block_on(web_remote::web_remote_set_config(
        handle.clone(),
        None,
        None,
        None,
        None,
        None,
        Some(false),
    ))
    .expect("relay off");
    let blocker = std::net::TcpListener::bind(("127.0.0.1", port)).expect("re-occupy port");
    let err = tauri::async_runtime::block_on(web_remote::web_remote_enable(handle.clone()))
        .expect_err("with relay off, a failed bind must fail the enable");
    assert!(!err.is_empty());
    let status = web_remote::web_remote_status(handle.clone());
    assert!(!status.enabled, "rolled back — nothing would be running");
    assert_eq!(status.bind_error, None);
    let persisted = {
        let db = codemux_lib::database::init_database().expect("reopen isolated db");
        web_remote::load_config_from_db(&db)
    };
    assert!(!persisted.enabled, "the rollback is persisted");

    drop(blocker);
    drop(app);
}
