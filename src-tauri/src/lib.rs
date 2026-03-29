use tauri::Manager;

pub mod agent_context;
pub mod ai;
pub mod auth;
pub mod branch_name;
pub mod mcp_server;
pub mod agent_browser;
pub mod browser;
pub mod cli;
pub mod commands;
pub mod database;
pub mod config;
pub mod git;
pub mod github;
pub mod control;
pub mod diagnostics;
pub mod execution;
pub mod indexing;
pub mod memory;
pub mod openflow;
pub mod observability;
pub mod os_input;
pub mod ports;
pub mod presets;
pub mod project;
pub mod scripts;
pub mod settings_sync;
pub mod state;
pub mod hooks;
pub mod stream_input;
pub mod terminal;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
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
        .manage(browser::BrowserManager::new())
        .manage(agent_browser::AgentBrowserManager::new())
        .manage(indexing::ProjectIndexStore::default())
        .manage(openflow::OpenFlowRuntimeStore::default())
        .manage(openflow::AgentSessionStore::default())
        .manage(observability::load_observability_store())
        .manage(terminal::PtyState::default())
        .manage(auth::AuthState::default())
        .manage(database::init_database().unwrap_or_else(|e| {
            eprintln!("[codemux] WARNING: Database init failed: {e}. Using in-memory fallback.");
            database::DatabaseStore::new_in_memory()
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
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

            // Initialize presets from SQLite (must happen after database is managed)
            {
                let db: tauri::State<'_, database::DatabaseStore> = app.handle().state();
                app.manage(presets::PresetStoreState::new(&db));
            }

            let handle = app.handle().clone();
            if let Some(snapshot) = state::load_persisted_state() {
                state::restore_session_ids(&snapshot);
                let stripped = state::strip_openflow_from_snapshot(snapshot);
                let state: tauri::State<'_, state::AppStateStore> = handle.state();
                state.replace_snapshot(stripped);
                state.migrate_tabs_if_needed();
                state.migrate_project_roots();
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

            // Restore window size from SQLite
            {
                let db: tauri::State<'_, database::DatabaseStore> = handle.state();
                if let (Some(w), Some(h)) = (db.get_ui_state("window_width"), db.get_ui_state("window_height")) {
                    if let (Ok(w), Ok(h)) = (w.parse::<f64>(), h.parse::<f64>()) {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.set_size(tauri::LogicalSize::new(w, h));
                        }
                    }
                }
                if let (Some(x), Some(y)) = (db.get_ui_state("window_x"), db.get_ui_state("window_y")) {
                    if let (Ok(x), Ok(y)) = (x.parse::<i32>(), y.parse::<i32>()) {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.set_position(tauri::LogicalPosition::new(x, y));
                        }
                    }
                }
            }

            // Save window state on close
            {
                let close_handle = handle.clone();
                if let Some(window) = app.get_webview_window("main") {
                    window.on_window_event(move |event| {
                        if matches!(event, tauri::WindowEvent::CloseRequested { .. }) {
                            let db: tauri::State<'_, database::DatabaseStore> = close_handle.state();
                            if let Ok(size) = close_handle.get_webview_window("main").and_then(|w| w.outer_size().ok()).ok_or(()) {
                                db.set_ui_state("window_width", &size.width.to_string()).ok();
                                db.set_ui_state("window_height", &size.height.to_string()).ok();
                            }
                            if let Ok(pos) = close_handle.get_webview_window("main").and_then(|w| w.outer_position().ok()).ok_or(()) {
                                db.set_ui_state("window_x", &pos.x.to_string()).ok();
                                db.set_ui_state("window_y", &pos.y.to_string()).ok();
                            }
                            // Flush any pending debounced state write before exit
                            let app_state: tauri::State<'_, state::AppStateStore> = close_handle.state();
                            state::flush_persisted_state(&app_state);
                        }
                    });
                }
            }

            // On Hyprland, xdg-desktop-portal-gtk file dialogs and file managers
            // tile by default. Inject window rules to float them.
            for rule in [
                "float on, center on, size 800 600, match:class ^(xdg-desktop-portal-gtk)$",
                "float on, center on, size 900 600, match:class ^(org.gnome.Nautilus|thunar|nemo|org.kde.dolphin|pcmanfm)$",
            ] {
                let _ = std::process::Command::new("hyprctl")
                    .args(["keyword", "windowrule", rule])
                    .output();
            }

            let observability: tauri::State<'_, observability::ObservabilityStore> = handle.state();
            observability.increment_metric("startup_count");
            observability.log("app", observability::LogLevel::Info, "Codemux startup".into(), vec![]);
            config::watch_theme_file(handle.clone());
            terminal::spawn_missing_ptys(handle);
            let index_store: tauri::State<'_, indexing::ProjectIndexStore> = app.handle().state();
            indexing::spawn_index_watcher(index_store);
            control::spawn_control_server(app.handle().clone());
            hooks::start_hook_server(app.handle().clone());
            hooks::register_claude_code_hooks();

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

                        // Only fetch PR info if gh CLI is available
                        if github::gh_available() {
                            let pr_info = github::get_branch_pr(&path).ok().flatten();
                            let pr_number = pr_info.as_ref().map(|p| p.number);
                            let pr_state = pr_info.as_ref().map(|p| p.state.clone());
                            let pr_url = pr_info.as_ref().map(|p| p.url.clone());
                            state.update_workspace_pr_info(&workspace_id, pr_number, pr_state, pr_url);
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
            commands::get_current_theme,
            commands::get_shell_appearance,
            commands::get_app_state,
            commands::create_workspace,
            commands::create_empty_workspace,
            commands::regenerate_mcp_config,
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
            commands::mark_workspace_notifications_read,
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
            commands::git_stash_push,
            commands::git_stash_pop,
            commands::git_discard_file,
            commands::git_log_entries,
            commands::refresh_workspace_git_info,
            commands::create_browser_pane,
            commands::browser_open_url,
            commands::browser_history_back,
            commands::browser_history_forward,
            commands::browser_reload,
            commands::browser_set_loading_state,
            commands::browser_spawn,
            commands::browser_navigate,
            commands::browser_screenshot,
            commands::browser_click,
            commands::browser_type,
            commands::browser_close,
            commands::browser_resize_viewport,
            commands::agent_browser_spawn,
            commands::agent_browser_run,
            commands::agent_browser_close,
            commands::agent_browser_get_stream_url,
            commands::agent_browser_screenshot,
            commands::start_browser_stream,
            commands::get_project_memory_snapshot,
            commands::update_project_memory_snapshot,
            commands::add_project_memory_entry,
            commands::generate_project_handoff,
            commands::rebuild_project_index,
            commands::get_project_index_status,
            commands::search_project_index,
            commands::get_openflow_design_spec,
            commands::get_openflow_runtime_snapshot,
            commands::create_openflow_run,
            commands::retry_openflow_run,
            commands::apply_openflow_review_result,
            commands::stop_openflow_run,
            commands::get_observability_snapshot,
            commands::add_structured_log,
            commands::update_feature_flags,
            commands::update_permission_policy,
            commands::update_safety_config,
            commands::add_replay_record,
            commands::pick_folder_dialog,
            commands::pick_files_dialog,
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
            commands::merge_pull_request,
            commands::get_pull_request_checks,
            commands::get_pr_review_comments,
            commands::get_pr_inline_comments,
            commands::submit_pr_review,
            commands::get_pr_deployments,
            commands::detect_package_manager,
            commands::get_detected_ports,
            commands::kill_port,
            commands::get_presets,
            commands::create_preset,
            commands::update_preset,
            commands::delete_preset,
            commands::set_preset_pinned,
            commands::set_preset_bar_visible,
            commands::apply_preset,
            commands::get_workspace_config,
            commands::run_workspace_setup,
            commands::run_project_dev_command,
            commands::list_directory,
            commands::search_in_files,
            commands::search_file_names,
            commands::reveal_in_file_manager,
            commands::start_oauth_flow,
            commands::signin_email,
            commands::signup_email,
            commands::forgot_password,
            commands::check_auth,
            commands::sign_out,
            commands::get_auth_token,
            commands::get_synced_settings,
            commands::update_synced_settings,
            commands::update_setting,
            commands::reset_synced_settings,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
