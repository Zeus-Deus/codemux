use tauri::{Emitter, Listener, Manager};

/// Subdirectory name used under XDG config/data dirs for all Codemux state
/// (sqlite db, auth tokens, scrollback, presets, etc.). Debug builds use a
/// distinct name so a locally-running dev build keeps its sessions, auth,
/// and synced state fully isolated from the installed release build.
#[cfg(debug_assertions)]
pub const APP_DIR_NAME: &str = "codemux-dev";
#[cfg(not(debug_assertions))]
pub const APP_DIR_NAME: &str = "codemux";

pub mod agent_capability;
pub mod agent_context;
pub mod agent_provider;
pub mod ai;
pub mod auth;
pub mod automations;
pub mod automations_sync;
pub mod branch_name;
pub mod json_rpc_child;
pub mod mcp_server;
pub mod agent_browser;
pub mod browser_viewport;
pub mod cli;
pub mod commands;
pub mod database;
pub mod config;
pub mod git;
pub mod github;
pub mod github_cache;
pub mod control;
pub mod diagnostics;
pub mod dialog_fallback;
pub mod dialog_preflight;
pub mod app_logs;
pub mod doctor;
pub mod execution;
pub mod indexing;
pub mod jobs;
pub mod mcp;
pub mod memory;
pub mod notifications;
pub mod observability;
pub mod os_input;
pub mod ports;
pub mod presets;
pub mod project;
// The PTY daemon is Unix-only for now. Windows builds get the in-process
// PTY path with zero regression (the `daemon_path_viable()` gate in
// `terminal/mod.rs` returns false on non-Unix and we never touch this
// module from any code path).
#[cfg(unix)]
pub mod pty_daemon;
#[cfg(unix)]
pub mod remote;
// SSH transport for the cloud-push feature. Unix-only — relies on
// the system `ssh` + `scp` binaries (with the user's existing
// `~/.ssh/config`, agent, and known_hosts).
#[cfg(unix)]
pub mod ssh;
pub mod resource_metrics;
pub mod scripts;
pub mod scrollback;
pub mod skills;
pub mod skills_sync;
pub mod session_adapters;
pub mod settings_sync;
pub mod hosts_sync;
#[cfg(unix)]
pub mod hosts_upgrade;
pub mod hosts_inventory;
pub mod workspace_paths;
pub mod project_identity;
pub mod workspaces_sync;
pub mod state;
pub mod hooks;
pub mod stream_input;
pub mod terminal;
// Opt-in cloud-push diagnostic tracing. Defines the crate-wide
// `trace_cloud_push!` macro (gated on `CODEMUX_TRACE_CLOUD_PUSH`).
pub mod trace;
// WebKitGTK renderer transport + smooth-scrolling tuning (Linux). Also owns
// the startup crash sentinel that picks the renderer flags for this process.
pub mod webview_tuning;
// Embedded web remote-access server (default-off HTTP+WebSocket). Lets a
// browser on another device drive this running instance as a second
// frontend. See docs/plans/web-remote-access.md.
pub mod web_remote;

/// Which surface the app is booting. The GUI desktop build runs
/// [`AppMode::Gui`] on Wry; the headless `codemux serve` daemon boots the
/// same command surface on `tauri::test::MockRuntime` under
/// [`AppMode::ServeHeadless`] with no display attached. The mode is managed
/// state so commands and the setup closure can read it via
/// `app.state::<AppMode>()`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AppMode {
    Gui,
    ServeHeadless,
}

/// Read the managed [`AppMode`]. Defaults to [`AppMode::Gui`] when the state
/// was never managed (e.g. a lightweight test harness that builds a mock app
/// without `.manage(mode)`), so behavior is unchanged for anything that does
/// not explicitly opt into headless mode.
pub fn app_mode<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> AppMode {
    app.try_state::<AppMode>()
        .map(|state| *state.inner())
        .unwrap_or(AppMode::Gui)
}

/// Guard for display-coupled commands (native dialogs, clipboard, updater)
/// that have no headless implementation. Returns a clean `Err` under
/// [`AppMode::ServeHeadless`] instead of reaching an unregistered plugin
/// (which would panic).
pub fn ensure_gui_mode<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<(), String> {
    if app_mode(app) == AppMode::Gui {
        Ok(())
    } else {
        Err("not available in headless serve mode".to_string())
    }
}

