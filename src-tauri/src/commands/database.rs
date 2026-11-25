use crate::database::{DatabaseStore, ProjectScripts, RecentProject, REVIEWED_PROJECT_CHANGED};
#[cfg(unix)]
use crate::database::ReviewedProjectRoot;
use std::collections::HashMap;
use tauri::State;

#[tauri::command]
pub fn db_get_setting(db: State<'_, DatabaseStore>, key: String) -> Option<String> {
    db.get_setting(&key)
}

#[tauri::command]
pub fn db_set_setting(
    db: State<'_, DatabaseStore>,
    mcp: State<'_, crate::mcp::registry::McpRegistry>,
    key: String,
    value: String,
) -> Result<(), String> {
    db.set_setting(&key, &value)?;
    // Live Claude chats re-list their tools, so the delegation toggle
    // reaches them without a restart.
    if key == crate::commands::delegation::DELEGATION_SETTING_KEY {
        mcp.notify_host_tools_changed();
    }
    Ok(())
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

#[derive(serde::Serialize)]
pub struct ProjectScriptsReview {
    #[serde(flatten)]
    scripts: ProjectScripts,
    #[serde(skip_serializing_if = "Option::is_none")]
    review_root: Option<std::path::PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    review_id: Option<String>,
}

#[derive(Default)]
pub struct ProjectScriptsReviews {
    // Settings displays one selected project at a time. A new read replaces
    // the previous review and releases its directory handle.
    #[cfg(unix)]
    pending: std::sync::Mutex<Option<(String, ReviewedProjectRoot)>>,
}

impl ProjectScriptsReviews {
    fn read(&self, db: &DatabaseStore, path: &str) -> Option<ProjectScriptsReview> {
        #[cfg(unix)]
        let mut pending = {
            let mut pending = self.pending.lock().ok()?;
            *pending = None;
            pending
        };

        if let Some(scripts) = db.get_project_scripts(path) {
            return Some(ProjectScriptsReview {
                scripts,
                review_root: None,
                review_id: None,
            });
        }
        #[cfg(unix)]
        if let Some((scripts, root)) = db.legacy_project_scripts_for_review(path) {
            let id = uuid::Uuid::new_v4().to_string();
            let review_root = root.path().to_path_buf();
            *pending = Some((id.clone(), root));
            return Some(ProjectScriptsReview {
                scripts,
                review_root: Some(review_root),
                review_id: Some(id),
            });
        }
        None
    }

    fn save(
        &self,
        db: &DatabaseStore,
        path: &str,
        review_id: &str,
        scripts: &ProjectScripts,
    ) -> Result<(), String> {
        #[cfg(unix)]
        {
            let mut pending = self.pending.lock().map_err(|_| REVIEWED_PROJECT_CHANGED)?;
            if !pending.as_ref().is_some_and(|(id, root)| {
                id == review_id && root.path().to_str() == Some(path)
            }) {
                return Err(REVIEWED_PROJECT_CHANGED.into());
            }
            let (_, root) = pending.take().ok_or(REVIEWED_PROJECT_CHANGED)?;
            db.save_reviewed_project_scripts(&root, scripts)
        }
        #[cfg(not(unix))]
        {
            let _ = (db, path, review_id, scripts);
            Err(REVIEWED_PROJECT_CHANGED.into())
        }
    }
}

#[tauri::command]
pub fn get_project_scripts_for_review(
    db: State<'_, DatabaseStore>,
    reviews: State<'_, ProjectScriptsReviews>,
    path: String,
) -> Option<ProjectScriptsReview> {
    reviews.read(&db, &path)
}

#[tauri::command]
pub fn set_project_scripts(
    db: State<'_, DatabaseStore>,
    reviews: State<'_, ProjectScriptsReviews>,
    path: String,
    scripts: ProjectScripts,
    reviewed: Option<bool>,
    review_id: Option<String>,
) -> Result<(), String> {
    // A client using the old review flag must obtain a fresh review ID.
    if reviewed == Some(true) || review_id.is_some() {
        let review_id = review_id.as_deref().ok_or(REVIEWED_PROJECT_CHANGED)?;
        reviews.save(&db, &path, review_id, &scripts)
    } else {
        db.set_project_scripts(&path, &scripts)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn legacy_review_fixture() -> (tempfile::TempDir, std::path::PathBuf, DatabaseStore, ProjectScripts) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("project");
        let alias = dir.path().join("old-project");
        fs::create_dir(&root).unwrap();
        std::os::unix::fs::symlink(&root, &alias).unwrap();
        let root = dunce::canonicalize(root).unwrap();
        let scripts = ProjectScripts {
            setup: vec!["setup fixture".into()],
            teardown: vec!["teardown fixture".into()],
            run: Some("run fixture".into()),
            ..Default::default()
        };
        let db = crate::database::init_test_database();
        db.set_setting(
            &format!("project.scripts:{}", alias.display()),
            &serde_json::to_string(&scripts).unwrap(),
        )
        .unwrap();
        (dir, root, db, scripts)
    }

    fn review(reviews: &ProjectScriptsReviews, db: &DatabaseStore, root: &Path) -> String {
        let preview = reviews.read(db, root.to_str().unwrap()).unwrap();
        assert_eq!(preview.review_root.as_deref(), Some(root));
        preview.review_id.unwrap()
    }

    #[test]
    fn legacy_project_script_approval_is_one_shot_and_bound_to_the_reviewed_directory() {
        let (dir, root, db, scripts) = legacy_review_fixture();
        let path = root.to_str().unwrap();
        let reviews = ProjectScriptsReviews::default();

        assert!(db.get_project_scripts(path).is_none());
        let stale_id = review(&reviews, &db, &root);
        let id = review(&reviews, &db, &root);
        assert!(reviews.save(&db, path, &stale_id, &scripts).is_err());
        let mut edited = scripts.clone();
        edited.setup = vec!["reviewed setup".into()];
        reviews.save(&db, path, &id, &edited).unwrap();
        assert_eq!(db.get_project_scripts(path), Some(edited.clone()));
        assert!(reviews.save(&db, path, &id, &scripts).is_err());

        edited.run = Some("edited run".into());
        db.set_project_scripts(path, &edited).unwrap();
        assert_eq!(db.get_project_scripts(path), Some(edited.clone()));

        // Keep the original directory alive while testing a fresh inode
        // at exactly the same canonical pathname.
        fs::rename(&root, dir.path().join("previous-project")).unwrap();
        fs::create_dir(&root).unwrap();
        assert!(db.get_project_scripts(path).is_none());
        assert_eq!(db.set_project_scripts(path, &scripts).unwrap_err(), REVIEWED_PROJECT_CHANGED);
        let preview = reviews.read(&db, path).unwrap();
        assert_eq!(preview.scripts, edited);
        let id = preview.review_id.unwrap();
        reviews.save(&db, path, &id, &edited).unwrap();
        assert_eq!(db.get_project_scripts(path), Some(edited));
    }

    #[test]
    fn legacy_project_script_save_refuses_replacement_at_same_path_or_a_symlink() {
        let (dir, root, db, scripts) = legacy_review_fixture();
        let path = root.to_str().unwrap();
        let reviews = ProjectScriptsReviews::default();
        let id = review(&reviews, &db, &root);
        let old = dir.path().join("previous-project");
        fs::rename(&root, &old).unwrap();
        fs::create_dir(&root).unwrap();
        assert_eq!(dunce::canonicalize(&root).unwrap(), root);
        assert_eq!(reviews.save(&db, path, &id, &scripts).unwrap_err(), REVIEWED_PROJECT_CHANGED);
        assert!(db.get_project_scripts(path).is_none());

        let fresh = review(&reviews, &db, &root);
        fs::remove_dir(&root).unwrap();
        std::os::unix::fs::symlink(&old, &root).unwrap();
        assert_eq!(reviews.save(&db, path, &fresh, &scripts).unwrap_err(), REVIEWED_PROJECT_CHANGED);
        assert!(db.get_project_scripts(old.to_str().unwrap()).is_none());
        assert!(db.get_setting(&format!("project.scripts:{path}")).is_none());
    }
}
