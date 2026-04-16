//! Integration tests for the Phase 2 virtual display manager.
//!
//! These tests cover:
//! - Graceful degrade when Xvfb isn't available.
//! - Full Xvfb lifecycle when it is available (skipped otherwise).
//! - Idempotent acquire for the same workspace.
//! - Release + cleanup artifacts.
//! - Orphan sweep on manager construction.
//!
//! Tests that need a real Xvfb are gated on `xvfb_installed()` and will
//! `println!` a skip message if Xvfb isn't on PATH — they don't fail CI on
//! machines that don't have X11 tooling. Tracked in
//! `docs/plans/sandboxing.md` — Phase 2 testing strategy.

use codemux_lib::execution::virtual_display::{
    find_free_display_number_for_tests, AcquireOptions, Error, VirtualDisplayManager,
};
use std::path::PathBuf;
use std::process::Command;

fn xvfb_installed() -> bool {
    Command::new("Xvfb")
        .arg("-help")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success() || s.code().is_some())
        .unwrap_or(false)
}

#[test]
fn is_supported_reflects_xvfb_presence() {
    let expected = xvfb_installed();
    let actual = VirtualDisplayManager::is_supported();
    // On non-Linux is_supported is always false regardless of Xvfb.
    if cfg!(target_os = "linux") {
        assert_eq!(actual, expected, "is_supported mismatched Xvfb availability");
    } else {
        assert!(!actual, "non-Linux should always report unsupported");
    }
}

#[test]
fn acquire_returns_error_when_unsupported() {
    if VirtualDisplayManager::is_supported() {
        println!("SKIP: Xvfb is installed; this test verifies the unsupported path");
        return;
    }
    let mgr = VirtualDisplayManager::new();
    let err = mgr.acquire("test-ws").unwrap_err();
    match err {
        Error::XvfbNotFound | Error::Unsupported => {}
        other => panic!("unexpected error variant: {other:?}"),
    }
    assert_eq!(mgr.active_count(), 0);
}

#[test]
fn release_and_shutdown_are_safe_when_empty() {
    let mgr = VirtualDisplayManager::new();
    mgr.release("never-acquired");
    mgr.shutdown_all();
    assert_eq!(mgr.active_count(), 0);
}

#[test]
fn find_free_display_skips_existing_sockets_and_locks() {
    // On CI/dev hosts there are typically sockets at /tmp/.X11-unix/X0. Start
    // probing at an unused base so this test doesn't depend on host state.
    let base = 20_000;
    let n = find_free_display_number_for_tests(base, 4).expect("should find free slot");
    assert!(
        (base..base + 4).contains(&n),
        "expected slot in [{base}, {base}+4), got :{n}"
    );
    // Sanity: repeated call returns the same slot (idempotent probe).
    let n2 = find_free_display_number_for_tests(base, 4).expect("should find free slot");
    assert_eq!(n, n2);
}

// ──────────────────────────────────────────────────────────────────────────
// Real Xvfb lifecycle — only runs when Xvfb is actually installed.
// ──────────────────────────────────────────────────────────────────────────

#[test]
#[cfg(target_os = "linux")]
fn acquire_starts_real_xvfb_and_release_cleans_up() {
    if !xvfb_installed() {
        println!("SKIP: Xvfb not installed — cannot exercise real lifecycle");
        return;
    }
    let mgr = VirtualDisplayManager::new();
    let env = mgr
        .acquire("integration-test-ws")
        .expect("real Xvfb acquire should succeed");
    assert!(env.display.starts_with(':'));
    assert!(env.display_number >= 1000);
    assert_eq!(mgr.active_count(), 1);

    // Socket must exist while the display is alive.
    let socket = PathBuf::from(format!("/tmp/.X11-unix/X{}", env.display_number));
    assert!(
        socket.exists(),
        "Xvfb socket {} should exist after acquire",
        socket.display()
    );

    // Idempotent acquire: same workspace → same display.
    let env2 = mgr.acquire("integration-test-ws").expect("acquire 2");
    assert_eq!(env.display, env2.display);
    assert_eq!(mgr.active_count(), 1);

    mgr.release("integration-test-ws");
    assert_eq!(mgr.active_count(), 0);

    // Socket and lock must be cleaned up after release.
    let lock = PathBuf::from(format!("/tmp/.X{}-lock", env.display_number));
    // Allow a short delay for FS metadata to settle.
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(
        !socket.exists() && !lock.exists(),
        "expected /tmp/.X11-unix/X{n} and /tmp/.X{n}-lock to be cleaned up after release",
        n = env.display_number
    );
}