/// Save window dimensions and position to SQLite before close.
fn save_window_state<R: tauri::Runtime>(handle: &tauri::AppHandle<R>) {
    let db: tauri::State<'_, database::DatabaseStore> = handle.state();
    if let Some(w) = handle.get_webview_window("main") {
        let sf = w.scale_factor().unwrap_or(1.0);

        if let Ok(size) = w.outer_size() {
            let lw = size.width as f64 / sf;
            let lh = size.height as f64 / sf;

            let fills_monitor = w
                .current_monitor()
                .ok()
                .flatten()
                .map_or(false, |m| {
                    let mw = m.size().width as f64 / sf;
                    let mh = m.size().height as f64 / sf;
                    lw + 100.0 >= mw && lh + 100.0 >= mh
                });

            if fills_monitor {
                db.delete_ui_state("window_width").ok();
                db.delete_ui_state("window_height").ok();
            } else {
                db.set_ui_state("window_width", &lw.to_string()).ok();
                db.set_ui_state("window_height", &lh.to_string()).ok();
            }
        }

        if let Ok(pos) = w.outer_position() {
            let lx = (pos.x as f64 / sf) as i32;
            let ly = (pos.y as f64 / sf) as i32;
            db.set_ui_state("window_x", &lx.to_string()).ok();
            db.set_ui_state("window_y", &ly.to_string()).ok();
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Set WebKitGTK env vars before any GTK/Tauri initialization.
    //
    // Renderer transport: `webview_tuning::configure_renderer_env` sets
    // WEBKIT_DMABUF_RENDERER_FORCE_SHM=1, which keeps accelerated compositing
    // (and WebKit's threaded scrolling) enabled while routing the final
    // compositor buffer handoff through shared memory. That avoids the
    // "Error 71 (Protocol error) dispatching to Wayland display" crash on
    // dual-GPU systems and the "Could not create GBM EGL display" failure on
    // some driver stacks — both come from the hardware/GBM buffer transport
    // this removes — without the app-wide scroll jank the old
    // WEBKIT_DISABLE_DMABUF_RENDERER + WEBKIT_DISABLE_COMPOSITING_MODE pair
    // caused by forcing CPU rendering (~56 ms per scroll frame vs ~16 ms
    // accelerated). See WebKit bug 280210. Any of those three env vars set by
    // the user wins (`WEBKIT_DMABUF_RENDERER_FORCE_SHM=0` is WebKit's own
    // opt-out), and a crash sentinel falls back to the legacy compatibility
    // flags on hardware where accelerated startup keeps dying. Renderer vars
    // inherited from a parent Codemux terminal are scrubbed first so they are
    // not mistaken for a user override; `CODEMUX_WEBKIT_COMPAT=1` is the
    // explicit opt-in to the legacy CPU renderer.
    //
    // GDK_BACKEND: forces Wayland-native when available, preventing XWayland fallback
    // (which triggers unintended Hyprland window rules on AppImage builds).
    #[cfg(target_os = "linux")]
    {
        webview_tuning::configure_renderer_env();
        if std::env::var("GDK_BACKEND").is_err() {
            unsafe { std::env::set_var("GDK_BACKEND", "wayland,x11") };
        }
    }

    #[cfg(debug_assertions)]
    {
        use std::time::SystemTime;
        let start = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        crate::diagnostics::stderr_line(&format!(
            "[DEBUG] codemux_lib::run() started at timestamp: {}",
            start
        ));
    }

    // Single-instance guard is GUI-only and must initialize first so a
    // duplicate launch is intercepted before any window is created.
    // Registered here on the concrete Wry builder (rather than inside
    // `build_core_app`) to preserve its historical first-plugin position;
    // the headless serve app never registers it (its callback drives
    // windows, which do not exist headless).
    let builder = tauri::Builder::default().plugin(tauri_plugin_single_instance::init(
        |app, args, cwd| {
            crate::diagnostics::stderr_line(&format!(
                "[codemux::single-instance] Duplicate launch redirected args={args:?} cwd={}",
                cwd
            ));
            #[cfg(debug_assertions)]
            {
                crate::diagnostics::native_startup_breadcrumb(&format!(
                    "[{}] component=single_instance event=redirected_duplicate args={:?} cwd={}",
                    chrono::Local::now().format("%s"),
                    args,
                    cwd
                ));
            }
            // A duplicate launch bumps the renderer crash sentinel in a
            // process that exits before it ever builds a window. This
            // instance is demonstrably healthy, so clear that increment
            // rather than let repeated duplicate launches look like repeated
            // renderer crashes.
            crate::webview_tuning::mark_startup_successful();
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        },
    ));

    build_core_app(builder, AppMode::Gui)
        .build(app_context())
        .expect("error while building tauri application")
        .run(|_app, event| {
            if let tauri::RunEvent::Exit = event {
                // A graceful run-loop exit is not a renderer crash: disarm the
                // renderer crash sentinel even when the main window never
                // finished a page load (e.g. the app was quit mid-startup).
                // Ownership-gated inside, so the sticky compatibility
                // fallback stays sticky.
                webview_tuning::mark_clean_exit();
                // Kill agent-browser daemons so they don't persist across restarts.
                agent_browser::kill_stream_daemons();
            }
        });
}

/// Run the headless `codemux serve` web-remote server on the main thread.
/// Boots the full backend on `MockRuntime`, binds the web-remote server
/// through the shared enable path, prints a pairing QR + link, and blocks
/// until SIGINT / SIGTERM. Dispatched from `main` (not `maybe_run_cli`)
/// because it is a long-lived foreground process, not a control round-trip.
pub fn run_serve(opts: web_remote::serve::ServeOptions) -> Result<(), String> {
    web_remote::serve::run_serve(opts)
}

/// Expand the embedded-asset context exactly once. `generate_context!`
/// embeds the entire frontend bundle into the binary; expanding it in a
/// single generic function (rather than at each `.build()` site) keeps the
/// assets from being embedded twice. The macro expands to a generic
/// `Context<R>`, so both the Wry GUI app and the MockRuntime headless app
/// share this one expansion.
fn app_context<R: tauri::Runtime>() -> tauri::Context<R> {
    tauri::generate_context!()
}

/// Wire the full command surface — managed state, channel interceptor,
/// plugins, setup closure, and invoke handler — onto `builder`. Shared
/// verbatim by the GUI desktop app (`run`, on Wry) and the headless serve
/// daemon (`build_headless_app`, on `MockRuntime`) so the ~350-command
/// surface is byte-identical across both. Display-coupled pieces (native
/// dialog/updater/clipboard plugins, window restore, close handling, the
/// Hyprland float rule) are gated on `mode == AppMode::Gui`.
fn build_core_app<R: tauri::Runtime>(
    builder: tauri::Builder<R>,
    mode: AppMode,
) -> tauri::Builder<R> {
    // Web-remote state must exist before the builder so the channel
    // interceptor (registered below) and the WS dispatcher share one
    // channel routing table. The interceptor reroutes a synthesized
    // invoke's channel frames to the owning browser instead of the
    // desktop webview.
    let web_remote_state = web_remote::WebRemoteState::default();
    let web_remote_channel_router = web_remote_state.channel_router();

    let builder = builder
        // Mode is managed state so commands and the setup closure can gate
        // display-coupled behavior on it.
        .manage(mode)
        .manage(state::AppStateStore::default())
        .manage(agent_browser::AgentBrowserManager::new_with_cleanup())
        .manage(indexing::ProjectIndexStore::default())
        .manage(observability::load_observability_store())
        .manage(terminal::PtyState::default())
        .manage(resource_metrics::ResourceMonitorState::default())
        .manage(session_adapters::AdapterState::new())
        .manage(scrollback::ScrollbackCache::default())
        .manage(auth::AuthState::default())
        .manage(skills_sync::SyncEngine::new())
        .manage(commands::agent_chat::ProviderRegistry::new())
        // Per-thread live event channels for the chat pane: each
        // mounted pane attaches a tauri::ipc::Channel keyed by
        // thread_id; forward_event routes that thread's runtime
        // events (incl. the content_delta token stream) to it instead
        // of broadcasting on the global event bus. See issue #75.
        .manage(commands::agent_chat::AgentChatChannelRegistry::default())
        // Per-thread running-subagent tracker: keeps the sidebar
        // "working" spinner alive when the parent chat turn finishes
        // while subagents it spawned are still running. Consulted by
        // forward_event → publish_pane_status. See agent_chat.rs.
        .manage(commands::agent_chat::SubagentTracker::default())
        .manage(commands::agent_chat::RunActivityTracker::default())
        // Step 12 Stage 2 — singleton supervisor for the lazily
        // spawned `opencode serve` child. `ensure_running()` is the
        // entry point used by `opencode_list_models`; the server is
        // not spawned until the first call. Shutting Codemux down
        // drops this state, which `kill_on_drop`-kills the child.
        .manage(std::sync::Arc::new(crate::agent_provider::opencode::OpenCodeServerManager::new()))
        // Step 12 Stage 9 — Codex capability cache. Holds the
        // memoised `model/list` harvest so the picker doesn't
        // re-spawn `codex app-server` on every render. Empty on app
        // boot; populated on first `list_chat_provider_capabilities`
        // call for the Codex provider.
        .manage(std::sync::Arc::new(
            crate::agent_provider::codex::capabilities::CodexCapabilityCache::new(),
        ))
        // Claude capability cache — populated lazily on the first
        // `list_chat_provider_capabilities` call for Claude when
        // `ANTHROPIC_API_KEY` is set (hybrid live harvest against
        // `/v1/models`); otherwise the dispatcher serves the
        // hand-maintained bundle without touching this cache.
        .manage(std::sync::Arc::new(
            crate::agent_provider::claude::capabilities::ClaudeCapabilityCache::new(),
        ))
        // Claude slash-command cache — populated lazily (per cwd) the
        // first time the chat composer's slash popup asks for the
        // provider command list (`list_chat_slash_commands`). Data is
        // harvested live from the deployed CLI via the sidecar's
        // `list-commands` probe; nothing is hardcoded.
        .manage(std::sync::Arc::new(
            crate::agent_provider::claude::slash_commands::ClaudeSlashCommandCache::new(),
        ))
        // MCP runtime registry. `agent_chat_start_session` reads this
        // via `app.state::<McpRegistry>()` to lazily prime servers
        // before launching a chat, and the `commands::mcp::*` Tauri
        // commands take it via `State<'_, McpRegistry>`. Registering
        // it here is mandatory — without `.manage()` the first
        // `state()` call panics with "state() called before manage()".
        .manage(mcp::registry::McpRegistry::default())
        .manage(skills::watcher::SkillsWatcherState::new())
        .manage(skills::inventory::SkillInventoryService::new())
        .manage(database::init_database().unwrap_or_else(|e| {
            eprintln!("[codemux] WARNING: Database init failed: {e}. Using in-memory fallback.");
            database::DatabaseStore::new_in_memory()
        }))
        .manage(web_remote_state)
        // Reroute channel frames opened by a web-remote invoke to the
        // browser that opened them. Returns true only for server-allocated
        // channel ids (the router owns them); genuine desktop-webview
        // channels fall through to Tauri's default delivery.
        .channel_interceptor(move |_webview, callback, index, body| {
            web_remote_channel_router.route(callback.0, index, body)
        })
        .plugin(tauri_plugin_opener::init())
        // Persistent native logging: app log dir + stderr. Warn-level
        // globally so dependency chatter stays out, which still
        // captures the failures that used to vanish — e.g. rfd's
        // "Failed to pick folder" when no dialog backend exists
        // (issue #95). Users can read the file via `codemux logs`.
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Warn)
                .targets([
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stderr),
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::LogDir {
                        file_name: Some(crate::app_logs::LOG_FILE_STEM.into()),
                    }),
                ])
                .max_file_size(2 * 1024 * 1024)
                .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepOne)
                .build(),
        )
        .plugin(tauri_plugin_process::init());

    // Display-coupled plugins — skipped under headless serve, where no
    // window/clipboard/updater backend exists. The matching commands
    // (native dialogs, clipboard paste, updater) short-circuit via
    // `ensure_gui_mode` so a headless caller gets a clean Err instead of
    // reaching an unregistered plugin (which would panic). All three plugin
    // inits are generic over `R: Runtime`; they are gated by mode, not by
    // runtime, so the Wry GUI path registers them exactly as before.
    let mut builder = builder;
    if mode == AppMode::Gui {
        builder = builder
            .plugin(tauri_plugin_dialog::init())
            .plugin(tauri_plugin_updater::Builder::new().build())
            .plugin(tauri_plugin_clipboard_manager::init());
    }

    builder
        // Every finished page load re-applies the current smooth-scrolling
        // preference, so windows created after `setup` (and any reload) get
        // it without a separate window-created hook — each webview owns its
        // own WebKitSettings object.
        //
        // A finished load on the main window is also the app's first proof
        // that the webview came up, which clears the renderer crash sentinel
        // armed before GTK init. `mode` is `Copy`, so the setup closure below
        // still captures it too.
        .on_page_load(move |webview, payload| {
            if !matches!(payload.event(), tauri::webview::PageLoadEvent::Finished) {
                return;
            }
            webview_tuning::apply_to_webview(webview);
            if mode == AppMode::Gui && webview.label() == "main" {
                webview_tuning::mark_startup_successful();
            }
        })
        .setup(move |app| {
            // Reap chat-image staging files leaked by a crash or an
            // abandoned draft (best-effort, off the startup path).
            tauri::async_runtime::spawn(
                commands::agent_chat::sweep_stale_staged_images(),
            );

            #[cfg(debug_assertions)]
            {
                let pid = std::process::id();
                let startup_id =
                    std::env::var("CODEMUX_STARTUP_ID").unwrap_or_else(|_| "<unset>".into());
                crate::diagnostics::native_startup_breadcrumb(&format!(
                    "[{}] startup_id={} pid={} component=tauri event=setup_enter",
                    chrono::Local::now().format("%s"),
                    startup_id,
                    pid
                ));
            }

            // Migrate auth token from file to SQLite (one-time)
            {
                let db: tauri::State<'_, database::DatabaseStore> = app.handle().state();
                let old_path = crate::auth::token_file_path();
                if old_path.exists() {
                    if db.load_auth_token().is_none() {
                        if let Ok(data) = std::fs::read(&old_path) {
                            let _ = db.save_auth_token(&data);
                        }
                    }
                    let _ = std::fs::remove_file(&old_path);
                }
            }

            // Initialize presets from SQLite (must happen after database is managed)
            {
                let db: tauri::State<'_, database::DatabaseStore> = app.handle().state();
                app.manage(presets::PresetStoreState::new(&db));
            }

            let handle = app.handle().clone();
            // Track whether the persisted layout actually loaded. The
            // Windows orphan-cleanup gate below uses this to skip the
            // destructive `cleanup_orphan_scrollbacks(&[])` call after
            // a force-close cascade. See `scrollback::should_run_orphan_cleanup`
            // for the full failure-mode write-up.
            let mut layout_loaded = false;
            if let Some(snapshot) = state::load_persisted_state() {
                let stripped = state::strip_removed_workspaces_from_snapshot(snapshot);
                state::restore_session_ids(&stripped);
                let state: tauri::State<'_, state::AppStateStore> = handle.state();
                state.replace_snapshot(stripped);
                state.migrate_tabs_if_needed();
                state.migrate_project_roots();
                // Backfill repo-root `protected` for workspaces that
                // predate the repo-unit-sync change. Runs off the boot
                // thread because it spawns git subprocesses per
                // workspace; new workspaces get it stamped at create
                // time in `set_workspace_project_root`.
                let protection_handle = handle.clone();
                std::thread::spawn(move || {
                    let state: tauri::State<'_, state::AppStateStore> = protection_handle.state();
                    state.backfill_workspace_protection();
                });
                layout_loaded = true;
            } else if mode == AppMode::Gui {
                // First launch — no persisted layout exists. Replace the
                // default_app_state (which creates a CWD workspace) with an
                // empty state so the user sees the splash screen instead.
                let state: tauri::State<'_, state::AppStateStore> = handle.state();
                state.clear_workspaces();
            }
            // Headless (`codemux serve`) deliberately keeps the CWD workspace
            // `default_app_state()` creates. The clear above exists purely to
            // show the GUI's splash screen, and there is no splash screen to
            // show here — a daemon that emptied its state on first launch
            // would hand the connecting browser an instance with no active
            // workspace, so every workspace-scoped command (`create_terminal_session`
            // first among them) would fail until someone opened a folder from
            // the desktop app. Serving the current directory is both the
            // useful default and what a fresh headless box has to offer.

            // Ensure .mcp.json exists for all active workspaces
            if mcp_server::is_auto_mcp_enabled(&handle) {
                let state: tauri::State<'_, state::AppStateStore> = handle.state();
                for ws in state.snapshot().workspaces.iter() {
                    mcp_server::upsert_mcp_config(
                        std::path::Path::new(&ws.cwd),
                        &ws.workspace_id.0,
                    );
                }
            }

            // Clean up orphan scrollback dirs and enforce disk limit
            {
                let state: tauri::State<'_, state::AppStateStore> = handle.state();
                let ws_ids: Vec<String> = state
                    .snapshot()
                    .workspaces
                    .iter()
                    .map(|w| w.workspace_id.0.clone())
                    .collect();

                // Windows defensive gate: skip orphan cleanup when layout
                // failed to load. Otherwise `cleanup_orphan_scrollbacks(&[])`
                // (called with the empty workspace list produced by
                // `state.clear_workspaces()`) would wipe every saved
                // scrollback dir on disk — the second half of the
                // "terminals start fresh on Windows" cascade. On Linux the
                // gate is a pass-through (always returns true), preserving
                // historical behavior verbatim. See
                // `scrollback::should_run_orphan_cleanup` for the full
                // failure-mode rationale.
                if scrollback::should_run_orphan_cleanup(layout_loaded) {
                    let removed = scrollback::cleanup_orphan_scrollbacks(&ws_ids);
                    if !removed.is_empty() {
                        eprintln!(
                            "[codemux::scrollback] Cleaned up {} orphan scrollback dirs",
                            removed.len()
                        );
                    }
                } else {
                    eprintln!(
                        "[codemux::scrollback] Skipping orphan cleanup — layout state was not loaded \
                         (Windows defensive gate to prevent force-close cascade from wiping scrollback)"
                    );
                }
                // Enforce disk limit from settings
                let max_mb = settings_sync::load_cache()
                    .map(|s| s.session_restore.max_total_mb)
                    .unwrap_or(100);
                let freed = scrollback::enforce_disk_limit(max_mb as u64 * 1024 * 1024);
                if freed > 0 {
                    eprintln!(
                        "[codemux::scrollback] Freed {} bytes to stay under {max_mb}MB limit",
                        freed
                    );
                }
            }

            // GUI-only display-coupled window wiring: size/position restore,
            // the save-on-close serialization handshake, and the Hyprland
            // file-dialog float rule. Headless serve has no real window to
            // size, position, save, or float, so the whole block is skipped
            // there.
            if mode == AppMode::Gui {
            // Turn WebKitGTK's smooth-scrolling animation off for every window
            // that already exists. Its 200 ms eased retarget restarts on each
            // high-resolution wheel event, so a fast flick scrolls *less* than
            // a slow one (WebKit bug 258926). Windows created later pick the
            // same setting up from the page-load hook, and the frontend can
            // flip it at runtime via the `set_smooth_scrolling` command.
            webview_tuning::refresh_all(&handle);

            // Restore window size from SQLite
            {
                let db: tauri::State<'_, database::DatabaseStore> = handle.state();
                let saved_w = db.get_ui_state("window_width").and_then(|v| v.parse::<f64>().ok());
                let saved_h = db.get_ui_state("window_height").and_then(|v| v.parse::<f64>().ok());

                if let (Some(w), Some(h), Some(window)) = (saved_w, saved_h, app.get_webview_window("main")) {
                    // Reject nonsensical or tiled/maximized dimensions
                    let too_small = w < 200.0 || h < 200.0;
                    let fills_monitor = window.current_monitor().ok().flatten().map_or(false, |m| {
                        let sf = m.scale_factor();
                        let mw = m.size().width as f64 / sf;
                        let mh = m.size().height as f64 / sf;
                        w + 100.0 >= mw && h + 100.0 >= mh
                    });
                    if !too_small && !fills_monitor {
                        let _ = window.set_size(tauri::LogicalSize::new(w, h));
                    }
                    // else: keep tauri.conf.json default (800×600)
                }

                // Restore position, but only if it falls within a visible monitor.
                // Skips off-screen positions (e.g. second monitor was unplugged).
                // On Wayland set_position is a compositor no-op regardless.
                if let (Some(x), Some(y)) = (
                    db.get_ui_state("window_x").and_then(|v| v.parse::<i32>().ok()),
                    db.get_ui_state("window_y").and_then(|v| v.parse::<i32>().ok()),
                ) {
                    if let Some(window) = app.get_webview_window("main") {
                        let on_screen = window.available_monitors().map_or(false, |monitors| {
                            monitors.iter().any(|m| {
                                let sf = m.scale_factor();
                                let pos = m.position();
                                let sz = m.size();
                                let mx = (pos.x as f64 / sf) as i32;
                                let my = (pos.y as f64 / sf) as i32;
                                let mw = (sz.width as f64 / sf) as i32;
                                let mh = (sz.height as f64 / sf) as i32;
                                x >= mx && x < mx + mw && y >= my && y < my + mh
                            })
                        });
                        if on_screen {
                            let _ = window.set_position(tauri::LogicalPosition::new(x, y));
                        }
                    }
                }
            }

            // Save window state and serialize terminal buffers on close
            {
                let close_handle = handle.clone();
                let close_in_progress = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                if let Some(window) = app.get_webview_window("main") {
                    let cip = close_in_progress.clone();
                    window.on_window_event(move |event| {
                        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                            // Prevent re-entry: if we already initiated the close sequence,
                            // let the window close normally this time.
                            if cip.swap(true, std::sync::atomic::Ordering::AcqRel) {
                                return;
                            }

                            // Save window dimensions
                            save_window_state(&close_handle);

                            // Check if session restore is enabled
                            let session_restore_enabled = {
                                let cache = settings_sync::load_cache();
                                cache
                                    .map(|s| s.session_restore.enabled)
                                    .unwrap_or(true)
                            };

                            if session_restore_enabled {
                                // Prevent the window from closing immediately —
                                // we need to ask the frontend to serialize terminal buffers.
                                api.prevent_close();

                                let async_handle = close_handle.clone();
                                let async_cip = cip.clone();
                                tauri::async_runtime::spawn(async move {
                                    // Emit event for the frontend to serialize buffers
                                    let _ = async_handle.emit("serialize-terminal-buffers", ());

                                    // Wait for the frontend to signal completion, or
                                    // timeout. The frontend emits
                                    // `scrollback-serialization-complete` when done.
                                    //
                                    // Timeout differs by platform: Linux/macOS IPC is
                                    // fast enough for 3 seconds to be comfortable. On
                                    // Windows, slower Tauri IPC + slower xterm
                                    // serialization for many panes can exceed 3s,
                                    // which silently truncates serialization and
                                    // leaves panes "fresh" on the next launch. 10s
                                    // gives enough headroom without making clean
                                    // closes feel sluggish.
                                    #[cfg(windows)]
                                    const SCROLLBACK_TIMEOUT_SECS: u64 = 10;
                                    #[cfg(not(windows))]
                                    const SCROLLBACK_TIMEOUT_SECS: u64 = 3;

                                    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
                                    let tx = std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));

                                    let listen_handle = async_handle.clone();
                                    let listener = listen_handle.listen("scrollback-serialization-complete", move |_| {
                                        if let Some(tx) = tx.lock().unwrap().take() {
                                            let _ = tx.send(());
                                        }
                                    });

                                    let serialize_result = tokio::time::timeout(
                                        std::time::Duration::from_secs(SCROLLBACK_TIMEOUT_SECS),
                                        rx,
                                    ).await;

                                    async_handle.unlisten(listener);

                                    if serialize_result.is_err() {
                                        eprintln!(
                                            "[codemux::lib] Frontend scrollback serialization timed out after {SCROLLBACK_TIMEOUT_SECS}s — \
                                             cached scrollback may be incomplete on next launch"
                                        );
                                    }

                                    // Windows backend backstop: flush any cached
                                    // scrollback entries that the frontend didn't
                                    // drain before the timeout. Cached entries come
                                    // from panes that were unmounted during the
                                    // session (tab/workspace switches) and exist
                                    // only in memory until either the frontend
                                    // calls `flushScrollbackCache` (happy path) or
                                    // this backstop fires (timeout path).
                                    //
                                    // Idempotent: if the frontend already drained
                                    // the cache, this is a no-op (returns 0).
                                    //
                                    // Windows-only because the timeout fires in
                                    // practice only on Windows. Linux IPC is fast
                                    // enough that the frontend reliably completes
                                    // the serialization dance well within budget.
                                    #[cfg(windows)]
                                    {
                                        let cache: tauri::State<'_, scrollback::ScrollbackCache> = async_handle.state();
                                        let flushed = scrollback::flush_cache_to_disk(&cache);
                                        if flushed > 0 {
                                            eprintln!(
                                                "[codemux::lib] Backend backstop flushed {flushed} cached scrollback entries \
                                                 after frontend serialization (timeout or partial completion)"
                                            );
                                        }
                                    }

                                    // Refresh scrollback metadata for sessions that were
                                    // never re-serialized by the frontend (inactive workspaces).
                                    let app_state: tauri::State<'_, state::AppStateStore> = async_handle.state();
                                    scrollback::refresh_stale_scrollback_metadata(&app_state);

                                    // Flush layout state
                                    state::flush_persisted_state(&app_state);

                                    // Now actually close the window. The re-entry guard
                                    // ensures this CloseRequested passes through.
                                    if let Some(w) = async_handle.get_webview_window("main") {
                                        let _ = w.destroy();
                                    }

                                    // Reset flag in case destroy failed
                                    async_cip.store(false, std::sync::atomic::Ordering::Release);
                                });
                            } else {
                                // Session restore disabled — close immediately
                                let app_state: tauri::State<'_, state::AppStateStore> = close_handle.state();
                                scrollback::refresh_stale_scrollback_metadata(&app_state);
                                state::flush_persisted_state(&app_state);
                            }
                        }
                    });
                }
            }

            // On Hyprland, xdg-desktop-portal-gtk file dialogs tile by default.
            // Inject a window rule to float them.
            #[cfg(target_os = "linux")]
            {
                let _ = std::process::Command::new("hyprctl")
                    .args([
                        "keyword",
                        "windowrule",
                        "float on, center on, size 800 600, match:class ^(xdg-desktop-portal-gtk)$",
                    ])
                    .output();
            }
            } // end GUI-only display-coupled window wiring

            let observability: tauri::State<'_, observability::ObservabilityStore> = handle.state();
            observability.increment_metric("startup_count");
            observability.log("app", observability::LogLevel::Info, "Codemux startup".into(), vec![]);
            config::watch_theme_file(handle.clone());

            // Hook server must be running BEFORE PTYs spawn so that restored
            // sessions get CODEMUX_HOOK_PORT in their environment.
            hooks::start_hook_server(app.handle().clone());
            // Register agent lifecycle hooks so the status dot tracks
            // working / needs-input / done. Claude Code always; Codex,
            // Gemini, OpenCode, and Pi only when their config dir already
            // exists (so we never create config for uninstalled tools).
            hooks::register_claude_code_hooks();
            hooks::register_codex_hooks();
            hooks::register_gemini_hooks();
            hooks::register_opencode_plugin();
            hooks::register_pi_extension();

            terminal::spawn_missing_ptys(handle);

            // Warm up the PTY daemon connection. The daemon is now the
            // default for every PTY spawn (subject to graceful fallback
            // for Windows + circuit-breaker reasons — see
            // `terminal::daemon_path_viable`), so we eagerly adopt or
            // spawn it during setup so the first agent spawn doesn't pay
            // the spawn-detached latency on the critical path.
            //
            // Skipping the warmup when `CODEMUX_DISABLE_PTY_DAEMON=1` is
            // set lets a user kill the daemon entirely if a regression
            // ever ships and they need to roll back without uninstalling.
            #[cfg(unix)]
            {
                if std::env::var_os("CODEMUX_DISABLE_PTY_DAEMON").is_none() {
                    tauri::async_runtime::spawn(async move {
                        match pty_daemon::ensure_daemon().await {
                            Ok(client) => match client.list().await {
                                Ok(sessions) => {
                                    eprintln!(
                                        "[codemux::pty_daemon] startup adoption ok: {} live sessions",
                                        sessions.len()
                                    );
                                }
                                Err(error) => {
                                    eprintln!(
                                        "[codemux::pty_daemon] startup adoption: list failed: {error}"
                                    );
                                }
                            },
                            Err(error) => {
                                eprintln!(
                                    "[codemux::pty_daemon] startup adoption failed: {error} \
                                     (falling back to in-process PTYs)"
                                );
                            }
                        }
                    });
                }
            }

            // Initialize the project index from the active workspace's CWD.
            // If no workspace exists yet, the index stays empty and the watcher
            // does not start — avoiding the old bug where $HOME was scanned.
            {
                let app_state: tauri::State<'_, state::AppStateStore> = app.handle().state();
                let index_store: tauri::State<'_, indexing::ProjectIndexStore> = app.handle().state();
                if let Some((_, cwd)) = app_state.active_workspace_cwd() {
                    index_store.initialize_for_project(std::path::PathBuf::from(cwd));
                }
                indexing::spawn_index_watcher(index_store);
            }

            control::spawn_control_server(app.handle().clone());

            // Background host-upgrade poller. ~5s after app setup it
            // walks every registered SSH host, version-checks its
            // codemux-remote against the one this build ships, and
            // silently re-bootstraps any host that's behind. So when
            // the user updates Codemux on the desktop, their hosts
            // catch up on their own — no Test/Push click required.
            // Safe to run on every app start: cheap when versions
            // match (one SSH probe), idempotent re-provision on
            // upgrade, never installs codemux-remote where it isn't
            // already present (that still requires the Install
            // button's consent).
            #[cfg(unix)]
            crate::hosts_upgrade::spawn(app.handle().clone());

            // Background host-inventory poller. Asymmetric companion
            // to the upgrade loop: where the upgrade loop keeps the
            // remote `codemux-remote` binary at the right version,
            // this loop keeps the remote daemon's *workspace registry*
            // visible to the user's account. Without it, a workspace
            // an agent creates on a host (via the MCP `workspace_create`
            // tool) never appears in any dev device's overview because
            // no laptop ever pushed it. See `docs/features/workspaces-sync.md`
            // for the "asymmetric publish" model: codemux-remote hosts
            // auto-publish, codemux-app dev devices keep their existing
            // explicit push/pull.
            #[cfg(unix)]
            crate::hosts_inventory::spawn(app.handle().clone());

            // Resolve the bundled claude-agent sidecar and pin the path via
            // env var so the adapter (which has no AppHandle access at
            // construction time) can find it. `resolve_sidecar` honors an
            // already-set override, then Tauri's resource dir (the
            // per-target binary staged into the `codemux-claude-sidecar-*`
            // resources glob for release builds), then — debug builds only —
            // the `src-tauri/binaries/` dev-tree copy that `npm run
            // tauri:dev` does not stage into the resource dir. Returns `None`
            // when the binary exists nowhere, leaving the provider slot empty
            // (provider_not_configured) rather than pinning a bad path.
            //
            // The bun --compile binary is huge (~100 MB) with the dynamic
            // section at offset 96 MB, which patchelf corrupts when run by
            // linuxdeploy during AppImage bundling. Shipping it as a resource
            // (usr/share/...) instead of an externalBin (usr/bin/) keeps it
            // out of linuxdeploy's scan path. See `tauri.conf.json` for the
            // mirror change.
            let resource_dir = app.handle().path().resource_dir().ok();
            if let Some(sidecar) = agent_provider::claude::sidecar_path::resolve_sidecar(
                resource_dir.as_deref(),
            ) {
                std::env::set_var(
                    agent_provider::claude::sidecar_path::SIDECAR_PATH_ENV,
                    sidecar,
                );
            }

            // Agent-chat provider registry initialisation.
            //
            // When `enable_agent_chat` is on, spawn concrete Claude /
            // Codex provider adapters into the
            // `commands::agent_chat::ProviderRegistry` managed state.
            // Claude's adapter resolves its sidecar binary eagerly,
            // which involves filesystem IO — run the whole block on a
            // background task so startup is not blocked, and treat
            // per-provider failures as recoverable (leave the slot
            // empty; commands routing to it return
            // `provider_not_configured`).
            //
            // When the flag is off this whole task is skipped so
            // memory is not consumed.
            {
                let observability: tauri::State<'_, observability::ObservabilityStore> =
                    app.handle().state();
                if observability.agent_chat_enabled() {
                    let registry_handle = app.handle().clone();
                    tauri::async_runtime::spawn(async move {
                        let registry: tauri::State<
                            '_,
                            commands::agent_chat::ProviderRegistry,
                        > = registry_handle.state();

                        match agent_provider::claude::ClaudeAgentProvider::new(
                            agent_provider::claude::ClaudeProviderConfig::default(),
                        )
                        .await
                        {
                            Ok(provider) => {
                                registry
                                    .set_claude(std::sync::Arc::new(provider) as _)
                                    .await;
                            }
                            Err(error) => {
                                eprintln!(
                                    "[codemux::agent_chat] Claude provider init failed: {error}; \
                                     commands routing to it will return provider_not_configured"
                                );
                            }
                        }

                        let codex = agent_provider::codex::CodexAgentProvider::new(
                            agent_provider::codex::CodexProviderConfig::default(),
                        );
                        registry
                            .set_codex(std::sync::Arc::new(codex) as _)
                            .await;

                        // OpenCode provider — Step 12 Stage 8. Shares
                        // the singleton OpenCodeServerManager held in
                        // Tauri-managed state (registered as
                        // `Arc<OpenCodeServerManager>` at lib.rs:155)
                        // so a single `opencode serve` child backs
                        // both the model-list discovery flow and the
                        // chat runtime. The server is spawned lazily
                        // on the first start_session call — a missing
                        // `opencode` binary surfaces as
                        // `NotInstalled` per session start, never as
                        // a startup failure.
                        let opencode_manager: tauri::State<
                            '_,
                            std::sync::Arc<agent_provider::opencode::OpenCodeServerManager>,
                        > = registry_handle.state();
                        let opencode_provider =
                            agent_provider::opencode::OpenCodeAgentProvider::new(
                                opencode_manager.inner().clone(),
                                agent_provider::opencode::OpenCodeProviderConfig::default(),
                            );
                        registry
                            .set_opencode(std::sync::Arc::new(opencode_provider) as _)
                            .await;

                        // Bridge provider events to the frontend
                        // exactly once, after both providers have been
                        // injected (or attempted). spawn_event_bridge
                        // subscribes every registered provider in a
                        // fresh Tokio task each.
                        commands::agent_chat::spawn_event_bridge(registry_handle.clone())
                            .await;
                        // Detect silently-dead runs: a periodic sweep that
                        // flags mid-turn threads gone quiet past the stall
                        // threshold (issue #154). Advisory only — it never
                        // kills a session.
                        commands::agent_chat::spawn_stall_watchdog(
                            registry_handle.clone(),
                        )
                        .await;
                    });
                }
            }

            // Periodically refresh git info for the workspaces due this tick
            // (so non-active sidebar rows stay honest when agents switch
            // branches), plus PR/issue info for the active workspace only
            // (where `gh` CLI calls are the expensive part and only the
            // visible row benefits from being up-to-date every 5s).
            //
            // Three properties keep this off the interaction path:
            //
            //  - **Tiered.** The active workspace refreshes every tick; the
            //    rest are spread across `GIT_SWEEP_STRIDE` ticks by list
            //    position, so a 79-workspace profile forks ~N/6 gathers per
            //    tick instead of N.
            //  - **Deduped.** Workspaces sharing a checkout gather once.
            //  - **Change-gated.** The snapshot emit happens only when a
            //    workspace's git/PR/issue metadata actually moved. An idle
            //    fleet costs zero serialise + IPC + render passes.
            //
            // Serial iteration on purpose — cost stays predictable — but the
            // git subprocesses run on the blocking pool so a slow disk can't
            // stall the Tokio worker that also drives in-flight IPC calls.
            // Skips the tick entirely when the main window is off screen.
            let git_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(jobs::startup_jitter(std::time::Duration::from_secs(2))).await;
                let mut tick: u64 = 0;
                // Last branch-PR lookup error logged by this loop. The loop
                // ticks every 5 s, so a persistent condition (expired `gh`
                // token, no network) would otherwise write the same line to
                // stderr ~17k times a day. Log each distinct message once and
                // reset on the next success, so a *new* failure is still loud.
                let mut last_pr_error: Option<String> = None;
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    tick = tick.wrapping_add(1);

                    // Pause polling only when the sidebar is genuinely not
                    // on screen. `is_focused()` would wrongly pause when the
                    // window is visible on another Hyprland/KDE/Gnome
                    // workspace — users there still expect the sidebar to
                    // reflect live branch state. `is_minimized()` returning
                    // Err defaults to "treat as minimized" so we skip the
                    // tick rather than busy-loop on a broken window handle.
                    let window_active = git_handle
                        .get_webview_window("main")
                        .map(|w| {
                            let visible = w.is_visible().unwrap_or(false);
                            let minimized = w.is_minimized().unwrap_or(true);
                            visible && !minimized
                        })
                        .unwrap_or(false);
                    if !window_active {
                        continue;
                    }

                    // Phase 0 attribution: a periodic tick that overruns a
                    // frame budget is a candidate explanation for a janky
                    // interaction. Scheduling is untouched here — only the
                    // measurement is new.
                    let tick_started = std::time::Instant::now();
                    let state: tauri::State<'_, state::AppStateStore> = git_handle.state();
                    // Sorted so a workspace keeps the same stride bucket
                    // across ticks — a shifting bucket could starve a row.
                    let mut all_workspaces: Vec<(String, String)> =
                        state.all_workspace_cwds().into_iter().collect();
                    all_workspaces.sort();
                    let active = state.active_workspace_cwd();

                    // Refresh the workspaces due this tick, gathering once
                    // per distinct checkout.
                    //
                    // `changed` tracks the domains that still ride a full
                    // snapshot (PR pill, linked issue). Git metadata ships as
                    // a per-workspace `WorkspaceGit` delta instead — the
                    // steady state of this loop is one workspace's branch
                    // counters moving, which has no business re-serialising
                    // and re-parsing the whole app state.
                    let mut changed = false;
                    let mut gathered: std::collections::HashMap<
                        String,
                        commands::workspace::WorkspaceGitInfo,
                    > = std::collections::HashMap::new();
                    let steps = jobs::plan_git_sweep(
                        tick,
                        &all_workspaces,
                        active.as_ref().map(|(id, _)| id.as_str()),
                        jobs::GIT_SWEEP_STRIDE,
                    );
                    for step in steps {
                        let info = if step.gather {
                            let path = std::path::PathBuf::from(&step.cwd);
                            let gather = tokio::task::spawn_blocking(move || {
                                commands::workspace::gather_workspace_git_info(&path)
                            })
                            .await;
                            // A panicked gather knows nothing about the
                            // checkout; the default it would fall back to says
                            // "not a repo", which wipes the branch and the PR
                            // pill. Leaving the last good values alone is the
                            // honest outcome — the next tick retries.
                            let Ok(info) = gather else {
                                eprintln!(
                                    "[codemux::job] git gather failed for {}, keeping last values",
                                    step.cwd
                                );
                                continue;
                            };
                            gathered.insert(step.cwd.clone(), info.clone());
                            info
                        } else {
                            match gathered.get(&step.cwd) {
                                Some(info) => info.clone(),
                                None => continue,
                            }
                        };
                        if let Some(delta) = state.update_workspace_git_info_delta(
                            &step.workspace_id,
                            commands::workspace::git_delta_of(info),
                        ) {
                            state::emit_app_state_delta(&git_handle, delta);
                        }
                    }

                    // PR / linked-issue refresh stays scoped to the active
                    // workspace — each call shells out to `gh`, which is the
                    // expensive part, and the user can only see one PR panel
                    // at a time anyway.
                    if let Some((workspace_id, cwd)) = active {
                        let path = std::path::PathBuf::from(&cwd);
                        // `is_github_repo` is a pure `git remote -v` read (no
                        // `gh`, no network), and it matches the gate the
                        // dedicated PR poller already applies. Without it a
                        // GitLab / local-only workspace would shell out to
                        // `gh` every tick just to fail.
                        if github::gh_available() && github::is_github_repo(&path) {
                            // Branch/repository identity owns association. A
                            // failed lookup preserves the last known state;
                            // a successful empty result is authoritative and
                            // clears any stale association.
                            let lookup = github::get_workspace_pr(&path);
                            match &lookup {
                                Err(e) => {
                                    let message = format!(
                                        "[codemux::github] Failed to fetch PR info for {cwd}: {e}"
                                    );
                                    if last_pr_error.as_deref() != Some(message.as_str()) {
                                        eprintln!("{message}");
                                        last_pr_error = Some(message);
                                    }
                                }
                                Ok(_) => last_pr_error = None,
                            }
                            // Decision matrix lives in `github::branch_pr_outcome`
                            // (Write / Clear / Preserve) so the poll loop and
                            // the on-demand refresh cannot drift apart. Each
                            // arm still feeds `changed`, because the PR pill is
                            // one of the two domains this loop ships on a FULL
                            // snapshot rather than a delta — dropping the flag
                            // would leave a badge change with nothing to emit
                            // it, and the pill would sit stale until some
                            // unrelated mutation happened to flush the state.
                            match github::branch_pr_outcome(lookup) {
                                github::BranchPrOutcome::Write(pr) => {
                                    changed |= state.update_workspace_pr_info(
                                        &workspace_id,
                                        Some(pr.number),
                                        Some(pr.display_state()),
                                        Some(pr.url),
                                        pr.head_branch,
                                    );
                                }
                                github::BranchPrOutcome::Clear => {
                                    changed |= state.update_workspace_pr_info(
                                        &workspace_id,
                                        None,
                                        None,
                                        None,
                                        None,
                                    );
                                }
                                // Nothing was written, so nothing is owed an
                                // emit — `Preserve` exists precisely to leave
                                // the last known pill alone.
                                github::BranchPrOutcome::Preserve => {}
                            }

                            // Refresh linked issue state (lightweight: only if workspace has one)
                            let issue_number = {
                                let snap = state.snapshot();
                                snap.workspaces.iter()
                                    .find(|w| w.workspace_id.0 == workspace_id)
                                    .and_then(|w| w.linked_issue.as_ref().map(|li| li.number))
                            };
                            if let Some(num) = issue_number {
                                if let Ok(issue) = github::get_github_issue(&path, num) {
                                    changed |= state.link_workspace_issue(&workspace_id, github::LinkedIssue {
                                        number: issue.number,
                                        title: issue.title,
                                        state: issue.state,
                                        labels: issue.labels,
                                    });
                                }
                            }
                        }
                    }

                    // At most one full emit per tick, and only when this tick
                    // moved something outside the delta domains (the PR pill
                    // or the linked issue). An idle fleet emits nothing.
                    //
                    // Coalesced because this loop fires every 5 s; if a user
                    // action emits within 16 ms of this tick, both collapse
                    // into one snapshot serialise + IPC + frontend render
                    // pass instead of two.
                    if changed {
                        state::schedule_emit_app_state(&git_handle);
                    }

                    let tick_ms = tick_started.elapsed().as_millis();
                    if tick_ms > 50 {
                        eprintln!("[codemux::perf::job] git-sweep tick={tick_ms}ms");
                    }
                }
            });

            // Periodically `git fetch` every workspace's remote so the 5s
            // read-only loop above can compare HEAD against fresh upstream
            // refs. Without this, `git rev-list --left-right HEAD...@{upstream}`
            // returns stale counts (typically 0/0) until something else
            // fetches, so the sidebar's ahead/behind arrows never light up
            // for commits pushed by teammates while Codemux is open.
            //
            // Runs at a coarser cadence (60s) than the read loop because
            // each fetch is a real network call. Each per-workspace fetch
            // is spawned independently with a 10s timeout so one slow or
            // offline remote can't stall the others. `GIT_TERMINAL_PROMPT=0`
            // prevents git from ever blocking on a credential prompt.
            // Errors are intentionally swallowed: repos without remotes,
            // transient offline state, and rejected creds are routine and
            // shouldn't pollute the log on every tick.

            // ── Automation scheduler ──
            //
            // Evaluates the automation schedule once a minute.
            // `automations::scheduler::tick` records a run row for every
            // due automation and advances its `next_run_at`; each
            // freshly-fired run is emitted as an `automations://fire`
            // event so the frontend executor can create the workspace
            // and start the agent. This is the desktop's scheduler —
            // remote hosts run the same `tick` via the
            // `codemux-remote scheduler` subcommand.
            let scheduler_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                // Recover runs orphaned by a previous crash or quit
                // before the first tick — a run stuck `running` would
                // otherwise keep its automation `skipped_busy` forever.
                {
                    let db: tauri::State<'_, database::DatabaseStore> =
                        scheduler_handle.state();
                    let ceiling = (chrono::Utc::now()
                        - chrono::Duration::hours(6))
                    .to_rfc3339();
                    let reconciled = db.reconcile_stale_runs(&ceiling);
                    if reconciled > 0 {
                        eprintln!(
                            "[codemux::scheduler] reconciled {reconciled} stale run(s)"
                        );
                    }
                }
                // Pull the automation registry from the account once at
                // startup so this device sees automations created
                // elsewhere.
                commands::automations::schedule_automations_sync(
                    scheduler_handle.clone(),
                );
                let mut ticker = tokio::time::interval(std::time::Duration::from_secs(
                    automations::scheduler::TICK_INTERVAL_SECS,
                ));
                // `interval`'s first tick is immediate — consume it so
                // the first real evaluation happens one interval in,
                // after the app has settled.
                ticker.tick().await;
                loop {
                    ticker.tick().await;
                    // Pull registry changes from the user's other
                    // devices on every tick so the local list stays
                    // fresh; the in-progress guard collapses overlap.
                    commands::automations::schedule_automations_sync(
                        scheduler_handle.clone(),
                    );
                    // Evaluate the schedule, then resolve each fired run
                    // to its automation. Both are short, synchronous DB
                    // touches scoped so the `State` borrow never crosses
                    // an await.
                    let fired: Vec<(
                        database::AutomationRecord,
                        database::AutomationRunRecord,
                    )> = {
                        let db: tauri::State<'_, database::DatabaseStore> =
                            scheduler_handle.state();
                        // `local_only = true`: the desktop runs only
                        // automations targeting this machine; a
                        // host-assigned automation is its host's job.
                        automations::scheduler::tick(&db, chrono::Utc::now(), true)
                            .into_iter()
                            .filter_map(|run| {
                                db.get_automation(run.automation_id)
                                    .map(|automation| (automation, run))
                            })
                            .collect()
                    };
                    for (automation, run) in fired {
                        // Surface the fire to any open window…
                        if let Err(error) =
                            scheduler_handle.emit("automations://fire", &run)
                        {
                            eprintln!(
                                "[codemux::scheduler] failed to emit fire event: {error}"
                            );
                        }
                        // …and execute it: create a worktree and run
                        // the agent, recording the terminal status.
                        tauri::async_runtime::spawn(
                            automations::executor::execute_run(
                                scheduler_handle.clone(),
                                automation,
                                run,
                            ),
                        );
                    }
                }
            });

            // ── Agent-browser orphan sweep (issue #126) ──
            //
            // Reap-on-close (see `reap_agent_browser_sessions` in
            // commands/workspace.rs) is the primary teardown mechanism: it
            // fires the instant a workspace closes on either close path.
            // This sweep is the backstop for any other route a workspace
            // can disappear through (e.g. a future removal path that
            // forgets to call the reap helper, or a state mutation this
            // review missed) — without it, a single missed teardown site
            // would leak a headless Chromium daemon for the rest of the
            // app's lifetime, which is exactly the bug reported in #126.
            //
            // Safety properties:
            //   - In-memory-keys-only: `manager.session_keys()` only ever
            //     returns sessions THIS app instance spawned, so the sweep
            //     can never touch a daemon owned by another codemux
            //     instance (no `agent-browser session list` shell-out).
            //   - `ws-*` filter: only workspace-scoped session names are
            //     eligible for the sweep (`orphaned_ws_sessions`). User
            //     browser-pane sessions (`browser-NNN`) and the `default`
            //     session are never touched.
            //   - Ordering: `tracked` is snapshotted from the manager
            //     BEFORE `live` is computed from app state. A session
            //     created concurrently with a sweep tick is therefore
            //     guaranteed to show up in `live` (its workspace still
            //     exists when we read state) even if it raced into
            //     `tracked` — so a session can never be reaped moments
            //     after it was created.
            let sweep_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                const SWEEP_INTERVAL_SECS: u64 = 300;
                let mut ticker = tokio::time::interval(std::time::Duration::from_secs(
                    SWEEP_INTERVAL_SECS,
                ));
                // Consume the immediate first tick — the sweep must not
                // run at t=0, before startup reconcile (PID-file adoption,
                // workspace hydration) has had room to settle.
                ticker.tick().await;
                loop {
                    ticker.tick().await;

                    let manager: tauri::State<'_, agent_browser::AgentBrowserManager> =
                        sweep_handle.state();
                    // FIRST: snapshot tracked keys before computing the
                    // live set — see the ordering note above.
                    let tracked = manager.session_keys().await;

                    let state: tauri::State<'_, state::AppStateStore> =
                        sweep_handle.state();
                    let live = state.live_agent_browser_session_names();

                    for name in agent_browser::orphaned_ws_sessions(&tracked, &live) {
                        eprintln!(
                            "[codemux::browser] sweep: closing orphaned agent-browser \
                             session {name} (owning workspace gone)"
                        );
                        if let Err(error) = manager.close(&name).await {
                            eprintln!(
                                "[codemux::browser] sweep: failed to close orphaned \
                                 session {name}: {error}"
                            );
                        }
                    }
                }
            });

            let fetch_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                use futures_util::stream::StreamExt as _;
                const TICK_SECS: u64 = 60;
                const PER_FETCH_TIMEOUT_SECS: u64 = 10;
                /// Two `git fetch` children at a time. Unbounded fan-out let
                /// a large profile start N network+disk children on one
                /// instant; the tick now drains its own queue and the next
                /// tick can't overlap it.
                const FETCH_CONCURRENCY: usize = 2;
                tokio::time::sleep(jobs::startup_jitter(std::time::Duration::from_secs(5))).await;
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(TICK_SECS)).await;

                    // Same window-active gate as the 5s loop — no point
                    // burning network when the sidebar isn't on screen.
                    let window_active = fetch_handle
                        .get_webview_window("main")
                        .map(|w| {
                            let visible = w.is_visible().unwrap_or(false);
                            let minimized = w.is_minimized().unwrap_or(true);
                            visible && !minimized
                        })
                        .unwrap_or(false);
                    if !window_active {
                        continue;
                    }

                    let state: tauri::State<'_, state::AppStateStore> = fetch_handle.state();
                    // Attach-in-place workspaces are filtered for the same
                    // reason `all_workspace_cwds` filters them: their cwd is
                    // a path on another device.
                    let entries = state.fetchable_workspace_checkouts();
                    // One fetch per repository: linked worktrees share the
                    // object store and remote refs of their project root.
                    let targets = jobs::dedupe_fetch_targets(&entries);
                    let tick_started = std::time::Instant::now();
                    let target_count = targets.len();

                    futures_util::stream::iter(targets)
                        .for_each_concurrent(FETCH_CONCURRENCY, |cwd| async move {
                            let _ = tokio::time::timeout(
                                std::time::Duration::from_secs(PER_FETCH_TIMEOUT_SECS),
                                tokio::process::Command::new("git")
                                    .args(["fetch", "--prune", "--quiet", "--no-tags"])
                                    .current_dir(&cwd)
                                    .env("GIT_TERMINAL_PROMPT", "0")
                                    // Timing out drops the future, which must
                                    // take the child with it — an unreachable
                                    // network otherwise leaves a `git fetch`
                                    // per repo per tick running forever.
                                    .kill_on_drop(true)
                                    .output(),
                            )
                            .await;
                            // Result intentionally discarded — see comment
                            // on the parent loop.
                        })
                        .await;

                    // The tick now drains its own queue, so ticks can't
                    // overlap — but a queue that outlives its period means
                    // fetches are the reason a repo's ahead/behind lags.
                    let tick_secs = tick_started.elapsed().as_secs();
                    if tick_secs >= TICK_SECS {
                        eprintln!(
                            "[codemux::perf::job] git-fetch tick={tick_secs}s repos={target_count} \
                             (drain outlasted the {TICK_SECS}s period)"
                        );
                    }
                }
            });

            // Background PR polling for the sidebar PR-status icon. The 5s
            // tick above only refreshes the active workspace's PR info; this
            // 60s tick walks every workspace so non-active sidebar rows
            // don't go stale (the user might leave the app open while a
            // teammate merges a PR on GitHub).
            //
            // Sequential per tick on purpose — each branch PR query is a
            // subprocess fork, and we'd rather take ~Nx longer than slam
            // `gh` with N parallel children. Each call gets a 10s timeout
            // so a single hanging `gh` doesn't block subsequent workspaces.
            //
            // Pauses the per-workspace walk when `gh` is missing or
            // unauthenticated (status is rechecked every tick — `gh auth
            // status` is itself a cheap fork+exec, ~50ms).
            let pr_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                const TICK_SECS: u64 = 60;
                const PER_CALL_TIMEOUT_SECS: u64 = 10;
                let mut last_status_authed: Option<bool> = None;
                tokio::time::sleep(jobs::startup_jitter(std::time::Duration::from_secs(3))).await;

                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(TICK_SECS)).await;

                    // Check gh status outside spawn_blocking because it's a
                    // fast call and we don't need the timeout machinery for it.
                    let gh_status = github::check_gh_status();
                    let authed = matches!(gh_status, github::GhStatus::Authenticated { .. });

                    // Log status transitions only — avoids spamming when
                    // the user is offline for an extended period.
                    if last_status_authed != Some(authed) {
                        eprintln!(
                            "[codemux::pr-poll] gh auth status: {} (was: {:?})",
                            if authed { "authenticated" } else { "unavailable" },
                            last_status_authed,
                        );
                        last_status_authed = Some(authed);
                    }

                    if !authed {
                        continue;
                    }

                    let tick_started = std::time::Instant::now();

                    let state: tauri::State<'_, state::AppStateStore> = pr_handle.state();
                    let workspaces: Vec<(String, String)> = {
                        let snapshot = state.snapshot();
                        snapshot
                            .workspaces
                            .iter()
                            .map(|w| {
                                let path = w
                                    .worktree_path
                                    .clone()
                                    .unwrap_or_else(|| w.cwd.clone());
                                (w.workspace_id.0.clone(), path)
                            })
                            .collect()
                    };

                    // `refreshed` counts workspaces this tick wrote to (the
                    // log's unit of work); `changed` is the narrower
                    // question the emit turns on — whether any of those
                    // writes actually moved a pill.
                    let mut changed = false;
                    let mut refreshed = 0usize;
                    let mut skipped_non_github = 0usize;
                    let mut timed_out = 0usize;

                    for (workspace_id, cwd) in workspaces {
                        let path = std::path::PathBuf::from(&cwd);

                        // Skip non-GitHub repos using the local remote config,
                        // avoiding an unnecessary network call.
                        let path_for_repo = path.clone();
                        let is_gh = tokio::time::timeout(
                            std::time::Duration::from_secs(PER_CALL_TIMEOUT_SECS),
                            tokio::task::spawn_blocking(move || {
                                github::is_github_repo(&path_for_repo)
                            }),
                        )
                        .await;
                        let is_gh = match is_gh {
                            Ok(Ok(b)) => b,
                            Ok(Err(_)) => false,
                            Err(_) => {
                                timed_out += 1;
                                continue;
                            }
                        };
                        if !is_gh {
                            skipped_non_github += 1;
                            continue;
                        }

                        let path_for_pr = path.clone();
                        let pr_result = tokio::time::timeout(
                            std::time::Duration::from_secs(PER_CALL_TIMEOUT_SECS),
                            tokio::task::spawn_blocking(move || {
                                github::get_workspace_pr(&path_for_pr)
                            }),
                        )
                        .await;

                        match pr_result {
                            Ok(Ok(lookup)) => {
                                if let Err(e) = &lookup {
                                    eprintln!(
                                        "[codemux::pr-poll] get_workspace_pr failed for {workspace_id}: {e}"
                                    );
                                }
                                // Shared matrix: match -> write, successful
                                // empty -> authoritative clear, error (or
                                // detached HEAD) -> retain the last known
                                // value. Same helper the 5 s sweep uses, so
                                // the two pollers cannot disagree about what
                                // a given lookup means.
                                //
                                // `changed` is still threaded through every
                                // writing arm: this loop is change-gated and
                                // emits at most one snapshot per pass, so an
                                // update that does not set the flag is an
                                // update the renderer never hears about.
                                match github::branch_pr_outcome(lookup) {
                                    github::BranchPrOutcome::Write(pr) => {
                                        changed |= state.update_workspace_pr_info(
                                            &workspace_id,
                                            Some(pr.number),
                                            Some(pr.display_state()),
                                            Some(pr.url),
                                            pr.head_branch,
                                        );
                                        refreshed += 1;
                                    }
                                    github::BranchPrOutcome::Clear => {
                                        changed |= state.update_workspace_pr_info(
                                            &workspace_id,
                                            None,
                                            None,
                                            None,
                                            None,
                                        );
                                        refreshed += 1;
                                    }
                                    github::BranchPrOutcome::Preserve => {}
                                }
                            }
                            Ok(Err(e)) => {
                                eprintln!(
                                    "[codemux::pr-poll] join error for {workspace_id}: {e}"
                                );
                            }
                            Err(_) => {
                                eprintln!(
                                    "[codemux::pr-poll] branch PR lookup timed out (>{PER_CALL_TIMEOUT_SECS}s) for {workspace_id}"
                                );
                                timed_out += 1;
                            }
                        }
                    }

                    if changed {
                        // Coalesced: PR poll runs on its own cadence; if
                        // it lands within 16 ms of the git poll the two
                        // emits collapse into one frontend render.
                        state::schedule_emit_app_state(&pr_handle);
                    }
                    // Log only ticks that did something or hit a timeout —
                    // the steady state is silent, so a line in the log means
                    // a real event instead of a heartbeat to scroll past.
                    if refreshed > 0 || timed_out > 0 {
                        eprintln!(
                            "[codemux::pr-poll] tick — refreshed={refreshed} skipped_non_github={skipped_non_github} timed_out={timed_out}"
                        );
                    }
                    let tick_ms = tick_started.elapsed().as_millis();
                    if tick_ms > 50 {
                        eprintln!("[codemux::perf::job] pr-poll tick={tick_ms}ms");
                    }
                }
            });

            // Periodically scan for listening TCP ports and associate with workspaces
            let port_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(jobs::startup_jitter(std::time::Duration::from_millis(1500)))
                    .await;
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;

                    // Off-screen gate. Detected ports are read only by sidebar
                    // surfaces (the ports popover, the workspace row pill), so
                    // a scan of every `/proc/*/fd/` entry while nobody can see
                    // the result buys nothing — the next tick repopulates
                    // within 3 s of the window coming back.
                    //
                    // "Nobody can see the result" is the load-bearing part, and
                    // the desktop window is not the only viewer. A paired
                    // web-remote browser renders the same sidebar from the same
                    // `detected_ports` domain, so gating on the desktop window
                    // alone froze every remote client's port list the moment
                    // the desktop was minimized — the exact situation where the
                    // remote page is the one being used. Any live socket
                    // therefore keeps the poll running. `try_state` because the
                    // server is optional: no web-remote state managed (or none
                    // ever started) reads as no remote viewers.
                    let window_active = port_handle
                        .get_webview_window("main")
                        .map(|w| {
                            let visible = w.is_visible().unwrap_or(false);
                            let minimized = w.is_minimized().unwrap_or(true);
                            visible && !minimized
                        })
                        .unwrap_or(false);
                    let remote_viewers = port_handle
                        .try_state::<web_remote::WebRemoteState>()
                        .map(|s| s.active_connection_count() > 0)
                        .unwrap_or(false);
                    if !window_active && !remote_viewers {
                        continue;
                    }

                    let tick_started = std::time::Instant::now();
                    let app_state: tauri::State<'_, state::AppStateStore> = port_handle.state();
                    let pty_state: tauri::State<'_, terminal::PtyState> = port_handle.state();

                    let session_pids = pty_state.get_session_pids();
                    let session_workspaces = app_state.all_session_workspaces();
                    let workspace_cwds = app_state.all_workspace_cwds();
                    let workspace_paths = app_state.all_workspace_paths();

                    let ports = ports::scan_ports(
                        &session_pids,
                        &session_workspaces,
                        &workspace_cwds,
                        &workspace_paths,
                    );
                    let port_snapshots: Vec<state::PortInfoSnapshot> = ports
                        .into_iter()
                        .map(|p| state::PortInfoSnapshot {
                            port: p.port,
                            pid: p.pid,
                            process_name: p.process_name,
                            workspace_id: p.workspace_id,
                            label: p.label,
                            source: p.source,
                        })
                        .collect();

                    // The detected-port list is its own domain: nothing else
                    // in the app state moves when it changes, so it ships as a
                    // delta rather than a full snapshot.
                    if let Some(delta) = app_state.update_detected_ports_delta(port_snapshots) {
                        state::emit_app_state_delta(&port_handle, delta);
                    }

                    let tick_ms = tick_started.elapsed().as_millis();
                    if tick_ms > 50 {
                        eprintln!("[codemux::perf::job] port-poll tick={tick_ms}ms");
                    }
                }
            });

            // Revision heartbeat — the resync safety net.
            //
            // Background loops are change-gated, so an idle app emits nothing
            // and the renderer's last snapshot could silently drift from the
            // backend if a mutation path ever mutates and forgets to emit, or
            // if an event is dropped in delivery. Once a minute we publish the
            // bare revision counter (no snapshot build, no serialise of state)
            // and let the renderer decide: it refetches only when it is
            // actually behind. Jittered so it doesn't line up with the 3 s /
            // 5 s / 60 s loops.
            let revision_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    tokio::time::sleep(
                        std::time::Duration::from_secs(60)
                            + jobs::startup_jitter(std::time::Duration::from_secs(10)),
                    )
                    .await;
                    state::emit_app_state_revision(&revision_handle);
                }
            });

            // Workspaces sync loop — mirror the local workspace list
            // into the `workspaces_sync` SQLite table, then push/pull
            // against /api/workspaces. Runs every 30 seconds so cross-
            // device visibility is eventually consistent within that
            // window without each individual workspace mutation having
            // to call out to the sync layer. The reconcile step
            // (snapshot → workspaces_sync) is cheap (a diff over a
            // small list); the push/pull only fires when there are
            // dirty rows OR the server has new rows to advertise.
            //
            // Failure mode: any reconcile or sync error is logged and
            // the loop continues. The dirty flag stays set so the
            // next tick re-tries.
            let sync_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                // Small initial delay so we don't fight startup IO.
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                loop {
                    let app_state: tauri::State<'_, state::AppStateStore> =
                        sync_handle.state();
                    let db: tauri::State<'_, database::DatabaseStore> =
                        sync_handle.state();
                    let snapshot = app_state.snapshot();
                    if let Err(error) = workspaces_sync::reconcile_from_snapshot(
                        db.inner(),
                        &snapshot,
                    ) {
                        eprintln!(
                            "[codemux::workspaces-sync] reconcile failed: {error}"
                        );
                    }
                    if let Err(error) =
                        workspaces_sync::try_sync_with_app(&sync_handle).await
                    {
                        eprintln!(
                            "[codemux::workspaces-sync] background sync failed: {error}"
                        );
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                }
            });

            // Window lifecycle breadcrumbs: this lets us tell whether a second process
            // actually reached window creation or if it exited early. GUI-only —
            // it registers an on_window_event listener on the main window, which
            // only exists in the desktop build.
            #[cfg(debug_assertions)]
            if mode == AppMode::Gui {
                let pid = std::process::id();
                let startup_id =
                    std::env::var("CODEMUX_STARTUP_ID").unwrap_or_else(|_| "<unset>".into());
                if let Some(window) = app.get_webview_window("main") {
                    crate::diagnostics::native_startup_breadcrumb(&format!(
                        "[{}] startup_id={} pid={} component=tauri event=main_window_available",
                        chrono::Local::now().format("%s"),
                        startup_id,
                        pid
                    ));
                    window.on_window_event(move |event: &tauri::WindowEvent| {
                        crate::diagnostics::native_startup_breadcrumb(&format!(
                            "[{}] startup_id={} pid={} component=window label=main event={:?}",
                            chrono::Local::now().format("%s"),
                            startup_id,
                            pid,
                            event
                        ));
                    });
                } else {
                    crate::diagnostics::native_startup_breadcrumb(&format!(
                        "[{}] startup_id={} pid={} component=tauri event=main_window_missing",
                        chrono::Local::now().format("%s"),
                        startup_id,
                        pid
                    ));
                }
            }

            // Dev/test-only: an automated harness can set
            // `CODEMUX_DEV_OFFLINE_LOGIN=1` (paired with an unreachable
            // `CODEMUX_API_URL`) to seed an offline dev login so the served
            // app is usable without a real account. No-op in release builds
            // and dormant without the env var.
            #[cfg(debug_assertions)]
            crate::auth::seed_dev_offline_login(&app.state::<crate::database::DatabaseStore>());

            // Restore web remote-access: if the user left it enabled, this
            // re-binds the server on boot (non-fatal if the port is taken).
            crate::web_remote::restore_on_boot(app.handle());

            // Dev/test-only: an automated end-to-end harness can set
            // `CODEMUX_WEB_REMOTE_E2E=1` to have the server come up and a
            // pairing URL written to disk on boot. No-op in release builds
            // (gated on `debug_assertions`) and dormant without the env var.
            #[cfg(debug_assertions)]
            crate::web_remote::e2e_autostart(app.handle());

            #[cfg(debug_assertions)]
            {
                let pid = std::process::id();
                let startup_id =
                    std::env::var("CODEMUX_STARTUP_ID").unwrap_or_else(|_| "<unset>".into());
                crate::diagnostics::native_startup_breadcrumb(&format!(
                    "[{}] startup_id={} pid={} component=tauri event=setup_exit",
                    chrono::Local::now().format("%s"),
                    startup_id,
                    pid
                ));
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_current_theme,
            commands::get_shell_appearance,
            commands::get_app_state,
            commands::create_workspace,
            commands::create_empty_workspace,
            commands::get_or_create_home_workspace,
            commands::regenerate_mcp_config,
            // MCP runtime registry commands. Frontend invokes these via
            // `src/tauri/commands.ts` and `src/hooks/use-mcp-runtime.ts`.
            // Defined in `commands/mcp.rs`; missing registration is what
            // produced the "Command get_mcp_runtime_status not found"
            // warnings.
            commands::list_mcp_servers,
            commands::get_mcp_runtime_status,
            commands::set_mcp_disabled_ids,
            commands::prime_mcp_runtime,
            commands::start_mcp_server_cmd,
            commands::stop_mcp_server_cmd,
            commands::restart_mcp_server_cmd,
            commands::list_mcp_tools,
            commands::list_mcp_tools_with_cap_info,
            commands::list_mcp_tools_for_server,
            commands::create_workspace_with_preset,
            commands::activate_workspace,
            commands::rename_workspace,
            commands::set_workspace_muted,
            commands::update_workspace_cwd,
            commands::close_workspace,
            commands::cycle_workspace,
            commands::detect_editors,
            commands::open_in_editor,
            commands::create_worktree_workspace,
            commands::import_worktree_workspace,
            commands::close_workspace_with_worktree,
            commands::archive_workspace,
            commands::unarchive_workspace,
            commands::delete_archived_workspace,
            commands::generate_branch_name,
            commands::generate_random_branch_name,
            commands::reorder_workspaces,
            commands::split_pane,
            commands::activate_pane,
            commands::cycle_pane,
            commands::close_pane,
            commands::swap_panes,
            commands::resize_split,
            commands::resize_active_pane,
            commands::notify_attention,
            commands::set_notification_sound_enabled,
            commands::set_ai_commit_message_enabled,
            commands::set_ai_commit_message_cli,
            commands::set_ai_commit_message_model,
            commands::set_ai_resolver_enabled,
            commands::set_ai_resolver_cli,
            commands::set_ai_resolver_model,
            commands::set_ai_resolver_strategy,
            commands::create_tab,
            commands::close_tab,
            commands::activate_tab,
            commands::rename_tab,
            commands::reorder_tabs,
            commands::git_pull_changes,
            commands::git_fetch_changes,
            commands::git_fetch_prune,
            commands::git_stash_push,
            commands::git_stash_pop,
            commands::git_amend_commit,
            commands::git_undo_last_commit,
            commands::get_commit_files,
            commands::git_discard_file,
            commands::git_log_entries,
            commands::refresh_workspace_git_info,
            commands::checkout_default_branch_in_workspace,
            commands::create_browser_pane,
            commands::browser_open_url,
            commands::browser_history_back,
            commands::browser_history_forward,
            commands::browser_reload,
            commands::browser_set_loading_state,
            commands::agent_browser_spawn,
            commands::agent_browser_run,
            commands::agent_browser_close,
            commands::agent_browser_screenshot,
            commands::start_browser_stream,
            commands::get_browser_data_size,
            commands::clear_browser_cookies,
            commands::clear_all_browser_data,
            commands::get_project_memory_snapshot,
            commands::update_project_memory_snapshot,
            commands::add_project_memory_entry,
            commands::generate_project_handoff,
            commands::rebuild_project_index,
            commands::get_project_index_status,
            commands::search_project_index,
            commands::list_project_files,
            commands::read_file_for_attachment,
            commands::list_project_folders,
            commands::read_folder_for_attachment,
            commands::get_observability_snapshot,
            commands::add_structured_log,
            commands::update_feature_flags,
            commands::get_feature_flags,
            commands::set_agent_chat_enabled,
            commands::quit_app,
            commands::get_home_dir,
            commands::agent_chat_create_pane,
            commands::agent_chat_close_pane,
            commands::dev_agent_chat_spawn_test_pane,
            commands::agent_chat_start_session,
            commands::agent_chat_send_turn,
            commands::agent_chat_stage_image,
            commands::agent_chat_discard_staged_image,
            commands::agent_chat_read_image,
            commands::agent_chat_read_local_image,
            commands::agent_chat_prime_mcp,
            commands::agent_chat_cancel_queued_turn,
            commands::agent_chat_send_queued_turn_now,
            commands::agent_chat_interrupt_turn,
            commands::agent_chat_stop_monitoring,
            commands::agent_chat_turn_active,
            commands::agent_chat_respond_to_request,
            commands::agent_chat_set_model,
            commands::agent_chat_set_permission_mode,
            commands::list_chat_provider_capabilities,
            commands::list_chat_slash_commands,
            commands::list_launch_gemini_models,
            commands::agent_chat_stop_session,
            commands::agent_chat_list_sessions,
            commands::agent_chat_get_session,
            commands::agent_chat_update_session_config,
            commands::agent_chat_rename_session,
            commands::agent_chat_delete_session,
            commands::agent_chat_list_messages,
            commands::agent_chat_list_messages_after,
            commands::agent_chat_thread_head_id,
            commands::agent_chat_get_tool_result,
            commands::agent_chat_get_checkpoint,
            commands::agent_chat_restore_checkpoint,
            commands::attach_agent_chat_output,
            commands::detach_agent_chat_output,
            commands::opencode_check_availability,
            commands::opencode_ping,
            commands::opencode_list_models,
            commands::list_tool_permissions,
            commands::remove_tool_permission,
            commands::list_skills,
            commands::start_skills_watcher,
            commands::stop_skills_watcher,
            commands::pick_folder_dialog,
            commands::pick_files_dialog,
            commands::pick_save_file_dialog,
            commands::pick_open_file_dialog,
            terminal::create_terminal_session,
            terminal::activate_terminal_session,
            terminal::close_terminal_session,
            terminal::restart_terminal_session,
            terminal::get_terminal_status,
            terminal::attach_pty_output,
            terminal::detach_pty_output,
            terminal::pause_pty_output,
            terminal::resume_pty_output,
            terminal::terminal_session_cwds,
            terminal::write_to_pty,
            terminal::resize_pty,
            terminal::clear_agent_status,
            commands::db_get_setting,
            commands::db_set_setting,
            commands::db_delete_setting,
            commands::db_get_all_settings,
            commands::db_get_ui_state,
            commands::db_set_ui_state,
            commands::db_add_recent_project,
            commands::db_get_recent_projects,
            commands::get_project_scripts,
            commands::set_project_scripts,
            commands::check_is_git_repo,
            commands::init_git_repo,
            commands::init_git_repo_no_commit,
            commands::create_empty_repo,
            commands::get_git_status,
            commands::get_git_diff,
            commands::get_git_diff_stat,
            commands::get_base_branch_diff,
            commands::get_base_branch_file_diff,
            commands::get_default_branch,
            commands::git_stage_files,
            commands::git_unstage_files,
            commands::git_commit_changes,
            commands::git_push_changes,
            commands::get_git_branch_info,
            commands::list_branches,
            commands::list_branches_detailed,
            commands::create_worktree,
            commands::remove_worktree,
            commands::list_worktrees,
            commands::merge_branch,
            commands::merge_into_base,
            commands::complete_merge_into_base,
            commands::abort_merge_into_base,
            commands::get_merge_state,
            commands::check_merge_conflicts,
            commands::resolve_conflict_ours,
            commands::resolve_conflict_theirs,
            commands::mark_conflict_resolved,
            commands::abort_merge,
            commands::continue_merge,
            commands::create_resolver_branch,
            commands::apply_resolution,
            commands::abort_resolution,
            commands::get_resolution_diff,
            commands::resolve_conflicts_with_agent,
            commands::git_clone_repo,
            commands::check_claude_available,
            commands::generate_ai_commit_message,
            commands::check_gh_available,
            commands::check_gh_status,
            commands::check_github_repo,
            commands::get_branch_pull_request,
            commands::create_pull_request,
            commands::list_pull_requests,
            commands::get_github_pr_by_path,
            commands::get_github_pr_diff_by_path,
            commands::list_incoming_prs,
            commands::merge_pull_request,
            commands::get_pull_request_checks,
            commands::get_pr_review_comments,
            commands::get_pr_inline_comments,
            commands::submit_pr_review,
            commands::get_pr_deployments,
            commands::list_github_issues,
            commands::list_github_issues_by_path,
            commands::get_github_issue_by_path,
            commands::get_github_issue,
            commands::link_workspace_issue,
            commands::unlink_workspace_issue,
            commands::refresh_workspace_issue,
            commands::refresh_workspace_pr,
            commands::suggest_issue_branch_name,
            commands::detect_package_manager,
            commands::get_detected_ports,
            commands::kill_port,
            commands::get_presets,
            commands::create_preset,
            commands::update_preset,
            commands::delete_preset,
            commands::set_preset_pinned,
            commands::set_preset_bar_visible,
            commands::reorder_presets,
            commands::apply_preset,
            commands::get_workspace_config,
            commands::has_codemuxinclude,
            commands::run_workspace_setup,
            commands::run_project_dev_command,
            commands::list_directory,
            commands::search_in_files,
            commands::search_file_names,
            commands::read_file,
            commands::write_file,
            commands::save_clipboard_image_bytes,
            commands::paste_clipboard_image_to_file,
            commands::paste_clipboard_image,
            commands::grep_count_pattern,
            commands::reveal_in_file_manager,
            commands::start_oauth_flow,
            commands::signin_email,
            commands::signup_email,
            commands::forgot_password,
            commands::check_auth,
            commands::sign_out,
            commands::get_auth_token,
            commands::get_sync_status,
            commands::skills_sync_now,
            commands::skills_sync_status,
            commands::get_export_recommended_filename,
            commands::export_skills_to_file,
            commands::import_skills_from_file,
            commands::get_synced_settings,
            commands::update_synced_settings,
            commands::update_setting,
            commands::reset_synced_settings,
            commands::hosts_list,
            commands::hosts_add,
            commands::hosts_update,
            commands::hosts_delete,
            commands::hosts_test_connection,
            commands::hosts_bootstrap_install,
            commands::hosts_reinstall_remote,
            commands::set_workspace_host,
            commands::workspace_push_to_host,
            commands::workspace_pull_back,
            commands::automations_list,
            commands::automations_get,
            commands::automations_create,
            commands::automations_update,
            commands::automations_set_enabled,
            commands::automations_delete,
            commands::automations_runs,
            commands::automations_check_repo_access,
            commands::workspaces_sync_list,
            commands::workspaces_sync_now,
            commands::workspaces_adoption_preview,
            commands::workspaces_adopt_synced,
            commands::workspaces_adopt_via_clone,
            commands::workspaces_adopt_project,
            commands::workspaces_reconcile_copy,
            commands::workspace_open_on_host,
            commands::get_package_format,
            resource_metrics::get_resource_metrics,
            commands::debug_log,
            commands::clear_adapter_captures,
            scrollback::save_terminal_scrollback,
            scrollback::get_terminal_scrollback,
            scrollback::cache_terminal_scrollback,
            scrollback::uncache_terminal_scrollback,
            scrollback::flush_scrollback_cache,
            session_adapters::validate_resume,
            session_adapters::get_adapter_info,
            session_adapters::get_scanner_captures,
            // Web remote access (docs/plans/web-remote-access.md).
            web_remote::web_remote_status,
            web_remote::web_remote_enable,
            web_remote::web_remote_disable,
            web_remote::web_remote_set_config,
            web_remote::web_remote_create_pairing,
            web_remote::web_remote_list_endpoints,
            web_remote::web_remote_list_sessions,
            web_remote::web_remote_revoke_session,
            web_remote::web_remote_approve_session,
            web_remote::web_remote_reject_session,
            web_remote::web_remote_publish_update_available,
            web_remote::web_remote_request_update,
            web_remote::web_remote_iroh_node_id,
            web_remote::web_remote_registration_status,
            // WebKitGTK smooth-scrolling toggle (Linux; no-op elsewhere).
            webview_tuning::set_smooth_scrolling,
            // Which renderer this process ended up on, so the UI can drop
            // composited-only effects when running CPU-rendered.
            webview_tuning::get_renderer_mode,
        ])
}

