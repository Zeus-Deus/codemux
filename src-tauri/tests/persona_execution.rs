//! End-to-end integration tests for the persona-based display-isolation split.
//!
//! Covers the wiring between `presets::Persona`, `ExecutionPolicy::
//! worktree_session_default_for_persona`, and `prepare_agent_command`.
//! Validates that a Human-persona pane keeps `DISPLAY` / `WAYLAND_DISPLAY`
//! / `HYPRLAND_INSTANCE_SIGNATURE` in the child env (so users can run GUI
//! apps from a plain terminal) while an Agent-persona pane strips them
//! (so AI-driven tool calls can't pop windows on the real desktop).
//!
//! Also covers the three-state `CODEMUX_ALLOW_DESKTOP_GUI` override that
//! forces allow (1/true/yes) or force-denies (0/false/no) regardless of
//! persona.
//!
//! Env-mutating tests share a process-wide Mutex so `cargo test`
//! parallelism can't race them against each other. A separate file-level
//! lock (the `persona_env_lock` below) coexists with the one in
//! `gui_leak_prevention.rs` — they run in the same test binary on some
//! targets and on different binaries on others, so each file owns its
//! own lock and the worst-case is over-serialization, never a race.

use codemux_lib::execution::{
    gui_env_keys, gui_env_overrides, prepare_agent_command, ExecutionPolicy,
};
use codemux_lib::presets::Persona;
use std::collections::HashMap;
use std::env;
use std::process::Command;
use std::sync::{Mutex, OnceLock};

fn persona_env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// RAII: snapshot env var on construct, restore on drop. Used to keep
/// cargo-test parallelism and earlier test state from bleeding into
/// each test.
struct EnvRestore {
    key: &'static str,
    prior: Option<String>,
}

impl EnvRestore {
    fn take(key: &'static str) -> Self {
        Self {
            key,
            prior: env::var(key).ok(),
        }
    }
}

impl Drop for EnvRestore {
    fn drop(&mut self) {
        // SAFETY: serialized by `persona_env_lock` held by the caller;
        // restores the value we captured in `take()`.
        unsafe {
            match &self.prior {
                Some(v) => env::set_var(self.key, v),
                None => env::remove_var(self.key),
            }
        }
    }
}

