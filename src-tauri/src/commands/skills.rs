// Tauri commands for the cross-provider skills system.
//
// Stage 1 ships only `list_skills`. Stage 2 wires the result into the
// frontend slash popup; Stage 4 wires it into the Settings UI.

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Runtime, State};

use crate::skills::{
    inventory::SkillInventoryService, watcher::SkillsWatcherState, SkillInventory,
};

/// Discover all skills under the locked directories and return them as a
/// flat list. The frontend deduplicates / groups; the backend stays
/// stateless. Lazy on-demand only — no scan happens until the frontend
/// asks. Per-skill errors (malformed YAML, unreadable file) are logged on
/// stderr and the offending skill is dropped.
#[tauri::command]
pub async fn list_skills(
    inventory: State<'_, SkillInventoryService>,
    opencode_manager: State<'_, std::sync::Arc<crate::agent_provider::opencode::OpenCodeServerManager>>,
    project_root: Option<String>,
    include_plugins: bool,
    force: bool,
) -> Result<SkillInventory, String> {
    inventory
        .list(
            project_root.as_deref().map(Path::new),
            include_plugins,
            force,
            Some(opencode_manager.inner()),
        )
        .await
}

/// Start the skills file watcher. Idempotent — calling again with new
/// `project_root` / `include_plugins` re-watches the new path set.
/// Returns the count of paths actually being watched (paths that
/// don't exist on disk are skipped silently).
#[tauri::command]
pub async fn start_skills_watcher<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, SkillsWatcherState>,
    project_root: Option<String>,
    include_plugins: bool,
) -> Result<usize, String> {
    let project_path = project_root.map(PathBuf::from);
    state.start(app, project_path, include_plugins)
}

/// Stop the skills file watcher. Safe to call when not running.
#[tauri::command]
pub async fn stop_skills_watcher(
    state: State<'_, SkillsWatcherState>,
) -> Result<(), String> {
    state.stop()
}
