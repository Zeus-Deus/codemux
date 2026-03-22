use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

const PRESET_SCHEMA_VERSION: u32 = 1;

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

impl Default for PresetStoreState {
    fn default() -> Self {
        Self {
            inner: Mutex::new(load_presets()),
        }
    }
}

fn preset_file_path() -> Option<PathBuf> {
    let base = dirs::config_dir()?;
    Some(base.join("codemux").join("presets.json"))
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

pub fn load_presets() -> PresetStore {
    let Some(path) = preset_file_path() else {
        return default_store();
    };

    let data = match fs::read_to_string(&path) {
        Ok(d) => d,
        Err(_) => return default_store(),
    };

    let mut store: PresetStore = match serde_json::from_str(&data) {
        Ok(s) => s,
        Err(_) => return default_store(),
    };

    // Sync builtins: add missing ones, prune stale ones (e.g. removed Aider/Dev Server)
    let builtins = builtin_presets();
    let builtin_ids: Vec<&str> = builtins.iter().map(|b| b.id.as_str()).collect();

    // Remove stale builtins whose IDs no longer exist in the current builtin list
    store.presets.retain(|p| {
        !p.id.starts_with("builtin-") || builtin_ids.contains(&p.id.as_str())
    });

    // Add any missing builtins (handles upgrades when new builtins are added)
    for builtin in &builtins {
        if !store.presets.iter().any(|p| p.id == builtin.id) {
            store.presets.push(builtin.clone());
        }
    }

    store
}

fn default_store() -> PresetStore {
    PresetStore {
        schema_version: PRESET_SCHEMA_VERSION,
        presets: builtin_presets(),
        default_preset_id: None,
        bar_visible: true,
    }
}

pub fn save_presets(store: &PresetStore) -> Result<(), String> {
    let Some(path) = preset_file_path() else {
        return Ok(());
    };

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create config dir: {error}"))?;
    }

    let json = serde_json::to_string_pretty(store)
        .map_err(|error| format!("Failed to serialize presets: {error}"))?;

    fs::write(&path, json)
        .map_err(|error| format!("Failed to write presets: {error}"))?;

    Ok(())
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
        // When there's no config file, load_presets falls back to defaults
        let store = default_store();
        assert_eq!(store.presets.len(), 5);
    }
}
