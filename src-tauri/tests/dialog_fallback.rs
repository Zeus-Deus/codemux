//! Integration tests for the direct zenity fallback (issue #95).
//!
//! Like `dialog_preflight.rs`, this lives in its own test binary and
//! runs everything in a single test: the checks mutate process-global
//! env (PATH-independent here, but `CODEMUX_ZENITY_TIMEOUT_MS` and the
//! hostile session vars), and cargo runs each tests/*.rs as its own
//! process, so nothing else races them.

#![cfg(target_os = "linux")]

use codemux_lib::dialog_fallback;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Fake zenity that records the session vars it actually received, then
/// "selects" a path. Proves our spawn sanitizes the environment.
const RECORDING_ZENITY: &str = r#"#!/bin/sh
{
  echo "DBUS=${DBUS_SESSION_BUS_ADDRESS-<unset>}"
  echo "GTK3=${GTK3_MODULES-<unset>}"
  echo "NOAT=${NO_AT_BRIDGE-<unset>}"
} > "$FAKE_ZENITY_DUMP"
printf '/picked/dir'
exit 0
"#;

fn write_exec(path: &std::path::Path, body: &str) {
    std::fs::write(path, body).expect("write fake zenity");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
}

#[tokio::test]
async fn fallback_sanitizes_env_returns_selection_and_times_out() {
    let dir = tempfile::tempdir().expect("tempdir");

    // ── 1. Sanitized environment + selection round-trip ──────────
    let dump = dir.path().join("env_dump");
    let recording = dir.path().join("zenity");
    write_exec(&recording, RECORDING_ZENITY);
    std::env::set_var("FAKE_ZENITY_DUMP", &dump);

    // Poison the env exactly the way a broken minimal-WM session does:
    // a dead session bus address and a blocking GTK module. Both would
    // hang a real zenity; our spawn must strip them.
    std::env::set_var(
        "DBUS_SESSION_BUS_ADDRESS",
        "unix:path=/nonexistent/codemux-dead-bus",
    );
    std::env::set_var("GTK3_MODULES", "some-blocking-module");

    let picked = dialog_fallback::pick_folder(&recording, "Choose folder")
        .await
        .expect("fallback should run the fake zenity");
    assert_eq!(picked, Some(PathBuf::from("/picked/dir")));

    let seen = std::fs::read_to_string(&dump).expect("env dump written");
    assert!(
        seen.contains("DBUS=<unset>"),
        "DBUS_SESSION_BUS_ADDRESS must be cleared for the child, got:\n{seen}"
    );
    assert!(
        seen.contains("GTK3=<unset>"),
        "GTK3_MODULES must be cleared for the child, got:\n{seen}"
    );
    assert!(
        seen.contains("NOAT=1"),
        "NO_AT_BRIDGE must be set for the child, got:\n{seen}"
    );

    // ── 2. A wedged zenity is bounded by the timeout ─────────────
    let hung = dir.path().join("hung-zenity");
    write_exec(&hung, "#!/bin/sh\nsleep 60\n");
    std::env::set_var("CODEMUX_ZENITY_TIMEOUT_MS", "300");

    let start = Instant::now();
    let result = dialog_fallback::pick_folder(&hung, "Choose folder").await;
    let elapsed = start.elapsed();

    assert!(
        result.is_err(),
        "a zenity that never returns must surface an error, got: {result:?}"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "the timeout must release the dialog promptly, took {elapsed:?}"
    );
}
