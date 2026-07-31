//! `codemux serve` must boot from the config the user persisted — and must not
//! overwrite the parts of it that it never touched.
//!
//! This is the regression test for the headless-boot defect: `codemux serve`
//! builds its app with `Builder::build()`, which does NOT run Tauri's `setup`
//! hook (Tauri runs it from the event loop, which serve never enters). With
//! `setup` skipped, `web_remote::restore_on_boot` never hydrated the persisted
//! config, so serve bound the default port instead of the configured one, then
//! persisted the un-hydrated in-memory default back over the settings row —
//! flipping a `relay_mode_enabled: true` written by `codemux connect` to
//! `false`, which meant iroh and device registration never started. The
//! control server was likewise never spawned, so `codemux remote pair` and
//! `codemux connect status` could not reach a running serve at all.
//!
//! What this asserts, against a settings row seeded exactly the way
//! `codemux connect` seeds it (headless, with nothing running):
//!   1. the server binds the PERSISTED port + scope (real HTTP `/api/health`),
//!   2. `relay_mode_enabled` stays `true` in both the live status and the
//!      settings row, and the relay runtime actually started (iroh node id
//!      present, registration loop ran),
//!   3. the control server is listening, and a real `web_remote_pair` control
//!      round-trip — the `codemux remote pair` path — succeeds,
//!   4. a subsequent plain enable (no relay argument) still does not modify
//!      the persisted `relay_mode_enabled` — the `enable_core`-level guard.
//!
//! CRITICAL ISOLATION (identical to `serve_headless_dispatch.rs`): a real
//! Codemux instance may be running on this machine, so `isolate_env` repoints
//! every state-dir resolver at a throwaway tempdir BEFORE the app boots. The
//! control socket, DB, and iroh identity key all land inside it. `scope` is
//! pinned to `loopback` so the test never opens a port off-box, and
//! `CODEMUX_API_URL` points at a dead address so no registration POST could
//! reach the real API even if the machine were signed in (it is not — the
//! isolated DB has no account, so registration stops before any network call).
//!
//! Unix-only: `tauri::test` needs WebView2Loader.dll at process startup on
//! Windows (same gate as the other headless-serve tests).

#![cfg(unix)]

use std::time::Duration;

use codemux_lib::control::{send_control_request, ControlRequest};
use codemux_lib::web_remote::serve::{serve_startup, ServeOptions};
use codemux_lib::web_remote;

/// Point every state-dir resolver at `tmp` and defuse boot side effects a
/// headless test should not perform. Must run BEFORE the DB is opened.
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
    std::env::remove_var("DISPLAY");
    std::env::remove_var("WAYLAND_DISPLAY");
}

/// Ask the OS for a currently-free loopback TCP port.
fn free_loopback_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local addr")
        .port()
}

