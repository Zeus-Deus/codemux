use std::path::Path;
use std::process::{Command, Stdio};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::state::NotificationTarget;

/// Global event mirrored to web clients alongside the native desktop
/// notification. Stage 3b's frontend surfaces it via the Web Notifications API
/// (with a toast fallback); the desktop path is unchanged.
const NOTIFICATION_EVENT: &str = "notification";

/// Emitted when the user clicks a native notification, so the desktop
/// frontend can switch to the workspace and pane the notification is about.
const NOTIFICATION_ACTIVATE_EVENT: &str = "notification-activate";

/// What an agent notification announces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentNotice {
    /// The turn is over and the output is waiting for review.
    Finished,
    /// The agent is blocked on an approval or a question.
    NeedsInput,
}

/// Payload for the global [`NOTIFICATION_EVENT`]. Serialized snake_case so web
/// clients see stable field names. `title`/`body` carry exactly what the
/// native notification shows; `workspace_id`/`pane_id` say where opening the
/// notification should lead.
#[derive(Debug, Clone, Serialize)]
pub struct NotificationPayload {
    pub title: String,
    pub body: String,
    pub workspace_title: String,
    pub workspace_id: String,
    pub pane_id: String,
}

/// Payload for [`NOTIFICATION_ACTIVATE_EVENT`].
#[derive(Debug, Clone, Serialize)]
struct NotificationActivatePayload {
    workspace_id: String,
    pane_id: String,
}

#[cfg(target_os = "linux")]
const FREEDESKTOP_COMPLETE: &str = "/usr/share/sounds/freedesktop/stereo/complete.oga";
#[cfg(target_os = "macos")]
const MACOS_COMPLETE: &str = "/System/Library/Sounds/Glass.aiff";

/// Whether to raise a notification for `target`. A muted workspace never
/// notifies. Otherwise mirror Superset's `shouldSuppressForVisiblePane`: stay
/// quiet only if the window is focused AND the agent's pane is in the
/// currently-active workspace, because the user is already looking at it.
fn should_notify(target: &NotificationTarget, window_focused: bool) -> bool {
    !target.muted && !(window_focused && target.in_active_workspace)
}

fn main_window_focused<R: Runtime>(app: &AppHandle<R>) -> bool {
    app.get_webview_window("main")
        .and_then(|w| w.is_focused().ok())
        .unwrap_or(false)
}

/// Tell the user about an agent in `target`, unless its workspace is muted or
/// they are already looking at it. The native popup and the sound follow the
/// synced notification settings; clicking the popup opens the agent's pane.
///
/// Must stay callable from async context: chat agents reach this from their
/// event bridge, a tokio task, so nothing here may block on a runtime (see
/// [`show_desktop_notification`]).
pub fn notify_agent<R: Runtime>(
    app: &AppHandle<R>,
    notice: AgentNotice,
    target: &NotificationTarget,
) {
    if !should_notify(target, main_window_focused(app)) {
        return;
    }
    let payload = agent_payload(notice, target);

    // Gated on GUI mode: under headless serve there is no notification
    // daemon, focusable window, or audio device. The global `notification`
    // event below still fires unconditionally — the web client consumes it.
    if crate::app_mode(app) == crate::AppMode::Gui {
        let prefs = crate::settings_sync::load_cache()
            .map(|settings| settings.notifications)
            .unwrap_or_default();
        if prefs.desktop_enabled {
            show_desktop_notification(app, &payload);
        }
        if prefs.sound_enabled {
            play_completion_sound();
        }
    }

    // Web-remote bridge: mirror the same notification onto the global event
    // bus so a paired browser can raise an OS notification client-side (Stage
    // 3b). Purely additive — emitting is independent of the native path and a
    // no-op when nobody is listening. Web-side suppression (e.g. the tab is
    // focused) is the frontend's policy, not ours.
    let _ = app.emit(NOTIFICATION_EVENT, payload);
}

/// Build the payload for an agent notification. Split out so the
/// title/body/serialization contract is unit-testable without firing a real
/// desktop notification. The native popup renders exactly these strings, so
/// the web and desktop notifications read identically.
fn agent_payload(notice: AgentNotice, target: &NotificationTarget) -> NotificationPayload {
    let (headline, body) = match notice {
        AgentNotice::Finished => ("Agent finished", "Codemux is waiting for your review."),
        AgentNotice::NeedsInput => (
            "Agent needs your input",
            "Approve or answer to let it continue.",
        ),
    };
    NotificationPayload {
        title: format!("{headline} — {}", target.workspace_title),
        body: body.to_string(),
        workspace_title: target.workspace_title.clone(),
        workspace_id: target.workspace_id.clone(),
        pane_id: target.pane_id.clone(),
    }
}

