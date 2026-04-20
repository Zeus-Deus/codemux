use serde::{Deserialize, Serialize};
use std::env;
use std::path::Path;

pub mod virtual_display;

/// Generic execution backend choice for agent-launched commands.
///
/// The key design goal is to keep OpenFlow capability-based and cross-platform:
/// Linux can use a real sandbox first, while macOS/Windows add their own
/// backends later without changing the orchestration model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionBackendKind {
    HostPassthrough,
    LinuxBubblewrap,
    MacOsSandbox,
    WindowsRestricted,
}

/// High-level policy describing what an execution environment should be allowed
/// to do. This is intentionally generic and not tied to Linux-specific tools.
///
/// ## Display isolation decision matrix
///
/// | `allow_desktop_gui` | `virtual_display` | Behavior                                           |
/// |---------------------|-------------------|----------------------------------------------------|
/// | `true`              | `false`           | Inherit host `DISPLAY` — the Codemux default for   |
/// |                     |                   | plain user shells.                                 |
/// | `false`             | `false`           | Strip `DISPLAY`/`WAYLAND_DISPLAY`/etc. — Phase 1   |
/// |                     |                   | popup fix, OpenFlow agent default.                 |
/// | `false`             | `true`            | Strip host display, then inject the workspace's    |
/// |                     |                   | virtual `DISPLAY=:N` — Phase 2 computer-use.       |
/// | `true`              | `true`            | Host display inherited but overwritten by virtual. |
/// |                     |                   | Unusual; prefer the `(false, true)` combination.   |
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutionPolicy {
    pub backend_preference: ExecutionBackendKind,
    pub allow_network: bool,
    pub allow_browser_automation: bool,
    pub allow_desktop_gui: bool,
    /// When `true`, the caller should resolve the workspace's virtual display
    /// (`VirtualDisplayManager::acquire`) and set `DISPLAY=:N` on the child
    /// after `env_unset` is applied. Gated by host support
    /// (`VirtualDisplayManager::is_supported()`); if the host lacks Xvfb the
    /// caller falls back to plain env-strip.
    #[serde(default)]
    pub virtual_display: bool,
}

impl ExecutionPolicy {
    fn platform_default_backend() -> ExecutionBackendKind {
        if cfg!(target_os = "linux") {
            ExecutionBackendKind::LinuxBubblewrap
        } else if cfg!(target_os = "macos") {
            ExecutionBackendKind::MacOsSandbox
        } else if cfg!(target_os = "windows") {
            ExecutionBackendKind::WindowsRestricted
        } else {
            ExecutionBackendKind::HostPassthrough
        }
    }

    /// Default policy for OpenFlow agents:
    /// - they can still build/test/network
    /// - browser automation remains allowed
    /// - direct host GUI launching is blocked (env-strip on fallback paths,
    ///   full `--unsetenv` on the bwrap path)
    pub fn openflow_agent_default() -> Self {
        // CODEMUX_VIRTUAL_DISPLAY=1 is the Phase 2 opt-in flag — flips the
        // OpenFlow default from "strip DISPLAY" to "route DISPLAY to a
        // per-workspace virtual Xvfb". Safe even if Xvfb isn't installed:
        // the spawn path checks `VirtualDisplayManager::is_supported()` and
        // falls back to plain env-strip if not. Per-workspace config surface
        // comes in a follow-up PR; the env var is the simplest opt-in for
        // users who want computer-use today without editing JSON.
        let virtual_display = env::var("CODEMUX_VIRTUAL_DISPLAY")
            .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
            .unwrap_or(false);
        Self {
            backend_preference: Self::platform_default_backend(),
            allow_network: true,
            allow_browser_automation: true,
            allow_desktop_gui: false,
            virtual_display,
        }
    }

