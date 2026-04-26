// Tauri commands for the cross-provider skills system.
//
// Stage 1 ships only `list_skills`. Stage 2 wires the result into the
// frontend slash popup; Stage 4 wires it into the Settings UI.

use std::path::{Path, PathBuf};

use tauri::{AppHandle, State};

use crate::skills::{
    paths::enumerate_scan_paths, scanner::scan_directory, watcher::SkillsWatcherState, Skill,
    SkillProvider, SkillScope,
};

/// Discover all skills under the locked directories and return them as a
/// flat list. The frontend deduplicates / groups; the backend stays
/// stateless. Lazy on-demand only — no scan happens until the frontend
/// asks. Per-skill errors (malformed YAML, unreadable file) are logged on
/// stderr and the offending skill is dropped.
#[tauri::command]
pub async fn list_skills(
    project_root: Option<String>,
    include_plugins: bool,
) -> Result<Vec<Skill>, String> {
    let project_path_owned = project_root.map(std::path::PathBuf::from);
    let project_path: Option<&Path> = project_path_owned.as_deref();
    let paths = enumerate_scan_paths(project_path, include_plugins);

    let mut all_skills: Vec<Skill> = Vec::new();

    for (path, provider) in paths.user_paths {
        all_skills.extend(scan_directory(&path, provider, SkillScope::User, None));
    }
    for (path, provider) in paths.project_paths {
        all_skills.extend(scan_directory(&path, provider, SkillScope::Project, None));
    }
    for (path, plugin_slug) in paths.plugin_paths {
        all_skills.extend(scan_directory(
            &path,
            SkillProvider::Claude,
            SkillScope::Plugin,
            Some(plugin_slug),
        ));
    }

    // Stable, predictable ordering for the popup: provider → scope → name.
    all_skills.sort_by(|a, b| {
        provider_rank(a.provider)
            .cmp(&provider_rank(b.provider))
            .then(scope_rank(a.scope).cmp(&scope_rank(b.scope)))
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });

    Ok(all_skills)
}

fn provider_rank(p: SkillProvider) -> u8 {
    match p {
        SkillProvider::Claude => 0,
        SkillProvider::Codex => 1,
        SkillProvider::Opencode => 2,
        SkillProvider::Codemux => 3,
    }
}

fn scope_rank(s: SkillScope) -> u8 {
    match s {
        SkillScope::Project => 0,
        SkillScope::User => 1,
        SkillScope::Plugin => 2,
    }
}

/// Start the skills file watcher. Idempotent — calling again with new
/// `project_root` / `include_plugins` re-watches the new path set.
/// Returns the count of paths actually being watched (paths that
/// don't exist on disk are skipped silently).
#[tauri::command]
pub async fn start_skills_watcher(
    app: AppHandle,
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
