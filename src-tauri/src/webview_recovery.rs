//! Recovery for a main window whose page can no longer help itself.
//!
//! The React UI runs in its own renderer process — `WebKitWebProcess` on
//! Linux, the WebView2/WKWebView renderer elsewhere. When that process dies
//! or wedges, the window goes blank or stops responding and nothing in the
//! page can run, not even its keyboard shortcuts. The backend, the PTY
//! daemon, and every agent sidecar live outside that process and keep
//! working, so reloading *only the page* is all it takes to get the UI back:
//! panes re-attach to their sessions and replay from the backend. Nothing
//! here restarts Codemux, the PTY daemon, sidecars, agents or commands.
//!
//! Three recovery paths:
//!
//! 1. **Auto-reload** on `web-process-terminated` (Linux), capped at
//!    [`MAX_AUTO_RELOADS`] per [`AUTO_RELOAD_WINDOW`] so a page that keeps
//!    crashing on load cannot spin in a reload loop.
//! 2. **The recovery chord** — [`RECOVERY_CHORD`] (Ctrl+Shift+R). On Linux it
//!    is handled in the app process, on the GTK toplevel, *before* the key
//!    reaches the page, so it works when the renderer is dead or wedged and
//!    React never sees a keystroke.
//! 3. **[`reload_main_webview`]** — the same reload as a command, for the
//!    in-app action (command palette, the confirmation dialog, and the chord
//!    itself on macOS/Windows, where a page that can dispatch a keybind is by
//!    definition alive).
//!
//! The chord does not yank a working UI out from under anyone. While the
//! renderer is alive the app process *asks* first: it emits
//! [`RELOAD_REQUESTED_EVENT`] and gives the page [`ACK_GRACE`] to answer with
//! [`ack_reload_request`]. A page that answers owns the flow and confirms with
//! the user; a page that stays silent is wedged, and the watchdog reloads it.
//!
//! Ctrl+R and F5 are *not* recovery keys: Ctrl+R is reverse-i-search in every
//! terminal pane and the frontend deliberately swallows both to stop
//! accidental reloads. They do reload, but only while the renderer is known
//! dead — at that point there is no page left to break and no pty that could
//! receive the key anyway.

use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tauri::Manager;

/// Window label the recovery reload targets. Headless `codemux serve` has no
/// such window and every entry point here no-ops there.
const MAIN_WINDOW: &str = "main";

/// The documented, always-available recovery chord. Kept in sync with the
/// `reloadInterface` entry in `src/lib/keybind-registry.ts`.
pub const RECOVERY_CHORD: &str = "Ctrl+Shift+R";

/// Sent to a live page when the recovery chord is pressed, so the page can
/// confirm with the user instead of being reloaded from under them.
pub const RELOAD_REQUESTED_EVENT: &str = "codemux://reload-requested";

/// How long the app process waits for a page to answer a reload request
/// before treating it as wedged and reloading it anyway. Generous enough that
/// a page busy with a render pass still answers in time.
pub const ACK_GRACE: Duration = Duration::from_millis(1000);

/// Reload requests handed to the page so far. Monotonic, so a late answer to
/// an old request cannot cancel a newer one.
static REQUEST_SEQ: AtomicU64 = AtomicU64::new(0);
/// Highest request the page has answered.
static ACKED_SEQ: AtomicU64 = AtomicU64::new(0);

/// Payload of [`RELOAD_REQUESTED_EVENT`]: the request the page must echo back
/// through [`ack_reload_request`] to claim the flow.
#[derive(Clone, Serialize)]
pub struct ReloadRequest {
    pub token: u64,
}

/// Whether the watchdog armed for `request` should reload — i.e. whether the
/// page stayed silent through [`ACK_GRACE`].
pub fn watchdog_should_reload(request: u64, acked: u64) -> bool {
    acked < request
}

/// The page answering a reload request: it is alive and will ask the user.
#[tauri::command]
pub fn ack_reload_request(token: u64) {
    ACKED_SEQ.fetch_max(token, Ordering::Relaxed);
}

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

/// The keys any reload path cares about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShortcutKey {
    R,
    F5,
    Other,
}