/// Poll `/api/health` until the listener accepts (bind→serve spawns a task).
fn wait_healthy(base: &str) {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("http client");
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
fn serve_honours_persisted_config_and_never_clobbers_relay_mode() {
    let tmp = tempfile::tempdir().expect("tempdir");
    isolate_env(tmp.path());

    let port = free_loopback_port();

    // ── Seed the settings row the way `codemux connect` does: headless
    //    load-modify-save through the same persistence the app reads on boot,
    //    with NOTHING running. ──
    let seeded = {
        let db = codemux_lib::database::init_database().expect("open isolated db");
        let cfg = web_remote::update_config_headless(&db, |cfg| {
            cfg.enabled = true;
            cfg.port = port;
            cfg.bind_scope = web_remote::BIND_SCOPE_LOOPBACK.to_string();
            cfg.relay_mode_enabled = true;
            Ok(())
        })
        .expect("seed persisted web-remote config");
        assert!(cfg.relay_mode_enabled && cfg.enabled);
        cfg
    };

    // ── Boot the headless backend the `codemux serve` daemon boots. ──
    let app = codemux_lib::build_headless_app().expect("headless serve app should build");
    let handle = app.handle().clone();

    // ── Run the production startup path with NO flags: everything must come
    //    from the persisted row. ──
    let result = tauri::async_runtime::block_on(serve_startup(
        &handle,
        &ServeOptions {
            scope: None,
            port: None,
            relay: false,
        },
    ))
    .expect("serve startup should bind the persisted configuration");

    // ── 1. Bound the persisted port + scope, not the defaults. ──
    assert_eq!(
        result.status.port, seeded.port,
        "serve must bind the persisted port, not the {} default",
        web_remote::DEFAULT_PORT
    );
    assert_ne!(
        result.status.port,
        web_remote::DEFAULT_PORT,
        "test would be vacuous if the persisted port were the default"
    );
    assert_eq!(result.status.bind_scope, web_remote::BIND_SCOPE_LOOPBACK);
    assert!(result.status.running, "the listener should be up");
    wait_healthy(&format!("http://127.0.0.1:{port}"));

    // ── 2. Relay mode survived, in the live status AND on disk. ──
    assert!(
        result.status.relay_mode_enabled,
        "a persisted relay_mode_enabled must survive serve startup"
    );
    let persisted_after = {
        let db = codemux_lib::database::init_database().expect("reopen isolated db");
        web_remote::load_config_from_db(&db)
    };
    assert!(
        persisted_after.relay_mode_enabled,
        "serve must not persist relay_mode_enabled=false over the seeded row"
    );
    assert_eq!(persisted_after.port, port, "persisted port unchanged");
    assert_eq!(
        persisted_after.bind_scope,
        web_remote::BIND_SCOPE_LOOPBACK,
        "persisted scope unchanged"
    );
    assert!(persisted_after.enabled);

    // ── 2b. The relay runtime actually started. `iroh_node_id` proves the
    //    endpoint/identity came up; the registration status proves the
    //    registration loop ran a pass. No network is touched: the isolated DB
    //    has no account session, so `register_once` records "signed out" and
    //    returns before any HTTP request. ──
    assert!(
        result.status.iroh_node_id.is_some(),
        "relay mode should have brought the iroh identity up"
    );
    let registration = {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let status = web_remote::web_remote_registration_status(handle.clone());
            if status.last_error.is_some() || status.registered {
                break status;
            }
            if std::time::Instant::now() > deadline {
                panic!("device registration never ran a pass under serve");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    assert!(
        !registration.registered,
        "an isolated, signed-out test must never actually register"
    );
    assert_eq!(
        registration.last_error.as_deref(),
        Some("desktop signed out"),
        "registration should have reached the sign-in check (i.e. the loop is live)"
    );

    // ── 3. The control server is listening: `codemux remote pair` and
    //    `codemux connect status` from another SSH session work against a
    //    running serve, and the GUI's boot-time guard sees this instance. ──
    assert!(
        codemux_lib::control::control_server_is_running(),
        "serve must hold the machine's control endpoint (mutual exclusion + CLI)"
    );
    let pair = tauri::async_runtime::block_on(send_control_request(ControlRequest {
        command: "web_remote_pair".to_string(),
        params: serde_json::json!({ "name": "Phone" }),
    }))
    .expect("control round-trip should reach the running serve instance");
    assert!(pair.ok, "web_remote_pair failed: {:?}", pair.error);
    let token = pair.data.as_ref().and_then(|d| d["token"].as_str());
    assert!(
        token.is_some_and(|t| !t.is_empty()),
        "the control pair round-trip should mint a token: {:?}",
        pair.data
    );

    // ── 4. `enable_core`-level guard: an enable that carries no relay argument
    //    must not rewrite the persisted relay flag. (Before the fix this is the
    //    exact write that flipped it to false.) ──
    let status = tauri::async_runtime::block_on(web_remote::web_remote_enable(handle.clone()))
        .expect("a plain enable on an already-running server should succeed");
    assert!(status.relay_mode_enabled, "live relay flag untouched by enable");
    let persisted_after_enable = {
        let db = codemux_lib::database::init_database().expect("reopen isolated db");
        web_remote::load_config_from_db(&db)
    };
    assert!(
        persisted_after_enable.relay_mode_enabled,
        "enable must never persist a relay_mode_enabled it was not asked to change"
    );
    assert_eq!(persisted_after_enable.port, port);

    // Unbind before the process exits so no listener outlives the test.
    let _ = web_remote::control_disable(&handle);
    drop(app);
}