/// Spawn `env` and parse KEY=VALUE lines. Linux-only because CI uses Linux
/// and every CI runner has `env` / `/bin/sh`.
#[cfg(target_os = "linux")]
fn run_env_and_parse(cmd: &mut Command) -> HashMap<String, String> {
    let out = cmd.output().expect("spawn env");
    assert!(
        out.status.success(),
        "env exited non-zero: {:?}",
        out.status
    );
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Re-implement the spawn-time env hygiene that `spawn_pty_for_session`
/// performs: apply env_unset, then env_set, on a `std::process::Command`.
/// Kept here (rather than exported from the lib) so the test can be
/// read top-to-bottom without cross-file tracing — it's ~6 lines.
fn apply_prepared_to_command(
    cmd: &mut Command,
    prepared: &codemux_lib::execution::PreparedExecutionCommand,
) {
    for key in &prepared.env_unset {
        cmd.env_remove(key);
    }
    for (key, val) in &prepared.env_set {
        cmd.env(key, val);
    }
}

// ---------------------------------------------------------------------------
// Persona → policy mapping (unit-level, cheap)
// ---------------------------------------------------------------------------

#[test]
fn human_persona_policy_allows_gui() {
    let _lock = persona_env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let _gui = EnvRestore::take("CODEMUX_ALLOW_DESKTOP_GUI");
    let _vd = EnvRestore::take("CODEMUX_VIRTUAL_DISPLAY");
    // SAFETY: serialized by lock; restored by _gui / _vd Drop.
    unsafe {
        env::remove_var("CODEMUX_ALLOW_DESKTOP_GUI");
        env::remove_var("CODEMUX_VIRTUAL_DISPLAY");
    }
    let p = ExecutionPolicy::worktree_session_default_for_persona(Persona::Human);
    assert!(p.allow_desktop_gui);
    assert!(!p.virtual_display);
}

#[test]
fn agent_persona_policy_allows_gui_by_default() {
    // Both personas default to full desktop env. Env-stripping is structurally
    // leaky on Linux (auto-discovery via XDG_RUNTIME_DIR) and broke legitimate
    // clipboard tools like wl-paste/xclip — so Agent persona now matches other
    // ADEs and inherits the user's desktop. Isolation remains available via
    // CODEMUX_ALLOW_DESKTOP_GUI=0 (see env_override_force_deny_* tests below)
    // or per-workspace `.codemux/config.json`.
    let _lock = persona_env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let _gui = EnvRestore::take("CODEMUX_ALLOW_DESKTOP_GUI");
    let _vd = EnvRestore::take("CODEMUX_VIRTUAL_DISPLAY");
    unsafe {
        env::remove_var("CODEMUX_ALLOW_DESKTOP_GUI");
        env::remove_var("CODEMUX_VIRTUAL_DISPLAY");
    }
    let p = ExecutionPolicy::worktree_session_default_for_persona(Persona::Agent);
    assert!(p.allow_desktop_gui);
    assert!(!p.virtual_display);
}

// ---------------------------------------------------------------------------
// Prepared command: env_unset / env_set match persona
// ---------------------------------------------------------------------------

#[test]
fn human_persona_prepared_command_has_empty_env_strip() {
    let _lock = persona_env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let _gui = EnvRestore::take("CODEMUX_ALLOW_DESKTOP_GUI");
    let _vd = EnvRestore::take("CODEMUX_VIRTUAL_DISPLAY");
    unsafe {
        env::remove_var("CODEMUX_ALLOW_DESKTOP_GUI");
        env::remove_var("CODEMUX_VIRTUAL_DISPLAY");
    }
    let policy = ExecutionPolicy::worktree_session_default_for_persona(Persona::Human);
    let prep = prepare_agent_command("echo".into(), vec![], "/tmp", &policy);
    assert!(
        prep.env_unset.is_empty(),
        "human persona must not strip any env keys (user typed the command themselves)"
    );
    assert!(prep.env_set.is_empty());
}

#[test]
fn agent_persona_prepared_command_leaves_env_alone_by_default() {
    // New default: Agent persona inherits full env — env_unset/env_set must
    // both be empty. The opt-in isolation path (CODEMUX_ALLOW_DESKTOP_GUI=0)
    // is covered separately by `env_override_force_deny_populates_env_strip`.
    let _lock = persona_env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let _gui = EnvRestore::take("CODEMUX_ALLOW_DESKTOP_GUI");
    let _vd = EnvRestore::take("CODEMUX_VIRTUAL_DISPLAY");
    unsafe {
        env::remove_var("CODEMUX_ALLOW_DESKTOP_GUI");
        env::remove_var("CODEMUX_VIRTUAL_DISPLAY");
    }
    let policy = ExecutionPolicy::worktree_session_default_for_persona(Persona::Agent);
    let prep = prepare_agent_command("echo".into(), vec![], "/tmp", &policy);
    assert!(
        prep.env_unset.is_empty(),
        "agent persona default must NOT strip any env keys — isolation is opt-in"
    );
    assert!(prep.env_set.is_empty());
}

#[test]
fn agent_persona_opt_in_isolation_strips_gui_keys() {
    // When a user explicitly opts into isolation via CODEMUX_ALLOW_DESKTOP_GUI=0,
    // every `gui_env_keys()` must land in env_unset and every
    // `gui_env_overrides()` neutralizer must land in env_set.
    let _lock = persona_env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let _gui = EnvRestore::take("CODEMUX_ALLOW_DESKTOP_GUI");
    let _vd = EnvRestore::take("CODEMUX_VIRTUAL_DISPLAY");
    unsafe {
        env::set_var("CODEMUX_ALLOW_DESKTOP_GUI", "0");
        env::remove_var("CODEMUX_VIRTUAL_DISPLAY");
    }
    let policy = ExecutionPolicy::worktree_session_default_for_persona(Persona::Agent);
    let prep = prepare_agent_command("echo".into(), vec![], "/tmp", &policy);
    for key in gui_env_keys() {
        assert!(
            prep.env_unset.iter().any(|k| k == key),
            "opt-in isolation must include {key} in env_unset"
        );
    }
    for (k, v) in gui_env_overrides() {
        let actual = prep
            .env_set
            .iter()
            .find(|(pk, _)| pk == k)
            .unwrap_or_else(|| panic!("opt-in isolation must set neutralizer {k}"));
        assert_eq!(actual.1, *v, "opt-in isolation must set {k}={v}");
    }
}

// ---------------------------------------------------------------------------
// Observable child behavior — spawn `env`, inspect its environment
// ---------------------------------------------------------------------------

/// The guarantee a Human-persona user shell must uphold: if the parent
/// Codemux process has `DISPLAY` set, children spawned by a Human-persona
/// pane see it unchanged. This is the whole point of the split — users
/// running `npm run tauri dev` from their own terminal must get a window.
#[test]
#[cfg(target_os = "linux")]
fn human_persona_child_inherits_display() {
    let _lock = persona_env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let _gui = EnvRestore::take("CODEMUX_ALLOW_DESKTOP_GUI");
    let _vd = EnvRestore::take("CODEMUX_VIRTUAL_DISPLAY");
    unsafe {
        env::remove_var("CODEMUX_ALLOW_DESKTOP_GUI");
        env::remove_var("CODEMUX_VIRTUAL_DISPLAY");
    }
    let policy = ExecutionPolicy::worktree_session_default_for_persona(Persona::Human);
    let prep = prepare_agent_command("env".into(), vec![], "/tmp", &policy);

    let mut cmd = Command::new("env");
    // Pretend the parent Codemux process has a display. We set these as
    // explicit child env so the test is independent of whether the CI
    // runner actually has them on its PATH-level env. The policy's
    // env_unset would strip them if the policy says to — which for
    // Human persona it doesn't. That's what we're proving.
    cmd.env("DISPLAY", ":42");
    cmd.env("WAYLAND_DISPLAY", "wayland-42");
    cmd.env("HYPRLAND_INSTANCE_SIGNATURE", "fake-sig");
    apply_prepared_to_command(&mut cmd, &prep);

    let env_map = run_env_and_parse(&mut cmd);
    assert_eq!(
        env_map.get("DISPLAY").map(String::as_str),
        Some(":42"),
        "Human persona must pass DISPLAY through to child"
    );
    assert_eq!(
        env_map.get("WAYLAND_DISPLAY").map(String::as_str),
        Some("wayland-42"),
        "Human persona must pass WAYLAND_DISPLAY through to child"
    );
    assert_eq!(
        env_map.get("HYPRLAND_INSTANCE_SIGNATURE").map(String::as_str),
        Some("fake-sig")
    );
    // Neutralizers must NOT be set for Human persona — the user gets a
    // real DBus session, real xdg-open handler, etc.
    assert!(
        env_map.get("DBUS_SESSION_BUS_ADDRESS").map(String::as_str)
            != Some("unix:path=/dev/null"),
        "Human persona must not neutralize DBus session bus"
    );
}

/// Agent persona now inherits the host display by default. Clipboard tools
/// (wl-paste, xclip — which Claude Code's Ctrl+V image paste relies on) need
/// WAYLAND_DISPLAY/DISPLAY to function, and env-stripping never actually
/// blocked GUI pop-ups anyway (socket auto-discovery via XDG_RUNTIME_DIR).
#[test]
#[cfg(target_os = "linux")]
fn agent_persona_child_inherits_display_by_default() {
    let _lock = persona_env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let _gui = EnvRestore::take("CODEMUX_ALLOW_DESKTOP_GUI");
    let _vd = EnvRestore::take("CODEMUX_VIRTUAL_DISPLAY");
    unsafe {
        env::remove_var("CODEMUX_ALLOW_DESKTOP_GUI");
        env::remove_var("CODEMUX_VIRTUAL_DISPLAY");
    }
    let policy = ExecutionPolicy::worktree_session_default_for_persona(Persona::Agent);
    let prep = prepare_agent_command("env".into(), vec![], "/tmp", &policy);

    let mut cmd = Command::new("env");
    cmd.env("DISPLAY", ":42");
    cmd.env("WAYLAND_DISPLAY", "wayland-42");
    cmd.env("HYPRLAND_INSTANCE_SIGNATURE", "fake-sig");
    apply_prepared_to_command(&mut cmd, &prep);

    let env_map = run_env_and_parse(&mut cmd);
    assert_eq!(
        env_map.get("DISPLAY").map(String::as_str),
        Some(":42"),
        "Agent persona default must pass DISPLAY so clipboard tools work"
    );
    assert_eq!(
        env_map.get("WAYLAND_DISPLAY").map(String::as_str),
        Some("wayland-42"),
        "Agent persona default must pass WAYLAND_DISPLAY so wl-paste/Ctrl+V image paste works"
    );
}

/// When a user opts into isolation via CODEMUX_ALLOW_DESKTOP_GUI=0, the Agent
/// persona must strip every GUI key and inject the neutralizers. This proves
/// the opt-in path still works for security-conscious deployments.
#[test]
#[cfg(target_os = "linux")]
fn agent_persona_opt_in_strips_display_from_child() {
    let _lock = persona_env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let _gui = EnvRestore::take("CODEMUX_ALLOW_DESKTOP_GUI");
    let _vd = EnvRestore::take("CODEMUX_VIRTUAL_DISPLAY");
    unsafe {
        env::set_var("CODEMUX_ALLOW_DESKTOP_GUI", "0");
        env::remove_var("CODEMUX_VIRTUAL_DISPLAY");
    }
    let policy = ExecutionPolicy::worktree_session_default_for_persona(Persona::Agent);
    let prep = prepare_agent_command("env".into(), vec![], "/tmp", &policy);

    let mut cmd = Command::new("env");
    cmd.env("DISPLAY", ":42");
    cmd.env("WAYLAND_DISPLAY", "wayland-42");
    cmd.env("HYPRLAND_INSTANCE_SIGNATURE", "fake-sig");
    cmd.env("XDG_CURRENT_DESKTOP", "Hyprland");
    cmd.env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/run/user/1000/bus");
    cmd.env("XAUTHORITY", "/home/user/.Xauthority");
    apply_prepared_to_command(&mut cmd, &prep);

    let env_map = run_env_and_parse(&mut cmd);
    for key in ["DISPLAY", "WAYLAND_DISPLAY", "HYPRLAND_INSTANCE_SIGNATURE", "XAUTHORITY"] {
        assert!(
            !env_map.contains_key(key),
            "opt-in isolation must strip {key}; got {:?}",
            env_map.get(key)
        );
    }
    assert_eq!(
        env_map.get("DBUS_SESSION_BUS_ADDRESS").map(String::as_str),
        Some("unix:path=/dev/null"),
        "opt-in isolation must neutralize DBus session bus"
    );
    assert_eq!(
        env_map.get("XDG_CURRENT_DESKTOP").map(String::as_str),
        Some("X-Generic"),
        "opt-in isolation must neutralize DE detection"
    );
    assert_eq!(
        env_map.get("BROWSER").map(String::as_str),
        Some("true"),
        "opt-in isolation must neutralize xdg-open BROWSER"
    );
}

// ---------------------------------------------------------------------------
// Global CODEMUX_ALLOW_DESKTOP_GUI override (three-state)
// ---------------------------------------------------------------------------

#[test]
#[cfg(target_os = "linux")]
fn env_override_force_allow_exposes_display_to_agent_child() {
    let _lock = persona_env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let _gui = EnvRestore::take("CODEMUX_ALLOW_DESKTOP_GUI");
    let _vd = EnvRestore::take("CODEMUX_VIRTUAL_DISPLAY");
    unsafe {
        env::set_var("CODEMUX_ALLOW_DESKTOP_GUI", "1");
        env::remove_var("CODEMUX_VIRTUAL_DISPLAY");
    }
    let policy = ExecutionPolicy::worktree_session_default_for_persona(Persona::Agent);
    // Force-allow: policy becomes allow=true, so env_unset / env_set are empty.
    assert!(policy.allow_desktop_gui);

    let prep = prepare_agent_command("env".into(), vec![], "/tmp", &policy);
    let mut cmd = Command::new("env");
    cmd.env("DISPLAY", ":7");
    apply_prepared_to_command(&mut cmd, &prep);
    let env_map = run_env_and_parse(&mut cmd);
    assert_eq!(
        env_map.get("DISPLAY").map(String::as_str),
        Some(":7"),
        "CODEMUX_ALLOW_DESKTOP_GUI=1 must pass DISPLAY even for Agent persona"
    );
}

#[test]
#[cfg(target_os = "linux")]
fn env_override_force_deny_hides_display_from_human_child() {
    let _lock = persona_env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let _gui = EnvRestore::take("CODEMUX_ALLOW_DESKTOP_GUI");
    let _vd = EnvRestore::take("CODEMUX_VIRTUAL_DISPLAY");
    unsafe {
        env::set_var("CODEMUX_ALLOW_DESKTOP_GUI", "0");
        env::remove_var("CODEMUX_VIRTUAL_DISPLAY");
    }
    let policy = ExecutionPolicy::worktree_session_default_for_persona(Persona::Human);
    // Force-deny: policy becomes allow=false, env_unset gets populated.
    assert!(!policy.allow_desktop_gui);

    let prep = prepare_agent_command("env".into(), vec![], "/tmp", &policy);
    let mut cmd = Command::new("env");
    cmd.env("DISPLAY", ":7");
    apply_prepared_to_command(&mut cmd, &prep);
    let env_map = run_env_and_parse(&mut cmd);
    assert!(
        !env_map.contains_key("DISPLAY"),
        "CODEMUX_ALLOW_DESKTOP_GUI=0 must strip DISPLAY even for Human persona"
    );
}

// ---------------------------------------------------------------------------
// Virtual display: agent + opt-in
// ---------------------------------------------------------------------------

#[test]
fn agent_persona_opts_into_virtual_display_when_env_set() {
    // Virtual display only engages when GUI is forbidden. Since Agent now
    // defaults to allow_desktop_gui=true, the user must ALSO opt into
    // isolation (CODEMUX_ALLOW_DESKTOP_GUI=0) to route agents through Xvfb.
    let _lock = persona_env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let _gui = EnvRestore::take("CODEMUX_ALLOW_DESKTOP_GUI");
    let _vd = EnvRestore::take("CODEMUX_VIRTUAL_DISPLAY");
    unsafe {
        env::set_var("CODEMUX_ALLOW_DESKTOP_GUI", "0");
        env::set_var("CODEMUX_VIRTUAL_DISPLAY", "1");
    }
    let p = ExecutionPolicy::worktree_session_default_for_persona(Persona::Agent);
    assert!(!p.allow_desktop_gui);
    assert!(
        p.virtual_display,
        "Agent + CODEMUX_ALLOW_DESKTOP_GUI=0 + CODEMUX_VIRTUAL_DISPLAY=1 must opt into Xvfb"
    );
}

#[test]
fn human_persona_never_opts_into_virtual_display() {
    // Virtual display only makes sense when GUI is forbidden — injecting
    // a synthetic DISPLAY=:N alongside the host's would just confuse
    // apps. For Human persona we always keep virtual_display off.
    let _lock = persona_env_lock().lock().unwrap_or_else(|e| e.into_inner());
    let _gui = EnvRestore::take("CODEMUX_ALLOW_DESKTOP_GUI");
    let _vd = EnvRestore::take("CODEMUX_VIRTUAL_DISPLAY");
    unsafe {
        env::remove_var("CODEMUX_ALLOW_DESKTOP_GUI");
        env::set_var("CODEMUX_VIRTUAL_DISPLAY", "1");
    }
    let p = ExecutionPolicy::worktree_session_default_for_persona(Persona::Human);
    assert!(p.allow_desktop_gui);
    assert!(!p.virtual_display);
}

// ---------------------------------------------------------------------------
// Preset wiring — make sure TerminalPreset carries Persona end-to-end
// ---------------------------------------------------------------------------

#[test]
fn builtin_agent_presets_are_marked_agent_persona() {
    use codemux_lib::database::DatabaseStore;
    use codemux_lib::presets::load_presets;
    let db = DatabaseStore::new_in_memory();
    let store = load_presets(&db);

    // Every builtin whose command is an AI CLI must be Agent persona.
    for agent_id in [
        "builtin-claude",
        "builtin-codex",
        "builtin-opencode",
        "builtin-gemini",
        "builtin-pi",
    ] {
        let p = store
            .presets
            .iter()
            .find(|p| p.id == agent_id)
            .unwrap_or_else(|| panic!("{agent_id} must exist in default store"));
        assert_eq!(
            p.persona,
            Persona::Agent,
            "{agent_id} must be Persona::Agent so its PTY strips desktop env"
        );
    }

    // Shell is the user's own shell. Must be Human.
    let shell = store
        .presets
        .iter()
        .find(|p| p.id == "builtin-shell")
        .expect("builtin-shell must exist");
    assert_eq!(shell.persona, Persona::Human);
}

#[test]
fn persona_json_round_trips_through_serde() {
    // The snapshot file format must round-trip persona verbatim so
    // startup restore respawns each session with the same persona it
    // had before (otherwise an Agent pane could silently "upgrade" to
    // Human persona after a restart — the exact class of bug we're
    // trying to prevent).
    use codemux_lib::presets::TerminalPreset;
    let preset_json = serde_json::json!({
        "id": "custom-agent",
        "name": "My Agent",
        "description": "test",
        "commands": ["my-agent-cli"],
        "working_directory": null,
        "launch_mode": "new_tab",
        "icon": null,
        "pinned": false,
        "is_builtin": false,
        "auto_run_on_workspace": false,
        "auto_run_on_new_tab": false,
        "persona": "agent"
    });
    let parsed: TerminalPreset = serde_json::from_value(preset_json).expect("parse");
    assert_eq!(parsed.persona, Persona::Agent);

    let back = serde_json::to_value(&parsed).expect("re-serialize");
    assert_eq!(
        back.get("persona").and_then(|v| v.as_str()),
        Some("agent"),
        "serialized JSON must carry `persona: \"agent\"` field for round-trip"
    );
}

#[test]
fn legacy_preset_json_without_persona_defaults_to_human() {
    // Upgrade path: a preset_store saved before this field existed
    // will lack `persona` in its JSON. Must deserialize cleanly with
    // `persona: "human"` — that's the safe default for unknown custom
    // presets (worst case: user sees their own GUI pop, same as
    // pre-fix behavior for custom presets).
    use codemux_lib::presets::TerminalPreset;
    let legacy_json = serde_json::json!({
        "id": "legacy",
        "name": "Legacy",
        "description": null,
        "commands": ["echo hi"],
        "working_directory": null,
        "launch_mode": "new_tab",
        "icon": null,
        "pinned": false,
        "is_builtin": false,
        "auto_run_on_workspace": false,
        "auto_run_on_new_tab": false
        // note: no "persona" key
    });
    let parsed: TerminalPreset = serde_json::from_value(legacy_json).expect("parse");
    assert_eq!(parsed.persona, Persona::Human);
}
