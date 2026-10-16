//! Tauri commands for plugins. Desktop-only: the web remote denies every
//! `plugins_*` command before dispatch (web_remote/dispatch.rs).

use super::{
    registry::{Registry, SafeMode, SafeModeStatus, REGISTRY_FILE},
    watch, DiskPluginEntry,
};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{Mutex, OnceLock},
};
use tauri::{AppHandle, Emitter, Runtime, State};

#[derive(Default)]
pub struct PluginsState {
    watcher: Mutex<Option<notify::RecommendedWatcher>>,
    /// Opened on first use; an open error is kept and puts the launch in
    /// safe mode instead of failing every call.
    registry: OnceLock<Result<Registry, String>>,
    safe_mode: OnceLock<SafeMode>,
}

impl PluginsState {
    fn registry(&self) -> &Result<Registry, String> {
        self.registry.get_or_init(|| {
            let dir = super::app_data_dir().ok_or("App data directory is unavailable")?;
            Registry::open(&dir.join(REGISTRY_FILE))
        })
    }

    fn safe_mode(&self) -> &SafeMode {
        self.safe_mode.get_or_init(|| {
            let error = self.registry().as_ref().err().cloned();
            let dir = super::app_data_dir().unwrap_or_default();
            SafeMode::detect(&dir, error.as_deref())
        })
    }
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

#[tauri::command]
pub fn plugins_get_decisions<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, PluginsState>,
) -> Result<BTreeMap<String, bool>, String> {
    gui(&app)?;
    match state.registry() {
        Ok(registry) => registry.decisions(),
        // Safe mode reports the problem; an empty map lets bundled plugins
        // fall back to their defaults.
        Err(_) => Ok(BTreeMap::new()),
    }
}

/// Written only by Settings → Plugins. The SDK has no verb for this, so one
/// plugin can't switch another.
#[tauri::command]
pub fn plugins_set_enabled<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, PluginsState>,
    id: String,
    on: bool,
) -> Result<(), String> {
    gui(&app)?;
    state.registry().as_ref().map_err(|e| e.clone())?.set_enabled(&id, on)
}

#[tauri::command]
pub fn plugins_safe_mode_status<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, PluginsState>,
) -> Result<SafeModeStatus, String> {
    gui(&app)?;
    Ok(state.safe_mode().status())
}

/// `phase`: `"loading"` before activation, `"ok"` once the app has stayed up.
#[tauri::command]
pub fn plugins_startup_checkpoint<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, PluginsState>,
    phase: String,
) -> Result<(), String> {
    gui(&app)?;
    let safe = state.safe_mode();
    match phase.as_str() {
        "loading" => safe.begin_loading(),
        "ok" => safe.startup_ok(),
        other => Err(format!("unknown checkpoint {other}")),
    }
}

#[tauri::command]
pub fn plugins_resume<R: Runtime>(app: AppHandle<R>, state: State<'_, PluginsState>) -> Result<(), String> {
    gui(&app)?;
    state.safe_mode().resume();
    Ok(())
}

/// Turn the chat event tap on (first `host.chat.onEvent` subscriber) or off
/// (last one gone). See `plugins/tap.rs`.
#[tauri::command]
pub fn plugins_set_event_tap<R: Runtime>(app: AppHandle<R>, on: bool) -> Result<(), String> {
    gui(&app)?;
    super::tap::set_enabled(on);
    Ok(())
}