#[test]
#[cfg(target_os = "linux")]
fn shutdown_all_kills_every_display() {
    if !xvfb_installed() {
        println!("SKIP: Xvfb not installed");
        return;
    }
    let mgr = VirtualDisplayManager::new();
    let _a = mgr.acquire("ws-a").expect("acquire a");
    let _b = mgr.acquire("ws-b").expect("acquire b");
    assert_eq!(mgr.active_count(), 2);
    mgr.shutdown_all();
    assert_eq!(mgr.active_count(), 0);
}

#[test]
#[cfg(target_os = "linux")]
fn separate_workspaces_get_separate_displays() {
    if !xvfb_installed() {
        println!("SKIP: Xvfb not installed");
        return;
    }
    let mgr = VirtualDisplayManager::new();
    let a = mgr.acquire("ws-a").expect("acquire a");
    let b = mgr.acquire("ws-b").expect("acquire b");
    assert_ne!(
        a.display_number, b.display_number,
        "distinct workspaces must get distinct displays"
    );
    mgr.shutdown_all();
}

fn xauth_installed() -> bool {
    Command::new("xauth")
        .arg("-v")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success() || s.code().is_some())
        .unwrap_or(false)
}

fn x11vnc_installed() -> bool {
    Command::new("x11vnc")
        .arg("-help")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success() || s.code().is_some())
        .unwrap_or(false)
}

#[test]
#[cfg(target_os = "linux")]
fn acquire_populates_xauthority_when_xauth_is_available() {
    if !xvfb_installed() || !xauth_installed() {
        println!("SKIP: needs Xvfb + xauth");
        return;
    }
    let mgr = VirtualDisplayManager::new();
    let env = mgr.acquire("xauth-ws").expect("acquire");
    let path = env
        .xauthority_path
        .as_ref()
        .expect("xauth path should be set when xauth is installed");
    let path = PathBuf::from(path);
    assert!(path.exists(), "xauth file {path:?} should exist on disk");

    // Phase 2.5 hardening: cookie must live under $XDG_RUNTIME_DIR when
    // that env is set (the normal case on any systemd/pam_systemd box).
    if let Ok(xdg) = std::env::var("XDG_RUNTIME_DIR") {
        let xdg_prefix = PathBuf::from(xdg);
        assert!(
            path.starts_with(&xdg_prefix),
            "xauth file {path:?} should live under XDG_RUNTIME_DIR {xdg_prefix:?}"
        );
        assert!(
            !path.starts_with("/tmp"),
            "xauth file must not land in /tmp when XDG_RUNTIME_DIR is set \
             (systemd-tmpfiles would age it at 10 days)"
        );
    }

    // File should be 0600 (secret) on Unix.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(&path).expect("stat");
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "xauth file must be 0600 but was {:o}", mode
        );
    }
    // env_pairs should include XAUTHORITY.
    let pairs = env.env_pairs();
    assert!(pairs.iter().any(|(k, _)| k == "XAUTHORITY"));

    mgr.shutdown_all();
    assert!(!path.exists(), "xauth file should be removed on shutdown");
}

#[test]
#[cfg(target_os = "linux")]
fn watch_vnc_option_starts_x11vnc_when_available() {
    if !xvfb_installed() {
        println!("SKIP: Xvfb not installed");
        return;
    }
    let mgr = VirtualDisplayManager::new();
    let env = mgr
        .acquire_with_options("vnc-ws", AcquireOptions { watch_vnc: true })
        .expect("acquire with vnc");

    if x11vnc_installed() {
        let port = env
            .vnc_port
            .expect("x11vnc installed but vnc_port is None — regression");
        assert!(port >= 5910 && port < 6000, "port {port} outside expected range");
        // VNC should be bound on loopback. Connecting succeeds if a listener
        // is there; we just probe the port is occupied.
        let blocked = std::net::TcpListener::bind(("127.0.0.1", port)).is_err();
        assert!(blocked, "x11vnc should own 127.0.0.1:{port}");

        // Phase 2.5: password must be populated when VNC comes up.
        let pw = env
            .vnc_password
            .as_deref()
            .expect("vnc_password should be set when VNC is spawned");
        assert_eq!(pw.len(), 32, "16-byte hex password expected to be 32 chars");
        // Password is NOT included in env_pairs — the agent sees DISPLAY +
        // XAUTHORITY only, not the VNC secret.
        let pairs = env.env_pairs();
        assert!(
            !pairs.iter().any(|(_, v)| v == pw),
            "VNC password must never leak into child env"
        );
    } else {
        // Graceful degrade — x11vnc missing should NOT fail acquire, just
        // leave vnc_port None.
        assert!(env.vnc_port.is_none());
        assert!(env.vnc_password.is_none());
    }
    mgr.shutdown_all();
}

#[test]
fn env_for_workspace_returns_none_when_not_acquired() {
    let mgr = VirtualDisplayManager::new();
    assert!(mgr.env_for_workspace("never-acquired").is_none());
}
