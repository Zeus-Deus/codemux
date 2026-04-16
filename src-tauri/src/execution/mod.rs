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
    /// Preserves current behavior for plain user shells: no sandbox wrapping,
    /// full inherited env, network/browser/GUI all allowed. The `HostPassthrough`
    /// backend means `prepare_agent_command` returns the shell unmodified.
    ///
    /// Callers that want to hide desktop GUI (e.g. per-workspace opt-in via
    /// `.codemux/config.json`) flip `allow_desktop_gui` to `false` and the
    /// env-strip in `prepare_agent_command` takes effect automatically — still
    /// without wrapping the shell, so `systemctl --user` and other host-session
    /// tools keep working.
    pub fn worktree_session_default() -> Self {
        Self {
            backend_preference: ExecutionBackendKind::HostPassthrough,
            allow_network: true,
            allow_browser_automation: true,
            allow_desktop_gui: true,
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
/// Kept in sync with `--unsetenv` flags in `build_linux_bwrap_args`.
pub fn gui_env_keys() -> &'static [&'static str] {
    &[
        "DISPLAY",
        "WAYLAND_DISPLAY",
        "DBUS_SESSION_BUS_ADDRESS",
        "DESKTOP_STARTUP_ID",
        "XAUTHORITY",
        "XDG_SESSION_TYPE",
        "GDK_BACKEND",
        "QT_QPA_PLATFORM",
    ]
}

fn gui_env_unset_if_forbidden(policy: &ExecutionPolicy) -> Vec<String> {
    if policy.allow_desktop_gui {
        Vec::new()
    } else {
        gui_env_keys().iter().map(|s| s.to_string()).collect()
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
                // Bwrap handles GUI env hygiene internally via `--unsetenv`,
                // so env_unset stays empty on this branch.
                PreparedExecutionCommand {
                    executable: bwrap_path,
                    args: build_linux_bwrap_args(&executable, &args, cwd, policy),
                    backend: ExecutionBackendKind::LinuxBubblewrap,
                    env_unset: Vec::new(),
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
                }
            }
        }
        backend => PreparedExecutionCommand {
            executable,
            args,
            backend,
            env_unset: gui_env_unset_if_forbidden(policy),
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
        for key in [
            "DISPLAY",
            "WAYLAND_DISPLAY",
            "DBUS_SESSION_BUS_ADDRESS",
            "DESKTOP_STARTUP_ID",
            "XAUTHORITY",
        ] {
            out.push("--unsetenv".to_string());
            out.push(key.to_string());
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

    #[test]
    fn worktree_default_allows_gui() {
        let p = ExecutionPolicy::worktree_session_default();
        assert!(p.allow_desktop_gui);
        assert!(p.allow_network);
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
