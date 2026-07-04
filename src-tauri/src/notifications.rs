use std::path::Path;
use std::process::{Command, Stdio};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::state::AppStateStore;

/// Global event mirrored to web clients alongside the native desktop
/// notification. Stage 3b's frontend surfaces it via the Web Notifications API
/// (with a toast fallback); the desktop path is unchanged.
const NOTIFICATION_EVENT: &str = "notification";

/// Payload for the global [`NOTIFICATION_EVENT`]. Serialized snake_case so web
/// clients see stable field names. `title`/`body` carry exactly what the
/// native notification shows; `workspace_title` is the originating workspace.
#[derive(Debug, Clone, Serialize)]
pub struct NotificationPayload {
    pub title: String,
    pub body: String,
    pub workspace_title: String,
}

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
    let payload = agent_complete_payload(workspace_title);

    // Native desktop path — byte-identical to before: same summary/body
    // strings, same show + sound calls in the same order.
    show_desktop_notification(app, &payload.title, &payload.body);
    play_completion_sound(app);

    // Web-remote bridge: mirror the same notification onto the global event
    // bus so a paired browser can raise an OS notification client-side (Stage
    // 3b). Purely additive — emitting is independent of the native path and a
    // no-op when nobody is listening. Web-side suppression (e.g. the tab is
    // focused) is the frontend's policy, not ours.
    let _ = app.emit(NOTIFICATION_EVENT, payload);
}

/// Build the payload for an "agent finished" notification. Split out so the
/// title/body/serialization contract is unit-testable without firing a real
/// desktop notification. The strings must match the native summary/body so the
/// web and desktop notifications read identically.
fn agent_complete_payload(workspace_title: &str) -> NotificationPayload {
    NotificationPayload {
        title: format!("Agent finished — {}", workspace_title),
        body: "Codemux is waiting for your review.".to_string(),
        workspace_title: workspace_title.to_string(),
    }
}

fn show_desktop_notification(app: &AppHandle, summary: &str, body: &str) {
    let mut notification = notify_rust::Notification::new();
    notification.summary(summary).body(body);

    #[cfg(unix)]
    {
        notification
            .hint(notify_rust::Hint::DesktopEntry(
                app.config().identifier.clone(),
            ))
            .hint(notify_rust::Hint::Transient(true))
            // Use Normal urgency, not Critical. On every common Linux
            // notification daemon (mako, dunst, GNOME Shell, KDE Plasma,
            // xfce4-notifyd), Critical is reserved for emergencies and
            // is intentionally non-expiring — the popup stays on screen
            // until the user clicks it. "Agent finished" is routine, so
            // Normal is the correct level and lets the daemon's normal
            // expire-timeout dismiss the popup automatically.
            .urgency(notify_rust::Urgency::Normal)
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
        let class = format!("class:{}", app.config().identifier);
        let _ = Command::new("hyprctl")
            .args(["dispatch", "focuswindow", &class])
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_complete_payload_matches_native_strings_and_serializes_snake_case() {
        let payload = agent_complete_payload("My Project");
        // Must equal the strings the native notification renders.
        assert_eq!(payload.title, "Agent finished — My Project");
        assert_eq!(payload.body, "Codemux is waiting for your review.");
        assert_eq!(payload.workspace_title, "My Project");

        // The web bridge event carries snake_case keys web clients rely on.
        let v = serde_json::to_value(&payload).unwrap();
        assert_eq!(v["title"], "Agent finished — My Project");
        assert_eq!(v["body"], "Codemux is waiting for your review.");
        assert_eq!(v["workspace_title"], "My Project");
        assert!(v.get("workspaceTitle").is_none(), "no camelCase leakage");
    }
}
