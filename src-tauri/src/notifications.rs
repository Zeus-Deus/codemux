use std::path::Path;
use std::process::{Command, Stdio};

use tauri::{AppHandle, Manager};

use crate::state::AppStateStore;

#[cfg(target_os = "linux")]
const FREEDESKTOP_COMPLETE: &str = "/usr/share/sounds/freedesktop/stereo/complete.oga";
#[cfg(target_os = "macos")]
const MACOS_COMPLETE: &str = "/System/Library/Sounds/Glass.aiff";

/// Whether a desktop notification should be suppressed because the user is
/// already looking at the pane. Mirrors Superset's `shouldSuppressForVisiblePane`:
/// suppress only if the window is focused AND the agent's pane is in the
/// currently-active workspace.
pub fn should_suppress(app: &AppHandle, pane_in_active_workspace: bool) -> bool {
    let window_focused = app
        .get_webview_window("main")
        .and_then(|w| w.is_focused().ok())
        .unwrap_or(false);
    window_focused && pane_in_active_workspace
}

/// Show a "agent finished" desktop notification and play the completion sound.
/// Clicking the notification focuses the Codemux window (and on Hyprland,
/// jumps to whichever workspace it lives on).
pub fn dispatch_agent_complete(app: &AppHandle, workspace_title: &str) {
    let summary = format!("Agent finished — {}", workspace_title);
    let body = "Codemux is waiting for your review.".to_string();

    show_desktop_notification(app, &summary, &body);
    play_completion_sound(app);
}

fn show_desktop_notification(app: &AppHandle, summary: &str, body: &str) {
    let mut notification = notify_rust::Notification::new();
    notification.summary(summary).body(body);

    #[cfg(unix)]
    {
        notification
            .hint(notify_rust::Hint::DesktopEntry(
                "com.codemux.app".to_string(),
            ))
            .hint(notify_rust::Hint::Transient(true))
            .urgency(notify_rust::Urgency::Critical)
            .action("default", "Open");
    }

    let handle = match notification.show() {
        Ok(h) => h,
        Err(err) => {
            eprintln!("[codemux::notifications] notify-rust show failed: {err}");
            return;
        }
    };

    // On Linux, wait for the user to click the notification (the libnotify
    // "default" action mako fires on left-click) and then focus the app.
    // On other platforms `wait_for_action` is a no-op or unavailable, so the
    // notify-rust click handler is the only path; we still spawn a thread so
    // the caller doesn't block.
    #[cfg(target_os = "linux")]
    {
        let app_clone = app.clone();
        std::thread::spawn(move || {
            handle.wait_for_action(|action| {
                if action == "default" {
                    focus_app(&app_clone);
                }
            });
        });
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = handle;
        let _ = app;
    }
}

/// Bring the Codemux window to the foreground. On Hyprland this includes
/// jumping to whichever workspace the window currently lives on.
fn focus_app(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
        let _ = window.request_user_attention(Some(tauri::UserAttentionType::Critical));
    }

    #[cfg(target_os = "linux")]
    {
        let _ = Command::new("hyprctl")
            .args(["dispatch", "focuswindow", "class:com.codemux.app"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .output();
    }
}

fn play_completion_sound(app: &AppHandle) {
    let state: tauri::State<'_, AppStateStore> = app.state();
    if !state.snapshot().config.notification_sound_enabled {
        return;
    }

    #[cfg(target_os = "linux")]
    {
        if Path::new(FREEDESKTOP_COMPLETE).exists() {
            let _ = Command::new("paplay")
                .arg(FREEDESKTOP_COMPLETE)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
        }
    }

    #[cfg(target_os = "macos")]
    {
        if Path::new(MACOS_COMPLETE).exists() {
            let _ = Command::new("afplay")
                .arg(MACOS_COMPLETE)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
        }
    }

    #[cfg(target_os = "windows")]
    {
        let _ = Command::new("powershell")
            .args(["-NoProfile", "-Command", "[System.Media.SystemSounds]::Asterisk.Play()"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}
