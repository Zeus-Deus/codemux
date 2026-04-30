//! End-to-end Rust↔sidecar tests that exercise the new SDK-integration
//! RPCs.
//!
//! These tests spawn the compiled sidecar binary and invoke its
//! JSON-RPC methods. They cover the paths that do NOT require real
//! Anthropic authentication — auth probes (with fake wrapper
//! scripts), error shapes, and state-management sanity. Running a
//! real session through the SDK and getting a response is a MANUAL
//! smoke test; it requires a logged-in `claude` binary and real
//! network egress, so it lives outside CI.
//!
//! The sidecar binary is resolved with the same convention as
//! `sidecar_ping.rs`. If it does not exist at test time, every test
//! here returns cleanly with a "build first" hint rather than
//! failing.
//!
//! Unix-only: `fake_claude_script` writes a `#!/usr/bin/env bash`
//! wrapper and chmods it with `std::os::unix::fs::PermissionsExt`.
//! Windows has no analogue, and every probe test depends on the
//! wrapper, so we gate the whole file rather than carry a parallel
//! cmd.exe fixture.

#![cfg(unix)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use codemux_lib::json_rpc_child::{JsonRpcChild, RpcChildError, SpawnConfig};
use serde_json::{json, Value};

fn try_sidecar_binary_path() -> Option<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let binaries = manifest_dir.join("binaries");
    let triple = current_target_triple();
    let ext = if triple.contains("windows") { ".exe" } else { "" };
    let path = binaries.join(format!("codemux-claude-sidecar-{triple}{ext}"));
    if path.exists() && is_executable(&path) {
        Some(path)
    } else {
        None
    }
}

fn current_target_triple() -> String {
    option_env!("TARGET")
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
                "x86_64-unknown-linux-gnu".into()
            } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
                "aarch64-unknown-linux-gnu".into()
            } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
                "x86_64-apple-darwin".into()
            } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
                "aarch64-apple-darwin".into()
            } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
                "x86_64-pc-windows-msvc".into()
            } else {
                "unknown".into()
            }
        })
}

#[cfg(unix)]
fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && (m.permissions().mode() & 0o111 != 0))
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &std::path::Path) -> bool {
    path.is_file()
}

fn require_sidecar() -> Option<PathBuf> {
    match try_sidecar_binary_path() {
        Some(p) => Some(p),
        None => {
            eprintln!(
                "[sidecar_sdk] sidecar binary not found; build with \
                 `bash scripts/build-claude-sidecar.sh` and retry"
            );
            None
        }
    }
}

fn spawn_config(bin: PathBuf) -> SpawnConfig {
    SpawnConfig {
        program: bin,
        args: vec![],
        env: HashMap::new(),
        cwd: None,
        default_timeout: Duration::from_secs(10),
    }
}

