use tauri::{Emitter, Listener, Manager};

pub mod agent_context;
pub mod agent_provider;
pub mod ai;
pub mod auth;
pub mod branch_name;
pub mod json_rpc_child;
pub mod mcp_server;
pub mod agent_browser;
pub mod cli;
pub mod commands;
pub mod database;
pub mod config;
pub mod git;
pub mod github;
pub mod github_cache;
pub mod control;
pub mod diagnostics;
pub mod encryption;
pub mod execution;
pub mod indexing;
pub mod mcp;
pub mod memory;
pub mod openflow;
pub mod observability;
pub mod os_input;
pub mod ports;
pub mod presets;
pub mod project;
pub mod scripts;
pub mod scrollback;
pub mod skills;
pub mod skills_sync;
pub mod session_adapters;
pub mod settings_sync;
pub mod state;
pub mod hooks;
pub mod stream_input;
pub mod terminal;

/// Save window dimensions and position to SQLite before close.
fn save_window_state(handle: &tauri::AppHandle) {
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
    // WEBKIT_DISABLE_DMABUF_RENDERER: fixes "Could not create GBM EGL display" crash
    // on certain GPU/driver combos.
    // WEBKIT_DISABLE_COMPOSITING_MODE: fixes "Error 71 (Protocol error) dispatching
    // to Wayland display" crash on dual-GPU systems (e.g. NVIDIA + AMD iGPU).
    // GDK_BACKEND: forces Wayland-native when available, preventing XWayland fallback
    // (which triggers unintended Hyprland window rules on AppImage builds).
    #[cfg(target_os = "linux")]
    {
        if std::env::var("WEBKIT_DISABLE_DMABUF_RENDERER").is_err() {
            unsafe { std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1") };
        }
        if std::env::var("WEBKIT_DISABLE_COMPOSITING_MODE").is_err() {
            unsafe { std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1") };
        }
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

    tauri::Builder::default()
        // This plugin should run before the rest of the app setup so duplicate
        // launches are intercepted before a second GUI is created.
        .plugin(tauri_plugin_single_instance::init(|app, args, cwd| {
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
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .manage(state::AppStateStore::default())
        .manage(agent_browser::AgentBrowserManager::new_with_cleanup())
        .manage(indexing::ProjectIndexStore::default())
        .manage(openflow::OpenFlowRuntimeStore::default())
        .manage(openflow::AgentSessionStore::default())
        .manage(observability::load_observability_store())
        .manage(terminal::PtyState::default())
        .manage(session_adapters::AdapterState::new())
        .manage(scrollback::ScrollbackCache::default())
        .manage(auth::AuthState::default())
        .manage(encryption::EncryptionManager::default())
        .manage(skills_sync::SyncEngine::new())
        .manage(commands::agent_chat::ProviderRegistry::new())
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
        // MCP runtime registry. `agent_chat_start_session` reads this
        // via `app.state::<McpRegistry>()` to lazily prime servers
        // before launching a chat, and the `commands::mcp::*` Tauri
        // commands take it via `State<'_, McpRegistry>`. Registering
        // it here is mandatory — without `.manage()` the first
        // `state()` call panics with "state() called before manage()".
        .manage(mcp::registry::McpRegistry::default())
        .manage(skills::watcher::SkillsWatcherState::new())
        // Phase 2 display-isolation: per-workspace virtual display manager.
        // `new()` performs an orphan sweep of stale `/tmp/.X*-lock` files from
        // prior Codemux crashes. Actual Xvfb spawning is lazy — first agent
        // that opts in via CODEMUX_VIRTUAL_DISPLAY=1 triggers acquire().
        .manage(execution::virtual_display::VirtualDisplayManager::new())
        .manage(database::init_database().unwrap_or_else(|e| {
            eprintln!("[codemux] WARNING: Database init failed: {e}. Using in-memory fallback.");
            database::DatabaseStore::new_in_memory()
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
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
                state::restore_session_ids(&snapshot);
                let stripped = state::strip_openflow_from_snapshot(snapshot);
                let state: tauri::State<'_, state::AppStateStore> = handle.state();
                state.replace_snapshot(stripped);
                state.migrate_tabs_if_needed();
                state.migrate_project_roots();
                layout_loaded = true;
            } else {
                // First launch — no persisted layout exists. Replace the
                // default_app_state (which creates a CWD workspace) with an
                // empty state so the user sees the splash screen instead.
                let state: tauri::State<'_, state::AppStateStore> = handle.state();
                state.clear_workspaces();
            }

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

            let observability: tauri::State<'_, observability::ObservabilityStore> = handle.state();
            observability.increment_metric("startup_count");
            observability.log("app", observability::LogLevel::Info, "Codemux startup".into(), vec![]);
            config::watch_theme_file(handle.clone());

            // Hook server must be running BEFORE PTYs spawn so that restored
            // sessions get CODEMUX_HOOK_PORT in their environment.
            hooks::start_hook_server(app.handle().clone());
            hooks::register_claude_code_hooks();

            terminal::spawn_missing_ptys(handle);

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
                    });
                }
            }

            // Periodically refresh git info for the active workspace
            let git_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    let state: tauri::State<'_, state::AppStateStore> = git_handle.state();
                    if let Some((workspace_id, cwd)) = state.active_workspace_cwd() {
                        let path = std::path::PathBuf::from(&cwd);
                        let branch_info = git::git_branch_info(&path).ok();
                        let diff_stat = git::git_diff_stat(&path).ok();
                        let changed_files = git::git_status(&path).map(|f| f.len() as u32).unwrap_or(0);
                        let branch = branch_info.as_ref().and_then(|i| i.branch.clone());
                        let ahead = branch_info.as_ref().map(|i| i.ahead).unwrap_or(0);
                        let behind = branch_info.as_ref().map(|i| i.behind).unwrap_or(0);
                        let additions = diff_stat.as_ref().map(|s| s.staged_additions + s.unstaged_additions).unwrap_or(0);
                        let deletions = diff_stat.as_ref().map(|s| s.staged_deletions + s.unstaged_deletions).unwrap_or(0);
                        state.update_workspace_git_info(&workspace_id, branch, ahead, behind, additions, deletions, changed_files);

                        // Only fetch PR/issue info if gh CLI is available
                        if github::gh_available() {
                            let pr_info = match github::get_branch_pr(&path) {
                                Ok(info) => info,
                                Err(e) => {
                                    eprintln!("[codemux::github] Failed to fetch PR info for {}: {e}", cwd);
                                    None
                                }
                            };
                            // Only update PR info if we found a PR, or if none was
                            // previously set.  This prevents the background refresh from
                            // wiping a pr_number that was explicitly stored during
                            // PR-checkout (fork branches where `gh pr view` can't resolve).
                            if pr_info.is_some() {
                                let pr_number = pr_info.as_ref().map(|p| p.number);
                                let pr_state = pr_info.as_ref().map(|p| p.state.clone());
                                let pr_url = pr_info.as_ref().map(|p| p.url.clone());
                                state.update_workspace_pr_info(&workspace_id, pr_number, pr_state, pr_url);
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
                                    state.link_workspace_issue(&workspace_id, github::LinkedIssue {
                                        number: issue.number,
                                        title: issue.title,
                                        state: issue.state,
                                        labels: issue.labels,
                                    });
                                }
                            }
                        }

                        // Single emit after both git and PR info are updated
                        state::emit_app_state(&git_handle);
                    }
                }
            });

            // Periodically scan for listening TCP ports and associate with workspaces
            let port_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                    let app_state: tauri::State<'_, state::AppStateStore> = port_handle.state();
                    let pty_state: tauri::State<'_, terminal::PtyState> = port_handle.state();

                    let session_pids = pty_state.get_session_pids();
                    let session_workspaces = app_state.all_session_workspaces();
                    let workspace_cwds = app_state.all_workspace_cwds();

                    let ports = ports::scan_ports(&session_pids, &session_workspaces, &workspace_cwds);
                    let port_snapshots: Vec<state::PortInfoSnapshot> = ports
                        .into_iter()
                        .map(|p| state::PortInfoSnapshot {
                            port: p.port,
                            pid: p.pid,
                            process_name: p.process_name,
                            workspace_id: p.workspace_id,
                            label: p.label,
                        })
                        .collect();

                    if app_state.update_detected_ports(port_snapshots) {
                        state::emit_app_state(&port_handle);
                    }
                }
            });

            // Window lifecycle breadcrumbs: this lets us tell whether a second process
            // actually reached window creation or if it exited early.
            #[cfg(debug_assertions)]
            {
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
            commands::get_platform,
            commands::get_workspace_virtual_display,
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
            commands::create_openflow_workspace,
            commands::activate_workspace,
            commands::rename_workspace,
            commands::update_workspace_cwd,
            commands::close_workspace,
            commands::cycle_workspace,
            commands::detect_editors,
            commands::open_in_editor,
            commands::create_worktree_workspace,
            commands::import_worktree_workspace,
            commands::close_workspace_with_worktree,
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
            commands::get_commit_files,
            commands::git_discard_file,
            commands::git_log_entries,
            commands::refresh_workspace_git_info,
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
            commands::get_openflow_design_spec,
            commands::get_openflow_runtime_snapshot,
            commands::create_openflow_run,
            commands::retry_openflow_run,
            commands::apply_openflow_review_result,
            commands::stop_openflow_run,
            commands::get_observability_snapshot,
            commands::add_structured_log,
            commands::update_feature_flags,
            commands::get_feature_flags,
            commands::get_home_dir,
            commands::agent_chat_create_pane,
            commands::agent_chat_close_pane,
            commands::dev_agent_chat_spawn_test_pane,
            commands::agent_chat_start_session,
            commands::agent_chat_send_turn,
            commands::agent_chat_interrupt_turn,
            commands::agent_chat_respond_to_request,
            commands::agent_chat_set_model,
            commands::agent_chat_set_permission_mode,
            commands::list_chat_provider_capabilities,
            commands::agent_chat_stop_session,
            commands::agent_chat_list_sessions,
            commands::agent_chat_rename_session,
            commands::agent_chat_delete_session,
            commands::agent_chat_list_messages,
            commands::opencode_check_availability,
            commands::opencode_ping,
            commands::opencode_list_models,
            commands::update_permission_policy,
            commands::list_tool_permissions,
            commands::remove_tool_permission,
            commands::list_skills,
            commands::start_skills_watcher,
            commands::stop_skills_watcher,
            commands::update_safety_config,
            commands::add_replay_record,
            commands::pick_folder_dialog,
            commands::pick_files_dialog,
            commands::pick_save_file_dialog,
            commands::pick_open_file_dialog,
            commands::list_available_cli_tools,
            commands::list_models_for_tool,
            commands::list_thinking_modes_for_tool,
            commands::spawn_openflow_agents,
            commands::get_agent_sessions_for_run,
            commands::get_communication_log,
            commands::inject_orchestrator_message,
            commands::trigger_orchestrator_cycle,
            terminal::create_terminal_session,
            terminal::activate_terminal_session,
            terminal::close_terminal_session,
            terminal::restart_terminal_session,
            terminal::get_terminal_status,
            terminal::attach_pty_output,
            terminal::detach_pty_output,
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
            commands::db_save_openflow_run,
            commands::db_get_openflow_history,
            commands::get_project_scripts,
            commands::set_project_scripts,
            commands::check_is_git_repo,
            commands::init_git_repo,
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
            commands::setup_sync_password,
            commands::provide_password_for_sync,
            commands::skills_sync_now,
            commands::skills_sync_status,
            commands::get_export_recommended_filename,
            commands::export_skills_to_file,
            commands::import_skills_from_file,
            commands::wipe_remote_skills_for_reset,
            commands::get_synced_settings,
            commands::update_synced_settings,
            commands::update_setting,
            commands::reset_synced_settings,
            commands::get_package_format,
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
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, event| {
            if let tauri::RunEvent::Exit = event {
                // Kill agent-browser daemons so they don't persist across restarts.
                agent_browser::kill_stream_daemons();
            }
        });
}
