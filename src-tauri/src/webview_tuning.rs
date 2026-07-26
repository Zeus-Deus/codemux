//! WebKitGTK renderer + scrolling tuning (Linux).
//!
//! Two independent knobs live here, both no-ops on non-Linux targets:
//!
//! 1. **Renderer transport** — [`configure_renderer_env`] picks the buffer
//!    path WebKitGTK uses to hand composited frames to the UI process. The
//!    choice has to be made through env vars *before* GTK/Tauri initialise,
//!    so it runs at the very top of `run()` and resolves its own paths with
//!    `dirs` (same logic as every other piece of local state) rather than
//!    through an `AppHandle`, which does not exist yet.
//!
//! 2. **Smooth scrolling** — WebKitGTK's `enable-smooth-scrolling` setting,
//!    applied per webview via `with_webview`. Each `WebviewWindow` owns its
//!    own `WebKitSettings` object, so this is applied to every window, not
//!    just `main`.
//!
//! Escape hatch: set `CODEMUX_WEBKIT_COMPAT=1` to force the legacy
//! compatibility renderer (`WEBKIT_DISABLE_DMABUF_RENDERER=1` +
//! `WEBKIT_DISABLE_COMPOSITING_MODE=1`, i.e. CPU rendering with slower
//! scrolling) for driver stacks that cannot start on the accelerated path.
//! That variable is the only unambiguous opt-in — it is never scrubbed, and
//! it also disables the transcript edge-fade in the UI via
//! [`get_renderer_mode`].

#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicBool, Ordering};

/// Routes the final compositor buffer handoff through shared memory while
/// leaving accelerated compositing (and therefore threaded scrolling) on.
pub const FORCE_SHM_ENV: &str = "WEBKIT_DMABUF_RENDERER_FORCE_SHM";
/// Legacy compatibility flag: disables the DMA-BUF renderer entirely.
pub const DISABLE_DMABUF_ENV: &str = "WEBKIT_DISABLE_DMABUF_RENDERER";
/// Legacy compatibility flag: drops WebKit to non-accelerated CPU rendering.
pub const DISABLE_COMPOSITING_ENV: &str = "WEBKIT_DISABLE_COMPOSITING_MODE";

/// Explicit opt-in to the legacy compatibility renderer. Unlike the raw
/// WebKit vars this one is unambiguous — nothing but a human sets it — so it
/// is never scrubbed and always wins.
pub const COMPAT_MODE_ENV: &str = "CODEMUX_WEBKIT_COMPAT";

/// Env vars the user may set themselves to take full manual control. Any of
/// them being present makes [`configure_renderer_env`] hands-off.
///
/// Also the list the terminal/PTY layer strips from child environments:
/// renderer transport is an app-process concern, and a shell that launches
/// another GTK/WebKit app must not inherit our choice (see
/// `terminal::strip_renderer_env`).
pub const RENDERER_ENV_VARS: [&str; 3] =
    [FORCE_SHM_ENV, DISABLE_DMABUF_ENV, DISABLE_COMPOSITING_ENV];

/// Marker every Codemux-spawned terminal carries. Its presence means this
/// process was started from inside another Codemux, so renderer vars in the
/// environment are inherited rather than user intent.
#[cfg(target_os = "linux")]
const CODEMUX_ENV: &str = "CODEMUX";

/// The value Codemux itself stamps onto renderer env vars. Only this exact
/// value is treated as contamination — anything else (notably `0`, WebKit's
/// own opt-out) is a deliberate user choice and survives the scrub.
#[cfg(target_os = "linux")]
const STAMPED_VALUE: &str = "1";

/// How many consecutive startups may fail before the accelerated path is
/// abandoned in favour of the legacy compatibility flags.
#[cfg(target_os = "linux")]
const MAX_CONSECUTIVE_FAILURES: u32 = 2;

/// File name of the crash sentinel, stored under the app data dir.
#[cfg(target_os = "linux")]
const SENTINEL_FILE: &str = "webview-renderer.sentinel";

