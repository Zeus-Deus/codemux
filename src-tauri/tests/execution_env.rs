//! Integration tests for the Phase 1 display-isolation env-strip.
//!
//! These tests verify that `prepare_agent_command` actually removes the
//! GUI-sensitive environment variables from a spawned child when
//! `allow_desktop_gui=false`, and leaves them alone when `true`. The tests
//! work cross-platform: on Linux with `bwrap` available the bwrap branch is
//! exercised; on macOS/Windows and on Linux without bwrap the HostPassthrough
//! fallback is exercised.
//!
//! Tracked in `docs/plans/sandboxing.md` — Phase 1 testing strategy.

use codemux_lib::execution::{
    gui_env_keys, prepare_agent_command, ExecutionBackendKind, ExecutionPolicy,
};
use std::process::Command;

fn policy(allow_desktop_gui: bool, backend: ExecutionBackendKind) -> ExecutionPolicy {
    ExecutionPolicy {
        backend_preference: backend,
        allow_network: true,
        allow_browser_automation: true,
        allow_desktop_gui,
        virtual_display: false,
    }
}

/// Run `prepared.executable prepared.args...`, applying `prepared.env_unset`
/// via `Command::env_remove`, with the test process's current env otherwise
/// inherited. Returns stdout as UTF-8 string.
fn run_prepared_printenv(
    prepared: &codemux_lib::execution::PreparedExecutionCommand,
    var: &str,
) -> String {
    // Use `printenv` on unix; Windows CI isn't exercised here but the same
    // approach would use `cmd /C echo %VAR%`. Tests that need Windows env
    // behavior are gated with `#[cfg(target_os = "windows")]` below.
    let mut cmd = Command::new(&prepared.executable);
    for a in &prepared.args {
        cmd.arg(a);
    }
    // Seed DISPLAY so the test is meaningful regardless of the host env.
    cmd.env("DISPLAY", "fake-test:99");
    cmd.env("WAYLAND_DISPLAY", "fake-wayland");
    cmd.env("XAUTHORITY", "/tmp/fake-xauthority");

    // If the prepared command is itself a wrapper (e.g. bwrap), args already
    // include the real program; we won't be able to run `printenv` through
    // bwrap in this unit-ish harness. The test only runs the HostPassthrough
    // path where executable == real program.
    if matches!(prepared.backend, ExecutionBackendKind::HostPassthrough) {
        // Replace executable with printenv and the arg with the key of interest.
        cmd = Command::new("printenv");
        cmd.arg(var);
        cmd.env("DISPLAY", "fake-test:99");
        cmd.env("WAYLAND_DISPLAY", "fake-wayland");
        cmd.env("XAUTHORITY", "/tmp/fake-xauthority");
    }

    for key in &prepared.env_unset {
        cmd.env_remove(key);
    }

    let output = cmd.output().expect("spawn printenv");
    String::from_utf8_lossy(&output.stdout).trim_end().to_string()
}

#[test]
#[cfg(unix)]
fn host_passthrough_gui_off_strips_display() {
    let policy = policy(false, ExecutionBackendKind::HostPassthrough);
    let prepared = prepare_agent_command("printenv".into(), vec![], "/tmp", &policy);
    assert!(matches!(
        prepared.backend,
        ExecutionBackendKind::HostPassthrough
    ));
    // env_unset must include every GUI key.
    for key in gui_env_keys() {
        assert!(
            prepared.env_unset.iter().any(|k| k == key),
            "env_unset missing {key}"
        );
    }
    let stdout = run_prepared_printenv(&prepared, "DISPLAY");
    assert_eq!(
        stdout, "",
        "DISPLAY should be empty after env-strip; got {stdout:?}"
    );
}

#[test]
#[cfg(unix)]
fn host_passthrough_gui_on_preserves_display() {
    let policy = policy(true, ExecutionBackendKind::HostPassthrough);
    let prepared = prepare_agent_command("printenv".into(), vec![], "/tmp", &policy);
    assert!(prepared.env_unset.is_empty());
    let stdout = run_prepared_printenv(&prepared, "DISPLAY");
    assert_eq!(
        stdout, "fake-test:99",
        "DISPLAY should pass through when allow_desktop_gui=true; got {stdout:?}"
    );
}

#[test]
#[cfg(unix)]
fn host_passthrough_gui_off_strips_wayland_and_xauthority() {
    let policy = policy(false, ExecutionBackendKind::HostPassthrough);
    let prepared = prepare_agent_command("printenv".into(), vec![], "/tmp", &policy);
    for key in ["WAYLAND_DISPLAY", "XAUTHORITY"] {
        let stdout = run_prepared_printenv(&prepared, key);
        assert_eq!(
            stdout, "",
            "{key} should be empty after env-strip; got {stdout:?}"
        );
    }
}

/// On non-Linux hosts, the Linux-preference policy falls back to
/// HostPassthrough — but the env_unset must still be populated so macOS and
/// Windows users get display isolation.
#[test]
fn linux_preference_off_platform_still_strips_env() {
    let policy = policy(false, ExecutionBackendKind::LinuxBubblewrap);
    let prepared = prepare_agent_command("true".into(), vec![], "/tmp", &policy);
    if cfg!(target_os = "linux") {
        // On Linux we either get bwrap (env_unset empty, bwrap handles it)
        // or fall back to HostPassthrough (env_unset populated).
        match prepared.backend {
            ExecutionBackendKind::LinuxBubblewrap => {
                assert!(prepared.env_unset.is_empty());
            }
            ExecutionBackendKind::HostPassthrough => {
                assert!(!prepared.env_unset.is_empty());
            }
            other => panic!("unexpected backend: {other:?}"),
        }
    } else {
        // Off-Linux: must fall back and populate env_unset.
        assert!(matches!(
            prepared.backend,
            ExecutionBackendKind::HostPassthrough
        ));
        assert!(!prepared.env_unset.is_empty());
    }
}

#[test]
fn worktree_session_default_is_noop_today() {
    // Regular shell default preserves current behavior: HostPassthrough +
    // GUI allowed + empty env_unset. Important for back-compat.
    let policy = ExecutionPolicy::worktree_session_default();
    let prepared = prepare_agent_command("bash".into(), vec![], "/tmp", &policy);
    assert!(matches!(
        prepared.backend,
        ExecutionBackendKind::HostPassthrough
    ));
    assert!(prepared.env_unset.is_empty());
    assert_eq!(prepared.executable, "bash");
}

#[test]
fn openflow_default_strips_gui_on_every_platform_fallback() {
    // The OpenFlow default has allow_desktop_gui=false. On any platform where
    // the real sandbox isn't available (or on macOS/Windows where the
    // backends are still stubs), env_unset MUST be populated so the strip
    // actually happens at spawn time.
    let policy = ExecutionPolicy::openflow_agent_default();
    let prepared = prepare_agent_command("opencode".into(), vec![], "/tmp", &policy);
    match prepared.backend {
        ExecutionBackendKind::LinuxBubblewrap => {
            // Real bwrap path: env handled by --unsetenv, env_unset empty.
            assert!(prepared.env_unset.is_empty());
        }
        ExecutionBackendKind::HostPassthrough => {
            // Fallback: strip list must cover every GUI key.
            for key in gui_env_keys() {
                assert!(
                    prepared.env_unset.iter().any(|k| k == key),
                    "env_unset missing {key} on HostPassthrough fallback"
                );
            }
        }
        other => panic!("unexpected effective backend: {other:?}"),
    }
}
