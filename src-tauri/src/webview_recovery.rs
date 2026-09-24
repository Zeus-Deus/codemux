//! Native recovery for the main window's web renderer.
//!
//! The React UI runs in a separate web content process (`WebKitWebProcess`
//! on Linux, a WebView2 renderer on Windows). When that process crashes,
//! hangs, or stops painting, nothing in the page can run — not even its
//! keyboard shortcuts. The backend, the PTY daemon, and every agent sidecar
//! live outside that process and keep working, so reloading the page is all
//! it takes to get the UI back: panes re-attach to their sessions and replay
//! from the backend.
//!
//! Everything here runs in the app process, never in the page:
//!
//! 1. **Auto-reload** when the renderer process dies, capped at
//!    [`MAX_AUTO_RELOADS`] per [`AUTO_RELOAD_WINDOW`] so a page that keeps
//!    crashing on load cannot spin in a reload loop.
//! 2. **Ctrl+R, Ctrl+Shift+R, F5** reload the page, but only while the
//!    renderer is dead or unresponsive. On a healthy page those keys pass
//!    straight through, so the frontend keeps blocking accidental reloads and
//!    terminal panes keep their own Ctrl+R.
//! 3. **Ctrl+Alt+R** ([`RECOVERY_SHORTCUT`]) always reloads, for a window
//!    that went blank or froze without the renderer being flagged. It is
//!    consumed before the page sees it, so it never reaches a terminal.
//! 4. **`codemux reload-ui`** asks the running app to reload over the control
//!    socket, for when the window cannot take keyboard input at all.
//!
//! A reload only ever touches the main webview: it reloads the page, and
//! restarts the web content process first when that process is unresponsive.
//! It never restarts the app, the backend, the PTY daemon, or any agent.
//!
//! The decisions live in [`RecoveryState`] and [`route_key`], which are plain
//! data and unit-tested on every platform. The platform modules only feed
//! them engine events and carry out the [`WebviewOp`]s they return.

use std::fmt;
use std::time::{Duration, Instant};

/// Automatic reloads allowed within [`AUTO_RELOAD_WINDOW`] before recovery
/// falls back to the manual shortcuts.
pub const MAX_AUTO_RELOADS: usize = 3;
/// Sliding window the auto-reload cap is counted over.
pub const AUTO_RELOAD_WINDOW: Duration = Duration::from_secs(60);
/// How long a reload may take to start a new page before it counts as
/// stalled. Until then, further reload requests are ignored, so a held-down
/// shortcut or a crash racing a key press reloads once.
pub const RELOAD_STALL_TIMEOUT: Duration = Duration::from_secs(5);
/// The always-available recovery shortcut. The keyboard-shortcuts page lists
/// it as the `reloadInterface` entry in `src/lib/keybind-registry.ts`.
pub const RECOVERY_SHORTCUT: &str = "Ctrl+Alt+R";
/// How long one "renderer unresponsive" report counts. WebView2 repeats the
/// report every few seconds while the renderer stays hung but never says
/// when it recovers, so a report has to expire on its own; otherwise a brief
/// hang would leave Ctrl+R and F5 reloading the page instead of reaching a
/// terminal long after the renderer came back.
pub const UNRESPONSIVE_REPORT_TTL: Duration = Duration::from_secs(10);

/// Short pause before an automatic reload, so WebKit finishes tearing down
/// the dead process before a new one is spawned.
#[cfg(target_os = "linux")]
const AUTO_RELOAD_DELAY: Duration = Duration::from_millis(250);

/// What the app knows about the web content process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RendererHealth {
    Healthy,
    /// Alive but not answering the app process.
    Unresponsive,
    /// Exited; the window shows nothing until a new page loads.
    Dead,
}

/// What asked for a reload. Only used for logs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// The renderer process died and auto-reload was allowed.
    Crash,
    /// A reload shortcut on the main window.
    Shortcut,
    /// `codemux reload-ui` over the control socket.
    ControlCommand,
}

impl fmt::Display for Trigger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Trigger::Crash => "renderer exited",
            Trigger::Shortcut => "keyboard shortcut",
            Trigger::ControlCommand => "codemux reload-ui",
        })
    }
}

/// A single operation on the webview. This is the whole vocabulary recovery
/// has: nothing here can reach the backend, the PTY daemon, or an agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WebviewOp {
    /// Kill the web content process. The engine starts a fresh one on the
    /// next load.
    TerminateRenderer,
    /// Reload the page.
    Reload,
}

/// How to reload, given the renderer's health.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReloadMethod {
    /// Plain page reload. A dead renderer is replaced by the engine itself.
    Reload,
    /// The renderer is alive but stuck, and a hung process cannot run a
    /// reload. Replace it first.
    RestartRenderer,
}