/// Persisted crash-sentinel state.
///
/// The counter is incremented before the webview is created and cleared once
/// the main window reports a finished page load, so a startup that dies
/// inside WebKit's renderer init leaves the increment behind.
#[cfg(target_os = "linux")]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RendererSentinel {
    #[serde(default)]
    pub consecutive_failures: u32,
}

/// Parse sentinel JSON, treating anything unreadable as "no failures". A
/// corrupt sentinel must never be able to keep the app on the slow renderer.
#[cfg(target_os = "linux")]
pub fn parse_sentinel(raw: &str) -> RendererSentinel {
    serde_json::from_str(raw).unwrap_or_default()
}

#[cfg(target_os = "linux")]
fn sentinel_path() -> Option<std::path::PathBuf> {
    Some(
        dirs::data_dir()?
            .join(crate::APP_DIR_NAME)
            .join(SENTINEL_FILE),
    )
}

#[cfg(target_os = "linux")]
fn read_sentinel() -> RendererSentinel {
    sentinel_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|raw| parse_sentinel(&raw))
        .unwrap_or_default()
}

#[cfg(target_os = "linux")]
fn write_sentinel(state: RendererSentinel) {
    let Some(path) = sentinel_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(raw) = serde_json::to_string(&state) {
        let _ = std::fs::write(&path, raw);
    }
}

/// Set when this process incremented the sentinel counter and therefore owns
/// clearing it. Stays false when the user drives the renderer flags manually
/// and when the compatibility fallback is active — the fallback is sticky on
/// purpose (see [`configure_renderer_env`]), so nothing may clear the counter
/// while it is in force.
#[cfg(target_os = "linux")]
static SENTINEL_OWNED: AtomicBool = AtomicBool::new(false);

/// True when this process ended up on the legacy CPU renderer — either
/// through `CODEMUX_WEBKIT_COMPAT=1`, the crash-sentinel fallback, or a
/// genuine user override that disables compositing. Read by
/// [`get_renderer_mode`] so the UI can drop effects that are only free on a
/// composited webview (the transcript edge-fade mask costs ~2x frame time
/// without compositing).
#[cfg(target_os = "linux")]
static COMPAT_MODE: AtomicBool = AtomicBool::new(false);

/// Should this inherited renderer env var be removed before the override
/// check runs?
///
/// Contamination only exists when this process was launched from a terminal
/// another Codemux spawned (`CODEMUX` set) *and* the value is the exact one
/// Codemux stamps (`1`). Both old builds (`WEBKIT_DISABLE_*=1`) and new ones
/// (`WEBKIT_DMABUF_RENDERER_FORCE_SHM=1`) leak that way; scrubbing our own
/// stamp is harmless because it is re-set below, and it keeps one rule for
/// all three vars. A value of `0` is never scrubbed — that is WebKit's own
/// opt-out and can only come from a human.
///
/// Pure so the decision is testable without mutating the process env.
#[cfg(target_os = "linux")]
pub fn scrub_decision(codemux_set: bool, name: &str, value: &str) -> bool {
    codemux_set && RENDERER_ENV_VARS.contains(&name) && value == STAMPED_VALUE
}