    /// Default policy for regular worktree shell sessions, **keyed by the
    /// principal driving the PTY** (the `Persona` stored on each session).
    ///
    /// Policy follows the principal:
    /// - `Persona::Human` — a person is typing. Inherits `DISPLAY` /
    ///   `WAYLAND_DISPLAY` / `HYPRLAND_INSTANCE_SIGNATURE` / etc. so
    ///   `npm run tauri dev`, `firefox`, and other GUI launches from a
    ///   user's own terminal pane work normally. This matches every
    ///   other terminal emulator (kitty, alacritty, ghostty, …) —
    ///   nothing surprising.
    /// - `Persona::Agent` — an AI CLI is driving the keystrokes (Claude
    ///   Code, OpenCode, Codex, Gemini, Pi, or any custom preset the
    ///   user flagged). Desktop GUI env is stripped so a tool call of
    ///   `npm run tauri dev` can't accidentally pop a window onto the
    ///   user's Hyprland/Wayland session. Users who want agent-driven
    ///   GUI testing set `virtual_display: true` per-workspace
    ///   (`.codemux/config.json` → `{"sandbox": {"virtual_display": true}}`)
    ///   or globally via `CODEMUX_VIRTUAL_DISPLAY=1` — the spawn path
    ///   will acquire an Xvfb and inject `DISPLAY=:N` so webkit/GTK apps
    ///   render into the virtual display instead of the host's.
    ///
    /// ### Global overrides
    ///
    /// `CODEMUX_ALLOW_DESKTOP_GUI` overrides the persona default:
    /// - `1` / `true` / `yes` → force-allow for every session, including
    ///   agents. Use when you explicitly trust your agents (e.g. running
    ///   Codemux inside a container that's already display-isolated).
    /// - `0` / `false` / `no` → force-deny for every session, including
    ///   humans. Use for kiosk / CI / shared-host setups where no pane
    ///   should ever reach the host display.
    /// - unset → per-persona default (described above).
    pub fn worktree_session_default_for_persona(
        persona: crate::presets::Persona,
    ) -> Self {
        let allow_desktop_gui = match parse_gui_override_env() {
            Some(forced) => forced,
            None => matches!(persona, crate::presets::Persona::Human),
        };

        // Virtual display is an orthogonal knob: it routes a virtual
        // `DISPLAY=:N` into the pane AFTER the strip. Only meaningful
        // when GUI is forbidden (allow_desktop_gui=false); with GUI
        // allowed the host DISPLAY is already inherited and Xvfb would
        // just compete. We gate acquisition on the policy field in
        // `terminal/mod.rs`, so setting it to `true` here for Human
        // persona is a no-op and stays off to avoid confusion.
        let virtual_display = !allow_desktop_gui
            && env::var("CODEMUX_VIRTUAL_DISPLAY")
                .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
                .unwrap_or(false);

        Self {
            backend_preference: ExecutionBackendKind::HostPassthrough,
            allow_network: true,
            allow_browser_automation: true,
            allow_desktop_gui,
            virtual_display,
        }
    }

    /// Convenience wrapper: `worktree_session_default_for_persona(Human)`.
    ///
    /// Kept as a named helper because callers at the top of `spawn_pty_for_session`
    /// instantiate a starting policy before they've looked up the session's
    /// actual persona — this gives them the human-safe default (plain shell
    /// inherits full env). The real persona-aware dispatch happens via
    /// `worktree_session_default_for_persona` after the lookup.
    pub fn worktree_session_default() -> Self {
        Self::worktree_session_default_for_persona(crate::presets::Persona::Human)
    }

    /// Current fallback behavior while platform backends are being built.
    pub fn effective_backend(&self) -> ExecutionBackendKind {
        match self.backend_preference {
            ExecutionBackendKind::LinuxBubblewrap if cfg!(target_os = "linux") => {
                ExecutionBackendKind::LinuxBubblewrap
            }
            ExecutionBackendKind::MacOsSandbox if cfg!(target_os = "macos") => {
                ExecutionBackendKind::MacOsSandbox
            }
            ExecutionBackendKind::WindowsRestricted if cfg!(target_os = "windows") => {
                ExecutionBackendKind::WindowsRestricted
            }
            _ => ExecutionBackendKind::HostPassthrough,
        }
    }