impl ReloadMethod {
    pub fn ops(self) -> &'static [WebviewOp] {
        match self {
            ReloadMethod::Reload => &[WebviewOp::Reload],
            ReloadMethod::RestartRenderer => &[WebviewOp::TerminateRenderer, WebviewOp::Reload],
        }
    }
}

/// A reload that [`RecoveryState::begin_reload`] approved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReloadPlan {
    /// Matches the reload up with its stall check.
    pub id: u64,
    pub method: ReloadMethod,
}

/// What to do after the renderer process exited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoReload {
    /// Reload now.
    Reload,
    /// The page crashed too often recently; wait for a manual reload.
    Paused,
    /// The app terminated the renderer itself and is already reloading.
    NotNeeded,
}

#[derive(Clone, Copy, Debug)]
struct PendingReload {
    id: u64,
    started: Instant,
}

/// Renderer health and reload bookkeeping for one webview. Lives on the UI
/// thread; the platform adapters call into it from engine callbacks.
#[derive(Debug, Default)]
pub struct RecoveryState {
    /// True from renderer exit until a new page commits.
    renderer_dead: bool,
    /// True while the engine reports the renderer as not answering. For
    /// engines that signal both directions (WebKit).
    renderer_unresponsive: bool,
    /// When the engine last reported the renderer as not answering, for
    /// engines that never report recovery (WebView2). Counts for
    /// [`UNRESPONSIVE_REPORT_TTL`].
    unresponsive_reported: Option<Instant>,
    /// When recent automatic reloads ran, for the loop cap.
    auto_reloads: Vec<Instant>,
    /// A reload that has not produced a new page yet.
    pending: Option<PendingReload>,
    next_id: u64,
}

impl RecoveryState {
    pub fn health(&self, now: Instant) -> RendererHealth {
        let recently_reported = self
            .unresponsive_reported
            .is_some_and(|at| now.saturating_duration_since(at) < UNRESPONSIVE_REPORT_TTL);
        if self.renderer_dead {
            RendererHealth::Dead
        } else if self.renderer_unresponsive || recently_reported {
            RendererHealth::Unresponsive
        } else {
            RendererHealth::Healthy
        }
    }

    /// The engine says the renderer stopped or started answering again.
    pub fn set_responsive(&mut self, responsive: bool) {
        self.renderer_unresponsive = !responsive;
    }

    /// The engine says the renderer is not answering, with no matching
    /// "responsive again" signal to follow. Counts as unresponsive for
    /// [`UNRESPONSIVE_REPORT_TTL`]; an ongoing hang keeps renewing it.
    pub fn report_unresponsive(&mut self, now: Instant) {
        self.unresponsive_reported = Some(now);
    }

    /// The renderer process exited. `by_app` is true when the app terminated
    /// it on purpose as part of a reload.
    pub fn renderer_terminated(&mut self, by_app: bool, now: Instant) -> AutoReload {
        self.renderer_dead = true;
        self.renderer_unresponsive = false;
        self.unresponsive_reported = None;
        if by_app {
            return AutoReload::NotNeeded;
        }
        // Whatever reload was in flight died with the process.
        self.pending = None;
        self.auto_reloads
            .retain(|at| now.saturating_duration_since(*at) < AUTO_RELOAD_WINDOW);
        if self.auto_reloads.len() >= MAX_AUTO_RELOADS {
            return AutoReload::Paused;
        }
        self.auto_reloads.push(now);
        AutoReload::Reload
    }

    /// A new page committed, so a live renderer is showing it.
    pub fn page_committed(&mut self) {
        self.renderer_dead = false;
        self.renderer_unresponsive = false;
        self.unresponsive_reported = None;
        self.pending = None;
    }

    /// Approve a reload, or return `None` while an earlier one is still
    /// starting.
    pub fn begin_reload(&mut self, now: Instant) -> Option<ReloadPlan> {
        if let Some(pending) = self.pending {
            if now.saturating_duration_since(pending.started) < RELOAD_STALL_TIMEOUT {
                return None;
            }
        }
        let method = match self.health(now) {
            RendererHealth::Unresponsive => ReloadMethod::RestartRenderer,
            RendererHealth::Healthy | RendererHealth::Dead => ReloadMethod::Reload,
        };
        self.next_id += 1;
        let id = self.next_id;
        self.pending = Some(PendingReload { id, started: now });
        Some(ReloadPlan { id, method })
    }

    /// The stall timer for reload `id` fired. Returns true when that reload
    /// never produced a new page, in which case the caller restarts the
    /// renderer once. Clears the pending reload either way, so a stuck load
    /// can never block later requests.
    pub fn reload_stalled(&mut self, id: u64) -> bool {
        match self.pending {
            Some(pending) if pending.id == id => {
                self.pending = None;
                true
            }
            _ => false,
        }
    }
}

