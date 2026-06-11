//! Integration tests for the Linux file-picker backend preflight
//! (issue #95: dialogs silently failing on minimal WM setups).
//!
//! Lives in its own integration-test binary on purpose: the checks
//! mutate process-global env (DBUS_SESSION_BUS_ADDRESS, PATH) and
//! cargo runs each tests/*.rs file as a separate process, so nothing
//! else can race those variables. Inside this file everything runs
//! sequentially in a single #[tokio::test] for the same reason.

#![cfg(target_os = "linux")]

use codemux_lib::dialog_preflight;

#[tokio::test]
async fn preflight_distinguishes_missing_backends_from_cancel() {
    // ── 1. No portal, no zenity → actionable error ───────────────
    // Point the session bus somewhere dead and strip PATH so neither
    // backend can be found. This replicates a minimal i3/Arch setup
    // (no xdg-desktop-portal running, zenity not installed).
    std::env::set_var(
        "DBUS_SESSION_BUS_ADDRESS",
        "unix:path=/nonexistent/codemux-preflight-test",
    );
    std::env::set_var("PATH", "/nonexistent-codemux-bin");

    let error = dialog_preflight::ensure_file_picker_backend()
        .await
        .expect_err("preflight must fail when no dialog backend exists");
    assert!(
        error.contains(dialog_preflight::NO_BACKEND_MARKER),
        "error must carry the frontend-matchable marker, got: {error}"
    );
    assert!(
        error.contains("zenity") && error.contains("xdg-desktop-portal"),
        "error must name the installable fixes, got: {error}"
    );

    // ── 2. zenity alone satisfies the preflight ──────────────────
    // rfd falls back to spawning zenity when the portal call fails,
    // so a present zenity binary means dialogs can open.
    let dir = tempfile::tempdir().expect("tempdir");
    let fake_zenity = dir.path().join("zenity");
    std::fs::write(&fake_zenity, "#!/bin/sh\nexit 0\n").expect("write fake zenity");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake_zenity, std::fs::Permissions::from_mode(0o755))
            .expect("chmod fake zenity");
    }
    std::env::set_var("PATH", dir.path());

    dialog_preflight::ensure_file_picker_backend()
        .await
        .expect("zenity on PATH must satisfy the preflight even without a portal");
}