    pub fn backend_label(&self) -> &'static str {
        match self.effective_backend() {
            ExecutionBackendKind::HostPassthrough => "host_passthrough",
            ExecutionBackendKind::LinuxBubblewrap => "linux_bubblewrap",
            ExecutionBackendKind::MacOsSandbox => "macos_sandbox",
            ExecutionBackendKind::WindowsRestricted => "windows_restricted",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PreparedExecutionCommand {
    pub executable: String,
    pub args: Vec<String>,
    pub backend: ExecutionBackendKind,
    /// Environment variables that must be removed from the child process env
    /// after the caller has finished setting its own env.
    ///
    /// This is the cross-platform fallback for the bwrap `--unsetenv` flags:
    /// on `HostPassthrough` (which is everything except real Linux + bwrap),
    /// the caller uses `CommandBuilder::env_remove` to strip these keys so
    /// `allow_desktop_gui=false` actually hides the host display on
    /// macOS/Windows/bwrap-less Linux instead of being an advisory flag.
    ///
    /// Stays empty on the `LinuxBubblewrap` branch because `build_linux_bwrap_args`
    /// already emits `--unsetenv` for the same set of keys.
    pub env_unset: Vec<String>,
    /// Environment variables that must be *set* on the child process env
    /// (e.g. `BROWSER=true`, `MOZ_NO_REMOTE=1`) after the caller has finished
    /// setting its own env, but after `env_unset` is applied.
    ///
    /// These are neutralizers — they defeat escape hatches that pure env-strip
    /// cannot close (DBus auto-discovery, `xdg-open` DE routing, firefox
    /// remote protocol handoff). Empty on the `LinuxBubblewrap` branch because
    /// `build_linux_bwrap_args` emits `--setenv` flags directly.
    pub env_set: Vec<(String, String)>,
}

/// Environment variable keys that leak host-display access to a child process.
///
/// Clearing these before spawn stops well-behaved GTK/Qt/Xlib/Wayland/DBus clients
/// from finding the user's desktop. This is not a security control — a determined
/// child can probe socket paths directly — but it stops ~95% of accidental GUI
/// popups from agent-spawned commands like `npm run dev`, `electron .`, and
/// `playwright --headed`. On Windows, these keys typically aren't set and the
/// strip is a no-op; on macOS, it covers XQuartz/X11 apps but not native Cocoa.
///
/// On Linux with Hyprland/Wayland, the full set is needed because:
/// - `WAYLAND_DISPLAY` overrides `DISPLAY` for most modern toolkits.
/// - `HYPRLAND_INSTANCE_SIGNATURE` lets apps call `hyprctl` to reach the host.
/// - `XDG_CURRENT_DESKTOP` / `DESKTOP_SESSION` / `GNOME_DESKTOP_SESSION_ID` /
///   `XDG_SESSION_DESKTOP` route `xdg-open` through DE-specific DBus handlers,
///   defeating the BROWSER override — strip them so `xdg-open` takes the
///   generic mimeapps.list path (or the `BROWSER=true` neutralizer we set).
/// - `NIXOS_OZONE_WL` / `MOZ_ENABLE_WAYLAND` force Electron/Firefox to Wayland.
/// - `WAYLAND_SOCKET` is an alternate wayland discovery path.
///
/// Kept in sync with `--unsetenv` flags in `build_linux_bwrap_args`.
pub fn gui_env_keys() -> &'static [&'static str] {
    &[
        // X11
        "DISPLAY",
        "XAUTHORITY",
        // Wayland
        "WAYLAND_DISPLAY",
        "WAYLAND_SOCKET",
        // Session bus (DBus activation escape hatch: running firefox/chromium re-opens windows)
        "DBUS_SESSION_BUS_ADDRESS",
        // DE detection (routes xdg-open through portals / DE-specific handlers)
        "XDG_SESSION_TYPE",
        "XDG_CURRENT_DESKTOP",
        "XDG_SESSION_DESKTOP",
        "DESKTOP_SESSION",
        "GNOME_DESKTOP_SESSION_ID",
        // Compositor-specific reach-back (Hyprland, sway, Hyprland ecosystem)
        "HYPRLAND_INSTANCE_SIGNATURE",
        "HYPRCURSOR_THEME",
        "HYPRCURSOR_SIZE",
        "AQ_DRM_DEVICES",
        "SWAYSOCK",
        // Toolkit backend selectors (force app back to Wayland/X if left set)
        "GDK_BACKEND",
        "QT_QPA_PLATFORM",
        "QT_QPA_PLATFORMTHEME",
        "CLUTTER_BACKEND",
        "SDL_VIDEODRIVER",
        "NIXOS_OZONE_WL",
        "MOZ_ENABLE_WAYLAND",
        "MOZ_X11_EGL",
        // Portal / gio bridges (DBus activation routes even without WAYLAND_DISPLAY)
        "GTK_USE_PORTAL",
        // Startup notification
        "DESKTOP_STARTUP_ID",
    ]
}

/// Environment variables to *set* (not unset) when desktop GUI is forbidden.
///
/// These neutralize the escape hatches that pure env-strip cannot close:
/// - `BROWSER=true` makes `xdg-open <url>` a no-op (runs `/usr/bin/true`
///   instead of reaching for the default browser).
/// - `MOZ_NO_REMOTE=1` stops a spawned firefox from handing URLs to an already-
///   running firefox instance via the X/DBus remote protocol.
/// - `DBUS_SESSION_BUS_ADDRESS=unix:path=/dev/null` uses the documented D-Bus
///   address spec to fail the session-bus connection cleanly without triggering
///   libdbus's autolaunch fallback that would re-discover `$XDG_RUNTIME_DIR/bus`.
/// - `XDG_CURRENT_DESKTOP=X-Generic` forces `xdg-open` onto the generic
///   mimeapps.list path instead of GNOME/KDE/portal DBus activation.
/// - `DE=generic` belt-and-suspenders for older xdg-utils code paths that
///   check `$DE` before `$XDG_CURRENT_DESKTOP`.
/// - `GTK_USE_PORTAL=0` blocks GTK3/4 apps from routing through
///   xdg-desktop-portal for file/URI opens even if they reach gio.
/// - `GIO_USE_VFS=local` stops gio from DBus-activating gvfsd when parsing URIs.
/// - `NO_AT_BRIDGE=1` suppresses at-spi2 bus activation attempts that would
///   otherwise wake dbus-daemon on demand.
pub fn gui_env_overrides() -> &'static [(&'static str, &'static str)] {
    &[
        ("BROWSER", "true"),
        ("MOZ_NO_REMOTE", "1"),
        ("DBUS_SESSION_BUS_ADDRESS", "unix:path=/dev/null"),
        ("XDG_CURRENT_DESKTOP", "X-Generic"),
        ("DE", "generic"),
        ("GTK_USE_PORTAL", "0"),
        ("GIO_USE_VFS", "local"),
        ("NO_AT_BRIDGE", "1"),
    ]
}

