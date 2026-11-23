//! Desktop integration that must behave the same on every OS: Codemux never
//! changes compositor configuration and never takes focus the user did not
//! ask for.
//!
//! Proper per-platform activation (xdg-activation tokens, a macOS
//! UNUserNotificationCenter delegate, Windows registered toast activation) is
//! tracked on the `feat/notification-activation` branch.

use tauri::{AppHandle, Manager, Runtime};

/// Raise and focus the main window in response to a user action, such as a
/// click on a native notification.
///
/// Tauri's show/unminimize/set_focus raise the window on macOS, Windows and
/// X11. Wayland only lets a client take focus with an xdg-activation token
/// from the user's action. The notification daemon sends one with the click
/// (the freedesktop `ActivationToken` signal), but notify-rust 4.18 drops it
/// and tao cannot present a window with a token, so Hyprland falls back to
/// `hyprctl`.
pub fn focus_main_window<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }

    #[cfg(target_os = "linux")]
    hyprland::focus_own_window();
}

/// Focus fallback for Hyprland, run once as the direct response to a click.
#[cfg(target_os = "linux")]
mod hyprland {
    use std::io::Read;
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    /// hyprctl answers over a local socket, so a reply takes milliseconds. The
    /// bound only keeps a hung compositor from parking the caller.
    const HYPRCTL_TIMEOUT: Duration = Duration::from_secs(1);

    /// Ask Hyprland to focus this process's window, switching to its
    /// workspace. Does nothing outside a Hyprland session. Matching by pid
    /// rather than class keeps a second Codemux instance, or another app
    /// sharing the app id, from being focused instead.
    ///
    /// Hyprland 0.55+ with a Lua config evaluates `hyprctl dispatch` arguments
    /// as Lua and rejects the old dispatcher syntax; older Hyprland and
    /// hyprlang configs reject the Lua form. hyprctl exits 0 for most rejected
    /// dispatches, so the reply, `ok` on success, decides whether to fall back.
    pub(super) fn focus_own_window() {
        if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() {
            return;
        }
        let pid = std::process::id();
        let mut reply = String::new();
        for args in [lua_focus_args(pid), legacy_focus_args(pid)] {
            match run_hyprctl(&args) {
                Ok(output) if is_ok(&output) => return,
                Ok(output) => reply = output,
                Err(err) => {
                    eprintln!("[codemux::desktop] hyprctl failed: {err}");
                    return;
                }
            }
        }
        eprintln!(
            "[codemux::desktop] hyprctl could not focus Codemux: {}",
            reply.trim()
        );
    }

    /// Run hyprctl and return its stdout, killing it if it outlives
    /// [`HYPRCTL_TIMEOUT`].
    fn run_hyprctl(args: &[String]) -> std::io::Result<String> {
        // `host_command` strips AppImage library paths so the host hyprctl
        // loads its own libraries.
        let mut child = crate::execution::host_command("hyprctl")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let deadline = Instant::now() + HYPRCTL_TIMEOUT;
        while child.try_wait()?.is_none() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "no reply from hyprctl",
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut reply = String::new();
        if let Some(mut stdout) = child.stdout.take() {
            stdout.read_to_string(&mut reply)?;
        }
        Ok(reply)
    }

    /// `hl.dsp.focus` for Lua-config Hyprland (0.55+), as Omarchy dispatches it.
    fn lua_focus_args(pid: u32) -> Vec<String> {
        vec![
            "dispatch".to_string(),
            format!("hl.dsp.focus({{ window = \"pid:{pid}\" }})"),
        ]
    }

    /// The `focuswindow` dispatcher of older Hyprland and hyprlang configs.
    fn legacy_focus_args(pid: u32) -> Vec<String> {
        vec![
            "dispatch".to_string(),
            "focuswindow".to_string(),
            format!("pid:{pid}"),
        ]
    }

    fn is_ok(reply: &str) -> bool {
        reply.trim() == "ok"
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn lua_dispatch_focuses_the_window_by_pid() {
            assert_eq!(
                lua_focus_args(4242),
                vec!["dispatch", r#"hl.dsp.focus({ window = "pid:4242" })"#]
            );
        }

        #[test]
        fn legacy_dispatch_uses_focuswindow() {
            assert_eq!(
                legacy_focus_args(4242),
                vec!["dispatch", "focuswindow", "pid:4242"]
            );
        }

        #[test]
        fn only_an_ok_reply_counts_as_success() {
            assert!(is_ok("ok"));
            assert!(is_ok("ok\n"));
            // Hyprland 0.57 replies with a Lua error to the legacy syntax, and
            // with this to any dispatch when the config is not Lua.
            assert!(!is_ok("current config provider doesn't support dispatch"));
            assert!(!is_ok(
                "error: [string \"return hl.dispatch(focuswindow pid:1)\"]:1: ')' expected"
            ));
            assert!(!is_ok("Invalid dispatcher"));
            assert!(!is_ok(""));
        }
    }
}