/// Drop renderer env vars inherited from a parent Codemux process.
///
/// Without this, a dev or release build launched from a Codemux terminal sees
/// the parent's `WEBKIT_DISABLE_DMABUF_RENDERER=1` / `_COMPOSITING_MODE=1`,
/// treats them as a user override and silently runs CPU-rendered. On
/// WebKitGTK 2.44+ even the DMA-BUF flag alone is fatal to scroll performance:
/// no DMA-BUF renderer means no accelerated backing store, so accelerated
/// compositing goes with it.
#[cfg(target_os = "linux")]
fn scrub_inherited_renderer_env() {
    let codemux_set = std::env::var_os(CODEMUX_ENV).is_some();
    if !codemux_set {
        return;
    }
    let mut scrubbed: Vec<&str> = Vec::new();
    for key in RENDERER_ENV_VARS {
        let Ok(value) = std::env::var(key) else {
            continue;
        };
        if scrub_decision(codemux_set, key, &value) {
            unsafe { std::env::remove_var(key) };
            scrubbed.push(key);
        }
    }
    if !scrubbed.is_empty() {
        crate::diagnostics::stderr_line(&format!(
            "[codemux::webview] ignoring inherited renderer env ({}) — this process was \
             launched from a Codemux terminal, so those values came from the parent app, \
             not from you. Set {}=1 for the legacy CPU renderer, or {}=0 to opt out of the \
             shared-memory transport.",
            scrubbed.join(", "),
            COMPAT_MODE_ENV,
            FORCE_SHM_ENV,
        ));
    }
}

/// Apply the legacy compatibility renderer: CPU rendering everywhere, no
/// DMA-BUF transport. Slower scrolling, but it starts on driver stacks the
/// accelerated path cannot survive.
#[cfg(target_os = "linux")]
fn enable_compatibility_renderer() {
    unsafe {
        std::env::set_var(DISABLE_DMABUF_ENV, "1");
        std::env::set_var(DISABLE_COMPOSITING_ENV, "1");
    }
    COMPAT_MODE.store(true, Ordering::Release);
}

/// Choose the WebKitGTK renderer transport for this process.
///
/// Default is `WEBKIT_DMABUF_RENDERER_FORCE_SHM=1`, which keeps accelerated
/// compositing — and with it WebKit's threaded/async scrolling — enabled while
/// forcing the compositor buffer handoff onto shared memory. That sidesteps
/// both the "Error 71 (Protocol error) dispatching to Wayland display" crash
/// seen on dual-GPU machines under explicit sync and the "Could not create GBM
/// EGL display" failure on some driver stacks, because neither the hardware
/// nor the GBM buffer transport is used any more. Previously those crashes
/// were avoided by disabling the DMA-BUF renderer and compositing mode
/// outright, which forced CPU rendering process-wide and cost ~56 ms per scroll
/// frame; on the SHM path the same scroll costs ~16 ms. See WebKit bug 280210.
///
/// Setting `WEBKIT_DMABUF_RENDERER_FORCE_SHM=0` opts out at the WebKit level:
/// WebKit treats any value other than `0` as "on", so the user keeps the final
/// say. Setting any of the three renderer env vars by hand also disables the
/// sentinel logic below entirely.
///
/// Inherited-env scrub: a Codemux launched from a terminal another Codemux
/// spawned inherits that parent's renderer vars, which would otherwise read as
/// a user override and silently pin the child to CPU rendering. Values that
/// match Codemux's own stamp are dropped first when `CODEMUX` is set — see
/// [`scrub_decision`]. `CODEMUX_WEBKIT_COMPAT=1` is the explicit, never-
/// scrubbed way to ask for the legacy renderer.
///
/// Compatibility fallback: hardware where even the accelerated+SHM path dies
/// during startup is detected with a small on-disk sentinel. The counter is
/// bumped before the webview is built and cleared when the main window
/// finishes loading, so `MAX_CONSECUTIVE_FAILURES` crashed startups in a row
/// flip this process onto the legacy `WEBKIT_DISABLE_*` flags. That fallback
/// is sticky (the counter is deliberately not cleared while it is active) so
/// affected machines do not alternate between a working and a crashing
/// renderer on every other launch; deleting the sentinel file re-arms the
/// accelerated path.
#[cfg(target_os = "linux")]
pub fn configure_renderer_env() {
    // Unambiguous opt-in to the legacy renderer. Checked first and never
    // scrubbed: only a human sets this.
    if std::env::var(COMPAT_MODE_ENV).as_deref() == Ok("1") {
        crate::diagnostics::stderr_line(&format!(
            "[codemux::webview] {COMPAT_MODE_ENV}=1 — using the legacy compatibility renderer \
             ({DISABLE_DMABUF_ENV}=1 / {DISABLE_COMPOSITING_ENV}=1, CPU rendering, slower \
             scrolling)."
        ));
        enable_compatibility_renderer();
        return;
    }

    // Renderer vars inherited from a parent Codemux are not user intent.
    scrub_inherited_renderer_env();

    // Explicit user configuration wins outright — no sentinel bookkeeping.
    if RENDERER_ENV_VARS
        .iter()
        .any(|key| std::env::var_os(key).is_some())
    {
        // A surviving override that disables compositing means this process
        // renders on the CPU, whatever set it. Report that honestly so the UI
        // drops composited-only effects.
        let non_composited = std::env::var_os(DISABLE_DMABUF_ENV).is_some()
            || std::env::var_os(DISABLE_COMPOSITING_ENV).is_some();
        COMPAT_MODE.store(non_composited, Ordering::Release);
        return;
    }

    let sentinel = read_sentinel();
    if sentinel.consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
        crate::diagnostics::stderr_line(&format!(
            "[codemux::webview] {} consecutive failed startups on the accelerated renderer — \
             falling back to {}=1 / {}=1 (CPU rendering, slower scrolling). Delete {} to retry \
             the accelerated path.",
            sentinel.consecutive_failures,
            DISABLE_DMABUF_ENV,
            DISABLE_COMPOSITING_ENV,
            sentinel_path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| SENTINEL_FILE.to_string()),
        ));
        enable_compatibility_renderer();
        return;
    }

    write_sentinel(RendererSentinel {
        consecutive_failures: sentinel.consecutive_failures + 1,
    });
    SENTINEL_OWNED.store(true, Ordering::Release);
    unsafe { std::env::set_var(FORCE_SHM_ENV, "1") };
}