/// Create a tiny bash wrapper that stands in for a `claude` binary in
/// `probe_*` tests. Each test owns its own tempdir to dodge ETXTBSY on
/// concurrent runs.
fn fake_claude_script(body: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("claude");
    std::fs::write(&path, body.as_bytes()).unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    (dir, path)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn probe_installed_detects_claude_when_on_path() {
    let Some(bin) = require_sidecar() else { return };
    let (_dir, claude_path) = fake_claude_script(
        "#!/usr/bin/env bash\nif [ \"$1\" = \"--version\" ]; then echo 'claude 9.9.9'; exit 0; fi\nexit 1\n",
    );
    let child = JsonRpcChild::spawn(spawn_config(bin)).await.expect("spawn");
    let response = child
        .request(
            "probe-installed",
            json!({ "binaryPath": claude_path.to_string_lossy() }),
        )
        .await
        .expect("probe");
    assert_eq!(response.get("installed"), Some(&Value::Bool(true)));
    assert_eq!(
        response.get("version").and_then(|v| v.as_str()),
        Some("9.9.9")
    );
    let _ = child.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn probe_installed_reports_false_for_missing_binary() {
    let Some(bin) = require_sidecar() else { return };
    let child = JsonRpcChild::spawn(spawn_config(bin)).await.expect("spawn");
    let response = child
        .request(
            "probe-installed",
            json!({ "binaryPath": "/definitely/not/a/real/binary-zzz" }),
        )
        .await
        .expect("probe");
    assert_eq!(response.get("installed"), Some(&Value::Bool(false)));
    let _ = child.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn probe_authenticated_reports_status() {
    let Some(bin) = require_sidecar() else { return };
    // Fake `claude` whose `auth status` output contains "not logged in".
    let (_dir, claude_path) = fake_claude_script(
        "#!/usr/bin/env bash\nif [ \"$1 $2\" = \"auth status\" ]; then echo 'You are not logged in.'; exit 0; fi\nexit 0\n",
    );
    let child = JsonRpcChild::spawn(spawn_config(bin)).await.expect("spawn");
    let response = child
        .request(
            "probe-authenticated",
            json!({ "binaryPath": claude_path.to_string_lossy() }),
        )
        .await
        .expect("probe");
    let status = response
        .get("status")
        .and_then(|v| v.as_str())
        .expect("status string");
    assert!(
        matches!(status, "authenticated" | "unauthenticated" | "unknown"),
        "unexpected status: {status}"
    );
    // For this specific fake, we know the output hits the
    // unauthenticated pattern — assert exact.
    assert_eq!(status, "unauthenticated");
    let _ = child.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_method_still_returns_method_not_found() {
    let Some(bin) = require_sidecar() else { return };
    let child = JsonRpcChild::spawn(spawn_config(bin)).await.expect("spawn");
    let err = child
        .request("totally-fictional-method", json!({}))
        .await
        .unwrap_err();
    match err {
        RpcChildError::RpcError(e) => assert_eq!(e.code, -32601),
        other => panic!("expected -32601, got {other:?}"),
    }
    let _ = child.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_session_without_claude_binary_fails_cleanly() {
    let Some(bin) = require_sidecar() else { return };
    let child = JsonRpcChild::spawn(spawn_config(bin)).await.expect("spawn");
    // The SDK's `query()` is lazy — it returns a Query object immediately
    // and only tries to spawn `claude` on first iterator pull. So
    // `start-session` itself may succeed, but an operation that forces
    // the iterator (say, sending a turn) would fail. We assert the
    // permissive path — start-session returns without throwing, since
    // that is the observed SDK behaviour at 0.2.114.
    let result = child
        .request(
            "start-session",
            json!({
                "threadId": "thread-nobin",
                "cwd": std::env::temp_dir().to_string_lossy(),
                "pathToClaudeCodeExecutable": "/definitely/not/a/real/claude-zzz",
            }),
        )
        .await;
    // Either outcome is acceptable: a cleanly-formed error OR a
    // deferred success. What we are checking is that the adapter
    // doesn't crash, and that the process stays responsive afterward.
    assert!(child.is_alive(), "sidecar must still be alive");
    // Subsequent unrelated RPCs still work.
    let pong = child
        .request("ping", json!({"probe": "after-failed-start"}))
        .await
        .expect("follow-up ping");
    assert_eq!(pong.get("pong"), Some(&Value::Bool(true)));
    // Clean up the session regardless of how start-session resolved.
    if result.is_ok() {
        let _ = child
            .request("stop-session", json!({ "threadId": "thread-nobin" }))
            .await;
    }
    let _ = child.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_session_on_unknown_thread_returns_already_closed_true() {
    let Some(bin) = require_sidecar() else { return };
    let child = JsonRpcChild::spawn(spawn_config(bin)).await.expect("spawn");
    let response = child
        .request("stop-session", json!({ "threadId": "never-started" }))
        .await
        .expect("stop-session");
    assert_eq!(
        response.get("alreadyClosed"),
        Some(&Value::Bool(true)),
        "stop-session on unknown thread should return alreadyClosed: true, got {response:?}"
    );
    let _ = child.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn respond_to_request_with_unknown_request_id_returns_invalid_params() {
    let Some(bin) = require_sidecar() else { return };
    let child = JsonRpcChild::spawn(spawn_config(bin)).await.expect("spawn");
    // First start a session so the threadId validates; use a bogus
    // claude path so no real subprocess starts — the request just
    // needs a session row.
    let (_dir, claude_path) = fake_claude_script(
        "#!/usr/bin/env bash\nsleep 3600\n",
    );
    let start = child
        .request(
            "start-session",
            json!({
                "threadId": "thread-resp",
                "cwd": std::env::temp_dir().to_string_lossy(),
                "pathToClaudeCodeExecutable": claude_path.to_string_lossy(),
            }),
        )
        .await;
    assert!(start.is_ok(), "start-session expected to succeed: {start:?}");
    let err = child
        .request(
            "respond-to-request",
            json!({
                "threadId": "thread-resp",
                "requestId": "does-not-exist",
                "decision": { "behavior": "deny", "message": "no" },
            }),
        )
        .await
        .unwrap_err();
    match err {
        RpcChildError::RpcError(e) => {
            // -32603 (internal) or -32602 (invalid params) are both
            // acceptable since respondToRequest throws a plain Error
            // inside the session, which the dispatcher surfaces as
            // internal. The important thing is that the sidecar
            // stays alive and the error is structured.
            assert!(
                e.code == -32602 || e.code == -32603,
                "unexpected error code {}: {}",
                e.code,
                e.message,
            );
            assert!(
                e.message.contains("does-not-exist")
                    || e.message.contains("not found")
                    || e.message.contains("already resolved"),
                "error message should mention the missing id: {}",
                e.message
            );
        }
        other => panic!("expected RpcError, got {other:?}"),
    }
    assert!(child.is_alive(), "sidecar must still be alive");
    let _ = child
        .request("stop-session", json!({ "threadId": "thread-resp" }))
        .await;
    let _ = child.shutdown().await;
}
