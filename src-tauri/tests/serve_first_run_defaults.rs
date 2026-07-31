//! `codemux serve` on a machine with nothing configured: first-run defaults,
//! and a settings row that afterwards describes what is actually running.
//!
//! Companion to `serve_persisted_config.rs` (which covers the
//! already-configured box). The documented scope resolution
//! (docs/features/web-remote-access.md § "Headless server mode") is: an
//! explicit `--scope` wins; no flag on an already-enabled config keeps its
//! scope; no flag on first run defaults to `all` — the away-from-home SSH case,
//! where nothing else would be reachable. This asserts the third branch
//! end-to-end, and that the enable writes back a row matching the live server
//! (so the next boot restores the same thing).
//!
//! Isolation: as in the sibling tests, every state dir is repointed at a
//! throwaway tempdir before the app boots. `--port` is an ephemeral free port
//! and the server is disabled again at the end, so the all-interfaces bind
//! this branch performs by design is short-lived and never reuses the real
//! default port.
//!
//! Unix-only: `tauri::test` needs WebView2Loader.dll at process startup on
//! Windows (same gate as the other headless-serve tests).

#![cfg(unix)]

use std::time::Duration;

use codemux_lib::web_remote::serve::{serve_startup, ServeOptions};
use codemux_lib::web_remote;

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
fn serve_first_run_defaults_and_persists_what_it_bound() {
    let tmp = tempfile::tempdir().expect("tempdir");
    isolate_env(tmp.path());

    // Nothing configured: the settings row does not exist yet.
    {
        let db = codemux_lib::database::init_database().expect("open isolated db");
        let cfg = web_remote::load_config_from_db(&db);
        assert!(!cfg.enabled, "precondition: nothing persisted yet");
        assert_eq!(cfg.port, web_remote::DEFAULT_PORT);
        assert!(!cfg.relay_mode_enabled);
    }

    let app = codemux_lib::build_headless_app().expect("headless serve app should build");
    let handle = app.handle().clone();

    let port = free_loopback_port();
    let result = tauri::async_runtime::block_on(serve_startup(
        &handle,
        &ServeOptions {
            scope: None,
            port: Some(port),
            relay: false,
        },
    ))
    .expect("first-run serve startup should bind");

    // First-run scope default is `all`; the explicit `--port` is honoured.
    assert_eq!(
        result.status.bind_scope,
        web_remote::BIND_SCOPE_ALL,
        "first run with no --scope binds every interface (the SSH case)"
    );
    assert_eq!(result.status.port, port);
    assert!(result.status.running);
    assert!(
        !result.status.relay_mode_enabled,
        "relay stays off unless asked for — serve never turns it on implicitly"
    );
    wait_healthy(&format!("http://127.0.0.1:{port}"));

    // The persisted row now describes what is actually running, so the next
    // boot (`restore_on_boot`, or another `serve`) restores the same thing.
    let persisted = {
        let db = codemux_lib::database::init_database().expect("reopen isolated db");
        web_remote::load_config_from_db(&db)
    };
    assert!(persisted.enabled, "the enable is persisted");
    assert_eq!(persisted.port, port);
    assert_eq!(persisted.bind_scope, web_remote::BIND_SCOPE_ALL);
    assert!(
        !persisted.relay_mode_enabled,
        "no relay was requested, so none is persisted"
    );

    let _ = web_remote::control_disable(&handle);
    drop(app);
}
