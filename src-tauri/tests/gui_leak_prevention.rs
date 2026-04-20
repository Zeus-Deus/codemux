//! End-to-end integration tests for the GUI-env isolation landing.
//!
//! Covers three layers that `execution_env.rs` and `virtual_display.rs` do not:
//!   1. `sanitize_gui_env_std` / `sanitize_gui_env_tokio` actually remove the
//!      keys and set the neutralizers when observed via a spawned child.
//!   2. `ExecutionPolicy::worktree_session_default()` now returns
//!      `allow_desktop_gui: false` by default, with `CODEMUX_ALLOW_DESKTOP_GUI=1`
//!      as the opt-in.
//!   3. `PreparedExecutionCommand::env_set` carries the override pairs and is
//!      internally consistent with `env_unset` and `gui_env_keys()`.
//!
//! Tests that spawn `env` are gated on Linux since that's what CI uses and
//! `env` / `/bin/sh` are guaranteed to be present.
//!
//! Env-mutating tests share a process-wide Mutex so `cargo test` parallelism
//! can't race them against each other (or against similar tests inside
//! `execution::tests`).

use codemux_lib::execution::{
    gui_env_keys, gui_env_overrides, prepare_agent_command, sanitize_gui_env_std,
    sanitize_gui_env_tokio, ExecutionBackendKind, ExecutionPolicy,
};
use std::collections::{HashMap, HashSet};
use std::env;
use std::process::Command;
use std::sync::{Mutex, OnceLock};