/// Clear the crash sentinel: this process reached a state that proves the
/// renderer it picked works. Called from the page-load hook for the main
/// window, and from the duplicate-launch handler — a duplicate launch
/// increments the counter in a process that exits before it ever builds a
/// window, so the healthy instance it hands off to clears that increment.
///
/// No-op unless this process armed the sentinel, which keeps the sticky
/// compatibility fallback sticky.
pub fn mark_startup_successful() {
    #[cfg(target_os = "linux")]
    if SENTINEL_OWNED.load(Ordering::Acquire) {
        write_sentinel(RendererSentinel::default());
    }
}

/// Which renderer this process actually ended up on: `"accelerated"` (GPU
/// compositing, threaded scrolling) or `"compatibility"` (CPU rendering).
///
/// The frontend uses this to drop effects that are only free when composited
/// — currently the transcript edge-fade mask, which roughly doubles frame time
/// on the CPU path. Always `"accelerated"` off Linux, where the compatibility
/// flags do not exist.
#[tauri::command]
pub fn get_renderer_mode() -> Result<String, String> {
    #[cfg(target_os = "linux")]
    {
        return Ok(if COMPAT_MODE.load(Ordering::Acquire) {
            "compatibility".to_string()
        } else {
            "accelerated".to_string()
        });
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok("accelerated".to_string())
    }
}

/// Desired `enable-smooth-scrolling` state for every webview.
///
/// Off by default: WebKitGTK's smooth-scrolling animation restarts its 200 ms
/// eased retarget on every high-resolution wheel event, so a fast flick ends up
/// travelling *less* than a slow one (WebKit bug 258926). With the setting off,
/// wheel deltas are applied directly and scrolling tracks the input device.
/// The frontend can flip it at runtime through [`set_smooth_scrolling`].
#[cfg(target_os = "linux")]
static SMOOTH_SCROLLING: AtomicBool = AtomicBool::new(false);

