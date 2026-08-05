use crate::database::{DatabaseStore, ProjectScripts, RecentProject};
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
pub fn get_project_scripts(db: State<'_, DatabaseStore>, path: String) -> Option<ProjectScripts> {
    db.get_project_scripts(&path)
}

#[tauri::command]
pub fn set_project_scripts(
    db: State<'_, DatabaseStore>,
    path: String,
    scripts: ProjectScripts,
) -> Result<(), String> {
    db.set_project_scripts(&path, &scripts)
}