/// The keys the reload shortcuts care about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShortcutKey {
    R,
    F5,
    Other,
}

/// A key press on the main window, as the platform reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyPress {
    pub key: ShortcutKey,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub logo: bool,
}

/// Whether the app handles a key press or the page gets it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyAction {
    /// Let the page (and a focused terminal) receive the key.
    PassThrough,
    /// Consume the key and reload the webview.
    Reload,
}

/// Ctrl+Alt+R, exactly.
pub fn is_recovery_shortcut(press: KeyPress) -> bool {
    press.key == ShortcutKey::R && press.ctrl && press.alt && !press.shift && !press.logo
}

/// Ctrl+R (with or without Shift) or F5 without Ctrl. Alt and Super combos
/// are left alone so they never collide with window-manager or app bindings.
pub fn is_reload_key(press: KeyPress) -> bool {
    if press.alt || press.logo {
        return false;
    }
    match press.key {
        ShortcutKey::R => press.ctrl,
        ShortcutKey::F5 => !press.ctrl,
        ShortcutKey::Other => false,
    }
}

/// Decide who handles a key press. `health` is only consulted for the
/// browser reload keys, which the page owns while it is healthy.
pub fn route_key(press: KeyPress, health: impl FnOnce() -> RendererHealth) -> KeyAction {
    if is_recovery_shortcut(press) {
        return KeyAction::Reload;
    }
    if is_reload_key(press) && health() != RendererHealth::Healthy {
        return KeyAction::Reload;
    }
    KeyAction::PassThrough
}

/// Wire renderer recovery onto a webview window. Linux and Windows only;
/// elsewhere this is a no-op and [`request_reload`] falls back to a plain
/// page reload.
#[allow(unused_variables)]
pub fn install<R: tauri::Runtime>(window: &tauri::WebviewWindow<R>) {
    let label = window.label().to_string();
    #[cfg(target_os = "linux")]
    let result = window.with_webview(move |platform| linux::attach(label, platform.inner()));
    #[cfg(windows)]
    let result = window.with_webview(move |platform| {
        if let Err(error) = webview2::attach(label, platform.controller()) {
            log::warn!("[codemux::webview] could not install renderer recovery: {error}");
        }
    });
    #[cfg(any(target_os = "linux", windows))]
    if let Err(error) = result {
        log::warn!("[codemux::webview] could not install renderer recovery: {error}");
    }
}

/// Reload the window's page from the app process. Safe to call from any
/// thread: the work is queued onto the UI thread, and a request that arrives
/// while an earlier reload is still starting is dropped.
pub fn request_reload<R: tauri::Runtime>(
    window: &tauri::WebviewWindow<R>,
    trigger: Trigger,
) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let label = window.label().to_string();
        window
            .with_webview(move |platform| linux::request(&label, &platform.inner(), trigger))
            .map_err(|error| error.to_string())
    }
    #[cfg(windows)]
    {
        let label = window.label().to_string();
        window
            .with_webview(move |platform| {
                if let Err(error) = webview2::request(&label, &platform.controller(), trigger) {
                    log::error!("[codemux::webview] reload failed: {error}");
                }
            })
            .map_err(|error| error.to_string())
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        log::warn!("[codemux::webview] reloading interface ({trigger})");
        window.reload().map_err(|error| error.to_string())
    }
}

/// The main page is being replaced (a reload, a recovery, a dev-server
/// reload). Its terminal and chat channels die with it, but sending on them
/// never fails — the frames just land nowhere — so the backend would keep
/// feeding every one of them. Drop them before the new page attaches its
/// own. Channels a connected web-remote browser opened are kept.
pub fn release_page_subscribers<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    use tauri::Manager;
    let remote = app
        .try_state::<crate::web_remote::WebRemoteState>()
        .map(|state| state.channel_router().routed_ids())
        .unwrap_or_default();
    let keep = |id: u32| remote.contains(&id);
    let terminals = app
        .try_state::<crate::terminal::PtyState>()
        .map(|state| crate::terminal::retain_output_subscribers(&state, keep))
        .unwrap_or(0);
    let chats = app
        .try_state::<crate::commands::agent_chat::AgentChatChannelRegistry>()
        .map(|registry| registry.retain_channels(keep))
        .unwrap_or(0);
    if terminals + chats > 0 {
        eprintln!(
            "[codemux::webview] page replaced; released {terminals} terminal and \
             {chats} chat subscriber(s) of the previous page"
        );
    }
}

/// Recovery state per webview label, owned by the UI thread.
#[cfg(any(target_os = "linux", windows))]
mod registry {
    use super::RecoveryState;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    pub type Shared = Rc<RefCell<RecoveryState>>;

    thread_local! {
        static STATES: RefCell<HashMap<String, Shared>> = RefCell::default();
    }