/// Parse `CODEMUX_ALLOW_DESKTOP_GUI` as a three-state override.
///
/// Returns:
/// - `Some(true)` for `1` / `true` / `yes` — force allow GUI for every
///   session regardless of persona
/// - `Some(false)` for `0` / `false` / `no` — force deny for every session
/// - `None` for unset or any other value — fall through to persona default
///
/// The unrecognized-value case returns `None` (not a parse error) so
/// typos in the env var can't accidentally lock users out of their
/// desktop — they just get the normal persona-based default, which
/// is the behavior they'd have gotten without the env var at all.
fn parse_gui_override_env() -> Option<bool> {
    match env::var("CODEMUX_ALLOW_DESKTOP_GUI").ok()?.as_str() {
        "1" | "true" | "yes" => Some(true),
        "0" | "false" | "no" => Some(false),
        _ => None,
    }
}

fn gui_env_unset_if_forbidden(policy: &ExecutionPolicy) -> Vec<String> {
    if policy.allow_desktop_gui {
        Vec::new()
    } else {
        gui_env_keys().iter().map(|s| s.to_string()).collect()
    }
}

fn gui_env_set_if_forbidden(policy: &ExecutionPolicy) -> Vec<(String, String)> {
    if policy.allow_desktop_gui {
        Vec::new()
    } else {
        gui_env_overrides()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }
}

/// Apply GUI-env hygiene to a `std::process::Command`.
///
/// This is the entry point for the ~20 direct `Command::new()` spawn sites
/// scattered across Codemux (agent_browser, git, mcp_server, ai, etc.) that
/// don't go through the PTY/ExecutionPolicy path. Call this *after* setting
/// any explicit `.env(K, V)` pairs so overrides win.
///
/// Effect: strips every key in `gui_env_keys()`, then sets the neutralizer
/// values from `gui_env_overrides()`. Children spawned this way cannot pop
/// windows onto the user's real Hyprland/Wayland/X11 session even if the
/// caller never touched env explicitly.
///
/// Safe to call unconditionally — the strip is a no-op on Windows (keys not
/// set) and macOS (X11 keys not usually set), so cross-platform callers
/// don't need an OS gate.
pub fn sanitize_gui_env_std(cmd: &mut std::process::Command) {
    for key in gui_env_keys() {
        cmd.env_remove(key);
    }
    for (k, v) in gui_env_overrides() {
        cmd.env(k, v);
    }
}

/// Same as `sanitize_gui_env_std` but for `tokio::process::Command`.
/// Kept as a separate function because the two types don't share a trait.
pub fn sanitize_gui_env_tokio(cmd: &mut tokio::process::Command) {
    for key in gui_env_keys() {
        cmd.env_remove(key);
    }
    for (k, v) in gui_env_overrides() {
        cmd.env(k, v);
    }
}

pub fn prepare_agent_command(
    executable: String,
    args: Vec<String>,
    cwd: &str,
    policy: &ExecutionPolicy,
) -> PreparedExecutionCommand {
    match policy.effective_backend() {
        ExecutionBackendKind::LinuxBubblewrap => {
            if let Some(bwrap_path) = find_executable("bwrap") {
                // Bwrap handles GUI env hygiene internally via `--unsetenv`
                // and `--setenv`, so env_unset / env_set stay empty here.
                PreparedExecutionCommand {
                    executable: bwrap_path,
                    args: build_linux_bwrap_args(&executable, &args, cwd, policy),
                    backend: ExecutionBackendKind::LinuxBubblewrap,
                    env_unset: Vec::new(),
                    env_set: Vec::new(),
                }
            } else {
                crate::diagnostics::stderr_line(
                    "[codemux::execution] Bubblewrap requested but not found; falling back to host passthrough with env-strip",
                );
                PreparedExecutionCommand {
                    executable,
                    args,
                    backend: ExecutionBackendKind::HostPassthrough,
                    env_unset: gui_env_unset_if_forbidden(policy),
                    env_set: gui_env_set_if_forbidden(policy),
                }
            }
        }
        backend => PreparedExecutionCommand {
            executable,
            args,
            backend,
            env_unset: gui_env_unset_if_forbidden(policy),
            env_set: gui_env_set_if_forbidden(policy),
        },
    }
}

fn find_executable(name: &str) -> Option<String> {
    let path = env::var_os("PATH")?;
    for dir in env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate.display().to_string());
        }
    }
    None
}