/// What a keystroke means to the recovery layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReloadTrigger {
    /// [`RECOVERY_CHORD`]: reloads whatever state the page is in.
    Recovery,
    /// Ctrl+R / F5: reloads only while the renderer is known dead, so a live
    /// page keeps its own handling of these keys.
    DeadRendererOnly,
    /// Not a reload key.
    None,
}

/// Classify a keystroke. Alt and Super combos are left alone so they never
/// collide with window-manager or app bindings.
pub fn classify(key: ShortcutKey, ctrl: bool, shift: bool, alt: bool, logo: bool) -> ReloadTrigger {
    if alt || logo {
        return ReloadTrigger::None;
    }
    match key {
        ShortcutKey::R if ctrl && shift => ReloadTrigger::Recovery,
        ShortcutKey::R if ctrl => ReloadTrigger::DeadRendererOnly,
        ShortcutKey::F5 if !ctrl && !shift => ReloadTrigger::DeadRendererOnly,
        _ => ReloadTrigger::None,
    }
}

/// Whether a classified keystroke should reload the page right now.
pub fn should_reload(trigger: ReloadTrigger, renderer_dead: bool) -> bool {
    match trigger {
        ReloadTrigger::Recovery => true,
        ReloadTrigger::DeadRendererOnly => renderer_dead,
        ReloadTrigger::None => false,
    }
}

/// Reload the main window's page, and nothing else.
///
/// This is a plain webview navigation: the Codemux process, the PTY daemon,
/// provider sidecars, agents and running commands are untouched, and the
/// fresh page re-attaches to them on load.
pub fn reload_main_webview<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<(), String> {
    let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
        // Headless `codemux serve`, or a window that is already gone.
        return Err("no main window to reload".to_string());
    };
    log::warn!("[codemux::webview] reloading the main webview (recovery)");
    window.reload().map_err(|error| error.to_string())
}

/// Reload the UI from the in-app action. Backend state is deliberately left
/// alone — see [`reload_main_webview`].
#[tauri::command]
pub fn reload_main_window<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Result<(), String> {
    reload_main_webview(&app)
}