/// Boot the full Codemux command surface headless on `MockRuntime` with no
/// display attached — the runtime backing the `codemux serve` daemon. Uses
/// the same `build_core_app` wiring as the GUI `run()` path, so every one of
/// the ~350 commands dispatches identically; only the display-coupled pieces
/// (native dialog/updater/clipboard plugins, window restore, close handling,
/// Hyprland float) are skipped via `AppMode::ServeHeadless`.
///
/// `Builder::<MockRuntime>::new()` is used rather than
/// `tauri::test::mock_builder()` so production invoke semantics are kept: the
/// app's own random invoke key is generated, not the test harness's fixed
/// `INVOKE_KEY`. (`Builder::default()` is only implemented for `Builder<Wry>`
/// when the `wry` feature is on — its generic `Default` impl is
/// `#[cfg(not(feature = "wry"))]` — and `default()` merely delegates to
/// `new()`, so `new()` is the identical, feature-independent constructor.)
/// `web_remote::dispatch::dispatch_invoke` reads
/// `app.invoke_key()` off this same app, so the WS dispatch path stays
/// consistent.
///
/// After build we run the `setup` hook (see below), which also creates the
/// in-memory "main" webview so `get_webview_window("main")` resolves — the
/// web-remote dispatcher, the notification bridge, and the background polling
/// loops all look it up. `MockRuntime::create_window` is a pure in-memory stub,
/// so this succeeds with no windowing system present.
///
/// ## Why `run_iteration` is called here
///
/// `Builder::build()` does **not** run the `.setup(…)` closure. Tauri defers
/// `setup` to the first `RuntimeRunEvent::Ready` its event loop delivers, i.e.
/// it runs from `App::run` / `App::run_return` / `App::run_iteration` — never
/// from `build()`. `codemux serve` deliberately never calls `app.run()` (it is
/// a foreground process blocking on a signal, and `MockRuntime::run` is an
/// infinite polling loop with no display to drive), so for as long as this
/// function only called `build()`, **none** of the setup side effects ran:
/// no control server (so `codemux remote pair`, `connect status`, and the
/// GUI/serve mutual-exclusion guard all failed against a live `serve`), no
/// `web_remote::restore_on_boot` (so the persisted port/scope/relay config was
/// never hydrated — and the un-hydrated in-memory default was then persisted
/// back over the user's settings row), no layout restore, no PTY/hook wiring,
/// no background loops. The command surface still answered because managed
/// state is registered at `Builder` time, which is why the gap stayed hidden.
///
/// `App::run_iteration` is the one public entry point that runs `setup`
/// without taking ownership of the app and blocking forever: it calls
/// `setup(self)` when it has not run yet, then delegates a single iteration to
/// the runtime — and `MockRuntime::run_iteration` is a no-op, so this returns
/// immediately. It is deprecated only for its "call me in a loop" usage (which
/// busy-loops); a single call to run setup on a runtime we own is exactly what
/// it still does correctly.
pub fn build_headless_app() -> tauri::Result<tauri::App<tauri::test::MockRuntime>> {
    let mut app = build_core_app(
        tauri::Builder::<tauri::test::MockRuntime>::new(),
        AppMode::ServeHeadless,
    )
    .build(app_context())?;

    #[allow(deprecated)]
    app.run_iteration(|_app, _event| {});

    // Tauri's own `setup` creates every window declared in `tauri.conf.json`
    // (that is where "main" comes from now). Keep a fallback so a config that
    // stops declaring it — or declares it with `create: false` — can't silently
    // leave the WS dispatcher without a webview to drive.
    if app.get_webview_window("main").is_none() {
        tauri::WebviewWindowBuilder::new(&app, "main", tauri::WebviewUrl::default()).build()?;
    }

    Ok(app)
}
