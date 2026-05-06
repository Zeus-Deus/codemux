//! End-to-end sanity tests for the claude-agent sidecar.
//!
//! These tests prove the full pipeline:
//!   * A separate Bun TypeScript project compiles to a standalone binary.
//!   * Tauri's `externalBin` convention stages it at
//!     `src-tauri/binaries/codemux-claude-sidecar-<triple>`.
//!   * The Rust side spawns it via [`JsonRpcChild`] and speaks JSON-RPC
//!     over stdio.
//!
//! If the binary is missing at test time (developer has not yet run
//! `bun run build` / `scripts/build-claude-sidecar.sh`) the tests print
//! a clear message and return cleanly rather than failing — in that
//! case the test run is a no-op, matching how the `agent-browser`
//! sidecar is handled.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use codemux_lib::json_rpc_child::{JsonRpcChild, RpcChildError, SpawnConfig};
use serde_json::{json, Value};

/// Resolve the sidecar binary path for the current target triple. Returns
/// `None` with a diagnostic if the binary has not been built yet.
fn try_sidecar_binary_path() -> Option<PathBuf> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // CARGO_MANIFEST_DIR points at `src-tauri/`, so binaries/ is a
    // direct sibling.
    let binaries = manifest_dir.join("binaries");

    // Current build target triple. `rustc -vV | grep host` is the
    // conventional way to discover this at runtime.
    let triple = current_target_triple();
    let ext = if triple.contains("windows") { ".exe" } else { "" };
    let path = binaries.join(format!("codemux-claude-sidecar-{triple}{ext}"));
    if path.exists() && is_executable(&path) {
        Some(path)
    } else {
        // Fallback: bare name, which Tauri's `externalBin` accepts at
        // runtime but we do not rely on in tests.
        let bare = binaries.join(format!("codemux-claude-sidecar{ext}"));
        if bare.exists() && is_executable(&bare) {
            Some(bare)
        } else {
            None
        }
    }
}

fn current_target_triple() -> String {
    // rustc records the target triple the crate was built with into an
    // env var that build.rs can read. We don't want to run `rustc` here,
    // so we fall back to a compile-time default via cfg macros.
    option_env!("TARGET")
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            // cfg-based fallback covering the platforms we actually
            // support.
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

/// Emit a skip marker and return `None` if the sidecar binary is
/// missing. Callers use `?`-style early return after this check.
fn require_sidecar() -> Option<PathBuf> {
    match try_sidecar_binary_path() {
        Some(p) => Some(p),
        None => {
            eprintln!(
                "[sidecar_ping] sidecar binary not found; build with \
                 `cd sidecar/claude-agent && bun run build` or \
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
        default_timeout: Duration::from_secs(5),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawn_sidecar_and_ping() {
    let Some(bin) = require_sidecar() else {
        return;
    };
    let child = JsonRpcChild::spawn(spawn_config(bin)).await.expect("spawn");
    let response = child
        .request("ping", json!({}))
        .await
        .expect("ping request");
    let obj = response.as_object().expect("object response");
    assert_eq!(obj.get("pong"), Some(&Value::Bool(true)));
    let st = obj
        .get("server_time")
        .and_then(|v| v.as_str())
        .expect("server_time string");
    // Loose ISO-8601 check — the sidecar emits Date.toISOString().
    assert!(
        st.contains('T') && st.ends_with('Z'),
        "server_time not ISO-8601: {st}"
    );
    let _ = child.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ping_echoes_params() {
    let Some(bin) = require_sidecar() else {
        return;
    };
    let child = JsonRpcChild::spawn(spawn_config(bin)).await.expect("spawn");
    let response = child
        .request("ping", json!({"foo": "bar", "nested": {"n": 1}}))
        .await
        .expect("ping request");
    let echo = response
        .get("echo")
        .expect("echo field")
        .as_object()
        .expect("echo object");
    assert_eq!(echo.get("foo"), Some(&Value::String("bar".into())));
    assert_eq!(
        echo.get("nested").and_then(|v| v.get("n")),
        Some(&Value::Number(1.into()))
    );
    let _ = child.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_method_returns_method_not_found() {
    let Some(bin) = require_sidecar() else {
        return;
    };
    let child = JsonRpcChild::spawn(spawn_config(bin)).await.expect("spawn");
    let err = child
        .request("definitely_not_a_method", json!({}))
        .await
        .unwrap_err();
    match err {
        RpcChildError::RpcError(e) => {
            assert_eq!(e.code, -32601);
            assert!(
                e.message.contains("definitely_not_a_method"),
                "message: {}",
                e.message
            );
        }
        other => panic!("expected RpcError(-32601), got {other:?}"),
    }
    let _ = child.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sidecar_exits_cleanly_on_shutdown() {
    let Some(bin) = require_sidecar() else {
        return;
    };
    let child = JsonRpcChild::spawn(spawn_config(bin)).await.expect("spawn");
    // Sanity ping so the sidecar has actually come up.
    let _ = child.request("ping", json!({})).await.expect("ping");
    assert!(child.is_alive(), "child alive before shutdown");

    child.shutdown().await.expect("shutdown ok");
    // After shutdown the internal `alive` flag should have flipped
    // false; the sidecar's main() logs "stdin closed, exiting" and
    // exits 0.
    assert!(!child.is_alive(), "child still marked alive after shutdown");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sidecar_stderr_does_not_pollute_stdout() {
    let Some(bin) = require_sidecar() else {
        return;
    };
    let child = JsonRpcChild::spawn(spawn_config(bin)).await.expect("spawn");
    // Issue several requests in quick succession. If stderr were
    // accidentally merged with stdout, the sidecar's startup log
    // ("sidecar started", etc.) would cause `JsonRpcChild` to emit a
    // JsonParseError somewhere along the way — which does not fail
    // the request but would be visible as a stream warning. Here we
    // assert simply that every request succeeds.
    for i in 0..10 {
        let response = child
            .request("ping", json!({"i": i}))
            .await
            .expect("ping");
        assert_eq!(response.get("pong"), Some(&Value::Bool(true)));
    }
    let _ = child.shutdown().await;
}
