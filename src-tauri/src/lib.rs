use tauri::Manager;

pub mod cli;
pub mod commands;
pub mod config;
pub mod control;
pub mod indexing;
pub mod memory;
pub mod openflow;
pub mod observability;
pub mod project;
pub mod state;
pub mod terminal;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(state::AppStateStore::default())
        .manage(commands::BrowserAutomationCoordinator::default())
        .manage(indexing::ProjectIndexStore::default())
        .manage(openflow::OpenFlowRuntimeStore::default())
        .manage(observability::load_observability_store())
        .manage(terminal::PtyState::default())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let handle = app.handle().clone();
            if let Some(snapshot) = state::load_persisted_state() {
                state::restore_session_ids(&snapshot);
                let state: tauri::State<'_, state::AppStateStore> = handle.state();
                state.replace_snapshot(snapshot);
            }
            let observability: tauri::State<'_, observability::ObservabilityStore> = handle.state();
            observability.increment_metric("startup_count");
            observability.log("app", observability::LogLevel::Info, "Codemux startup".into(), vec![]);
            config::watch_theme_file(handle.clone());
            terminal::spawn_missing_ptys(handle);
            let index_store: tauri::State<'_, indexing::ProjectIndexStore> = app.handle().state();
            indexing::spawn_index_watcher(index_store);
            control::spawn_control_server(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_current_theme,
            commands::get_shell_appearance,
            commands::get_app_state,
            commands::create_workspace,
            commands::create_workspace_with_preset,
            commands::activate_workspace,
            commands::rename_workspace,
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