/// Push the current desired smooth-scrolling state onto a single webview.
/// Used by the page-load hook so windows created after setup — and any page
/// that reloads — pick the setting up without extra bookkeeping.
#[allow(unused_variables)]
pub fn apply_to_webview<R: tauri::Runtime>(webview: &tauri::Webview<R>) {
    #[cfg(target_os = "linux")]
    {
        let enabled = SMOOTH_SCROLLING.load(Ordering::Relaxed);
        let _ = webview.with_webview(move |platform| {
            use webkit2gtk::{SettingsExt, WebViewExt};
            if let Some(settings) = platform.inner().settings() {
                settings.set_enable_smooth_scrolling(enabled);
            }
        });
    }
}

/// Push the current desired smooth-scrolling state onto every existing
/// webview window.
#[allow(unused_variables)]
pub fn refresh_all<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    #[cfg(target_os = "linux")]
    {
        use tauri::Manager;
        for (_label, window) in app.webview_windows() {
            apply_to_webview(window.as_ref());
        }
    }
}

/// Toggle WebKitGTK smooth scrolling for every webview window at runtime.
///
/// Linux-only in effect; other platforms have no equivalent setting and
/// return `Ok(())` unchanged so the frontend can call this unconditionally.
#[tauri::command]
#[allow(unused_variables)]
pub fn set_smooth_scrolling<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    enabled: bool,
) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        SMOOTH_SCROLLING.store(enabled, Ordering::Relaxed);
        refresh_all(&app);
    }
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn parses_a_written_sentinel_round_trip() {
        let state = RendererSentinel {
            consecutive_failures: 2,
        };
        let raw = serde_json::to_string(&state).unwrap();
        assert_eq!(parse_sentinel(&raw), state);
    }

    #[test]
    fn treats_corrupt_or_empty_sentinel_as_no_failures() {
        for raw in ["", "not json", "{", "[]", "{\"consecutive_failures\":\"x\"}"] {
            assert_eq!(parse_sentinel(raw).consecutive_failures, 0, "raw = {raw:?}");
        }
    }

    #[test]
    fn tolerates_a_sentinel_missing_the_counter_field() {
        assert_eq!(parse_sentinel("{}").consecutive_failures, 0);
    }

    #[test]
    fn scrubs_every_stamped_renderer_var_under_a_codemux_terminal() {
        for key in RENDERER_ENV_VARS {
            assert!(
                scrub_decision(true, key, "1"),
                "{key}=1 under CODEMUX is inherited contamination"
            );
        }
    }

    #[test]
    fn keeps_renderer_vars_when_codemux_is_not_set() {
        // No parent Codemux ⇒ whatever is in the environment is user intent.
        for key in RENDERER_ENV_VARS {
            assert!(!scrub_decision(false, key, "1"), "{key}");
        }
    }

    #[test]
    fn keeps_deliberate_opt_out_values() {
        // `0` is WebKit's own opt-out and can only come from a human, so it
        // survives even inside a Codemux terminal. Same for any other value
        // Codemux never stamps.
        for key in RENDERER_ENV_VARS {
            assert!(!scrub_decision(true, key, "0"), "{key}=0");
            assert!(!scrub_decision(true, key, "true"), "{key}=true");
            assert!(!scrub_decision(true, key, ""), "{key} empty");
        }
    }

    #[test]
    fn never_scrubs_unrelated_or_opt_in_env() {
        assert!(!scrub_decision(true, COMPAT_MODE_ENV, "1"));
        assert!(!scrub_decision(true, "GDK_BACKEND", "1"));
        assert!(!scrub_decision(true, "CODEMUX", "1"));
    }

    #[test]
    fn sentinel_lives_under_the_app_data_dir() {
        let path = sentinel_path().expect("data dir resolves in the test environment");
        assert!(path.ends_with(SENTINEL_FILE));
        assert!(
            path.parent()
                .and_then(|p| p.file_name())
                .map(|name| name == crate::APP_DIR_NAME)
                .unwrap_or(false),
            "sentinel must be scoped by APP_DIR_NAME so dev and release builds stay isolated: {}",
            path.display()
        );
    }
}
