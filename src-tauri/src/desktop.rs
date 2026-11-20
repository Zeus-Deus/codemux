//! Desktop integration that must behave the same on every OS: Codemux never
//! changes compositor configuration and never takes focus the user did not
//! ask for.

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
    focus_own_hyprland_window();
}

/// Ask Hyprland to focus this process's window, switching to its workspace.
/// Does nothing outside a Hyprland session, and the result is ignored because
/// this is a best-effort fallback. Matching by pid rather than class keeps a
/// second Codemux instance, or another app sharing the app id, from being
/// focused instead.
#[cfg(target_os = "linux")]
fn focus_own_hyprland_window() {
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() {
        return;
    }
    // `host_command` strips AppImage library paths so the host hyprctl loads
    // its own libraries.
    let _ = crate::execution::host_command("hyprctl")
        .args(["dispatch", "focuswindow", &format!("pid:{}", std::process::id())])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}