/// Wire crash recovery onto a webview window. Linux-only in effect: other
/// platforms reach the same reload through the in-app action, because their
/// webviews do not hand the app process a pre-page key hook.
#[allow(unused_variables)]
pub fn install<R: tauri::Runtime>(window: &tauri::WebviewWindow<R>) {
    #[cfg(target_os = "linux")]
    {
        let handle = window.clone();
        let result = window.with_webview(move |platform| linux::attach(platform.inner(), handle));
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

    pub(super) fn attach<R: tauri::Runtime>(view: WebView, window: tauri::WebviewWindow<R>) {
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
                         auto-reload paused, press {RECOVERY_CHORD} to reload"
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
        // focus ended up, and — for the recovery chord — before the key is
        // forwarded to the page or to a focused terminal pane.
        let key_target: gtk::Widget = view
            .toplevel()
            .filter(|widget| widget.is::<gtk::Window>())
            .unwrap_or_else(|| view.clone().upcast());
        let weak_view = view.downgrade();
        key_target.connect_key_press_event(move |_, event| {
            let key = match event.keyval().to_lower() {
                gdk::keys::constants::r => ShortcutKey::R,
                gdk::keys::constants::F5 => ShortcutKey::F5,
                _ => ShortcutKey::Other,
            };
            let modifiers = event.state();
            let trigger = classify(
                key,
                modifiers.contains(gdk::ModifierType::CONTROL_MASK),
                modifiers.contains(gdk::ModifierType::SHIFT_MASK),
                modifiers.contains(gdk::ModifierType::MOD1_MASK),
                modifiers.contains(gdk::ModifierType::SUPER_MASK),
            );
            let renderer_dead = recovery.renderer_dead.get();
            if !should_reload(trigger, renderer_dead) {
                return glib::Propagation::Proceed;
            }
            let Some(view) = weak_view.upgrade() else {
                return glib::Propagation::Stop;
            };
            if trigger == ReloadTrigger::Recovery && !renderer_dead {
                // The page still looks alive — let it confirm with the user
                // (and keep whatever it has in flight) before anything
                // reloads. The watchdog covers the case where "alive" only
                // means "has not crashed yet".
                ask_page_first(&window, &view);
            } else {
                log::warn!("[codemux::webview] reloading a dead renderer from the keyboard");
                view.reload();
            }
            glib::Propagation::Stop
        });
    }

    /// Hand the reload request to the page and arm the watchdog that reloads
    /// anyway if the page never answers.
    fn ask_page_first<R: tauri::Runtime>(window: &tauri::WebviewWindow<R>, view: &WebView) {
        use tauri::{Emitter, EventTarget};

        let token = REQUEST_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
        let emitted = window.emit_to(
            EventTarget::webview_window(window.label()),
            RELOAD_REQUESTED_EVENT,
            ReloadRequest { token },
        );
        match emitted {
            // Warn, like the other recovery paths: a chord press is rare and
            // worth having in a log when someone reports a blank window.
            Ok(()) => log::warn!(
                "[codemux::webview] recovery chord: asked the page to confirm (request {token})"
            ),
            Err(error) => {
                log::warn!("[codemux::webview] could not ask the page to reload: {error}")
            }
        }

        let weak_view = view.downgrade();
        glib::timeout_add_local_once(ACK_GRACE, move || {
            if !watchdog_should_reload(token, ACKED_SEQ.load(Ordering::Relaxed)) {
                return;
            }
            if let Some(view) = weak_view.upgrade() {
                log::warn!(
                    "[codemux::webview] page did not answer the reload request; reloading it"
                );
                view.reload();
            }
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
    fn ctrl_shift_r_is_the_recovery_chord() {
        assert_eq!(
            classify(ShortcutKey::R, true, true, false, false),
            ReloadTrigger::Recovery
        );
        // Works on a healthy page, which is the whole point of a chord that
        // does not collide with terminal or browser reload habits.
        assert!(should_reload(ReloadTrigger::Recovery, false));
        assert!(should_reload(ReloadTrigger::Recovery, true));
    }

    #[test]
    fn ctrl_r_and_f5_only_reload_a_dead_renderer() {
        for trigger in [
            classify(ShortcutKey::R, true, false, false, false),
            classify(ShortcutKey::F5, false, false, false, false),
        ] {
            assert_eq!(trigger, ReloadTrigger::DeadRendererOnly);
            assert!(!should_reload(trigger, false));
            assert!(should_reload(trigger, true));
        }
    }

    #[test]
    fn the_watchdog_only_fires_for_an_unanswered_request() {
        // Page answered the request it was asked about: it owns the flow.
        assert!(!watchdog_should_reload(1, 1));
        // A later answer than the request is still an answer (ack raced in
        // while a newer request was being armed).
        assert!(!watchdog_should_reload(1, 2));
        // Silence, or an answer to an older request only: wedged page.
        assert!(watchdog_should_reload(1, 0));
        assert!(watchdog_should_reload(7, 6));
    }

    #[test]
    fn acking_records_the_highest_answered_request() {
        let base = REQUEST_SEQ.fetch_add(10, Ordering::Relaxed) + 10;
        ack_reload_request(base);
        assert!(!watchdog_should_reload(
            base,
            ACKED_SEQ.load(Ordering::Relaxed)
        ));
        // A stale, out-of-order ack cannot un-answer a newer request.
        ack_reload_request(base - 5);
        assert!(!watchdog_should_reload(
            base,
            ACKED_SEQ.load(Ordering::Relaxed)
        ));
    }

    #[test]
    fn leaves_every_other_combo_alone() {
        for (key, ctrl, shift, alt, logo) in [
            (ShortcutKey::R, false, false, false, false), // plain r
            (ShortcutKey::R, false, true, false, false),  // Shift+R
            (ShortcutKey::R, true, false, true, false),   // Ctrl+Alt+R
            (ShortcutKey::R, true, true, false, true),    // Super+Ctrl+Shift+R
            (ShortcutKey::F5, true, false, false, false), // Ctrl+F5
            (ShortcutKey::F5, false, true, false, false), // Shift+F5
            (ShortcutKey::Other, true, true, false, false),
        ] {
            let trigger = classify(key, ctrl, shift, alt, logo);
            assert_eq!(trigger, ReloadTrigger::None, "{key:?} {ctrl} {shift}");
            assert!(!should_reload(trigger, true));
        }
    }
}
