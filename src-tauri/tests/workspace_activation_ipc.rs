//! Activation side effects are the same for every surface that switches
//! workspaces.
//!
//! `cycle_workspace` (Ctrl+Tab) used to diverge from `activate_workspace`: it
//! spawned PTYs synchronously on the IPC thread, skipped the git refresh, and
//! never wrote `active_workspace` to the DB — so a workspace reached by
//! cycling was forgotten at restart. Both commands now route through one
//! shared `run_activation_side_effects`, and this test pins the observable
//! half of that: the persisted active workspace.
//!
//! Driven through the real invoke path on `tauri::test::MockRuntime`, which
//! also proves `cycle_workspace`'s newly injected `DatabaseStore` state
//! resolves at dispatch time (a `State<'_, T>` mismatch is a runtime error the
//! compiler cannot catch).
//!
//! CRITICAL ISOLATION: a real Codemux instance may be running on this machine.
//! `isolate_env` repoints every state-dir resolver at a throwaway tempdir
//! BEFORE the app boots, so the test never touches the real user's DB.
//!
//! Unix-only: `tauri::test` pulls in runtime imports that need
//! WebView2Loader.dll at process startup on Windows (same gate as the other
//! `*_ipc.rs` tests).

#![cfg(unix)]

use codemux_lib::database::DatabaseStore;
use codemux_lib::state::AppStateStore;
use serde_json::{json, Value};
use tauri::ipc::{CallbackFn, InvokeResponseBody};
use tauri::test::get_ipc_response;
use tauri::webview::{InvokeRequest, WebviewWindow};
use tauri::Manager;

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
    // Lazy PTY hydration runs on activation. A shell that exits immediately
    // keeps the test from leaving interactive children behind.
    std::env::set_var("SHELL", "/bin/true");
    std::env::remove_var("DISPLAY");
    std::env::remove_var("WAYLAND_DISPLAY");
}

/// Persistence is intentionally off the activation path: `emit_app_state`
/// hands the selection to a coalesced background worker
/// (`active_workspace_persistence`), so the DB record lands shortly *after*
/// the invoke returns. Reading immediately would race that worker — poll
/// instead, with a deadline far beyond any healthy write.
fn await_persisted_active_workspace(db: &DatabaseStore, expected: &str, context: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let last_seen = db.get_ui_state("active_workspace");
        if last_seen.as_deref() == Some(expected) {
            return;
        }
        if std::time::Instant::now() >= deadline {
            panic!(
                "{context}: expected active_workspace {expected:?} to be persisted \
                 within 10s, last saw {last_seen:?}"
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn invoke(
    webview: &WebviewWindow<tauri::test::MockRuntime>,
    invoke_key: String,
    cmd: &str,
    body: Value,
) -> Result<Value, String> {
    let response = get_ipc_response(
        webview,
        InvokeRequest {
            cmd: cmd.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: webview.url().expect("webview url"),
            body: body.into(),
            headers: Default::default(),
            invoke_key,
        },
    )
    .map_err(|error| error.to_string())?;
    Ok(match response {
        InvokeResponseBody::Json(raw) => {
            serde_json::from_str(&raw).unwrap_or(Value::Null)
        }
        InvokeResponseBody::Raw(bytes) => panic!("expected JSON, got raw bytes: {bytes:?}"),
    })
}

#[test]
fn activate_and_cycle_both_persist_the_active_workspace() {
    let tmp = tempfile::tempdir().expect("tempdir");
    isolate_env(tmp.path());

    let app = codemux_lib::build_headless_app().expect("headless app should build");
    let webview = app
        .get_webview_window("main")
        .expect("headless app should expose a \"main\" webview");
    let invoke_key = app.handle().invoke_key().to_string();

    // Two workspaces, created straight on the managed store: this test is
    // about the activation path, not about workspace creation.
    let state = app.state::<AppStateStore>();
    let first = state
        .create_empty_workspace_at_path(tmp.path().join("one"))
        .0;
    let second = state
        .create_empty_workspace_at_path(tmp.path().join("two"))
        .0;

    let db = app.state::<DatabaseStore>();

    invoke(
        &webview,
        invoke_key.clone(),
        "activate_workspace",
        json!({ "workspaceId": first }),
    )
    .expect("activate_workspace should dispatch Ok");
    await_persisted_active_workspace(
        &db,
        &first,
        "activate_workspace must persist the active workspace",
    );

    let cycled = invoke(&webview, invoke_key, "cycle_workspace", json!({ "step": 1 }))
        .expect("cycle_workspace should dispatch Ok");
    let cycled = cycled.as_str().expect("cycle_workspace returns a workspace id");

    assert_eq!(
        state.snapshot().active_workspace_id.0,
        cycled,
        "the cycled-to workspace must be the active one in memory"
    );
    await_persisted_active_workspace(
        &db,
        cycled,
        "cycle_workspace must persist the active workspace so Ctrl+Tab \
         switches survive a restart",
    );
    // Sanity: the two workspaces are distinct, so the cycle genuinely moved.
    assert_ne!(first, second);
}
