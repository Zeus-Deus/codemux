use crate::database::{DatabaseStore, OpenFlowHistoryEntry, RecentProject};
use std::collections::HashMap;
use tauri::State;

#[tauri::command]
pub fn db_get_setting(db: State<'_, DatabaseStore>, key: String) -> Option<String> {
    db.get_setting(&key)
}

#[tauri::command]
pub fn db_set_setting(db: State<'_, DatabaseStore>, key: String, value: String) -> Result<(), String> {
    db.set_setting(&key, &value)
}

#[tauri::command]
pub fn db_delete_setting(db: State<'_, DatabaseStore>, key: String) -> Result<(), String> {
    db.delete_setting(&key)
}

#[tauri::command]
pub fn db_get_all_settings(db: State<'_, DatabaseStore>) -> HashMap<String, String> {
    db.get_all_settings()
}

#[tauri::command]
pub fn db_get_ui_state(db: State<'_, DatabaseStore>, key: String) -> Option<String> {
    db.get_ui_state(&key)
}

#[tauri::command]
pub fn db_set_ui_state(db: State<'_, DatabaseStore>, key: String, value: String) -> Result<(), String> {
    db.set_ui_state(&key, &value)
}

#[tauri::command]
pub fn db_add_recent_project(db: State<'_, DatabaseStore>, path: String, name: String) -> Result<(), String> {
    db.add_recent_project(&path, &name)
}

#[tauri::command]
pub fn db_get_recent_projects(db: State<'_, DatabaseStore>, limit: Option<u32>) -> Vec<RecentProject> {
    db.get_recent_projects(limit.unwrap_or(20))
}

#[tauri::command]
pub fn db_save_openflow_run(
    db: State<'_, DatabaseStore>,
    run_id: String,
    title: Option<String>,
    goal: Option<String>,
    status: Option<String>,
    agent_count: Option<i32>,
    started_at: Option<String>,
    completed_at: Option<String>,
) -> Result<(), String> {
    db.save_openflow_run(
        &run_id,
        title.as_deref(),
        goal.as_deref(),
        status.as_deref(),
        agent_count,
        started_at.as_deref(),
        completed_at.as_deref(),
    )
}

#[tauri::command]
pub fn db_get_openflow_history(db: State<'_, DatabaseStore>, limit: Option<u32>) -> Vec<OpenFlowHistoryEntry> {
    db.get_openflow_history(limit.unwrap_or(50))
}
