//! Headless serve-mode boot + dispatch test (issue #176, steps 2.4–2.6).
//!
//! Proves the `codemux serve` runtime — `codemux_lib::build_headless_app()`,
//! the same `build_core_app` wiring the desktop GUI uses, booted on
//! `tauri::test::MockRuntime` under `AppMode::ServeHeadless` — comes up with
//! NO display attached and dispatches the real command surface.
//!
//! What it asserts:
//!   1. With `DISPLAY` / `WAYLAND_DISPLAY` unset, the app builds and the
//!      in-memory "main" webview resolves (MockRuntime's `create_window` is a
//!      pure stub, so no windowing system is needed).
//!   2. A real command (`get_app_state`) round-trips through the exact
//!      `WebviewWindow::on_message` dispatch path that
//!      `web_remote::dispatch::dispatch_invoke` drives in production, using
//!      the app's own production invoke key.
//!   3. The embedded-asset resolver is queryable, degrading gracefully in the
//!      dev/debug profile where the frontend is not embedded.
//!
//! CRITICAL ISOLATION: a real Codemux instance may be running on this machine
//! (this suite is itself launched from inside one). `isolate_env` repoints
//! every state-dir resolver (`dirs::config_dir` / `dirs::data_dir` /
//! `XDG_RUNTIME_DIR` for the control socket) at a throwaway tempdir BEFORE the
//! app boots, so the test never reads or writes the real user's DB, auth,
//! scrollback, presets, or control socket.
//!
//! Unix-only: `tauri::test` pulls in runtime imports that need
//! WebView2Loader.dll at process startup on Windows (same gate as the other
//! `*_ipc.rs` tests).

#![cfg(unix)]

use serde_json::{json, Value};
use tauri::ipc::{CallbackFn, InvokeResponseBody};
use tauri::test::get_ipc_response;
use tauri::webview::InvokeRequest;
use tauri::Manager;

/// Point every state-dir resolver at `tmp` and defuse the boot side effects a
/// headless test should not perform. Must run BEFORE `build_headless_app()`.
fn isolate_env(tmp: &std::path::Path) {
    // `dirs::config_dir()` (DB, presets), `dirs::data_dir()` (auth,
    // scrollback, observability, settings), and `XDG_RUNTIME_DIR` (the
    // control socket) all derive from these. Overriding HOME too covers the
    // `$HOME/.config` / `$HOME/.local/share` fallbacks when an XDG var is
    // absent.
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

    // The PTY-daemon warmup in setup spawns a detached daemon process; skip
    // it so a boot test never forks one.
    std::env::set_var("CODEMUX_DISABLE_PTY_DAEMON", "1");

    // Keep the background sync loops from reaching a real API endpoint.
    std::env::set_var("CODEMUX_API_URL", "http://127.0.0.1:1");

    // Assertion 1's precondition: no display available.
    std::env::remove_var("DISPLAY");
    std::env::remove_var("WAYLAND_DISPLAY");
}

#[test]
fn serve_headless_boots_and_dispatches_without_display() {
    let tmp = tempfile::tempdir().expect("tempdir");
    isolate_env(tmp.path());

    // ── Assertion 1: builds headless with no display; "main" webview exists ──
    let app = codemux_lib::build_headless_app()
        .expect("headless serve app should build without a display");
    let webview = app
        .get_webview_window("main")
        .expect("headless serve app should expose an in-memory \"main\" webview");

    // ── Assertion 2: a real read-only command round-trips through the same
    //    on_message dispatch path web_remote::dispatch::dispatch_invoke uses.
    //    build_headless_app() boots with Builder::new(), so the app carries a
    //    production random invoke key (not the test harness's fixed
    //    INVOKE_KEY) — read it back off the app for the request. ──
    let response = get_ipc_response(
        &webview,
        InvokeRequest {
            cmd: "get_app_state".into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            // Reuse the webview's own local origin so ACL resolves exactly as
            // a desktop-initiated invoke of the same command would.
            url: webview.url().expect("webview url"),
            body: json!({}).into(),
            headers: Default::default(),
            invoke_key: app.handle().invoke_key().to_string(),
        },
    )
    .expect("get_app_state should dispatch Ok in headless serve mode");

    let snapshot: Value = match response {
        InvokeResponseBody::Json(s) => serde_json::from_str(&s).expect("valid JSON response"),
        InvokeResponseBody::Raw(bytes) => panic!("expected JSON response, got raw bytes: {bytes:?}"),
    };
    // The AppStateSnapshot always carries a `workspaces` array — empty on a
    // fresh tempdir-isolated boot. Its presence proves the command deserialized
    // its State, ran, and serialized a real result back through the invoke path.
    assert!(
        snapshot.get("workspaces").is_some(),
        "get_app_state snapshot should carry a workspaces field, got: {snapshot}"
    );

    // ── Assertion 3: the embedded-asset resolver is queryable. In the
    //    dev/debug profile (cargo test) tauri's codegen emits an EMPTY
    //    EmbeddedAssets because tauri.conf.json configures a devUrl — the
    //    desktop dev build serves the frontend from Vite rather than the
    //    binary. Release builds embed `../dist`, so there `index.html`
    //    resolves. Probe and degrade gracefully. ──
    let index = app.asset_resolver().get("index.html".into());
    if cfg!(debug_assertions) {
        assert!(
            index.is_none(),
            "dev/debug profile serves the frontend from Vite (devUrl), so no asset is embedded"
        );
    } else {
        assert!(
            index.is_some(),
            "release build should serve the embedded frontend index.html"
        );
    }
}
