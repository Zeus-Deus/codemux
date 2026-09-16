//! Main-window recovery when the WebKitGTK web process dies (Linux).
//!
//! The React UI runs in a separate `WebKitWebProcess`. When that process
//! crashes or is killed, WebKit leaves the window blank and nothing in the
//! page can run — not even its keyboard shortcuts. The backend, the PTY
//! daemon, and every agent sidecar live outside that process and keep
//! working, so reloading the page is all it takes to get the UI back: panes
//! re-attach to their sessions and replay from the backend.
//!
//! Two recovery paths, both handled in the app (UI) process:
//!
//! 1. **Auto-reload** on `web-process-terminated`, capped at
//!    [`MAX_AUTO_RELOADS`] per [`AUTO_RELOAD_WINDOW`] so a page that keeps
//!    crashing on load cannot spin in a reload loop.
//! 2. **Manual reload** — Ctrl+R, Ctrl+Shift+R, or F5 reloads the page, but
//!    only while the renderer is dead. On a live page those keys pass straight
//!    through, so the frontend keeps blocking accidental reloads and terminal
//!    panes keep their own Ctrl+R.

use std::time::{Duration, Instant};

/// Automatic reloads allowed within [`AUTO_RELOAD_WINDOW`] before recovery
/// falls back to the manual shortcut.
pub const MAX_AUTO_RELOADS: usize = 3;
/// Sliding window the auto-reload cap is counted over.
pub const AUTO_RELOAD_WINDOW: Duration = Duration::from_secs(60);

/// Short pause before reloading, so WebKit finishes tearing down the dead
/// process before a new one is spawned.
#[cfg(target_os = "linux")]
const AUTO_RELOAD_DELAY: Duration = Duration::from_millis(250);

/// Whether another automatic reload may run, given when earlier ones did.
pub fn auto_reload_allowed(previous: &[Instant], now: Instant) -> bool {
    let recent = previous
        .iter()
        .filter(|at| now.saturating_duration_since(**at) < AUTO_RELOAD_WINDOW)
        .count();
    recent < MAX_AUTO_RELOADS
}

/// The keys the manual reload shortcut cares about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShortcutKey {
    R,
    F5,
    Other,
}

/// Ctrl+R (with or without Shift) or a bare F5. Alt and Super combos are left
/// alone so they never collide with window-manager or app bindings.
pub fn is_reload_shortcut(key: ShortcutKey, ctrl: bool, alt: bool, logo: bool) -> bool {
    if alt || logo {
        return false;
    }
    match key {
        ShortcutKey::R => ctrl,
        ShortcutKey::F5 => !ctrl,
        ShortcutKey::Other => false,
    }
}

/// Wire crash recovery onto a webview window. Linux-only in effect; other
/// platforms use a different webview engine and this is a no-op there.
#[allow(unused_variables)]
pub fn install<R: tauri::Runtime>(window: &tauri::WebviewWindow<R>) {
    #[cfg(target_os = "linux")]
    {
        let result = window.with_webview(|platform| linux::attach(platform.inner()));
        if let Err(error) = result {
            log::warn!("[codemux::webview] could not install renderer crash recovery: {error}");
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use gtk::prelude::*;
    use gtk::{gdk, glib};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use webkit2gtk::{LoadEvent, WebProcessTerminationReason, WebView, WebViewExt};

    #[derive(Default)]
    struct Recovery {
        /// True from `web-process-terminated` until a new page commits.
        renderer_dead: Cell<bool>,
        /// When recent automatic reloads ran, for the loop cap.
        auto_reloads: RefCell<Vec<Instant>>,
    }

    pub(super) fn attach(view: WebView) {
        let recovery = Rc::new(Recovery::default());

        view.connect_web_process_terminated({
            let recovery = recovery.clone();
            move |view, reason| {
                recovery.renderer_dead.set(true);
                if reason == WebProcessTerminationReason::TerminatedByApi {
                    return;
                }

                let now = Instant::now();
                let allowed = {
                    let mut reloads = recovery.auto_reloads.borrow_mut();
                    reloads.retain(|at| now.saturating_duration_since(*at) < AUTO_RELOAD_WINDOW);
                    let allowed = auto_reload_allowed(&reloads, now);
                    if allowed {
                        reloads.push(now);
                    }
                    allowed
                };
                if !allowed {
                    log::error!(
                        "[codemux::webview] web process terminated ({reason:?}) again; \
                         auto-reload paused, press Ctrl+R to reload"
                    );
                    return;
                }

                log::warn!("[codemux::webview] web process terminated ({reason:?}); reloading");
                let view = view.downgrade();
                glib::timeout_add_local_once(AUTO_RELOAD_DELAY, move || {
                    if let Some(view) = view.upgrade() {
                        view.reload();
                    }
                });
            }
        });

        view.connect_load_changed({
            let recovery = recovery.clone();
            move |_, event| {
                if event == LoadEvent::Committed {
                    recovery.renderer_dead.set(false);
                }
            }
        });

        // Listen on the toplevel window so the shortcut works wherever GTK
        // focus ended up when the page died.
        let key_target: gtk::Widget = view
            .toplevel()
            .filter(|widget| widget.is::<gtk::Window>())
            .unwrap_or_else(|| view.clone().upcast());
        let weak_view = view.downgrade();
        key_target.connect_key_press_event(move |_, event| {
            if !recovery.renderer_dead.get() {
                return glib::Propagation::Proceed;
            }
            let key = match event.keyval().to_lower() {
                gdk::keys::constants::r => ShortcutKey::R,
                gdk::keys::constants::F5 => ShortcutKey::F5,
                _ => ShortcutKey::Other,
            };
            let modifiers = event.state();
            if !is_reload_shortcut(
                key,
                modifiers.contains(gdk::ModifierType::CONTROL_MASK),
                modifiers.contains(gdk::ModifierType::MOD1_MASK),
                modifiers.contains(gdk::ModifierType::SUPER_MASK),
            ) {
                return glib::Propagation::Proceed;
            }
            if let Some(view) = weak_view.upgrade() {
                log::warn!("[codemux::webview] reloading dead renderer from keyboard shortcut");
                view.reload();
            }
            glib::Propagation::Stop
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_reloads_until_the_cap() {
        let now = Instant::now();
        assert!(auto_reload_allowed(&[], now));
        let two = vec![now - Duration::from_secs(5); MAX_AUTO_RELOADS - 1];
        assert!(auto_reload_allowed(&two, now));
        let full = vec![now - Duration::from_secs(5); MAX_AUTO_RELOADS];
        assert!(!auto_reload_allowed(&full, now));
    }

    #[test]
    fn reloads_outside_the_window_do_not_count() {
        let now = Instant::now() + AUTO_RELOAD_WINDOW * 2;
        let stale = vec![now - AUTO_RELOAD_WINDOW - Duration::from_secs(1); MAX_AUTO_RELOADS];
        assert!(auto_reload_allowed(&stale, now));
    }

    #[test]
    fn matches_ctrl_r_and_f5_only() {
        assert!(is_reload_shortcut(ShortcutKey::R, true, false, false));
        assert!(is_reload_shortcut(ShortcutKey::F5, false, false, false));
        assert!(!is_reload_shortcut(ShortcutKey::R, false, false, false));
        assert!(!is_reload_shortcut(ShortcutKey::F5, true, false, false));
        assert!(!is_reload_shortcut(ShortcutKey::R, true, true, false));
        assert!(!is_reload_shortcut(ShortcutKey::R, true, false, true));
        assert!(!is_reload_shortcut(ShortcutKey::Other, true, false, false));
    }
}
