use serde::{Deserialize, Serialize};
use std::env;
use std::path::Path;

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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutionPolicy {
    pub backend_preference: ExecutionBackendKind,
    pub allow_network: bool,
    pub allow_browser_automation: bool,
    pub allow_desktop_gui: bool,
}

impl ExecutionPolicy {
    /// Default policy for OpenFlow agents:
    /// - they can still build/test/network
    /// - browser automation remains allowed
    /// - direct host GUI launching should eventually be isolated
    pub fn openflow_agent_default() -> Self {
        Self {
            backend_preference: if cfg!(target_os = "linux") {
                ExecutionBackendKind::LinuxBubblewrap
            } else if cfg!(target_os = "macos") {
                ExecutionBackendKind::MacOsSandbox
            } else if cfg!(target_os = "windows") {
                ExecutionBackendKind::WindowsRestricted
            } else {
                ExecutionBackendKind::HostPassthrough
            },
            allow_network: true,
            allow_browser_automation: true,
            allow_desktop_gui: false,
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
                PreparedExecutionCommand {
                    executable: bwrap_path,
                    args: build_linux_bwrap_args(&executable, &args, cwd, policy),
                    backend: ExecutionBackendKind::LinuxBubblewrap,
                }
            } else {
                crate::diagnostics::stderr_line(
                    "[codemux::execution] Bubblewrap requested but not found; falling back to host passthrough",
                );
                PreparedExecutionCommand {
                    executable,
                    args,
                    backend: ExecutionBackendKind::HostPassthrough,
                }
            }
        }
        backend => PreparedExecutionCommand {
            executable,
            args,
            backend,
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
