use tauri::Manager;

pub mod agent_browser;
pub mod browser;
pub mod cli;
pub mod commands;
pub mod config;
pub mod control;
pub mod diagnostics;
pub mod execution;
pub mod indexing;
pub mod memory;
pub mod openflow;
pub mod observability;
pub mod project;
pub mod state;
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
        .manage(commands::BrowserAutomationCoordinator::default())
        .manage(browser::BrowserManager::new())
        .manage(agent_browser::AgentBrowserManager::new())
        .manage(indexing::ProjectIndexStore::default())
        .manage(openflow::OpenFlowRuntimeStore::default())
        .manage(openflow::AgentSessionStore::default())
        .manage(observability::load_observability_store())
        .manage(terminal::PtyState::default())
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

            let handle = app.handle().clone();
            if let Some(snapshot) = state::load_persisted_state() {
                state::restore_session_ids(&snapshot);
                let stripped = state::strip_openflow_from_snapshot(snapshot);
                let state: tauri::State<'_, state::AppStateStore> = handle.state();
                state.replace_snapshot(stripped);
            }
            let observability: tauri::State<'_, observability::ObservabilityStore> = handle.state();
            observability.increment_metric("startup_count");
            observability.log("app", observability::LogLevel::Info, "Codemux startup".into(), vec![]);
            config::watch_theme_file(handle.clone());
            terminal::spawn_missing_ptys(handle);
            let index_store: tauri::State<'_, indexing::ProjectIndexStore> = app.handle().state();
            indexing::spawn_index_watcher(index_store);
            control::spawn_control_server(app.handle().clone());

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
            commands::create_workspace_with_preset,
            commands::create_openflow_workspace,
            commands::activate_workspace,
            commands::rename_workspace,
            commands::update_workspace_cwd,
            commands::close_workspace,
            commands::cycle_workspace,
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
            commands::create_browser_pane,
            commands::browser_open_url,
            commands::browser_history_back,
            commands::browser_history_forward,
            commands::browser_reload,
            commands::browser_set_loading_state,
            commands::browser_capture_screenshot,
            commands::browser_automation_run,
            commands::browser_automation_complete,
            commands::browser_proxy_fetch,
            commands::browser_proxy_screenshot,
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
            commands::advance_openflow_run_phase,
            commands::retry_openflow_run,
            commands::run_openflow_autonomous_loop,
            commands::apply_openflow_review_result,
            commands::stop_openflow_run,
            commands::get_observability_snapshot,
            commands::add_structured_log,
            commands::update_feature_flags,
            commands::update_permission_policy,
            commands::update_safety_config,
            commands::add_replay_record,
            commands::pick_folder_dialog,
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
            terminal::resize_pty
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
