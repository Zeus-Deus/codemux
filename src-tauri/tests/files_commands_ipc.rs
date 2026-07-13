//! IPC-layer end-to-end tests for the `commands::files` surface.
//!
//! `commands/files.rs` converted its blocking commands from sync to
//! `async fn` + `spawn_blocking` so they run off the GTK main thread.
//! The unit tests in that module exercise the command *functions*;
//! these tests drive the real Tauri dispatch pipeline instead —
//! `tauri::test::get_ipc_response` routes through the same
//! `generate_handler!` machinery production uses, so they prove:
//!
//! - the converted commands still resolve by name over IPC,
//! - camelCase wire args (`showHidden`, `maxResults`) still
//!   deserialize into the snake_case Rust parameters,
//! - the async command futures complete and serialize their results
//!   back through the invoke response path (a sync→async conversion
//!   changes exactly this dispatch path; the frontend `invoke()`
//!   contract must stay identical).
//!
//! Unix-only: `tauri::test` pulls in runtime imports that need
//! WebView2Loader.dll at process startup on Windows — the GitHub
//! Windows runner doesn't ship it, so the test binary dies before
//! any test runs. Same gate pattern as `agent_chat_commands.rs`.

#![cfg(unix)]

use std::fs;

use serde_json::{json, Value};
use tauri::ipc::{CallbackFn, InvokeResponseBody};
use tauri::test::{mock_builder, mock_context, noop_assets, INVOKE_KEY};
use tauri::webview::InvokeRequest;
use tauri::WebviewWindow;

fn build_app() -> tauri::App<tauri::test::MockRuntime> {
    mock_builder()
        .invoke_handler(tauri::generate_handler![
            codemux_lib::commands::list_directory,
            codemux_lib::commands::search_in_files,
            codemux_lib::commands::search_file_names,
            codemux_lib::commands::read_file,
            codemux_lib::commands::write_file,
            codemux_lib::commands::save_clipboard_image_bytes,
        ])
        .build(mock_context(noop_assets()))
        .expect("failed to build mock app")
}

fn invoke(
    webview: &WebviewWindow<tauri::test::MockRuntime>,
    cmd: &str,
    args: Value,
) -> Result<Value, Value> {
    let body = tauri::test::get_ipc_response(
        webview,
        InvokeRequest {
            cmd: cmd.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            // Reuse the window's own origin (its real local URL, e.g.
            // `tauri://localhost`) rather than a hand-written string.
            //
            // Tauri 2.11 hardened the IPC ACL gate: a request is now forced
            // through capability resolution unless its origin is *local*
            // (`webview::on_message` added `|| !is_local` to the pre-dispatch
            // reject in 2.11; 2.10 gated only on `plugin_command || app_manifest`).
            // The mock harness carries no ACL manifest, so a non-local origin
            // makes every app command reject with "not allowed. Plugin not
            // found". A literal `http://tauri.localhost` is *not* the local
            // origin on Unix (there it is `tauri://localhost`), so it tripped
            // the new clause. Reusing `webview.url()` models a genuine local
            // desktop invoke — exactly what `web_remote::dispatch` does in
            // production so ACL resolves identically — and is origin-agnostic
            // across platforms/future Tauri versions.
            url: webview.url().expect("webview url"),
            body: args.into(),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_string(),
        },
    )?;
    match body {
        InvokeResponseBody::Json(s) => Ok(serde_json::from_str(&s).expect("valid JSON response")),
        InvokeResponseBody::Raw(bytes) => {
            panic!("expected JSON response, got raw bytes: {bytes:?}")
        }
    }
}

#[test]
fn files_commands_round_trip_through_real_ipc_dispatch() {
    let app = build_app();
    let webview = tauri::WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .expect("failed to build mock webview");

    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_string_lossy().to_string();
    fs::create_dir(dir.path().join("sub")).unwrap();
    fs::write(dir.path().join(".dotfile"), "hidden").unwrap();
    fs::write(
        dir.path().join("needle-file.txt"),
        "first line\nthe needle is on line two\n",
    )
    .unwrap();

    // ── write_file → read_file (camelCase-free args, but proves the
    //    async write/read pair completes through dispatch) ──
    let note = dir.path().join("note.txt");
    let note_path = note.to_string_lossy().to_string();
    invoke(
        &webview,
        "write_file",
        json!({ "path": note_path, "content": "written over IPC\n" }),
    )
    .expect("write_file should succeed over IPC");
    let read_back = invoke(&webview, "read_file", json!({ "path": note_path }))
        .expect("read_file should succeed over IPC");
    assert_eq!(read_back, json!("written over IPC\n"));

    // ── list_directory with the camelCase `showHidden` wire arg ──
    let listed = invoke(
        &webview,
        "list_directory",
        json!({ "path": root, "showHidden": true }),
    )
    .expect("list_directory should succeed over IPC");
    let names: Vec<&str> = listed
        .as_array()
        .expect("array of entries")
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&".dotfile"),
        "showHidden=true must surface dotfiles (camelCase arg must \
         deserialize); got {names:?}"
    );
    assert!(names.contains(&"sub") && names.contains(&"needle-file.txt"));

    // ── search_in_files with the camelCase `maxResults` wire arg ──
    let found = invoke(
        &webview,
        "search_in_files",
        json!({ "path": root, "query": "needle", "maxResults": 10 }),
    )
    .expect("search_in_files should succeed over IPC");
    let found = found.as_array().expect("array of results");
    assert_eq!(found.len(), 1, "one matching line expected: {found:?}");
    assert_eq!(found[0]["line_number"], json!(2));
    assert!(found[0]["file_path"]
        .as_str()
        .unwrap()
        .ends_with("needle-file.txt"));

    // ── search_file_names ──
    let files = invoke(
        &webview,
        "search_file_names",
        json!({ "path": root, "query": "needle", "maxResults": 10 }),
    )
    .expect("search_file_names should succeed over IPC");
    assert_eq!(files, json!(["needle-file.txt"]));

    // ── save_clipboard_image_bytes ──
    let saved = invoke(
        &webview,
        "save_clipboard_image_bytes",
        json!({ "bytes": [0x89, 0x50, 0x4e, 0x47], "mime": "image/png" }),
    )
    .expect("save_clipboard_image_bytes should succeed over IPC");
    let saved_path = saved.as_str().expect("path string");
    assert!(saved_path.ends_with(".png"), "got {saved_path}");
    let _ = fs::remove_file(saved_path);

    // ── error path still maps to the IPC error channel ──
    let err = invoke(
        &webview,
        "read_file",
        json!({ "path": format!("{root}/does-not-exist.txt") }),
    )
    .expect_err("missing file must reject");
    assert!(
        err.as_str().unwrap_or_default().contains("Not a file"),
        "got {err:?}"
    );
}
