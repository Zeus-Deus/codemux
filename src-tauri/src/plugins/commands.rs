//! Tauri commands for plugins. Desktop-only: the web remote denies every
//! `plugins_*` command before dispatch (web_remote/dispatch.rs).

use super::{watch, DiskPluginEntry};
use serde::Serialize;
use std::{path::PathBuf, sync::Mutex};
use tauri::{AppHandle, Emitter, Runtime, State};

#[derive(Default)]
pub struct PluginsState {
    watcher: Mutex<Option<notify::RecommendedWatcher>>,
}

fn gui<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    crate::ensure_gui_mode(app).map_err(|_| "Plugins are available only in the desktop app".to_string())
}

pub(crate) fn root() -> Result<PathBuf, String> {
    let root = super::default_root().ok_or("App data directory is unavailable")?;
    std::fs::create_dir_all(&root).map_err(|e| format!("Cannot create the plugins folder: {e}"))?;
    Ok(root)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginsRoot {
    pub path: String,
    /// `CODEMUX_DISABLE_PLUGINS=1`: the renderer loads nothing.
    pub disabled_by_env: bool,
}

#[tauri::command]
pub fn plugins_root<R: Runtime>(app: AppHandle<R>) -> Result<PluginsRoot, String> {
    gui(&app)?;
    Ok(PluginsRoot {
        path: root()?.to_string_lossy().to_string(),
        disabled_by_env: super::disabled_by_env(),
    })
}

#[tauri::command]
pub async fn plugins_list<R: Runtime>(app: AppHandle<R>) -> Result<Vec<DiskPluginEntry>, String> {
    gui(&app)?;
    let root = root()?;
    tauri::async_runtime::spawn_blocking(move || super::list(&root).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn plugins_read_source<R: Runtime>(app: AppHandle<R>, id: String) -> Result<String, String> {
    gui(&app)?;
    let root = root()?;
    tauri::async_runtime::spawn_blocking(move || super::read_source(&root, &id))
        .await
        .map_err(|e| e.to_string())?
}

/// Start the root watcher (idempotent). Emits `plugins_changed { id }`.
#[tauri::command]
pub fn plugins_watch<R: Runtime>(app: AppHandle<R>, state: State<'_, PluginsState>) -> Result<(), String> {
    gui(&app)?;
    let mut slot = state.watcher.lock().unwrap();
    if slot.is_some() {
        return Ok(());
    }
    let root = root()?;
    let handle = app.clone();
    let watcher = watch::start(root, move |changes| {
        for change in changes {
            let _ = handle.emit_to("main", watch::CHANGED_EVENT, change);
        }
    })
    .map_err(|e| format!("Cannot watch the plugins folder: {e}"))?;
    *slot = Some(watcher);
    Ok(())
}

/// Open the plugins folder in the system file manager.
#[tauri::command]
pub fn plugins_reveal_root<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    gui(&app)?;
    let root = root()?;
    tauri_plugin_opener::open_path(root.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| e.to_string())
}
