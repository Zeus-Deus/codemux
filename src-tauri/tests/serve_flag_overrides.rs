//! Explicit `codemux serve` flags win over the persisted config.
//!
//! The other half of the precedence rule the sibling tests cover: honouring the
//! persisted row must not turn into ignoring the command line. `--scope`,
//! `--port`, and `--relay` are the operator overriding a configured box, so
//! each is applied AND persisted (a deliberate change is exactly what the
//! settings row is supposed to record).
//!
//! Isolation matches the sibling tests; the override target is `loopback`, so
//! nothing is exposed off-box.
//!
//! Unix-only: `tauri::test` needs WebView2Loader.dll at process startup on
//! Windows (same gate as the other headless-serve tests).

#![cfg(unix)]

use std::time::Duration;

use codemux_lib::web_remote::serve::{serve_startup, ServeOptions};
use codemux_lib::web_remote;

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
fn serve_flags_override_the_persisted_config() {
    let tmp = tempfile::tempdir().expect("tempdir");
    isolate_env(tmp.path());

    let persisted_port = free_loopback_port();
    let flag_port = free_loopback_port();
    assert_ne!(persisted_port, flag_port);

    // A box configured for all-interfaces, no relay.
    {
        let db = codemux_lib::database::init_database().expect("open isolated db");
        web_remote::update_config_headless(&db, |cfg| {
            cfg.enabled = true;
            cfg.port = persisted_port;
            cfg.bind_scope = web_remote::BIND_SCOPE_ALL.to_string();
            cfg.relay_mode_enabled = false;
            Ok(())
        })
        .expect("seed persisted web-remote config");
    }

    let app = codemux_lib::build_headless_app().expect("headless serve app should build");
    let handle = app.handle().clone();

    // `codemux serve --scope loopback --port <flag_port> --relay`
    let result = tauri::async_runtime::block_on(serve_startup(
        &handle,
        &ServeOptions {
            scope: Some(web_remote::BIND_SCOPE_LOOPBACK.to_string()),
            port: Some(flag_port),
            relay: true,
        },
    ))
    .expect("serve startup with explicit flags should bind");

    assert_eq!(result.status.port, flag_port, "--port wins");
    assert_eq!(
        result.status.bind_scope,
        web_remote::BIND_SCOPE_LOOPBACK,
        "--scope wins"
    );
    assert!(result.status.relay_mode_enabled, "--relay forces relay on");
    wait_healthy(&format!("http://127.0.0.1:{flag_port}"));

    // The persisted port was never bound — the override replaced it rather
    // than binding both.
    assert!(
        std::net::TcpListener::bind(("127.0.0.1", persisted_port)).is_ok(),
        "the overridden port should be free, not still bound by serve"
    );

    // A deliberate override is persisted, so the next boot restores it.
    let persisted = {
        let db = codemux_lib::database::init_database().expect("reopen isolated db");
        web_remote::load_config_from_db(&db)
    };
    assert_eq!(persisted.port, flag_port);
    assert_eq!(persisted.bind_scope, web_remote::BIND_SCOPE_LOOPBACK);
    assert!(persisted.relay_mode_enabled, "--relay is persisted, like the Settings switch");
    assert!(persisted.enabled);

    let _ = web_remote::control_disable(&handle);
    drop(app);
}
