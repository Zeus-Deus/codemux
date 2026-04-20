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
    gui_env_keys, gui_env_overrides, prepare_agent_command, ExecutionBackendKind, ExecutionPolicy,
};
use std::collections::HashSet;
use std::process::Command;
use std::sync::{Mutex, OnceLock};

/// Shared guard for tests that mutate `CODEMUX_ALLOW_DESKTOP_GUI` — Rust's
/// default test harness parallelizes across tests, and env mutation is
/// process-global. Any test that reads or writes that key must take this lock
/// for the full duration of its read/compute/restore sequence.
fn env_mutation_guard() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// RAII save/restore for a single env var. Dropping restores the prior value
/// (or removes the key if it was unset before).
struct EnvVarGuard {
    key: &'static str,
    prev: Option<String>,
}

impl EnvVarGuard {
    fn save(key: &'static str) -> Self {
        let prev = std::env::var(key).ok();
        Self { key, prev }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: same rationale as the set/remove calls in the tests below —
        // env mutation is unsafe in Rust 2024; callers serialize via
        // `env_mutation_guard()`.
        unsafe {
            match &self.prev {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

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
fn worktree_session_default_allows_gui_for_human_persona() {
    // As of the persona-based split, `worktree_session_default()` returns
    // the Human-persona policy — the pane the user just opened in their own
    // terminal. That policy inherits the host DISPLAY/WAYLAND_DISPLAY so
    // `npm run tauri dev`, `firefox`, etc. work normally.
    //
    // Agent-persona panes (Claude/Codex/OpenCode presets) go through
    // `worktree_session_default_for_persona(Persona::Agent)` instead, which
    // strips GUI env. The split is tested end-to-end in
    // `tests/persona_execution.rs`.
    let _lock = env_mutation_guard().lock().unwrap_or_else(|e| e.into_inner());
    let restore = EnvVarGuard::save("CODEMUX_ALLOW_DESKTOP_GUI");
    // SAFETY: serialized via env_mutation_guard; restored by `restore` Drop.
    unsafe {
        std::env::remove_var("CODEMUX_ALLOW_DESKTOP_GUI");
    }

    let policy = ExecutionPolicy::worktree_session_default();
    let prepared = prepare_agent_command("bash".into(), vec![], "/tmp", &policy);
    assert!(matches!(
        prepared.backend,
        ExecutionBackendKind::HostPassthrough
    ));
    assert!(
        prepared.env_unset.is_empty(),
        "Human default must NOT strip GUI env — user is driving the keystrokes"
    );
    assert!(
        prepared.env_set.is_empty(),
        "Human default must NOT inject neutralizers — they'd break dbus / xdg-open"
    );
    assert_eq!(prepared.executable, "bash");
    drop(restore);
}

#[test]
fn openflow_default_strips_gui_on_every_platform_fallback() {
    // The OpenFlow default has allow_desktop_gui=false. There are four
    // possible resolved backends depending on host OS + bwrap availability:
    //
    //   Linux + bwrap installed   -> LinuxBubblewrap (env stripped via
    //                                 --unsetenv inside bwrap; env_unset
    //                                 stays empty)
    //   Linux - bwrap missing     -> HostPassthrough (env_unset populated)
    //   macOS                     -> MacOsSandbox stub (env_unset populated)
    //   Windows                   -> WindowsRestricted stub (env_unset populated)
    //
    // The invariant: except for the real-bwrap branch, every backend MUST
    // populate env_unset so `allow_desktop_gui: false` is actually enforced
    // at spawn time. Previously this test only handled the Linux cases and
    // panicked on Windows CI with "unexpected effective backend:
    // WindowsRestricted" when the runtime behavior was actually correct.
    let policy = ExecutionPolicy::openflow_agent_default();
    let prepared = prepare_agent_command("opencode".into(), vec![], "/tmp", &policy);

    let is_real_bwrap = matches!(prepared.backend, ExecutionBackendKind::LinuxBubblewrap);
    if is_real_bwrap {
        assert!(
            prepared.env_unset.is_empty(),
            "bwrap handles env strip via --unsetenv; env_unset should be empty"
        );
    } else {
        for key in gui_env_keys() {
            assert!(
                prepared.env_unset.iter().any(|k| k == key),
                "env_unset missing {key} on {:?} backend — GUI env would leak",
                prepared.backend
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Edge-case / regression tests for the extended GUI-env isolation.
// ---------------------------------------------------------------------------

#[test]
fn gui_env_keys_is_deduplicated() {
    let keys = gui_env_keys();
    let unique: HashSet<&&str> = keys.iter().collect();
    assert_eq!(
        keys.len(),
        unique.len(),
        "gui_env_keys() contains duplicates: {keys:?}"
    );
}

#[test]
fn gui_env_keys_all_uppercase_ascii() {
    for key in gui_env_keys() {
        assert!(
            !key.is_empty(),
            "gui_env_keys() contains an empty string"
        );
        for ch in key.chars() {
            assert!(
                ch.is_ascii_uppercase() || ch == '_' || ch.is_ascii_digit(),
                "key {key:?} contains non-uppercase/underscore char {ch:?}"
            );
        }
    }
}

#[test]
fn gui_env_overrides_has_no_duplicate_keys() {
    let overrides = gui_env_overrides();
    let mut seen = HashSet::new();
    for (k, _) in overrides {
        assert!(
            seen.insert(*k),
            "gui_env_overrides() has duplicate key {k:?}"
        );
    }
}

#[test]
fn bwrap_fallback_applies_env_set() {
    // HostPassthrough with gui forbidden is the same code path taken when
    // bwrap is requested but the binary is missing — both populate env_set
    // from gui_env_overrides(). Exercise it directly so the regression is
    // caught on any host.
    let policy = ExecutionPolicy {
        backend_preference: ExecutionBackendKind::HostPassthrough,
        allow_network: true,
        allow_browser_automation: true,
        allow_desktop_gui: false,
        virtual_display: false,
    };
    let prepared = prepare_agent_command("echo".into(), vec![], "/tmp", &policy);
    assert!(
        !prepared.env_set.is_empty(),
        "HostPassthrough fallback must populate env_set when gui is forbidden"
    );
    let keys: HashSet<&str> = prepared.env_set.iter().map(|(k, _)| k.as_str()).collect();
    for (k, _) in gui_env_overrides() {
        assert!(
            keys.contains(k),
            "env_set missing neutralizer key {k} on fallback path"
        );
    }
}

#[test]
fn allow_desktop_gui_opt_in_empty_string_is_none_fallback() {
    // Three-state semantics: `""` is neither a recognized allow token
    // (`1`/`true`/`yes`) nor a recognized deny token (`0`/`false`/`no`).
    // It falls through to the persona default — Human → allow.
    let _g = env_mutation_guard()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _restore = EnvVarGuard::save("CODEMUX_ALLOW_DESKTOP_GUI");
    // SAFETY: env mutation guarded by env_mutation_guard + restored via Drop.
    unsafe {
        std::env::set_var("CODEMUX_ALLOW_DESKTOP_GUI", "");
    }
    let p = ExecutionPolicy::worktree_session_default();
    assert!(
        p.allow_desktop_gui,
        "empty CODEMUX_ALLOW_DESKTOP_GUI must fall through to persona default (Human → allow)"
    );
}

#[test]
fn allow_desktop_gui_opt_in_arbitrary_value_is_none_fallback() {
    let _g = env_mutation_guard()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _restore = EnvVarGuard::save("CODEMUX_ALLOW_DESKTOP_GUI");
    unsafe {
        std::env::set_var("CODEMUX_ALLOW_DESKTOP_GUI", "banana");
    }
    let p = ExecutionPolicy::worktree_session_default();
    assert!(
        p.allow_desktop_gui,
        "unrecognized token must fall through to persona default — typos shouldn't lock users out"
    );
}

#[test]
fn allow_desktop_gui_opt_in_case_sensitive() {
    // Documents current behavior: the matcher is case-sensitive, so an
    // uppercase `TRUE` is neither an allow token nor a deny token — it
    // falls through to the persona default. The worktree default is
    // Human persona, so the effect is "GUI allowed". If a future
    // refactor makes the matcher case-insensitive, flip this assertion
    // intentionally — the test exists to surface the change in review.
    let _g = env_mutation_guard()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _restore = EnvVarGuard::save("CODEMUX_ALLOW_DESKTOP_GUI");
    unsafe {
        std::env::set_var("CODEMUX_ALLOW_DESKTOP_GUI", "TRUE");
    }
    let p = ExecutionPolicy::worktree_session_default();
    assert!(
        p.allow_desktop_gui,
        "CODEMUX_ALLOW_DESKTOP_GUI matcher is case-sensitive; uppercase TRUE falls through \
         to the persona default (Human → allow). If this assertion is surprising, the matcher \
         has been changed to accept mixed case."
    );
}

#[test]
fn env_unset_contains_every_gui_key_when_forbidden() {
    let policy = ExecutionPolicy {
        backend_preference: ExecutionBackendKind::HostPassthrough,
        allow_network: true,
        allow_browser_automation: true,
        allow_desktop_gui: false,
        virtual_display: false,
    };
    let prepared = prepare_agent_command("echo".into(), vec![], "/tmp", &policy);
    assert_eq!(
        prepared.env_unset.len(),
        gui_env_keys().len(),
        "env_unset size must match gui_env_keys(); drift means one key is skipped"
    );
    let unset: HashSet<&str> = prepared.env_unset.iter().map(|s| s.as_str()).collect();
    for key in gui_env_keys() {
        assert!(
            unset.contains(*key),
            "env_unset missing {key} — strip will leak this var to children"
        );
    }
}

#[test]
fn env_set_exactly_matches_gui_env_overrides_when_forbidden() {
    let policy = ExecutionPolicy {
        backend_preference: ExecutionBackendKind::HostPassthrough,
        allow_network: true,
        allow_browser_automation: true,
        allow_desktop_gui: false,
        virtual_display: false,
    };
    let prepared = prepare_agent_command("echo".into(), vec![], "/tmp", &policy);
    let got: HashSet<(String, String)> = prepared.env_set.iter().cloned().collect();
    let want: HashSet<(String, String)> = gui_env_overrides()
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    assert_eq!(got, want, "env_set must exactly match gui_env_overrides()");
}

#[test]
fn openflow_default_policy_also_has_env_set() {
    // Regression: when the new env_set field was added, the OpenFlow code path
    // needed to populate it too — not only the worktree path. Cover the
    // non-bwrap backends (the bwrap path emits --setenv directly and leaves
    // env_set empty, which is correct).
    let policy = ExecutionPolicy::openflow_agent_default();
    let prepared = prepare_agent_command("opencode".into(), vec![], "/tmp", &policy);
    match prepared.backend {
        ExecutionBackendKind::LinuxBubblewrap => {
            assert!(
                prepared.env_set.is_empty(),
                "bwrap path emits --setenv; env_set should stay empty"
            );
        }
        _ => {
            assert!(
                !prepared.env_set.is_empty(),
                "openflow default on {:?} must populate env_set so neutralizers apply",
                prepared.backend
            );
            let keys: HashSet<&str> =
                prepared.env_set.iter().map(|(k, _)| k.as_str()).collect();
            assert!(keys.contains("BROWSER"));
            assert!(keys.contains("DBUS_SESSION_BUS_ADDRESS"));
        }
    }
}

#[test]
fn execution_policy_serde_round_trip_with_new_defaults() {
    // Round-trip the worktree default (Human persona → allow_desktop_gui=true)
    // through serde_json to catch any missed serde attribute on new fields.
    let _g = env_mutation_guard()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _restore = EnvVarGuard::save("CODEMUX_ALLOW_DESKTOP_GUI");
    unsafe {
        std::env::remove_var("CODEMUX_ALLOW_DESKTOP_GUI");
    }
    let p = ExecutionPolicy::worktree_session_default();
    let s = serde_json::to_string(&p).expect("serialize ExecutionPolicy");
    let back: ExecutionPolicy = serde_json::from_str(&s).expect("deserialize ExecutionPolicy");
    assert_eq!(p, back);
    assert!(
        back.allow_desktop_gui,
        "Human default round-trips with allow_desktop_gui=true"
    );
}

#[test]
fn worktree_default_is_stable_without_env_var() {
    let _g = env_mutation_guard()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let _restore = EnvVarGuard::save("CODEMUX_ALLOW_DESKTOP_GUI");
    unsafe {
        std::env::remove_var("CODEMUX_ALLOW_DESKTOP_GUI");
    }
    for i in 0..3 {
        let p = ExecutionPolicy::worktree_session_default();
        assert!(
            p.allow_desktop_gui,
            "worktree default iteration {i} is Human persona → allow_desktop_gui=true"
        );
        assert_eq!(p.backend_preference, ExecutionBackendKind::HostPassthrough);
    }
}
