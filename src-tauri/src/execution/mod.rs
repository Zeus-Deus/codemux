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

    /// Default policy for regular worktree shell sessions (non-OpenFlow panes).
    ///
    /// **Safe-by-default (as of April 2026):** `allow_desktop_gui` is `false`,
    /// so plain user shells cannot accidentally pop Firefox/Chromium/Electron
    /// windows onto the user's real Hyprland/Wayland session. The
    /// `HostPassthrough` backend means the shell is not wrapped by bwrap —
    /// we only strip GUI env keys and set neutralizers (`BROWSER=true`,
    /// `MOZ_NO_REMOTE=1`, `DBUS_SESSION_BUS_ADDRESS=disabled:`,
    /// `XDG_CURRENT_DESKTOP=X-Generic`) so `systemctl --user`, `ssh-agent`,
    /// and other host-session tools keep working.
    ///
    /// Users who want GUI apps launched from a pane (e.g. `npm run dev`
    /// preview windows) can opt in per-workspace via `.codemux/config.json`
    /// `{"sandbox": {"allow_desktop_gui": true}}`, or globally by setting
    /// `CODEMUX_ALLOW_DESKTOP_GUI=1` before launching Codemux.
    pub fn worktree_session_default() -> Self {
        let allow_desktop_gui = env::var("CODEMUX_ALLOW_DESKTOP_GUI")
            .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
            .unwrap_or(false);
        Self {
            backend_preference: ExecutionBackendKind::HostPassthrough,
            allow_network: true,
            allow_browser_automation: true,
            allow_desktop_gui,
            virtual_display: false,
        }
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

/// Variant of `sanitize_gui_env_std` that *preserves* the DBus session
/// bus address.
///
/// Use this for credential-handling CLIs (`gh`, `git credential-libsecret`,
/// `glab`, `aws sso` w/ keyring, etc.) that round-trip to a keyring
/// daemon over DBus — `gnome-keyring-daemon`, `kwalletd5/6`,
/// `keepassxc-secret-service`, anything implementing the freedesktop
/// `org.freedesktop.secrets` API. The default sanitiser overrides
/// `DBUS_SESSION_BUS_ADDRESS=unix:path=/dev/null` so that arbitrary
/// agent-spawned children can't escape via portals/dbus-activation;
/// trusted internal CLIs that store their own token in the user's
/// keyring legitimately need bus access to fetch it back, otherwise
/// `gh auth status` reports unauthenticated even when `gh auth login`
/// previously succeeded.
///
/// Strips/sets the same keys as `sanitize_gui_env_std` minus
/// `DBUS_SESSION_BUS_ADDRESS`. Display/wayland/portal hygiene is
/// preserved; only the bus address is allowed through.
pub fn sanitize_gui_env_std_keep_dbus(cmd: &mut std::process::Command) {
    for key in gui_env_keys() {
        if *key == "DBUS_SESSION_BUS_ADDRESS" {
            continue;
        }
        cmd.env_remove(key);
    }
    for (k, v) in gui_env_overrides() {
        if *k == "DBUS_SESSION_BUS_ADDRESS" {
            continue;
        }
        cmd.env(k, v);
    }
}

/// Tokio variant of `sanitize_gui_env_std_keep_dbus`. See that function
/// for the rationale and the exhaustive call-site policy.
pub fn sanitize_gui_env_tokio_keep_dbus(cmd: &mut tokio::process::Command) {
    for key in gui_env_keys() {
        if *key == "DBUS_SESSION_BUS_ADDRESS" {
            continue;
        }
        cmd.env_remove(key);
    }
    for (k, v) in gui_env_overrides() {
        if *k == "DBUS_SESSION_BUS_ADDRESS" {
            continue;
        }
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
    fn worktree_default_forbids_gui_by_default() {
        let _lock = env_guard();
        let _restore = EnvVarRestore::snapshot("CODEMUX_ALLOW_DESKTOP_GUI");
        // SAFETY: guarded by `env_guard()` above; restored by `_restore` Drop.
        unsafe {
            env::remove_var("CODEMUX_ALLOW_DESKTOP_GUI");
        }
        let p = ExecutionPolicy::worktree_session_default();
        assert!(!p.allow_desktop_gui, "worktree default must strip GUI env");
        assert!(p.allow_network);
    }

    #[test]
    fn worktree_default_honors_opt_in_env_one() {
        let _lock = env_guard();
        let _restore = EnvVarRestore::snapshot("CODEMUX_ALLOW_DESKTOP_GUI");
        unsafe {
            env::set_var("CODEMUX_ALLOW_DESKTOP_GUI", "1");
        }
        let p = ExecutionPolicy::worktree_session_default();
        assert!(p.allow_desktop_gui, "value `1` must re-enable GUI passthrough");
    }

    #[test]
    fn worktree_default_honors_opt_in_env_true() {
        let _lock = env_guard();
        let _restore = EnvVarRestore::snapshot("CODEMUX_ALLOW_DESKTOP_GUI");
        unsafe {
            env::set_var("CODEMUX_ALLOW_DESKTOP_GUI", "true");
        }
        let p = ExecutionPolicy::worktree_session_default();
        assert!(
            p.allow_desktop_gui,
            "value `true` must re-enable GUI passthrough"
        );
    }

    #[test]
    fn worktree_default_honors_opt_in_env_yes() {
        let _lock = env_guard();
        let _restore = EnvVarRestore::snapshot("CODEMUX_ALLOW_DESKTOP_GUI");
        unsafe {
            env::set_var("CODEMUX_ALLOW_DESKTOP_GUI", "yes");
        }
        let p = ExecutionPolicy::worktree_session_default();
        assert!(
            p.allow_desktop_gui,
            "value `yes` must re-enable GUI passthrough"
        );
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

    /// `Command::get_envs()` reports the per-command env mutations made
    /// via `.env()` / `.env_remove()`:
    ///   - `(K, Some(V))` for `cmd.env(K, V)`
    ///   - `(K, None)`    for `cmd.env_remove(K)`
    ///   - keys absent entirely → child inherits the parent's value
    ///
    /// We use that to assert the keep-dbus variant leaves
    /// `DBUS_SESSION_BUS_ADDRESS` untouched (so the parent's bus
    /// address passes through to the child) while still applying the
    /// rest of the GUI hygiene.
    #[test]
    fn sanitize_gui_env_std_keep_dbus_preserves_bus_address() {
        use std::ffi::OsStr;
        use std::process::Command;
        let mut cmd = Command::new("true");
        sanitize_gui_env_std_keep_dbus(&mut cmd);

        let envs: Vec<(&OsStr, Option<&OsStr>)> = cmd.get_envs().collect();

        // Bus address must NOT be touched — neither stripped nor
        // overridden — so the child inherits the user's session bus.
        assert!(
            !envs
                .iter()
                .any(|(k, _)| *k == OsStr::new("DBUS_SESSION_BUS_ADDRESS")),
            "keep_dbus must not touch DBUS_SESSION_BUS_ADDRESS, got envs: {envs:?}",
        );

        // Sanity: other GUI keys are still stripped (env_remove → None).
        let display_entry = envs
            .iter()
            .find(|(k, _)| *k == OsStr::new("DISPLAY"))
            .expect("DISPLAY should be stripped by keep_dbus");
        assert_eq!(display_entry.1, None);

        // Sanity: non-DBus overrides are still applied.
        let browser_entry = envs
            .iter()
            .find(|(k, _)| *k == OsStr::new("BROWSER"))
            .expect("BROWSER override should still be set by keep_dbus");
        assert_eq!(browser_entry.1, Some(OsStr::new("true")));
    }

    /// Companion test against the strict variant — proves the contract
    /// difference between the two. The strict sanitiser DOES override
    /// the bus address; the keep-dbus variant does not.
    #[test]
    fn sanitize_gui_env_std_overrides_bus_address() {
        use std::ffi::OsStr;
        use std::process::Command;
        let mut cmd = Command::new("true");
        sanitize_gui_env_std(&mut cmd);

        let envs: Vec<(&OsStr, Option<&OsStr>)> = cmd.get_envs().collect();

        let bus_entry = envs
            .iter()
            .find(|(k, _)| *k == OsStr::new("DBUS_SESSION_BUS_ADDRESS"))
            .expect("strict sanitiser must override DBUS_SESSION_BUS_ADDRESS");
        // The override value documented in `gui_env_overrides()`.
        assert_eq!(
            bus_entry.1,
            Some(OsStr::new("unix:path=/dev/null")),
        );
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
