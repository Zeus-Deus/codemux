//! End-to-end tests for AppImage environment hygiene on child processes.
//!
//! When Codemux runs from an AppImage, `AppRun` rewrites `LD_LIBRARY_PATH`,
//! `PATH`, `PYTHONHOME` and friends to point into the mounted AppDir so the
//! bundled binary finds its bundled libraries. Those values are actively
//! harmful to any child: a shell that inherits them resolves system binaries
//! against our bundled `libssl`/`libpcre2`, which on a host with newer
//! libraries fails before `main` (e.g. `cargo` dying with
//! "version `OPENSSL_3.5.0' not found").
//!
//! The sanitizers only act when `APPDIR` is set, so proving they work requires
//! a process that actually has `APPDIR` in its environment. Mutating the
//! current process's environment is racy across parallel tests (and `unsafe`),
//! so each test re-executes *this test binary* with a simulated AppImage
//! environment and runs the real assertions in that child.

#![cfg(target_os = "linux")]

use std::collections::HashMap;
use std::process::Command;

/// Marker telling a re-executed child it is the inner half of a test.
const CHILD_GUARD: &str = "CODEMUX_APPIMAGE_HYGIENE_CHILD";

/// Stand-in for a real `/tmp/.mount_codemuXXXXXX` AppImage mount.
const FAKE_APPDIR: &str = "/tmp/.mount_codemuTESTXX";

/// True when we are the re-executed child and should run the real assertions.
fn is_child() -> bool {
    std::env::var(CHILD_GUARD).is_ok()
}

/// Re-run one `#[ignore]`d test in a child process carrying a simulated
/// AppImage environment, and assert it passed.
///
/// The inner test is `#[ignore]`d so a normal `cargo test` run never executes
/// it without the environment it needs; `--ignored --exact` opts it back in.
fn run_inner_test(name: &str) {
    let exe = std::env::current_exe().expect("current_exe");
    let host_path = std::env::var("PATH").unwrap_or_default();

    let output = Command::new(&exe)
        .args(["--exact", name, "--ignored", "--nocapture", "--test-threads=1"])
        .env(CHILD_GUARD, "1")
        // The environment AppRun would have handed us:
        .env("APPDIR", FAKE_APPDIR)
        .env("APPIMAGE", "/home/user/Downloads/codemux_0.0.0_amd64.AppImage")
        .env(
            "LD_LIBRARY_PATH",
            format!("{FAKE_APPDIR}/usr/lib/:{FAKE_APPDIR}/lib/:"),
        )
        .env("PYTHONHOME", format!("{FAKE_APPDIR}/usr/"))
        .env(
            "PATH",
            format!("{FAKE_APPDIR}/usr/bin/:{FAKE_APPDIR}/bin/:{host_path}"),
        )
        .output()
        .expect("re-exec test binary");

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    assert!(
        output.status.success(),
        "inner test `{name}` failed\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
    );
    // A libtest filter that matches nothing still exits 0, which would make
    // this whole test vacuously green if the inner test were ever renamed.
    // Require positive evidence that exactly one test actually ran.
    assert!(
        stdout.contains("1 passed"),
        "inner test `{name}` did not run (filter matched nothing?)\
         \n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
    );
}

