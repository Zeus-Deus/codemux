//! A bare `codemux serve` must keep an explicit relay-only setup relay-only.
//!
//! A user who switched "On my network" off (remote access on, relay mode on,
//! LAN listener off) has chosen that nothing listens on their network. The
//! `codemux connect` systemd unit runs a bare `serve` on every boot, so if
//! serve re-opened the listener, that user would be exposed on every
//! interface again after a restart.
//!
//! What this drives, through the production `codemux serve` startup path with
//! no flags against a seeded relay-only settings row:
//!   1. startup succeeds with the listener off (`!running`, no `bind_error`)
//!      and the relay transport up,
//!   2. nothing is bound on the configured port,
//!   3. `lan_enabled` is still `false` on disk,
//!   4. pairing refuses (a pairing link would point at nothing),
//!   5. an explicit `--port` is a request for the listener and opens it.
//!
//! Isolation matches the other headless-serve tests: every state-dir resolver
//! points into a tempdir before the app boots, scope is `loopback`, and
//! `CODEMUX_API_URL` points at a dead address — never the real API (the
//! isolated DB is signed out, so registration never makes a request anyway).
//!
//! Unix-only: `tauri::test` needs WebView2Loader.dll at process startup on
//! Windows (same gate as the other headless-serve tests).

#![cfg(unix)]

use codemux_lib::web_remote;
use codemux_lib::web_remote::serve::{serve_startup, ServeOptions};

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

fn free_loopback_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local addr")
        .port()
}

fn persisted() -> web_remote::WebRemoteConfig {
    let db = codemux_lib::database::init_database().expect("reopen isolated db");
    web_remote::load_config_from_db(&db)
}

#[test]
fn bare_serve_keeps_a_relay_only_setup_off_the_network() {
    let tmp = tempfile::tempdir().expect("tempdir");
    isolate_env(tmp.path());
    let port = free_loopback_port();

    {
        let db = codemux_lib::database::init_database().expect("open isolated db");
        web_remote::update_config_headless(&db, |cfg| {
            cfg.enabled = true;
            cfg.lan_enabled = false;
            cfg.relay_mode_enabled = true;
            cfg.port = port;
            cfg.bind_scope = web_remote::BIND_SCOPE_LOOPBACK.to_string();
            Ok(())
        })
        .expect("seed relay-only config");
    }

    let app = codemux_lib::build_headless_app().expect("headless serve app should build");
    let handle = app.handle().clone();

    // ── 1. A bare serve comes up relay-only. ──
    let result = tauri::async_runtime::block_on(serve_startup(
        &handle,
        &ServeOptions {
            scope: None,
            port: None,
            relay: false,
        },
    ))
    .expect("relay-only serve startup succeeds");
    assert!(result.status.enabled, "remote access stays on");
    assert!(!result.status.lan_enabled, "the listener stays switched off");
    assert!(!result.status.running, "nothing listens on the network");
    assert_eq!(result.status.bind_error, None, "an unwanted listener has no error");
    assert!(result.status.relay_mode_enabled);
    assert!(result.status.relay_running, "the relay transport is up");

    // ── 2. The configured port is free. ──
    let probe = std::net::TcpListener::bind(("127.0.0.1", port))
        .expect("the relay-only serve must not hold the LAN port");
    drop(probe);

    // ── 3. The switch was not flipped behind the user's back. ──
    let cfg = persisted();
    assert!(cfg.enabled && cfg.relay_mode_enabled);
    assert!(!cfg.lan_enabled, "lan_enabled is still false on disk");

    // ── 4. No pairing link without a listener. ──
    let err = web_remote::control_pair(&handle, None).expect_err("pairing must refuse");
    assert!(err.contains("On my network"), "names the switch: {err}");

    // ── 5. An explicit `--port` asks for the listener. ──
    let result = tauri::async_runtime::block_on(serve_startup(
        &handle,
        &ServeOptions {
            scope: None,
            port: Some(port),
            relay: false,
        },
    ))
    .expect("serve --port opens the listener");
    assert!(result.status.lan_enabled && result.status.running);
    assert!(result.status.relay_running, "the relay is unaffected");
    assert!(persisted().lan_enabled);

    web_remote::control_disable(&handle).expect("disable");
    drop(app);
}