fn build_linux_bwrap_args(
    executable: &str,
    args: &[String],
    cwd: &str,
    policy: &ExecutionPolicy,
) -> Vec<String> {
    let mut out = vec![
        "--die-with-parent".to_string(),
        "--new-session".to_string(),
        "--unshare-pid".to_string(),
        "--unshare-ipc".to_string(),
        "--bind".to_string(),
        "/".to_string(),
        "/".to_string(),
        "--proc".to_string(),
        "/proc".to_string(),
        "--dev-bind".to_string(),
        "/dev".to_string(),
        "/dev".to_string(),
        "--chdir".to_string(),
        cwd.to_string(),
    ];

    if !policy.allow_network {
        out.push("--unshare-net".to_string());
    }

    if !policy.allow_desktop_gui {
        // Strip the full extended key set — matches gui_env_keys() so bwrap
        // and HostPassthrough paths behave identically. Missing a key on one
        // path but not the other is the exact class of bug that let
        // WAYLAND_DISPLAY slip through before.
        for key in gui_env_keys() {
            out.push("--unsetenv".to_string());
            out.push((*key).to_string());
        }

        // Apply neutralizer overrides (BROWSER=true, MOZ_NO_REMOTE=1, etc.)
        // so xdg-open / firefox / chromium can't escape via DBus activation
        // or single-instance handoff even if env-strip is bypassed.
        for (k, v) in gui_env_overrides() {
            out.push("--setenv".to_string());
            out.push((*k).to_string());
            out.push((*v).to_string());
        }

        // Block X11 socket access even if a child process tries to recreate DISPLAY.
        out.push("--tmpfs".to_string());
        out.push("/tmp/.X11-unix".to_string());

        // Preserve Codemux IPC while hiding desktop-session sockets from the sandboxed process.
        if let Some(runtime_dir) = env::var_os("XDG_RUNTIME_DIR") {
            let runtime_dir = runtime_dir.to_string_lossy().to_string();
            out.push("--tmpfs".to_string());
            out.push(runtime_dir.clone());

            if let Some(socket_path) = crate::control::control_socket_path() {
                if socket_path.exists() {
                    let socket_path = socket_path.display().to_string();
                    out.push("--ro-bind".to_string());
                    out.push(socket_path.clone());
                    out.push(socket_path);
                }
            }
        }
    }

    if policy.allow_browser_automation {
        out.push("--setenv".to_string());
        out.push("CODEMUX_BROWSER_AUTOMATION".to_string());
        out.push("1".to_string());
    }

    // Make it explicit to the child which directory should be treated as the task root.
    if Path::new(cwd).exists() {
        out.push("--setenv".to_string());
        out.push("PWD".to_string());
        out.push(cwd.to_string());
    }

    out.push("--".to_string());
    out.push(executable.to_string());
    out.extend(args.iter().cloned());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy_gui(gui: bool, backend: ExecutionBackendKind) -> ExecutionPolicy {
        ExecutionPolicy {
            backend_preference: backend,
            allow_network: true,
            allow_browser_automation: true,
            allow_desktop_gui: gui,
            virtual_display: false,
        }
    }

    #[test]
    fn gui_env_keys_covers_common_gui_vars() {
        let keys = gui_env_keys();
        for expected in [
            "DISPLAY",
            "WAYLAND_DISPLAY",
            "DBUS_SESSION_BUS_ADDRESS",
            "DESKTOP_STARTUP_ID",
            "XAUTHORITY",
        ] {
            assert!(
                keys.contains(&expected),
                "gui_env_keys is missing {expected}"
            );
        }
    }

    #[test]
    fn host_passthrough_strips_gui_env_when_forbidden() {
        let policy = policy_gui(false, ExecutionBackendKind::HostPassthrough);
        let prepared = prepare_agent_command(
            "echo".into(),
            vec!["hello".into()],
            "/tmp",
            &policy,
        );
        assert!(matches!(
            prepared.backend,
            ExecutionBackendKind::HostPassthrough
        ));
        assert!(!prepared.env_unset.is_empty());
        for key in gui_env_keys() {
            assert!(
                prepared.env_unset.iter().any(|k| k == key),
                "expected env_unset to contain {key}"
            );
        }
        // executable/args must be pass-through (no wrapping).
        assert_eq!(prepared.executable, "echo");
        assert_eq!(prepared.args, vec!["hello".to_string()]);
    }

    #[test]
    fn host_passthrough_keeps_gui_env_when_allowed() {
        let policy = policy_gui(true, ExecutionBackendKind::HostPassthrough);
        let prepared = prepare_agent_command("echo".into(), vec![], "/tmp", &policy);
        assert!(prepared.env_unset.is_empty());
    }

    #[test]
    fn stub_backends_still_populate_env_unset_on_fallback() {
        // The macOS/Windows backends are stubs today; `effective_backend` on
        // non-matching hosts returns HostPassthrough. Either way, when
        // allow_desktop_gui=false, env_unset must be populated so the fallback
        // spawn path still hides the host display.
        for backend in [
            ExecutionBackendKind::MacOsSandbox,
            ExecutionBackendKind::WindowsRestricted,
        ] {
            let policy = policy_gui(false, backend.clone());
            let prepared = prepare_agent_command("echo".into(), vec![], "/tmp", &policy);
            assert!(
                !prepared.env_unset.is_empty(),
                "backend {backend:?} should populate env_unset when gui is forbidden"
            );
        }
    }

    #[test]
    fn openflow_default_forbids_gui() {
        let p = ExecutionPolicy::openflow_agent_default();
        assert!(!p.allow_desktop_gui);
        assert!(p.allow_network);
        assert!(p.allow_browser_automation);
    }

    // Serializes every test in this module that mutates `CODEMUX_ALLOW_DESKTOP_GUI`.
    // Integration tests in `tests/execution_env.rs` and `tests/gui_leak_prevention.rs`
    // have their own per-crate mutex (separate process, so no contention with this one).
    // Within this unit-test binary, every env-mutating test MUST acquire this guard.
    fn env_guard() -> std::sync::MutexGuard<'static, ()> {
        use std::sync::{Mutex, OnceLock};
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    struct EnvVarRestore {
        key: &'static str,
        prior: Option<String>,
    }

    impl EnvVarRestore {
        fn snapshot(key: &'static str) -> Self {
            Self {
                key,
                prior: env::var(key).ok(),
            }
        }
    }

    impl Drop for EnvVarRestore {
        fn drop(&mut self) {
            // SAFETY: std::env mutation in Rust 2024 is `unsafe`; safe here
            // because `env_guard()` serializes every mutator in this module.
            unsafe {
                match &self.prior {
                    Some(v) => env::set_var(self.key, v),
                    None => env::remove_var(self.key),
                }
            }
        }
    }

    #[test]
    fn worktree_default_allows_gui_for_human_persona() {
        // New baseline: a human-driven pane (the plain "+" Terminal tab,
        // the Shell preset, setup/teardown run button) inherits the host
        // desktop env. Without this, users can't `npm run tauri dev` or
        // launch any other GUI app from their own terminal.
        let _lock = env_guard();
        let _restore = EnvVarRestore::snapshot("CODEMUX_ALLOW_DESKTOP_GUI");
        // SAFETY: guarded by `env_guard()` above; restored by `_restore` Drop.
        unsafe {
            env::remove_var("CODEMUX_ALLOW_DESKTOP_GUI");
        }
        let p = ExecutionPolicy::worktree_session_default();
        assert!(
            p.allow_desktop_gui,
            "human-driven pane must inherit host DISPLAY/WAYLAND_DISPLAY"
        );
        assert!(p.allow_network);
    }

    #[test]
    fn worktree_default_denies_gui_for_agent_persona() {
        // Agent-driven panes (Claude, Codex, OpenCode, …) must NOT inherit
        // the host display — otherwise agent tool calls pop windows on the
        // user's real desktop, which was the whole motivation for the
        // 0e4e558 lockdown.
        let _lock = env_guard();
        let _restore_gui = EnvVarRestore::snapshot("CODEMUX_ALLOW_DESKTOP_GUI");
        let _restore_vd = EnvVarRestore::snapshot("CODEMUX_VIRTUAL_DISPLAY");
        unsafe {
            env::remove_var("CODEMUX_ALLOW_DESKTOP_GUI");
            env::remove_var("CODEMUX_VIRTUAL_DISPLAY");
        }
        let p = ExecutionPolicy::worktree_session_default_for_persona(
            crate::presets::Persona::Agent,
        );
        assert!(
            !p.allow_desktop_gui,
            "agent-driven pane must strip DISPLAY/WAYLAND_DISPLAY"
        );
        // Without CODEMUX_VIRTUAL_DISPLAY, virtual_display stays off —
        // agents run headless. Opting in is a separate toggle.
        assert!(!p.virtual_display);
    }

    #[test]
    fn worktree_default_agent_persona_opts_into_virtual_display() {
        // When CODEMUX_VIRTUAL_DISPLAY=1 and the pane is Agent-driven,
        // the policy asks `terminal/mod.rs` to acquire an Xvfb and inject
        // `DISPLAY=:N` so webkit/GTK apps can render headlessly.
        let _lock = env_guard();
        let _restore_gui = EnvVarRestore::snapshot("CODEMUX_ALLOW_DESKTOP_GUI");
        let _restore_vd = EnvVarRestore::snapshot("CODEMUX_VIRTUAL_DISPLAY");
        unsafe {
            env::remove_var("CODEMUX_ALLOW_DESKTOP_GUI");
            env::set_var("CODEMUX_VIRTUAL_DISPLAY", "1");
        }
        let p = ExecutionPolicy::worktree_session_default_for_persona(
            crate::presets::Persona::Agent,
        );
        assert!(!p.allow_desktop_gui);
        assert!(p.virtual_display);
    }

    #[test]
    fn worktree_default_human_persona_ignores_virtual_display() {
        // Virtual display is only meaningful when GUI is forbidden. For
        // a Human persona, `allow_desktop_gui=true` means the host DISPLAY
        // is already inherited — Xvfb would just compete. So even with
        // CODEMUX_VIRTUAL_DISPLAY=1, we keep virtual_display=false and
        // let the user's real desktop take precedence.
        let _lock = env_guard();
        let _restore_gui = EnvVarRestore::snapshot("CODEMUX_ALLOW_DESKTOP_GUI");
        let _restore_vd = EnvVarRestore::snapshot("CODEMUX_VIRTUAL_DISPLAY");
        unsafe {
            env::remove_var("CODEMUX_ALLOW_DESKTOP_GUI");
            env::set_var("CODEMUX_VIRTUAL_DISPLAY", "1");
        }
        let p = ExecutionPolicy::worktree_session_default_for_persona(
            crate::presets::Persona::Human,
        );
        assert!(p.allow_desktop_gui);
        assert!(
            !p.virtual_display,
            "Human + CODEMUX_VIRTUAL_DISPLAY=1 should stay off — no point competing with host DISPLAY"
        );
    }

    #[test]
    fn env_override_true_forces_gui_for_agent_persona() {
        // Global "I trust my agents" override: even Agent-driven panes
        // get the host DISPLAY inherited.
        let _lock = env_guard();
        let _restore = EnvVarRestore::snapshot("CODEMUX_ALLOW_DESKTOP_GUI");
        unsafe {
            env::set_var("CODEMUX_ALLOW_DESKTOP_GUI", "1");
        }
        let p = ExecutionPolicy::worktree_session_default_for_persona(
            crate::presets::Persona::Agent,
        );
        assert!(
            p.allow_desktop_gui,
            "CODEMUX_ALLOW_DESKTOP_GUI=1 must force-allow, overriding Agent persona"
        );
    }

    #[test]
    fn env_override_false_forces_deny_for_human_persona() {
        // Kiosk / CI / shared-host lockdown: even Human-driven panes lose
        // the host DISPLAY. Useful when Codemux is running in a context
        // where no pane should reach the host desktop.
        let _lock = env_guard();
        let _restore = EnvVarRestore::snapshot("CODEMUX_ALLOW_DESKTOP_GUI");
        unsafe {
            env::set_var("CODEMUX_ALLOW_DESKTOP_GUI", "0");
        }
        let p = ExecutionPolicy::worktree_session_default_for_persona(
            crate::presets::Persona::Human,
        );
        assert!(
            !p.allow_desktop_gui,
            "CODEMUX_ALLOW_DESKTOP_GUI=0 must force-deny, overriding Human persona"
        );
    }

    #[test]
    fn env_override_accepts_case_sensitive_tokens() {
        // Spec: only lowercase `1`/`true`/`yes`/`0`/`false`/`no` count.
        // `True`, `TRUE`, etc. return None (fall back to persona default)
        // rather than risk ambiguity. Users who care can type the
        // canonical form; typos don't lock them out — they get the safe
        // per-persona default.
        let _lock = env_guard();
        let _restore = EnvVarRestore::snapshot("CODEMUX_ALLOW_DESKTOP_GUI");
        unsafe {
            env::set_var("CODEMUX_ALLOW_DESKTOP_GUI", "TRUE"); // uppercase → None
        }
        let p = ExecutionPolicy::worktree_session_default_for_persona(
            crate::presets::Persona::Agent,
        );
        // Persona default (Agent → deny) should take effect, not the
        // uppercase-True that would otherwise have forced allow.
        assert!(!p.allow_desktop_gui);
    }

    #[test]
    fn env_override_honors_every_allow_token() {
        for token in ["1", "true", "yes"] {
            let _lock = env_guard();
            let _restore = EnvVarRestore::snapshot("CODEMUX_ALLOW_DESKTOP_GUI");
            unsafe {
                env::set_var("CODEMUX_ALLOW_DESKTOP_GUI", token);
            }
            let p = ExecutionPolicy::worktree_session_default_for_persona(
                crate::presets::Persona::Agent,
            );
            assert!(
                p.allow_desktop_gui,
                "token `{token}` must force-allow regardless of persona"
            );
        }
    }

    #[test]
    fn env_override_honors_every_deny_token() {
        for token in ["0", "false", "no"] {
            let _lock = env_guard();
            let _restore = EnvVarRestore::snapshot("CODEMUX_ALLOW_DESKTOP_GUI");
            unsafe {
                env::set_var("CODEMUX_ALLOW_DESKTOP_GUI", token);
            }
            let p = ExecutionPolicy::worktree_session_default_for_persona(
                crate::presets::Persona::Human,
            );
            assert!(
                !p.allow_desktop_gui,
                "token `{token}` must force-deny regardless of persona"
            );
        }
    }

    #[test]
    fn gui_env_keys_covers_wayland_and_dbus_escape_hatches() {
        let keys = gui_env_keys();
        for expected in [
            "WAYLAND_DISPLAY",
            "WAYLAND_SOCKET",
            "DBUS_SESSION_BUS_ADDRESS",
            "XDG_CURRENT_DESKTOP",
            "HYPRLAND_INSTANCE_SIGNATURE",
            "MOZ_ENABLE_WAYLAND",
            "NIXOS_OZONE_WL",
        ] {
            assert!(
                keys.contains(&expected),
                "gui_env_keys missing Wayland/DBus escape-hatch key {expected}"
            );
        }
    }

    #[test]
    fn gui_env_overrides_neutralizes_browser_and_dbus() {
        let overrides = gui_env_overrides();
        let map: std::collections::HashMap<_, _> = overrides.iter().copied().collect();
        assert_eq!(map.get("BROWSER").copied(), Some("true"));
        assert_eq!(map.get("MOZ_NO_REMOTE").copied(), Some("1"));
        // DBus neutralizer must use valid spec syntax (`unix:path=<path>`); a
        // bare `disabled:` causes some libdbus clients to autolaunch-fall-back
        // to `$XDG_RUNTIME_DIR/bus` and reach the real session bus anyway.
        assert_eq!(
            map.get("DBUS_SESSION_BUS_ADDRESS").copied(),
            Some("unix:path=/dev/null")
        );
        assert_eq!(map.get("XDG_CURRENT_DESKTOP").copied(), Some("X-Generic"));
        assert_eq!(map.get("DE").copied(), Some("generic"));
        assert_eq!(map.get("GTK_USE_PORTAL").copied(), Some("0"));
        assert_eq!(map.get("GIO_USE_VFS").copied(), Some("local"));
        assert_eq!(map.get("NO_AT_BRIDGE").copied(), Some("1"));
    }

    #[test]
    fn host_passthrough_populates_env_set_when_forbidden() {
        let policy = policy_gui(false, ExecutionBackendKind::HostPassthrough);
        let prepared = prepare_agent_command("echo".into(), vec![], "/tmp", &policy);
        assert!(
            !prepared.env_set.is_empty(),
            "env_set must carry neutralizer overrides when gui is forbidden"
        );
        let keys: Vec<&str> = prepared.env_set.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"BROWSER"));
        assert!(keys.contains(&"MOZ_NO_REMOTE"));
    }

    #[test]
    fn host_passthrough_env_set_empty_when_gui_allowed() {
        let policy = policy_gui(true, ExecutionBackendKind::HostPassthrough);
        let prepared = prepare_agent_command("echo".into(), vec![], "/tmp", &policy);
        assert!(prepared.env_set.is_empty());
    }

    #[test]
    fn sanitize_gui_env_std_strips_and_sets() {
        use std::process::Command;
        let mut cmd = Command::new("true");
        cmd.env("DISPLAY", ":0")
            .env("WAYLAND_DISPLAY", "wayland-1")
            .env("XDG_CURRENT_DESKTOP", "Hyprland");
        sanitize_gui_env_std(&mut cmd);
        // We can't observe env_remove after the fact without spawning, so
        // smoke-test by spawning `env` (if available) is done in the
        // integration tests. Here we simply verify the function returns
        // without panicking and builds a valid command.
        let _program = cmd.get_program().to_string_lossy().into_owned();
    }

    #[test]
    fn execution_policy_serde_round_trip() {
        let p = ExecutionPolicy::openflow_agent_default();
        let s = serde_json::to_string(&p).expect("serialize");
        let back: ExecutionPolicy = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(p, back);
    }

    #[test]
    fn effective_backend_falls_back_off_platform() {
        // Linux variant on non-Linux must fall back.
        let linux = ExecutionPolicy {
            backend_preference: ExecutionBackendKind::LinuxBubblewrap,
            allow_network: true,
            allow_browser_automation: true,
            allow_desktop_gui: false,
            virtual_display: false,
        };
        let effective = linux.effective_backend();
        if cfg!(target_os = "linux") {
            assert_eq!(effective, ExecutionBackendKind::LinuxBubblewrap);
        } else {
            assert_eq!(effective, ExecutionBackendKind::HostPassthrough);
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn linux_bwrap_args_contain_unsetenv_for_gui_when_forbidden() {
        let policy = policy_gui(false, ExecutionBackendKind::LinuxBubblewrap);
        let args = build_linux_bwrap_args("true", &[], "/tmp", &policy);
        // The flag and key appear as consecutive entries.
        for key in ["DISPLAY", "WAYLAND_DISPLAY", "XAUTHORITY"] {
            let pos = args.iter().position(|a| a == key);
            assert!(pos.is_some(), "bwrap args missing key {key}");
            let idx = pos.unwrap();
            assert!(
                idx > 0 && args[idx - 1] == "--unsetenv",
                "key {key} not preceded by --unsetenv in {args:?}"
            );
        }
        // /tmp/.X11-unix must be tmpfs-shadowed too.
        let tmpfs_idx = args.iter().position(|a| a == "/tmp/.X11-unix");
        assert!(tmpfs_idx.is_some(), "missing /tmp/.X11-unix shadow");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn linux_bwrap_args_keep_gui_when_allowed() {
        let policy = policy_gui(true, ExecutionBackendKind::LinuxBubblewrap);
        let args = build_linux_bwrap_args("true", &[], "/tmp", &policy);
        assert!(
            !args.iter().any(|a| a == "DISPLAY"),
            "DISPLAY should not be in --unsetenv when allow_desktop_gui=true"
        );
    }
}
