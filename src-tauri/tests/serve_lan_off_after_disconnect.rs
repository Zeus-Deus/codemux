//! A retained headless unit must not reopen a LAN listener after `connect off`.
//! The unit runs a bare `serve`, so this exercises restart from a managed
//! deployment that had both relay and LAN access on before disconnect.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;

use codemux_lib::web_remote;
use codemux_lib::web_remote::serve::{serve_startup, ServeOptions};

#[test]
fn bare_serve_keeps_managed_lan_off_after_relay_disconnect() {
    let tmp = tempfile::tempdir().expect("tempdir");
    for (key, sub) in [
        ("HOME", ""),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_RUNTIME_DIR", "run"),
    ] {
        let dir = tmp.path().join(sub);
        std::fs::create_dir_all(&dir).expect("create isolated state dir");
        if key == "XDG_RUNTIME_DIR" {
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
                .expect("make isolated runtime dir private");
        }
        std::env::set_var(key, &dir);
    }
    std::env::set_var("CODEMUX_DISABLE_PTY_DAEMON", "1");
    std::env::set_var("CODEMUX_API_URL", "http://127.0.0.1:1");
    std::env::remove_var("DISPLAY");
    std::env::remove_var("WAYLAND_DISPLAY");

    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local addr")
        .port();
    {
        let db = codemux_lib::database::init_database().expect("open isolated db");
        web_remote::update_config_headless(&db, |cfg| {
            cfg.enabled = true;
            cfg.lan_enabled = true;
            cfg.relay_mode_enabled = true;
            cfg.port = port;
            cfg.bind_scope = web_remote::BIND_SCOPE_LOOPBACK.to_string();
            assert!(
                !cfg.lan_explicitly_disabled,
                "the managed service initially had its LAN listener on"
            );
            Ok(())
        })
        .expect("seed relay and LAN config");
        web_remote::update_config_headless(&db, |cfg| {
            web_remote::connect::apply_managed_disconnect_request(cfg);
            Ok(())
        })
        .expect("persist managed disconnect");
        let cfg = web_remote::load_config_from_db(&db);
        assert!(!cfg.relay_mode_enabled && !cfg.lan_enabled);
        assert!(cfg.lan_explicitly_disabled);
    }

    let app = codemux_lib::build_headless_app().expect("headless serve app should build");
    let handle = app.handle().clone();
    let result = tauri::async_runtime::block_on(serve_startup(
        &handle,
        &ServeOptions {
            scope: None,
            port: None,
            relay: false,
        },
    ))
    .expect("bare serve startup succeeds");
    assert!(result.status.enabled);
    assert!(!result.status.lan_enabled && !result.status.running);
    assert!(!result.status.relay_mode_enabled && !result.status.relay_running);
    assert_eq!(result.status.bind_scope, web_remote::BIND_SCOPE_LOOPBACK);
    assert!(result.status.bind_error.is_none());
    let probe = std::net::TcpListener::bind(("127.0.0.1", port))
        .expect("bare serve must not reopen the LAN port");
    let error = tauri::async_runtime::block_on(serve_startup(
        &handle,
        &ServeOptions {
            scope: None,
            port: Some(port),
            relay: false,
        },
    ))
    .expect_err("an explicitly requested, occupied LAN port must fail");
    assert!(error.contains("could not enable"), "{error}");
    drop(probe);
    let db = codemux_lib::database::init_database().expect("reopen isolated db");
    let cfg = web_remote::load_config_from_db(&db);
    assert!(!cfg.lan_enabled && cfg.lan_explicitly_disabled);
    let result = tauri::async_runtime::block_on(serve_startup(
        &handle,
        &ServeOptions {
            scope: None,
            port: None,
            relay: false,
        },
    ))
    .expect("a failed explicit bind does not undo LAN-off");
    assert!(!result.status.lan_enabled && !result.status.running);
    drop(app);
}