    pub fn register(label: String) -> Shared {
        let state = Shared::default();
        STATES.with(|states| states.borrow_mut().insert(label, state.clone()));
        state
    }

    pub fn get(label: &str) -> Option<Shared> {
        STATES.with(|states| states.borrow().get(label).cloned())
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::registry::{self, Shared};
    use super::*;
    use gtk::prelude::*;
    use gtk::{gdk, glib};
    use webkit2gtk::{LoadEvent, WebProcessTerminationReason, WebView, WebViewExt};

    pub(super) fn attach(label: String, view: WebView) {
        let state = registry::register(label);

        view.connect_web_process_terminated({
            let state = state.clone();
            move |view, reason| {
                let by_app = reason == WebProcessTerminationReason::TerminatedByApi;
                let decision = state.borrow_mut().renderer_terminated(by_app, Instant::now());
                match decision {
                    AutoReload::NotNeeded => {}
                    AutoReload::Paused => log::error!(
                        "[codemux::webview] web process terminated ({reason:?}) again; \
                         auto-reload paused, press Ctrl+R to reload"
                    ),
                    AutoReload::Reload => {
                        log::warn!("[codemux::webview] web process terminated ({reason:?})");
                        let view = view.downgrade();
                        let state = state.clone();
                        glib::timeout_add_local_once(AUTO_RELOAD_DELAY, move || {
                            if let Some(view) = view.upgrade() {
                                reload(&state, &view, Trigger::Crash);
                            }
                        });
                    }
                }
            }
        });

        view.connect_load_changed({
            let state = state.clone();
            move |_, event| {
                if event == LoadEvent::Committed {
                    state.borrow_mut().page_committed();
                }
            }
        });

        view.connect_is_web_process_responsive_notify({
            let state = state.clone();
            move |view| {
                let responsive = view.is_web_process_responsive();
                if !responsive {
                    log::warn!(
                        "[codemux::webview] web process stopped responding; \
                         press Ctrl+R to restart it"
                    );
                }
                state.borrow_mut().set_responsive(responsive);
            }
        });

        // Listen on the toplevel window: its handler runs before GTK hands
        // the key to the focused webview, so a consumed shortcut never
        // reaches the page or a terminal in it, and it works wherever focus
        // ended up when the page died.
        let key_target: gtk::Widget = view
            .toplevel()
            .filter(|widget| widget.is::<gtk::Window>())
            .unwrap_or_else(|| view.clone().upcast());
        let weak_view = view.downgrade();
        key_target.connect_key_press_event(move |_, event| {
            let key = shortcut_key(event);
            if key == ShortcutKey::Other {
                return glib::Propagation::Proceed;
            }
            let modifiers = event.state();
            let press = KeyPress {
                key,
                ctrl: modifiers.contains(gdk::ModifierType::CONTROL_MASK),
                shift: modifiers.contains(gdk::ModifierType::SHIFT_MASK),
                alt: modifiers.contains(gdk::ModifierType::MOD1_MASK),
                logo: modifiers.intersects(
                    gdk::ModifierType::SUPER_MASK | gdk::ModifierType::MOD4_MASK,
                ),
            };
            let health = || state.borrow().health(Instant::now());
            if route_key(press, health) == KeyAction::PassThrough {
                return glib::Propagation::Proceed;
            }
            if let Some(view) = weak_view.upgrade() {
                reload(&state, &view, Trigger::Shortcut);
            }
            glib::Propagation::Stop
        });
    }

    pub(super) fn request(label: &str, view: &WebView, trigger: Trigger) {
        match registry::get(label) {
            Some(state) => reload(&state, view, trigger),
            None => {
                log::warn!("[codemux::webview] reloading interface ({trigger})");
                view.reload();
            }
        }
    }

    /// Which reload key this is. Looks at the first keyboard layout too, the
    /// way GTK accelerators do, so Ctrl+R still works on a Cyrillic or Greek
    /// layout.
    fn shortcut_key(event: &gdk::EventKey) -> ShortcutKey {
        let key = classify(event.keyval());
        if key != ShortcutKey::Other || event.group() == 0 {
            return key;
        }
        let Some(keymap) = event
            .window()
            .and_then(|window| gdk::Keymap::for_display(&window.display()))
        else {
            return key;
        };
        keymap
            .translate_keyboard_state(u32::from(event.hardware_keycode()), event.state(), 0)
            .map(|(keyval, ..)| classify(keyval.into()))
            .unwrap_or(key)
    }

    fn classify(keyval: gdk::keys::Key) -> ShortcutKey {
        match keyval.to_lower() {
            gdk::keys::constants::r => ShortcutKey::R,
            gdk::keys::constants::F5 => ShortcutKey::F5,
            _ => ShortcutKey::Other,
        }
    }

    fn reload(state: &Shared, view: &WebView, trigger: Trigger) {
        // Never hold the borrow across a WebKit call: terminating the
        // renderer re-enters the `web-process-terminated` handler.
        let plan = state.borrow_mut().begin_reload(Instant::now());
        let Some(plan) = plan else {
            log::info!("[codemux::webview] reload already in progress; ignoring {trigger}");
            return;
        };
        log::warn!(
            "[codemux::webview] reloading interface ({trigger}, {:?})",
            plan.method
        );
        run(view, plan.method.ops());

        // A reload that never commits usually means the renderer is wedged
        // in a way WebKit has not flagged yet. Replace it once.
        let view = view.downgrade();
        let state = state.clone();
        glib::timeout_add_local_once(RELOAD_STALL_TIMEOUT, move || {
            let stalled = state.borrow_mut().reload_stalled(plan.id);
            if let (true, Some(view)) = (stalled, view.upgrade()) {
                log::error!(
                    "[codemux::webview] reload did not load a page within {}s; \
                     restarting the web process",
                    RELOAD_STALL_TIMEOUT.as_secs()
                );
                run(&view, ReloadMethod::RestartRenderer.ops());
            }
        });
    }

    fn run(view: &WebView, ops: &[WebviewOp]) {
        for op in ops {
            match op {
                WebviewOp::TerminateRenderer => view.terminate_web_process(),
                WebviewOp::Reload => view.reload(),
            }
        }
    }
}

#[cfg(windows)]
mod webview2 {
    use super::registry::{self, Shared};
    use super::*;
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        ICoreWebView2, ICoreWebView2Controller, COREWEBVIEW2_KEY_EVENT_KIND,
        COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN, COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN,
        COREWEBVIEW2_PROCESS_FAILED_KIND, COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED,
        COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE,
    };
    use webview2_com::{
        AcceleratorKeyPressedEventHandler, ContentLoadingEventHandler, ProcessFailedEventHandler,
    };
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetKeyState, VK_CONTROL, VK_F5, VK_LMENU, VK_LWIN, VK_RMENU, VK_RWIN, VK_SHIFT,
    };

    /// Virtual-key code of the R key (letters use their ASCII capital).
    const VK_R: u32 = b'R' as u32;

    pub(super) fn attach(label: String, controller: ICoreWebView2Controller) -> Result<(), String> {
        let state = registry::register(label);
        let webview = unsafe { controller.CoreWebView2() }.map_err(|error| error.to_string())?;
        let mut token = 0i64;

        let on_key = AcceleratorKeyPressedEventHandler::create(Box::new({
            let state = state.clone();
            move |controller, args| {
                let (Some(controller), Some(args)) = (controller, args) else {
                    return Ok(());
                };
                let mut kind = COREWEBVIEW2_KEY_EVENT_KIND::default();
                unsafe { args.KeyEventKind(&mut kind)? };
                // Alt combos arrive as system keys.
                if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN
                    && kind != COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN
                {
                    return Ok(());
                }
                // Right Alt is AltGr on many layouts, where Ctrl+Alt+<key>
                // types a character (AltGr+R is ® on US-International).
                if key_down(VK_RMENU) {
                    return Ok(());
                }
                let mut virtual_key = 0u32;
                unsafe { args.VirtualKey(&mut virtual_key)? };
                let key = match virtual_key {
                    VK_R => ShortcutKey::R,
                    vk if vk == u32::from(VK_F5) => ShortcutKey::F5,
                    _ => return Ok(()),
                };
                let press = KeyPress {
                    key,
                    ctrl: key_down(VK_CONTROL),
                    shift: key_down(VK_SHIFT),
                    alt: key_down(VK_LMENU),
                    logo: key_down(VK_LWIN) || key_down(VK_RWIN),
                };
                let health = || state.borrow().health(Instant::now());
                if route_key(press, health) == KeyAction::PassThrough {
                    return Ok(());
                }
                unsafe { args.SetHandled(true)? };
                let webview = unsafe { controller.CoreWebView2()? };
                reload(&state, &webview, Trigger::Shortcut);
                Ok(())
            }
        }));
        unsafe { controller.add_AcceleratorKeyPressed(&on_key, &mut token) }
            .map_err(|error| error.to_string())?;

        let on_failure = ProcessFailedEventHandler::create(Box::new({
            let state = state.clone();
            move |webview, args| {
                let (Some(webview), Some(args)) = (webview, args) else {
                    return Ok(());
                };
                let mut kind = COREWEBVIEW2_PROCESS_FAILED_KIND::default();
                unsafe { args.ProcessFailedKind(&mut kind)? };
                if kind == COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE {
                    log::warn!(
                        "[codemux::webview] renderer stopped responding; press Ctrl+R to reload"
                    );
                    state.borrow_mut().report_unresponsive(Instant::now());
                    return Ok(());
                }
                if kind != COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED {
                    log::warn!("[codemux::webview] WebView2 process failed ({kind:?})");
                    return Ok(());
                }
                let decision = state.borrow_mut().renderer_terminated(false, Instant::now());
                match decision {
                    AutoReload::NotNeeded => {}
                    AutoReload::Paused => log::error!(
                        "[codemux::webview] renderer exited again; \
                         auto-reload paused, press Ctrl+R to reload"
                    ),
                    AutoReload::Reload => {
                        log::warn!("[codemux::webview] renderer exited");
                        reload(&state, &webview, Trigger::Crash);
                    }
                }
                Ok(())
            }
        }));
        unsafe { webview.add_ProcessFailed(&on_failure, &mut token) }
            .map_err(|error| error.to_string())?;

        let on_content = ContentLoadingEventHandler::create(Box::new(move |_, _| {
            state.borrow_mut().page_committed();
            Ok(())
        }));
        unsafe { webview.add_ContentLoading(&on_content, &mut token) }
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    pub(super) fn request(
        label: &str,
        controller: &ICoreWebView2Controller,
        trigger: Trigger,
    ) -> Result<(), String> {
        let webview = unsafe { controller.CoreWebView2() }.map_err(|error| error.to_string())?;
        match registry::get(label) {
            Some(state) => reload(&state, &webview, trigger),
            None => {
                log::warn!("[codemux::webview] reloading interface ({trigger})");
                unsafe { webview.Reload() }.map_err(|error| error.to_string())?;
            }
        }
        Ok(())
    }

    fn key_down(virtual_key: u16) -> bool {
        unsafe { GetKeyState(i32::from(virtual_key)) < 0 }
    }

    fn reload(state: &Shared, webview: &ICoreWebView2, trigger: Trigger) {
        let plan = state.borrow_mut().begin_reload(Instant::now());
        let Some(plan) = plan else {
            log::info!("[codemux::webview] reload already in progress; ignoring {trigger}");
            return;
        };
        log::warn!(
            "[codemux::webview] reloading interface ({trigger}, {:?})",
            plan.method
        );
        // WebView2 has no call to kill a renderer, but its `Reload` is the
        // documented recovery for an unresponsive one, so both methods end
        // in a plain reload here.
        if plan.method.ops().contains(&WebviewOp::Reload) {
            if let Err(error) = unsafe { webview.Reload() } {
                log::error!("[codemux::webview] reload failed: {error}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(key: ShortcutKey, ctrl: bool, shift: bool, alt: bool, logo: bool) -> KeyPress {
        KeyPress { key, ctrl, shift, alt, logo }
    }

    fn ctrl(key: ShortcutKey) -> KeyPress {
        press(key, true, false, false, false)
    }

    fn bare(key: ShortcutKey) -> KeyPress {
        press(key, false, false, false, false)
    }

    const RECOVERY: KeyPress = KeyPress {
        key: ShortcutKey::R,
        ctrl: true,
        shift: false,
        alt: true,
        logo: false,
    };

    #[test]
    fn recovery_shortcut_is_ctrl_alt_r_exactly() {
        assert_eq!(RECOVERY_SHORTCUT, "Ctrl+Alt+R");
        assert!(is_recovery_shortcut(RECOVERY));
        assert!(!is_recovery_shortcut(press(ShortcutKey::R, true, true, true, false)));
        assert!(!is_recovery_shortcut(press(ShortcutKey::R, true, false, true, true)));
        assert!(!is_recovery_shortcut(press(ShortcutKey::R, false, false, true, false)));
        assert!(!is_recovery_shortcut(ctrl(ShortcutKey::R)));
        assert!(!is_recovery_shortcut(press(ShortcutKey::F5, true, false, true, false)));
    }

    #[test]
    fn reload_keys_are_ctrl_r_and_f5_only() {
        assert!(is_reload_key(ctrl(ShortcutKey::R)));
        assert!(is_reload_key(press(ShortcutKey::R, true, true, false, false)));
        assert!(is_reload_key(bare(ShortcutKey::F5)));
        assert!(is_reload_key(press(ShortcutKey::F5, false, true, false, false)));
        assert!(!is_reload_key(bare(ShortcutKey::R)));
        assert!(!is_reload_key(ctrl(ShortcutKey::F5)));
        assert!(!is_reload_key(RECOVERY));
        assert!(!is_reload_key(press(ShortcutKey::R, true, false, false, true)));
        assert!(!is_reload_key(ctrl(ShortcutKey::Other)));
    }

    #[test]
    fn healthy_page_keeps_its_reload_keys() {
        for key in [ctrl(ShortcutKey::R), bare(ShortcutKey::F5)] {
            assert_eq!(route_key(key, || RendererHealth::Healthy), KeyAction::PassThrough);
        }
    }

    #[test]
    fn broken_renderer_takes_the_reload_keys() {
        for health in [RendererHealth::Dead, RendererHealth::Unresponsive] {
            for key in [
                ctrl(ShortcutKey::R),
                press(ShortcutKey::R, true, true, false, false),
                bare(ShortcutKey::F5),
            ] {
                assert_eq!(route_key(key, || health), KeyAction::Reload, "{key:?} {health:?}");
            }
        }
    }

    #[test]
    fn recovery_shortcut_reloads_regardless_of_health() {
        for health in [
            RendererHealth::Healthy,
            RendererHealth::Unresponsive,
            RendererHealth::Dead,
        ] {
            assert_eq!(route_key(RECOVERY, || health), KeyAction::Reload);
        }
    }

    #[test]
    fn other_keys_always_reach_the_page() {
        for health in [RendererHealth::Healthy, RendererHealth::Dead] {
            for key in [
                ctrl(ShortcutKey::Other),
                bare(ShortcutKey::R),
                press(ShortcutKey::R, false, true, false, false),
                press(ShortcutKey::R, true, false, false, true),
            ] {
                assert_eq!(route_key(key, || health), KeyAction::PassThrough, "{key:?}");
            }
        }
    }

    #[test]
    fn health_is_only_checked_for_reload_keys() {
        let unreachable = || -> RendererHealth { panic!("health should not be read") };
        assert_eq!(route_key(ctrl(ShortcutKey::Other), unreachable), KeyAction::PassThrough);
        assert_eq!(route_key(RECOVERY, unreachable), KeyAction::Reload);
    }

    #[test]
    fn reload_only_touches_the_webview() {
        assert_eq!(ReloadMethod::Reload.ops(), &[WebviewOp::Reload]);
        assert_eq!(
            ReloadMethod::RestartRenderer.ops(),
            &[WebviewOp::TerminateRenderer, WebviewOp::Reload]
        );
    }

    #[test]
    fn reload_method_follows_health() {
        let now = Instant::now();

        let mut healthy = RecoveryState::default();
        assert_eq!(healthy.begin_reload(now).unwrap().method, ReloadMethod::Reload);

        let mut hung = RecoveryState::default();
        hung.set_responsive(false);
        assert_eq!(hung.health(now), RendererHealth::Unresponsive);
        assert_eq!(hung.begin_reload(now).unwrap().method, ReloadMethod::RestartRenderer);

        let mut dead = RecoveryState::default();
        dead.set_responsive(false);
        assert_eq!(dead.renderer_terminated(false, now), AutoReload::Reload);
        assert_eq!(dead.health(now), RendererHealth::Dead);
        assert_eq!(dead.begin_reload(now).unwrap().method, ReloadMethod::Reload);
    }

    #[test]
    fn repeated_requests_reload_once() {
        let now = Instant::now();
        let mut state = RecoveryState::default();
        let first = state.begin_reload(now).expect("first reload runs");
        assert_eq!(state.begin_reload(now + Duration::from_millis(30)), None);
        assert_eq!(state.begin_reload(now + RELOAD_STALL_TIMEOUT / 2), None);

        state.page_committed();
        let second = state.begin_reload(now + Duration::from_secs(1)).expect("runs after commit");
        assert_ne!(first.id, second.id);
    }

    #[test]
    fn stalled_reload_stops_blocking_requests() {
        let now = Instant::now();
        let mut state = RecoveryState::default();
        state.begin_reload(now).unwrap();
        assert!(state.begin_reload(now + RELOAD_STALL_TIMEOUT).is_some(), "expires on its own");

        let mut state = RecoveryState::default();
        let plan = state.begin_reload(now).unwrap();
        assert!(!state.reload_stalled(plan.id + 1));
        assert!(state.reload_stalled(plan.id));
        assert!(!state.reload_stalled(plan.id), "only escalates once");
        assert!(state.begin_reload(now).is_some());
    }

    #[test]
    fn committed_reload_is_not_stalled() {
        let now = Instant::now();
        let mut state = RecoveryState::default();
        let plan = state.begin_reload(now).unwrap();
        state.page_committed();
        assert!(!state.reload_stalled(plan.id));
    }

    #[test]
    fn page_commit_clears_the_broken_state() {
        let now = Instant::now();
        let mut state = RecoveryState::default();
        state.renderer_terminated(false, now);
        assert_eq!(state.health(now), RendererHealth::Dead);
        state.page_committed();
        assert_eq!(state.health(now), RendererHealth::Healthy);

        state.set_responsive(false);
        state.page_committed();
        assert_eq!(state.health(now), RendererHealth::Healthy);
    }

    #[test]
    fn unresponsive_report_expires_on_its_own() {
        let now = Instant::now();
        let mut state = RecoveryState::default();
        state.report_unresponsive(now);
        assert_eq!(state.health(now), RendererHealth::Unresponsive);
        assert_eq!(state.begin_reload(now).unwrap().method, ReloadMethod::RestartRenderer);
        state.page_committed();

        // A hang that keeps being reported stays unresponsive.
        let mut state = RecoveryState::default();
        state.report_unresponsive(now);
        let renewed = now + UNRESPONSIVE_REPORT_TTL - Duration::from_secs(1);
        state.report_unresponsive(renewed);
        let later = now + UNRESPONSIVE_REPORT_TTL + Duration::from_secs(1);
        assert_eq!(state.health(later), RendererHealth::Unresponsive);

        // Once the reports stop, the renderer has recovered and the page
        // gets its Ctrl+R and F5 back.
        let recovered = renewed + UNRESPONSIVE_REPORT_TTL;
        assert_eq!(state.health(recovered), RendererHealth::Healthy);
        for key in [ctrl(ShortcutKey::R), bare(ShortcutKey::F5)] {
            assert_eq!(route_key(key, || state.health(recovered)), KeyAction::PassThrough);
        }
        assert_eq!(state.begin_reload(recovered).unwrap().method, ReloadMethod::Reload);
    }

    #[test]
    fn explicit_unresponsive_signal_does_not_expire() {
        let now = Instant::now();
        let mut state = RecoveryState::default();
        state.set_responsive(false);
        let later = now + UNRESPONSIVE_REPORT_TTL * 10;
        assert_eq!(state.health(later), RendererHealth::Unresponsive);
        state.set_responsive(true);
        assert_eq!(state.health(later), RendererHealth::Healthy);
    }

    #[test]
    fn page_commit_and_exit_clear_an_unresponsive_report() {
        let now = Instant::now();
        let mut state = RecoveryState::default();
        state.report_unresponsive(now);
        state.page_committed();
        assert_eq!(state.health(now), RendererHealth::Healthy);

        state.report_unresponsive(now);
        state.renderer_terminated(false, now);
        state.page_committed();
        assert_eq!(state.health(now), RendererHealth::Healthy);
    }

    #[test]
    fn restarting_a_hung_renderer_keeps_its_reload_pending() {
        let now = Instant::now();
        let mut state = RecoveryState::default();
        state.set_responsive(false);
        let plan = state.begin_reload(now).unwrap();
        assert_eq!(plan.method, ReloadMethod::RestartRenderer);

        // Terminating the renderer ourselves fires the exit callback.
        assert_eq!(state.renderer_terminated(true, now), AutoReload::NotNeeded);
        assert_eq!(state.begin_reload(now), None, "no second reload on top");
        assert!(state.reload_stalled(plan.id));
    }

    #[test]
    fn crash_during_a_reload_starts_a_new_one() {
        let now = Instant::now();
        let mut state = RecoveryState::default();
        let plan = state.begin_reload(now).unwrap();
        assert_eq!(state.renderer_terminated(false, now), AutoReload::Reload);
        let retry = state.begin_reload(now).expect("crash clears the pending reload");
        assert_ne!(retry.id, plan.id);
        assert!(!state.reload_stalled(plan.id));
    }

    #[test]
    fn auto_reload_is_capped() {
        let now = Instant::now();
        let mut state = RecoveryState::default();
        for i in 0..MAX_AUTO_RELOADS {
            let at = now + Duration::from_secs(i as u64);
            assert_eq!(state.renderer_terminated(false, at), AutoReload::Reload);
        }
        let at = now + Duration::from_secs(MAX_AUTO_RELOADS as u64);
        assert_eq!(state.renderer_terminated(false, at), AutoReload::Paused);

        // Manual recovery still works while auto-reload is paused.
        assert_eq!(route_key(ctrl(ShortcutKey::R), || state.health(now)), KeyAction::Reload);
        assert!(state.begin_reload(at).is_some());
    }

    #[test]
    fn auto_reloads_outside_the_window_do_not_count() {
        let now = Instant::now();
        let mut state = RecoveryState::default();
        for _ in 0..MAX_AUTO_RELOADS {
            state.renderer_terminated(false, now);
        }
        let later = now + AUTO_RELOAD_WINDOW + Duration::from_secs(1);
        assert_eq!(state.renderer_terminated(false, later), AutoReload::Reload);
    }

    #[test]
    fn app_initiated_termination_never_counts_toward_the_cap() {
        let now = Instant::now();
        let mut state = RecoveryState::default();
        for _ in 0..MAX_AUTO_RELOADS * 2 {
            assert_eq!(state.renderer_terminated(true, now), AutoReload::NotNeeded);
        }
        assert_eq!(state.renderer_terminated(false, now), AutoReload::Reload);
    }
}