/// Raise the native popup on a dedicated OS thread. On Linux notify-rust's
/// `show()` drives D-Bus through `zbus::block_on`, which with zbus's `tokio`
/// feature enabled (pulled in by the dialog plugin's xdg portal) enters a
/// tokio runtime's `block_on` and panics with "Cannot start a runtime from
/// within a runtime" when called from a tokio worker. Chat agents notify from
/// their async event bridge, so a panic there would kill the bridge and stop
/// every later event from that provider. A plain thread also keeps
/// `wait_for_action` from blocking the caller.
fn show_desktop_notification<R: Runtime>(app: &AppHandle<R>, payload: &NotificationPayload) {
    let app = app.clone();
    let payload = payload.clone();
    std::thread::spawn(move || show_desktop_notification_blocking(&app, &payload));
}

fn show_desktop_notification_blocking<R: Runtime>(
    app: &AppHandle<R>,
    payload: &NotificationPayload,
) {
    let mut notification = notify_rust::Notification::new();
    notification.summary(&payload.title).body(&payload.body);

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
    // "default" action mako fires on left-click), then focus the app and
    // open the agent's pane. This already runs on its own thread, so the wait
    // does not block the caller. On other platforms `wait_for_action` is a
    // no-op or unavailable, so the notify-rust click handler is the only path.
    #[cfg(target_os = "linux")]
    handle.wait_for_action(|action| {
        if action == "default" {
            focus_app(app);
            let _ = app.emit(
                NOTIFICATION_ACTIVATE_EVENT,
                NotificationActivatePayload {
                    workspace_id: payload.workspace_id.clone(),
                    pane_id: payload.pane_id.clone(),
                },
            );
        }
    });

    #[cfg(not(target_os = "linux"))]
    {
        let _ = handle;
        let _ = app;
    }
}

/// Bring the Codemux window to the foreground. On Hyprland this includes
/// jumping to whichever workspace the window currently lives on.
fn focus_app<R: Runtime>(app: &AppHandle<R>) {
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

fn play_completion_sound() {
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
            .args([
                "-NoProfile",
                "-Command",
                "[System.Media.SystemSounds]::Asterisk.Play()",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> NotificationTarget {
        NotificationTarget {
            workspace_id: "ws-1".into(),
            workspace_title: "My Project".into(),
            pane_id: "pane-7".into(),
            muted: false,
            in_active_workspace: false,
        }
    }

    #[test]
    fn finished_payload_matches_native_strings_and_serializes_snake_case() {
        let payload = agent_payload(AgentNotice::Finished, &target());
        // Must equal the strings the native notification renders.
        assert_eq!(payload.title, "Agent finished — My Project");
        assert_eq!(payload.body, "Codemux is waiting for your review.");
        assert_eq!(payload.workspace_title, "My Project");

        // The web bridge event carries snake_case keys web clients rely on,
        // including where opening the notification should lead.
        let v = serde_json::to_value(&payload).unwrap();
        assert_eq!(v["title"], "Agent finished — My Project");
        assert_eq!(v["body"], "Codemux is waiting for your review.");
        assert_eq!(v["workspace_title"], "My Project");
        assert_eq!(v["workspace_id"], "ws-1");
        assert_eq!(v["pane_id"], "pane-7");
        assert!(v.get("workspaceTitle").is_none(), "no camelCase leakage");
    }

    #[test]
    fn notifies_unless_muted_or_already_on_screen() {
        let background = target();
        assert!(should_notify(&background, true));
        assert!(should_notify(&background, false));

        let visible = NotificationTarget {
            in_active_workspace: true,
            ..target()
        };
        assert!(!should_notify(&visible, true));
        // Active workspace but Codemux is behind another window.
        assert!(should_notify(&visible, false));

        let muted = NotificationTarget {
            muted: true,
            ..target()
        };
        assert!(!should_notify(&muted, false));
    }

    #[test]
    fn needs_input_payload_says_the_agent_is_blocked() {
        let payload = agent_payload(AgentNotice::NeedsInput, &target());
        assert_eq!(payload.title, "Agent needs your input — My Project");
        assert_eq!(payload.body, "Approve or answer to let it continue.");
        assert_eq!(payload.pane_id, "pane-7");
    }
}
