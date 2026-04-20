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

/// Who drives the keystrokes in a terminal session.
///
/// - `Human` — a person typed the command (plain shell, setup-script run
///   button, `Shell` preset, custom presets). Inherits the full desktop
///   env so `npm run tauri dev`, `firefox`, etc. work.
/// - `Agent` — an AI CLI drives the PTY (built-in Claude/Codex/OpenCode/
///   Gemini/Pi presets, or any custom preset the user flags). Desktop GUI
///   env is stripped so the agent can't accidentally pop windows onto the
///   user's real session; users who want agent-driven GUI testing opt
///   into virtual-display routing per-workspace.
///
/// This is the principal that drives display policy — see
/// `ExecutionPolicy::worktree_session_default_for_persona`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Persona {
    Human,
    Agent,
}

impl Default for Persona {
    fn default() -> Self {
        Persona::Human
    }
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
    #[serde(default)]
    pub auto_run_on_workspace: bool,
    #[serde(default)]
    pub auto_run_on_new_tab: bool,
    /// Who drives keystrokes in the resulting session. Controls whether
    /// `DISPLAY`/`WAYLAND_DISPLAY`/etc. are inherited (Human) or stripped
    /// (Agent). Missing in saved stores from before this field existed —
    /// `#[serde(default)]` falls back to `Human`, which is the safe
    /// backwards-compatible choice for custom presets (worst case: user
    /// sees their own GUI app pop, same as pre-fix behavior). Built-in
    /// agent presets override this to `Agent` via `sync_builtins`.
    #[serde(default)]
    pub persona: Persona,
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
            auto_run_on_workspace: false,
            auto_run_on_new_tab: false,
            persona: Persona::Agent,
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
            auto_run_on_workspace: false,
            auto_run_on_new_tab: false,
            persona: Persona::Agent,
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
            auto_run_on_workspace: false,
            auto_run_on_new_tab: false,
            persona: Persona::Agent,
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
            auto_run_on_workspace: false,
            auto_run_on_new_tab: false,
            persona: Persona::Agent,
        },
        TerminalPreset {
            id: "builtin-pi".into(),
            name: "Pi".into(),
            description: Some("Launch Pi coding agent".into()),
            commands: vec!["pi".into()],
            working_directory: None,
            launch_mode: LaunchMode::NewTab,
            icon: Some("pi".into()),
            pinned: true,
            is_builtin: true,
            auto_run_on_workspace: false,
            auto_run_on_new_tab: false,
            persona: Persona::Agent,
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
            auto_run_on_workspace: false,
            auto_run_on_new_tab: false,
            persona: Persona::Human,
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
///
/// Also force-refreshes the `persona` field on existing builtins from the
/// template. Persona is a sandboxing decision tied to the *identity* of the
/// builtin (Claude Code is an agent CLI; the Shell builtin is a human shell) —
/// not something users should override by accident on a store migrated from
/// a schema without the field. Old saves deserialize `persona` as
/// `Persona::Human` (the `#[serde(default)]` value); without this refresh,
/// upgrading users would silently get `Human` personas on Claude/Codex/etc.
/// and lose the GUI-strip guarantee that was the whole point of this fix.
///
/// User-editable fields (name, commands, icon, pinned, …) are preserved —
/// only `persona` is overwritten, and only on builtins.
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

    // Refresh persona on every existing builtin from the template.
    for builtin in &builtins {
        if let Some(existing) = store.presets.iter_mut().find(|p| p.id == builtin.id) {
            existing.persona = builtin.persona;
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
        assert_eq!(store.presets.len(), 6);
        assert!(store.bar_visible);
        assert!(store.presets.iter().all(|p| p.is_builtin));
    }

    #[test]
    fn load_presets_returns_defaults_on_missing_file() {
        let db = DatabaseStore::new_in_memory();
        let store = load_presets(&db);
        assert_eq!(store.presets.len(), 6);
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
        assert_eq!(loaded.presets.len(), 6);
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
        assert_eq!(loaded.presets.len(), 6);
    }

    fn make_custom_preset(id: &str, name: &str) -> TerminalPreset {
        TerminalPreset {
            id: id.into(),
            name: name.into(),
            description: Some("Test preset".into()),
            commands: vec!["echo hello".into()],
            working_directory: None,
            launch_mode: LaunchMode::NewTab,
            icon: None,
            pinned: true,
            is_builtin: false,
            auto_run_on_workspace: false,
            auto_run_on_new_tab: false,
            persona: Persona::Human,
        }
    }

    #[test]
    fn update_preset_name() {
        let db = DatabaseStore::new_in_memory();
        let mut store = default_store();
        store.presets.push(make_custom_preset("custom-1", "My Preset"));
        save_presets(&db, &store).unwrap();

        // Update name
        let mut loaded = load_presets(&db);
        let p = loaded.presets.iter_mut().find(|p| p.id == "custom-1").unwrap();
        p.name = "Renamed Preset".into();
        save_presets(&db, &loaded).unwrap();

        let reloaded = load_presets(&db);
        let p = reloaded.presets.iter().find(|p| p.id == "custom-1").unwrap();
        assert_eq!(p.name, "Renamed Preset");
        assert_eq!(p.commands, vec!["echo hello"]);
    }

    #[test]
    fn update_preset_commands() {
        let db = DatabaseStore::new_in_memory();
        let mut store = default_store();
        store.presets.push(make_custom_preset("custom-1", "Test"));
        save_presets(&db, &store).unwrap();

        let mut loaded = load_presets(&db);
        let p = loaded.presets.iter_mut().find(|p| p.id == "custom-1").unwrap();
        p.commands = vec!["npm run dev".into(), "npm run watch".into()];
        save_presets(&db, &loaded).unwrap();

        let reloaded = load_presets(&db);
        let p = reloaded.presets.iter().find(|p| p.id == "custom-1").unwrap();
        assert_eq!(p.commands, vec!["npm run dev", "npm run watch"]);
    }

    #[test]
    fn update_preset_launch_mode() {
        let db = DatabaseStore::new_in_memory();
        let mut store = default_store();
        store.presets.push(make_custom_preset("custom-1", "Test"));
        save_presets(&db, &store).unwrap();

        let mut loaded = load_presets(&db);
        let p = loaded.presets.iter_mut().find(|p| p.id == "custom-1").unwrap();
        assert_eq!(p.launch_mode, LaunchMode::NewTab);
        p.launch_mode = LaunchMode::SplitPane;
        save_presets(&db, &loaded).unwrap();

        let reloaded = load_presets(&db);
        let p = reloaded.presets.iter().find(|p| p.id == "custom-1").unwrap();
        assert_eq!(p.launch_mode, LaunchMode::SplitPane);
    }

    #[test]
    fn update_preset_pinned() {
        let db = DatabaseStore::new_in_memory();
        let mut store = default_store();
        store.presets.push(make_custom_preset("custom-1", "Test"));
        save_presets(&db, &store).unwrap();

        let mut loaded = load_presets(&db);
        let p = loaded.presets.iter_mut().find(|p| p.id == "custom-1").unwrap();
        assert!(p.pinned);
        p.pinned = false;
        save_presets(&db, &loaded).unwrap();

        let reloaded = load_presets(&db);
        let p = reloaded.presets.iter().find(|p| p.id == "custom-1").unwrap();
        assert!(!p.pinned);
    }

    #[test]
    fn update_preset_description() {
        let db = DatabaseStore::new_in_memory();
        let mut store = default_store();
        store.presets.push(make_custom_preset("custom-1", "Test"));
        save_presets(&db, &store).unwrap();

        let mut loaded = load_presets(&db);
        let p = loaded.presets.iter_mut().find(|p| p.id == "custom-1").unwrap();
        p.description = Some("Updated description".into());
        save_presets(&db, &loaded).unwrap();

        let reloaded = load_presets(&db);
        let p = reloaded.presets.iter().find(|p| p.id == "custom-1").unwrap();
        assert_eq!(p.description, Some("Updated description".into()));
    }

    #[test]
    fn delete_custom_preset() {
        let db = DatabaseStore::new_in_memory();
        let mut store = default_store();
        store.presets.push(make_custom_preset("custom-1", "To Delete"));
        save_presets(&db, &store).unwrap();
        assert_eq!(load_presets(&db).presets.len(), 7);

        let mut loaded = load_presets(&db);
        loaded.presets.retain(|p| p.id != "custom-1");
        save_presets(&db, &loaded).unwrap();

        let reloaded = load_presets(&db);
        assert_eq!(reloaded.presets.len(), 6); // only builtins remain
        assert!(reloaded.presets.iter().all(|p| p.is_builtin));
    }

    #[test]
    fn builtin_presets_survive_custom_delete() {
        let db = DatabaseStore::new_in_memory();
        let mut store = default_store();
        store.presets.push(make_custom_preset("custom-1", "Custom"));
        save_presets(&db, &store).unwrap();

        // Delete only custom
        let mut loaded = load_presets(&db);
        loaded.presets.retain(|p| p.id != "custom-1");
        save_presets(&db, &loaded).unwrap();

        let reloaded = load_presets(&db);
        assert!(reloaded.presets.iter().any(|p| p.id == "builtin-claude"));
        assert!(reloaded.presets.iter().any(|p| p.id == "builtin-codex"));
        assert!(reloaded.presets.iter().any(|p| p.id == "builtin-gemini"));
    }

    #[test]
    fn update_persists_across_reload() {
        let db = DatabaseStore::new_in_memory();
        let mut store = default_store();
        store.presets.push(make_custom_preset("custom-1", "Original"));
        save_presets(&db, &store).unwrap();

        // Update multiple fields
        let mut loaded = load_presets(&db);
        let p = loaded.presets.iter_mut().find(|p| p.id == "custom-1").unwrap();
        p.name = "Updated".into();
        p.commands = vec!["new-cmd".into()];
        p.launch_mode = LaunchMode::SplitPane;
        p.pinned = false;
        p.description = Some("New desc".into());
        save_presets(&db, &loaded).unwrap();

        // Fresh load from DB
        let fresh = load_presets(&db);
        let p = fresh.presets.iter().find(|p| p.id == "custom-1").unwrap();
        assert_eq!(p.name, "Updated");
        assert_eq!(p.commands, vec!["new-cmd"]);
        assert_eq!(p.launch_mode, LaunchMode::SplitPane);
        assert!(!p.pinned);
        assert_eq!(p.description, Some("New desc".into()));
    }

    #[test]
    fn builtin_preset_fields_are_editable() {
        let db = DatabaseStore::new_in_memory();
        let mut store = load_presets(&db);

        // Edit the builtin claude preset
        let p = store.presets.iter_mut().find(|p| p.id == "builtin-claude").unwrap();
        p.name = "My Claude".into();
        p.description = Some("Custom desc".into());
        p.commands = vec!["claude --dangerously-skip-permissions --verbose".into()];
        p.launch_mode = LaunchMode::SplitPane;
        save_presets(&db, &store).unwrap();

        let reloaded = load_presets(&db);
        let p = reloaded.presets.iter().find(|p| p.id == "builtin-claude").unwrap();
        assert_eq!(p.name, "My Claude");
        assert_eq!(p.description, Some("Custom desc".into()));
        assert_eq!(p.commands, vec!["claude --dangerously-skip-permissions --verbose"]);
        assert_eq!(p.launch_mode, LaunchMode::SplitPane);
        assert!(p.is_builtin); // still marked as builtin
    }

    #[test]
    fn auto_run_fields_default_to_false() {
        let db = DatabaseStore::new_in_memory();
        let store = load_presets(&db);
        for p in &store.presets {
            assert!(!p.auto_run_on_workspace, "preset {} should default to false", p.id);
            assert!(!p.auto_run_on_new_tab, "preset {} should default to false", p.id);
        }
    }

    #[test]
    fn auto_run_fields_persist_across_reload() {
        let db = DatabaseStore::new_in_memory();
        let mut store = default_store();
        store.presets.push(make_custom_preset("custom-1", "Test"));
        save_presets(&db, &store).unwrap();

        // Toggle auto_run_on_workspace on
        let mut loaded = load_presets(&db);
        let p = loaded.presets.iter_mut().find(|p| p.id == "custom-1").unwrap();
        assert!(!p.auto_run_on_workspace);
        assert!(!p.auto_run_on_new_tab);
        p.auto_run_on_workspace = true;
        save_presets(&db, &loaded).unwrap();

        let reloaded = load_presets(&db);
        let p = reloaded.presets.iter().find(|p| p.id == "custom-1").unwrap();
        assert!(p.auto_run_on_workspace);
        assert!(!p.auto_run_on_new_tab);

        // Toggle auto_run_on_new_tab on too
        let mut loaded2 = load_presets(&db);
        let p = loaded2.presets.iter_mut().find(|p| p.id == "custom-1").unwrap();
        p.auto_run_on_new_tab = true;
        save_presets(&db, &loaded2).unwrap();

        let reloaded2 = load_presets(&db);
        let p = reloaded2.presets.iter().find(|p| p.id == "custom-1").unwrap();
        assert!(p.auto_run_on_workspace);
        assert!(p.auto_run_on_new_tab);
    }

    // ── Persona: built-in + custom behavior ─────────────────────────────

    #[test]
    fn builtin_agent_presets_carry_agent_persona() {
        // Claude/Codex/OpenCode/Gemini/Pi are AI agent CLIs. They must
        // spawn with the Agent persona so `DISPLAY`/`WAYLAND_DISPLAY`/etc.
        // are stripped from their PTY and they can't accidentally pop
        // Firefox/Electron windows onto the user's real desktop.
        let presets = builtin_presets();
        for agent_id in [
            "builtin-claude",
            "builtin-codex",
            "builtin-opencode",
            "builtin-gemini",
            "builtin-pi",
        ] {
            let p = presets
                .iter()
                .find(|p| p.id == agent_id)
                .unwrap_or_else(|| panic!("builtin preset {agent_id} should exist"));
            assert_eq!(
                p.persona,
                Persona::Agent,
                "{agent_id} must default to Persona::Agent — it's an AI CLI"
            );
        }
    }

    #[test]
    fn builtin_shell_preset_is_human_persona() {
        let presets = builtin_presets();
        let shell = presets
            .iter()
            .find(|p| p.id == "builtin-shell")
            .expect("builtin-shell should exist");
        assert_eq!(
            shell.persona,
            Persona::Human,
            "Shell preset is user-driven — must keep desktop GUI env"
        );
    }

    #[test]
    fn custom_preset_defaults_to_human_persona() {
        // Rule: user-created presets default to Human. Worst case if the
        // user picks a wrong default is that their GUI app pops on their
        // desktop — same as pre-fix behavior for that preset. Safer than
        // silently denying GUI access to a custom shell wrapper.
        let custom = make_custom_preset("custom-foo", "My Preset");
        assert_eq!(custom.persona, Persona::Human);
    }

    #[test]
    fn persona_round_trips_through_save_and_load() {
        let db = DatabaseStore::new_in_memory();
        let mut store = default_store();
        let mut custom = make_custom_preset("custom-agent", "Custom Agent");
        custom.persona = Persona::Agent;
        store.presets.push(custom);
        save_presets(&db, &store).unwrap();

        let reloaded = load_presets(&db);
        let p = reloaded
            .presets
            .iter()
            .find(|p| p.id == "custom-agent")
            .expect("custom-agent should round-trip");
        assert_eq!(p.persona, Persona::Agent);
    }

    #[test]
    fn sync_builtins_repairs_agent_persona_on_old_schema() {
        // Simulate a store saved before `persona` existed: load the
        // default builtins, then manually downgrade the Claude builtin's
        // persona to Human (as if it deserialized from old JSON with no
        // persona field), save, and reload. `sync_builtins` must restore
        // Persona::Agent because the builtin identity dictates it.
        let db = DatabaseStore::new_in_memory();
        let mut store = default_store();
        let claude = store
            .presets
            .iter_mut()
            .find(|p| p.id == "builtin-claude")
            .unwrap();
        claude.persona = Persona::Human; // the bug state we must repair
        save_presets(&db, &store).unwrap();

        let reloaded = load_presets(&db);
        let claude = reloaded
            .presets
            .iter()
            .find(|p| p.id == "builtin-claude")
            .unwrap();
        assert_eq!(
            claude.persona,
            Persona::Agent,
            "sync_builtins must force Claude's persona back to Agent"
        );
    }

    #[test]
    fn sync_builtins_preserves_user_edits_except_persona() {
        // User may have renamed a builtin or changed its command — those
        // edits must survive sync. Only `persona` is refreshed.
        let db = DatabaseStore::new_in_memory();
        let mut store = default_store();
        let claude = store
            .presets
            .iter_mut()
            .find(|p| p.id == "builtin-claude")
            .unwrap();
        claude.name = "My Renamed Claude".into();
        claude.commands = vec!["claude --verbose".into()];
        claude.persona = Persona::Human; // will be refreshed
        save_presets(&db, &store).unwrap();

        let reloaded = load_presets(&db);
        let claude = reloaded
            .presets
            .iter()
            .find(|p| p.id == "builtin-claude")
            .unwrap();
        assert_eq!(claude.name, "My Renamed Claude");
        assert_eq!(claude.commands, vec!["claude --verbose".to_string()]);
        assert_eq!(claude.persona, Persona::Agent); // refreshed
    }

    #[test]
    fn persona_deserializes_missing_field_as_human() {
        // Backwards-compat: a JSON blob written by an older Codemux
        // version lacks the `persona` field entirely. `#[serde(default)]`
        // must kick in and yield `Persona::Human` — the safe default for
        // custom presets. (Builtins get corrected by `sync_builtins`.)
        let json = r#"{
            "id": "legacy-preset",
            "name": "Legacy",
            "description": null,
            "commands": ["echo hi"],
            "working_directory": null,
            "launch_mode": "new_tab",
            "icon": null,
            "pinned": false,
            "is_builtin": false
        }"#;
        let p: TerminalPreset = serde_json::from_str(json)
            .expect("old-schema JSON must still deserialize");
        assert_eq!(p.persona, Persona::Human);
    }
}
