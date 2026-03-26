use serde::{Deserialize, Serialize};
use std::sync::Mutex;

use crate::database::DatabaseStore;

const PRESET_SCHEMA_VERSION: u32 = 1;
const PRESET_STORE_KEY: &str = "preset_store";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum LaunchMode {
    SplitPane,
    NewTab,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TerminalPreset {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub commands: Vec<String>,
    pub working_directory: Option<String>,
    pub launch_mode: LaunchMode,
    pub icon: Option<String>,
    pub pinned: bool,
    pub is_builtin: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresetStore {
    pub schema_version: u32,
    pub presets: Vec<TerminalPreset>,
    pub default_preset_id: Option<String>,
    pub bar_visible: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresetStoreSnapshot {
    pub presets: Vec<TerminalPreset>,
    pub bar_visible: bool,
    pub default_preset_id: Option<String>,
}

pub struct PresetStoreState {
    pub inner: Mutex<PresetStore>,
}

impl PresetStoreState {
    pub fn new(db: &DatabaseStore) -> Self {
        Self {
            inner: Mutex::new(load_presets(db)),
        }
    }
}

fn builtin_presets() -> Vec<TerminalPreset> {
    vec![
        TerminalPreset {
            id: "builtin-claude".into(),
            name: "Claude Code".into(),
            description: Some("Launch Claude Code agent".into()),
            commands: vec!["claude --dangerously-skip-permissions".into()],
            working_directory: None,
            launch_mode: LaunchMode::NewTab,
            icon: Some("claude".into()),
            pinned: true,
            is_builtin: true,
        },
        TerminalPreset {
            id: "builtin-codex".into(),
            name: "Codex".into(),
            description: Some("Launch OpenAI Codex agent".into()),
            commands: vec!["codex --full-auto".into()],
            working_directory: None,
            launch_mode: LaunchMode::NewTab,
            icon: Some("codex".into()),
            pinned: true,
            is_builtin: true,
        },
        TerminalPreset {
            id: "builtin-opencode".into(),
            name: "OpenCode".into(),
            description: Some("Launch OpenCode agent".into()),
            commands: vec!["opencode".into()],
            working_directory: None,
            launch_mode: LaunchMode::NewTab,
            icon: Some("opencode".into()),
            pinned: true,
            is_builtin: true,
        },
        TerminalPreset {
            id: "builtin-gemini".into(),
            name: "Gemini".into(),
            description: Some("Launch Gemini CLI agent".into()),
            commands: vec!["gemini --yolo".into()],
            working_directory: None,
            launch_mode: LaunchMode::NewTab,
            icon: Some("gemini".into()),
            pinned: true,
            is_builtin: true,
        },
        TerminalPreset {
            id: "builtin-shell".into(),
            name: "Shell".into(),
            description: Some("Open a new shell".into()),
            commands: vec![],
            working_directory: None,
            launch_mode: LaunchMode::NewTab,
            icon: Some("terminal".into()),
            pinned: false,
            is_builtin: true,
        },
    ]
}

/// Load presets from SQLite. On first run, migrates from legacy JSON file if it exists.
pub fn load_presets(db: &DatabaseStore) -> PresetStore {
    // Try SQLite first
    if let Some(json) = db.get_setting(PRESET_STORE_KEY) {
        if let Ok(mut store) = serde_json::from_str::<PresetStore>(&json) {
            sync_builtins(&mut store);
            return store;
        }
    }

    // Try migrating from legacy JSON file
    if let Some(store) = migrate_from_json_file() {
        // Persist to SQLite
        let _ = save_presets(db, &store);
        return store;
    }

    // Fresh install — use defaults
    let store = default_store();
    let _ = save_presets(db, &store);
    store
}

/// Save presets to SQLite settings table.
pub fn save_presets(db: &DatabaseStore, store: &PresetStore) -> Result<(), String> {
    let json = serde_json::to_string(store)
        .map_err(|e| format!("Failed to serialize presets: {e}"))?;
    db.set_setting(PRESET_STORE_KEY, &json)
}

/// Sync built-in presets: add missing ones, remove stale ones.
fn sync_builtins(store: &mut PresetStore) {
    let builtins = builtin_presets();
    let builtin_ids: Vec<&str> = builtins.iter().map(|b| b.id.as_str()).collect();

    // Remove stale builtins whose IDs no longer exist
    store.presets.retain(|p| {
        !p.id.starts_with("builtin-") || builtin_ids.contains(&p.id.as_str())
    });

    // Add any missing builtins
    for builtin in &builtins {
        if !store.presets.iter().any(|p| p.id == builtin.id) {
            store.presets.push(builtin.clone());
        }
    }
}

/// Try to load from the legacy ~/.config/codemux/presets.json file.
/// If successful, deletes the JSON file after loading.
fn migrate_from_json_file() -> Option<PresetStore> {
    let path = dirs::config_dir()?.join("codemux").join("presets.json");
    let data = std::fs::read_to_string(&path).ok()?;
    let mut store: PresetStore = serde_json::from_str(&data).ok()?;
    sync_builtins(&mut store);

    // Remove the legacy file now that data is in SQLite
    let _ = std::fs::remove_file(&path);
    eprintln!("[codemux::presets] Migrated presets from JSON to SQLite");

    Some(store)
}

fn default_store() -> PresetStore {
    PresetStore {
        schema_version: PRESET_SCHEMA_VERSION,
        presets: builtin_presets(),
        default_preset_id: None,
        bar_visible: true,
    }
}

pub fn snapshot_from_store(store: &PresetStore) -> PresetStoreSnapshot {
    PresetStoreSnapshot {
        presets: store.presets.clone(),
        bar_visible: store.bar_visible,
        default_preset_id: store.default_preset_id.clone(),
    }
}

pub fn emit_presets_changed(app: &tauri::AppHandle) {
    use tauri::Emitter;
    use tauri::Manager;

    let presets: tauri::State<'_, PresetStoreState> = app.state();
    let store = presets.inner.lock().unwrap_or_else(|e| e.into_inner());
    let snapshot = snapshot_from_store(&store);
    if let Err(error) = app.emit("presets-changed", &snapshot) {
        eprintln!("[codemux::presets] Failed to emit presets-changed: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_presets_have_unique_ids() {
        let presets = builtin_presets();
        let mut ids: Vec<&str> = presets.iter().map(|p| p.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), presets.len());
    }

    #[test]
    fn default_store_has_all_builtins() {
        let store = default_store();
        assert_eq!(store.presets.len(), 5);
        assert!(store.bar_visible);
        assert!(store.presets.iter().all(|p| p.is_builtin));
    }

    #[test]
    fn load_presets_returns_defaults_on_missing_file() {
        let db = DatabaseStore::new_in_memory();
        let store = load_presets(&db);
        assert_eq!(store.presets.len(), 5);
        assert!(store.bar_visible);
    }

    #[test]
    fn save_and_load_roundtrip() {
        let db = DatabaseStore::new_in_memory();
        let mut store = default_store();
        store.bar_visible = false;
        save_presets(&db, &store).unwrap();

        let loaded = load_presets(&db);
        assert!(!loaded.bar_visible);
        assert_eq!(loaded.presets.len(), 5);
    }

    #[test]
    fn sync_adds_missing_builtins() {
        let db = DatabaseStore::new_in_memory();
        // Save a store with only 2 presets
        let mut store = default_store();
        store.presets.retain(|p| p.id == "builtin-claude" || p.id == "builtin-shell");
        save_presets(&db, &store).unwrap();

        // Load should re-add missing builtins
        let loaded = load_presets(&db);
        assert_eq!(loaded.presets.len(), 5);
    }
}