/// Run `env` through a real PTY using the production sanitizer, and parse the
/// child's environment out of the terminal output.
fn pty_child_env() -> HashMap<String, String> {
    use portable_pty::{native_pty_system, CommandBuilder, PtySize};

    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 200,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");

    let mut cmd = CommandBuilder::new("/usr/bin/env");
    // The call under test — the exact function the PTY spawn paths use.
    codemux_lib::execution::sanitize_appimage_env_pty(&mut cmd);

    let mut child = pair.slave.spawn_command(cmd).expect("spawn env in pty");
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().expect("clone reader");
    // Drop the master writer so the child's read side sees EOF and our read
    // loop below terminates instead of blocking forever on an open PTY.
    drop(pair.master);

    let mut buf = Vec::new();
    std::io::Read::read_to_end(&mut reader, &mut buf).expect("read pty");
    child.wait().expect("wait child");

    String::from_utf8_lossy(&buf)
        .lines()
        .filter_map(|line| line.trim_end_matches('\r').split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

// ── PTY path ────────────────────────────────────────────────────────────

#[test]
fn pty_children_do_not_inherit_appimage_library_path() {
    if is_child() {
        return;
    }
    run_inner_test("inner_pty_child_env_is_clean");
}

#[test]
#[ignore = "re-executed by pty_children_do_not_inherit_appimage_library_path"]
fn inner_pty_child_env_is_clean() {
    assert!(
        is_child(),
        "inner test must only run via its re-exec parent"
    );
    let env = pty_child_env();

    // The headline bug: an all-AppDir LD_LIBRARY_PATH must be gone entirely.
    assert!(
        !env.contains_key("LD_LIBRARY_PATH"),
        "LD_LIBRARY_PATH leaked into PTY child: {:?}",
        env.get("LD_LIBRARY_PATH")
    );
    // Scalar AppDir-rooted vars break system python/perl the same way.
    assert!(
        !env.contains_key("PYTHONHOME"),
        "PYTHONHOME leaked: {:?}",
        env.get("PYTHONHOME")
    );
    // Launch markers must not convince a child it is running inside a bundle.
    assert!(!env.contains_key("APPDIR"), "APPDIR leaked");
    assert!(!env.contains_key("APPIMAGE"), "APPIMAGE leaked");

    // PATH keeps the host entries but loses the AppDir bin dirs, so the
    // bundle's binaries cannot shadow the user's toolchain.
    let path = env.get("PATH").expect("PATH must survive, not be removed");
    assert!(
        !path.contains(FAKE_APPDIR),
        "AppDir bin dirs left on child PATH: {path}"
    );
    assert!(
        path.contains("/usr/bin"),
        "host PATH entries were lost: {path}"
    );
}

// ── Agent-CLI (tokio) path ──────────────────────────────────────────────

#[test]
fn agent_cli_children_do_not_inherit_appimage_library_path() {
    if is_child() {
        return;
    }
    run_inner_test("inner_tokio_child_env_is_clean");
}

#[test]
#[ignore = "re-executed by agent_cli_children_do_not_inherit_appimage_library_path"]
fn inner_tokio_child_env_is_clean() {
    assert!(
        is_child(),
        "inner test must only run via its re-exec parent"
    );
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let env: HashMap<String, String> = rt.block_on(async {
        let mut cmd = tokio::process::Command::new("/usr/bin/env");
        codemux_lib::execution::sanitize_appimage_env_tokio(&mut cmd);
        let out = cmd.output().await.expect("spawn env");
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|line| line.split_once('='))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    });

    assert!(!env.contains_key("LD_LIBRARY_PATH"), "LD_LIBRARY_PATH leaked");
    assert!(!env.contains_key("PYTHONHOME"), "PYTHONHOME leaked");
    assert!(!env.contains_key("APPDIR"), "APPDIR leaked");
    let path = env.get("PATH").expect("PATH must survive");
    assert!(!path.contains(FAKE_APPDIR), "AppDir bin dirs on PATH: {path}");
}

// ── host_command constructors ───────────────────────────────────────────

#[test]
fn host_command_strips_appimage_env() {
    if is_child() {
        return;
    }
    run_inner_test("inner_host_command_env_is_clean");
}

#[test]
#[ignore = "re-executed by host_command_strips_appimage_env"]
fn inner_host_command_env_is_clean() {
    assert!(is_child(), "inner test must only run via its re-exec parent");

    let output = codemux_lib::execution::host_command("/usr/bin/env")
        .output()
        .expect("spawn env");
    let env: HashMap<String, String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

    // `git` is the motivating case: under an AppImage it otherwise links
    // against our bundled libpcre2 and warns/fails before doing any work.
    assert!(!env.contains_key("LD_LIBRARY_PATH"), "LD_LIBRARY_PATH leaked");
    assert!(!env.contains_key("PYTHONHOME"), "PYTHONHOME leaked");
    assert!(!env.contains_key("APPDIR"), "APPDIR leaked");
    let path = env.get("PATH").expect("PATH must survive");
    assert!(!path.contains(FAKE_APPDIR), "AppDir bin dirs on PATH: {path}");
}

#[test]
fn host_command_preserves_gui_env_for_editor_launch() {
    // `open_in_editor` deliberately does NOT call the GUI sanitizers: the user
    // clicked "open this file", so the editor must inherit DISPLAY/WAYLAND to
    // put a window on screen. AppImage hygiene is orthogonal and must not
    // regress that — if it ever starts stripping GUI keys, the editor spawn
    // succeeds silently with no visible window, which is near-impossible to
    // debug from a bug report.
    let mut cmd = codemux_lib::execution::host_command("/usr/bin/env");
    cmd.env("DISPLAY", ":0")
        .env("WAYLAND_DISPLAY", "wayland-1")
        .env("XAUTHORITY", "/home/user/.Xauthority");
    let output = cmd.output().expect("spawn env");
    let env: HashMap<String, String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

    assert_eq!(env.get("DISPLAY").map(String::as_str), Some(":0"));
    assert_eq!(
        env.get("WAYLAND_DISPLAY").map(String::as_str),
        Some("wayland-1")
    );
    assert_eq!(
        env.get("XAUTHORITY").map(String::as_str),
        Some("/home/user/.Xauthority")
    );
}

// ── Non-AppImage installs (AUR / deb / rpm) ─────────────────────────────

#[test]
fn non_appimage_install_is_completely_untouched() {
    // No APPDIR in this process (the harness does not set one), so the
    // sanitizers must be a strict no-op — this is what guarantees the AUR
    // build behaves exactly as it did before.
    if std::env::var("APPDIR").is_ok() || is_child() {
        return;
    }

    let mut cmd = Command::new("/usr/bin/env");
    cmd.env("LD_LIBRARY_PATH", "/opt/mylibs")
        .env("PYTHONHOME", "/home/user/venv");
    codemux_lib::execution::sanitize_appimage_env_std(&mut cmd);
    let output = cmd.output().expect("spawn env");
    let env: HashMap<String, String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

    assert_eq!(
        env.get("LD_LIBRARY_PATH").map(String::as_str),
        Some("/opt/mylibs"),
        "a user's own LD_LIBRARY_PATH must survive on non-AppImage installs"
    );
    assert_eq!(
        env.get("PYTHONHOME").map(String::as_str),
        Some("/home/user/venv"),
        "a user's own PYTHONHOME must survive on non-AppImage installs"
    );
}