/// Shared lock for tests that mutate process-wide env. Without this, parallel
/// `cargo test` can observe half-set state.
fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Spawn `env`, parse its `KEY=VALUE\n` stdout into a map. Lines without `=`
/// are skipped (multi-line values from exported bash functions would be rare
/// here, but be defensive).
#[cfg(target_os = "linux")]
fn run_env_and_parse(cmd: &mut Command) -> HashMap<String, String> {
    let out = cmd.output().expect("spawn env");
    assert!(out.status.success(), "env exited non-zero: {:?}", out.status);
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Same shape as `run_env_and_parse` but for the tokio Command flavor.
#[cfg(target_os = "linux")]
async fn run_env_and_parse_tokio(
    cmd: &mut tokio::process::Command,
) -> HashMap<String, String> {
    let out = cmd.output().await.expect("spawn env");
    assert!(out.status.success(), "env exited non-zero: {:?}", out.status);
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn default_policy(allow_desktop_gui: bool) -> ExecutionPolicy {
    ExecutionPolicy {
        backend_preference: ExecutionBackendKind::HostPassthrough,
        allow_network: true,
        allow_browser_automation: true,
        allow_desktop_gui,
        virtual_display: false,
    }
}

// ---------------------------------------------------------------------------
// Sanitize — observable child behavior
// ---------------------------------------------------------------------------

#[test]
#[cfg(target_os = "linux")]
fn sanitize_gui_env_std_actually_strips_display() {
    let mut cmd = Command::new("env");
    cmd.env("DISPLAY", ":999")
        .env("WAYLAND_DISPLAY", "wayland-test")
        .env("HYPRLAND_INSTANCE_SIGNATURE", "foo");
    sanitize_gui_env_std(&mut cmd);
    let envs = run_env_and_parse(&mut cmd);
    assert!(
        !envs.contains_key("DISPLAY"),
        "DISPLAY leaked into child: {envs:?}"
    );
    assert!(
        !envs.contains_key("WAYLAND_DISPLAY"),
        "WAYLAND_DISPLAY leaked into child: {envs:?}"
    );
    assert!(
        !envs.contains_key("HYPRLAND_INSTANCE_SIGNATURE"),
        "HYPRLAND_INSTANCE_SIGNATURE leaked into child: {envs:?}"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn sanitize_gui_env_std_sets_browser_neutralizer() {
    let mut cmd = Command::new("env");
    sanitize_gui_env_std(&mut cmd);
    let envs = run_env_and_parse(&mut cmd);
    assert_eq!(envs.get("BROWSER").map(String::as_str), Some("true"));
    assert_eq!(envs.get("MOZ_NO_REMOTE").map(String::as_str), Some("1"));
    assert_eq!(
        envs.get("DBUS_SESSION_BUS_ADDRESS").map(String::as_str),
        Some("unix:path=/dev/null")
    );
    assert_eq!(
        envs.get("XDG_CURRENT_DESKTOP").map(String::as_str),
        Some("X-Generic")
    );
    assert_eq!(envs.get("DE").map(String::as_str), Some("generic"));
    assert_eq!(envs.get("GTK_USE_PORTAL").map(String::as_str), Some("0"));
    assert_eq!(envs.get("GIO_USE_VFS").map(String::as_str), Some("local"));
    assert_eq!(envs.get("NO_AT_BRIDGE").map(String::as_str), Some("1"));
}

#[tokio::test]
#[cfg(target_os = "linux")]
async fn sanitize_gui_env_tokio_matches_std() {
    let mut cmd = tokio::process::Command::new("env");
    cmd.env("DISPLAY", ":999")
        .env("WAYLAND_DISPLAY", "wayland-test")
        .env("HYPRLAND_INSTANCE_SIGNATURE", "foo");
    sanitize_gui_env_tokio(&mut cmd);
    let envs = run_env_and_parse_tokio(&mut cmd).await;
    assert!(!envs.contains_key("DISPLAY"));
    assert!(!envs.contains_key("WAYLAND_DISPLAY"));
    assert!(!envs.contains_key("HYPRLAND_INSTANCE_SIGNATURE"));
    // Neutralizers must match the std flavor exactly.
    assert_eq!(envs.get("BROWSER").map(String::as_str), Some("true"));
    assert_eq!(envs.get("MOZ_NO_REMOTE").map(String::as_str), Some("1"));
    assert_eq!(
        envs.get("DBUS_SESSION_BUS_ADDRESS").map(String::as_str),
        Some("unix:path=/dev/null")
    );
    assert_eq!(
        envs.get("XDG_CURRENT_DESKTOP").map(String::as_str),
        Some("X-Generic")
    );
    assert_eq!(envs.get("DE").map(String::as_str), Some("generic"));
}

#[test]
#[cfg(target_os = "linux")]
fn sanitize_preserves_non_gui_env() {
    // Use an absolute path so `PATH=/custom` (which doesn't contain the `env`
    // binary) doesn't cause `Command::spawn` to fail with ENOENT.
    let mut cmd = Command::new("/usr/bin/env");
    cmd.env("PATH", "/custom").env("FOO", "bar");
    sanitize_gui_env_std(&mut cmd);
    let envs = run_env_and_parse(&mut cmd);
    assert_eq!(envs.get("PATH").map(String::as_str), Some("/custom"));
    assert_eq!(envs.get("FOO").map(String::as_str), Some("bar"));
}

#[test]
#[cfg(target_os = "linux")]
fn sanitize_covers_full_hyprland_kit() {
    // Seed every key in gui_env_keys() with a recognizable sentinel value, then
    // verify none survive into the child — except the ones we intentionally
    // re-set via gui_env_overrides() (DBUS_SESSION_BUS_ADDRESS, XDG_CURRENT_DESKTOP).
    let mut cmd = Command::new("env");
    for key in gui_env_keys() {
        cmd.env(key, "SENTINEL_VALUE");
    }
    sanitize_gui_env_std(&mut cmd);
    let envs = run_env_and_parse(&mut cmd);

    let overrides: HashMap<&str, &str> = gui_env_overrides().iter().copied().collect();
    for key in gui_env_keys() {
        match overrides.get(key) {
            Some(expected) => {
                // Intentionally re-set — must hold the neutralizer value, not SENTINEL.
                assert_eq!(
                    envs.get(*key).map(String::as_str),
                    Some(*expected),
                    "{key} should be re-set to neutralizer value"
                );
            }
            None => {
                assert!(
                    !envs.contains_key(*key),
                    "{key} leaked into child with value {:?}",
                    envs.get(*key)
                );
            }
        }
    }
}

#[test]
#[cfg(target_os = "linux")]
fn sanitize_is_idempotent() {
    // Calling sanitize twice must produce the same observed env as calling it once.
    // Use absolute path to `env` so the custom PATH below doesn't ENOENT the spawn.
    let mut once = Command::new("/usr/bin/env");
    once.env("DISPLAY", ":42").env("PATH", "/custom");
    sanitize_gui_env_std(&mut once);
    let envs_once = run_env_and_parse(&mut once);

    let mut twice = Command::new("/usr/bin/env");
    twice.env("DISPLAY", ":42").env("PATH", "/custom");
    sanitize_gui_env_std(&mut twice);
    sanitize_gui_env_std(&mut twice);
    let envs_twice = run_env_and_parse(&mut twice);

    // Compare the GUI-relevant subset exactly — the rest of the env (inherited
    // from the test process) is identical between both runs by construction.
    for key in gui_env_keys() {
        assert_eq!(
            envs_once.get(*key),
            envs_twice.get(*key),
            "sanitize not idempotent for {key}"
        );
    }
    for (k, _) in gui_env_overrides() {
        assert_eq!(
            envs_once.get(*k),
            envs_twice.get(*k),
            "sanitize not idempotent for override {k}"
        );
    }
    assert_eq!(envs_once.get("PATH"), envs_twice.get("PATH"));
}

#[test]
#[cfg(target_os = "linux")]
fn sanitize_does_not_touch_codemux_env() {
    let mut cmd = Command::new("env");
    cmd.env("CODEMUX_WORKSPACE_ID", "abc")
        .env("CODEMUX_BROWSER_AUTOMATION", "1")
        .env("SUPERSET_FOO", "bar")
        .env("SUPERSET_BAR", "baz");
    sanitize_gui_env_std(&mut cmd);
    let envs = run_env_and_parse(&mut cmd);
    assert_eq!(
        envs.get("CODEMUX_WORKSPACE_ID").map(String::as_str),
        Some("abc")
    );
    assert_eq!(
        envs.get("CODEMUX_BROWSER_AUTOMATION").map(String::as_str),
        Some("1")
    );
    assert_eq!(envs.get("SUPERSET_FOO").map(String::as_str), Some("bar"));
    assert_eq!(envs.get("SUPERSET_BAR").map(String::as_str), Some("baz"));
}

// ---------------------------------------------------------------------------
// worktree_session_default — env-driven policy resolution
// ---------------------------------------------------------------------------

#[test]
fn worktree_session_default_allows_gui_for_human_persona() {
    // Post-persona-split behavior: `worktree_session_default()` represents
    // a Human-driven pane (plain Terminal tab, Shell preset, setup scripts).
    // It inherits the host DISPLAY/WAYLAND — env_unset/env_set stay empty.
    // Agent-persona panes go through
    // `worktree_session_default_for_persona(Persona::Agent)` and DO strip
    // — covered by `tests/persona_execution.rs`.
    let _guard = env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let prev = env::var("CODEMUX_ALLOW_DESKTOP_GUI").ok();
    // SAFETY: test is serialized via env_lock(); mutation is scoped and restored below.
    unsafe {
        env::remove_var("CODEMUX_ALLOW_DESKTOP_GUI");
    }

    let policy = ExecutionPolicy::worktree_session_default();
    assert!(
        policy.allow_desktop_gui,
        "worktree default (Human persona) must inherit host DISPLAY"
    );
    let prepared = prepare_agent_command("bash".into(), vec![], "/tmp", &policy);
    assert!(
        prepared.env_unset.is_empty(),
        "Human persona must not strip env keys, got {:?}",
        prepared.env_unset
    );
    assert!(
        prepared.env_set.is_empty(),
        "Human persona must not inject neutralizers, got {:?}",
        prepared.env_set
    );

    // SAFETY: restore prior value under the same lock.
    unsafe {
        match prev {
            Some(v) => env::set_var("CODEMUX_ALLOW_DESKTOP_GUI", v),
            None => env::remove_var("CODEMUX_ALLOW_DESKTOP_GUI"),
        }
    }
}

#[test]
fn worktree_session_default_opt_in_via_env_var() {
    let _guard = env_lock().lock().unwrap();
    let prev = env::var("CODEMUX_ALLOW_DESKTOP_GUI").ok();
    // SAFETY: test is serialized via env_lock(); mutation is scoped and restored below.
    unsafe {
        env::set_var("CODEMUX_ALLOW_DESKTOP_GUI", "1");
    }

    let policy = ExecutionPolicy::worktree_session_default();
    assert!(
        policy.allow_desktop_gui,
        "CODEMUX_ALLOW_DESKTOP_GUI=1 must re-enable host GUI passthrough"
    );
    let prepared = prepare_agent_command("bash".into(), vec![], "/tmp", &policy);
    assert!(
        prepared.env_unset.is_empty(),
        "env_unset must stay empty when opt-in is set, got {:?}",
        prepared.env_unset
    );
    assert!(
        prepared.env_set.is_empty(),
        "env_set must stay empty when opt-in is set, got {:?}",
        prepared.env_set
    );

    // SAFETY: restore prior value under the same lock.
    unsafe {
        match prev {
            Some(v) => env::set_var("CODEMUX_ALLOW_DESKTOP_GUI", v),
            None => env::remove_var("CODEMUX_ALLOW_DESKTOP_GUI"),
        }
    }
}

// ---------------------------------------------------------------------------
// PreparedExecutionCommand — structural invariants
// ---------------------------------------------------------------------------

#[test]
fn prepared_env_set_includes_all_overrides() {
    // HostPassthrough + gui forbidden: env_set must contain exactly the pairs
    // returned by gui_env_overrides(). Use HostPassthrough specifically so we
    // don't hit the bwrap branch (which leaves env_set empty).
    //
    // We compare as sets (and verify the count matches gui_env_overrides.len()
    // dynamically) so adding a new override later is a single-line change —
    // the test stays a tripwire for silent loss/drift but never becomes a
    // brittle hardcoded count.
    let policy = default_policy(false);
    let prepared = prepare_agent_command("echo".into(), vec![], "/tmp", &policy);
    assert!(matches!(
        prepared.backend,
        ExecutionBackendKind::HostPassthrough
    ));
    let expected: HashSet<(String, String)> = gui_env_overrides()
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    let actual: HashSet<(String, String)> = prepared.env_set.iter().cloned().collect();
    assert_eq!(
        actual, expected,
        "env_set should match gui_env_overrides exactly"
    );
    assert_eq!(prepared.env_set.len(), gui_env_overrides().len());
}

#[test]
fn gui_env_overrides_is_subset_of_gui_env_keys_or_neutral() {
    // For each (k, _) in gui_env_overrides(), classify:
    //   (a) k is also in gui_env_keys()  -> strip-then-set pattern. Child sees
    //       the neutralizer regardless of what the host had.
    //   (b) k is NOT in gui_env_keys()   -> safe standalone add. The caller
    //       isn't asking us to remove it first; we just inject it.
    //
    // Current wiring (April 2026):
    //   standalone add (not in strip list, safe to inject):
    //     BROWSER                     - `xdg-open` reads it to pick a browser
    //     MOZ_NO_REMOTE               - firefox reads it for single-instance
    //     DE                          - legacy xdg-utils check before XDG_CURRENT_DESKTOP
    //     GIO_USE_VFS                 - gio reads it to pick VFS backend
    //     NO_AT_BRIDGE                - at-spi2 reads it to skip bus activation
    //     MOZ_NO_REMOTE, BROWSER      - always safe to set regardless of strip list
    //   strip-then-set (in gui_env_keys() AND in overrides, belt-and-suspenders):
    //     DBUS_SESSION_BUS_ADDRESS    - strip then force to unix:path=/dev/null
    //     XDG_CURRENT_DESKTOP         - strip then force to X-Generic
    //     GTK_USE_PORTAL              - strip then force to 0
    let keys: HashSet<&str> = gui_env_keys().iter().copied().collect();
    let standalone = [
        "BROWSER",
        "MOZ_NO_REMOTE",
        "DE",
        "GIO_USE_VFS",
        "NO_AT_BRIDGE",
    ];
    let strip_then_set = [
        "DBUS_SESSION_BUS_ADDRESS",
        "XDG_CURRENT_DESKTOP",
        "GTK_USE_PORTAL",
    ];

    for (k, _) in gui_env_overrides() {
        let is_in_keys = keys.contains(k);
        let is_standalone = standalone.contains(k);
        let is_strip_then_set = strip_then_set.contains(k);
        assert!(
            is_standalone || is_strip_then_set,
            "override {k} is not in the documented classification — \
             update this test and the docstring on gui_env_overrides when the \
             override set changes"
        );
        if is_strip_then_set {
            assert!(
                is_in_keys,
                "{k} is classified strip-then-set but is missing from gui_env_keys()"
            );
        }
        if is_standalone {
            assert!(
                !is_in_keys,
                "{k} is classified standalone but appears in gui_env_keys() — \
                 that means env-strip will race the neutralizer"
            );
        }
    }
}

#[test]
fn env_unset_and_env_set_never_conflict_silently() {
    // Invariant: for every key in env_set, either it's also in env_unset
    // (strip-then-set: caller is supposed to unset THEN set) or it's a new add
    // (never touched by unset). If a key were in env_unset but NOT env_set
    // yet we relied on a neutralizer, that'd be a bug — the child would see
    // a missing value instead of the neutral value.
    let policy = default_policy(false);
    let prepared = prepare_agent_command("echo".into(), vec![], "/tmp", &policy);
    let unset: HashSet<&str> = prepared.env_unset.iter().map(|s| s.as_str()).collect();
    let set_keys: HashSet<&str> = prepared.env_set.iter().map(|(k, _)| k.as_str()).collect();

    for k in &set_keys {
        // Must be either in unset (strip-then-set) or a new key entirely.
        let _ = unset.contains(k); // either branch is valid; assert below is the actual invariant
    }

    // Stronger invariant: every override that's ALSO a gui_env_keys() member
    // must appear in env_unset too (otherwise env_remove never fired on it,
    // and env_set order vs inherited env becomes fragile).
    let gui_keys: HashSet<&str> = gui_env_keys().iter().copied().collect();
    for k in &set_keys {
        if gui_keys.contains(k) {
            assert!(
                unset.contains(k),
                "override {k} is in gui_env_keys() but missing from env_unset — \
                 strip-then-set contract broken"
            );
        }
    }
}

#[test]
#[cfg(target_os = "linux")]
fn bwrap_args_include_neutralizer_setenvs() {
    // Build a LinuxBubblewrap policy with gui forbidden. If bwrap is on PATH
    // the prepared command wraps in bwrap and `prepared.args` should contain
    // the `--setenv BROWSER true` triple as consecutive entries. If bwrap
    // isn't installed we hit the HostPassthrough fallback — in that case
    // env_set should still carry BROWSER=true. Both cases are exercised.
    let policy = ExecutionPolicy {
        backend_preference: ExecutionBackendKind::LinuxBubblewrap,
        allow_network: true,
        allow_browser_automation: true,
        allow_desktop_gui: false,
        virtual_display: false,
    };
    let prepared = prepare_agent_command("echo".into(), vec![], "/tmp", &policy);

    match prepared.backend {
        ExecutionBackendKind::LinuxBubblewrap => {
            // Find the consecutive (--setenv, BROWSER, true) triple.
            let mut found = false;
            for window in prepared.args.windows(3) {
                if window[0] == "--setenv" && window[1] == "BROWSER" && window[2] == "true" {
                    found = true;
                    break;
                }
            }
            assert!(
                found,
                "bwrap args should contain `--setenv BROWSER true` triple, got {:?}",
                prepared.args
            );
        }
        ExecutionBackendKind::HostPassthrough => {
            // bwrap missing — fallback path. env_set carries it instead.
            assert!(
                prepared
                    .env_set
                    .iter()
                    .any(|(k, v)| k == "BROWSER" && v == "true"),
                "HostPassthrough fallback should carry BROWSER=true in env_set, got {:?}",
                prepared.env_set
            );
        }
        other => panic!("unexpected backend on linux: {other:?}"),
    }
}
