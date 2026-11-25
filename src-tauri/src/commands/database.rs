use crate::database::{
    DatabaseStore, ProjectScripts, RecentProject, ReviewedProjectRoot, REVIEWED_PROJECT_CHANGED,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
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
    review_root: Option<PathBuf>,
    review_id: String,
}

struct SelectedProject {
    id: String,
    selection: String,
    root: ReviewedProjectRoot,
    needs_review: bool,
}

#[derive(Default)]
pub struct ProjectScriptsReviews {
    // Settings displays one selected project at a time. A new read replaces
    // the previous review and releases its directory handle.
    pending: std::sync::Mutex<Option<SelectedProject>>,
}

impl ProjectScriptsReviews {
    fn read(&self, db: &DatabaseStore, path: &str) -> Result<ProjectScriptsReview, String> {
        let mut pending = self.pending.lock().map_err(|_| REVIEWED_PROJECT_CHANGED)?;
        *pending = None;
        let (scripts, root, needs_review) = db.project_scripts_for_settings(path)?;
        let id = uuid::Uuid::new_v4().to_string();
        let review_root = needs_review.then(|| root.path().to_path_buf());
        *pending = Some(SelectedProject {
            id: id.clone(),
            selection: path.to_string(),
            root,
            needs_review,
        });
        Ok(ProjectScriptsReview { scripts, review_root, review_id: id })
    }

    fn save(
        &self,
        db: &DatabaseStore,
        path: &str,
        review_id: &str,
        scripts: &ProjectScripts,
        reviewed: bool,
    ) -> Result<(), String> {
        let mut pending = self.pending.lock().map_err(|_| REVIEWED_PROJECT_CHANGED)?;
        let selected = pending.as_mut().ok_or(REVIEWED_PROJECT_CHANGED)?;
        let expected_path = if reviewed {
            selected.root.path().to_str().ok_or(REVIEWED_PROJECT_CHANGED)?
        } else {
            &selected.selection
        };
        if selected.id != review_id || selected.needs_review != reviewed || path != expected_path {
            return Err(REVIEWED_PROJECT_CHANGED.into());
        }
        if !selected.root.matches_path(Path::new(&selected.selection)) {
            *pending = None;
            return Err(REVIEWED_PROJECT_CHANGED.into());
        }
        db.save_reviewed_project_scripts(&selected.root, scripts, reviewed)?;
        selected.needs_review = false;
        Ok(())
    }
}

#[tauri::command]
pub fn get_project_scripts_for_review(
    db: State<'_, DatabaseStore>,
    reviews: State<'_, ProjectScriptsReviews>,
    path: String,
) -> Result<ProjectScriptsReview, String> {
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
    if let Some(review_id) = review_id.as_deref() {
        reviews.save(&db, &path, review_id, &scripts, reviewed.unwrap_or_default())
    } else if reviewed == Some(true) {
        Err(REVIEWED_PROJECT_CHANGED.into())
    } else {
        // Onboarding saves explicitly without opening the Settings form.
        db.set_project_scripts(&path, &scripts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    #[cfg(unix)]
    fn legacy_review_fixture() -> (tempfile::TempDir, PathBuf, DatabaseStore, ProjectScripts) {
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

    #[cfg(unix)]
    fn review(reviews: &ProjectScriptsReviews, db: &DatabaseStore, root: &Path) -> String {
        let preview = reviews.read(db, root.to_str().unwrap()).unwrap();
        assert_eq!(preview.review_root.as_deref(), Some(root));
        preview.review_id
    }

    #[cfg(unix)]
    #[test]
    fn legacy_project_script_approval_is_one_shot_and_bound_to_the_reviewed_directory() {
        let (dir, root, db, scripts) = legacy_review_fixture();
        let path = root.to_str().unwrap();
        let reviews = ProjectScriptsReviews::default();

        assert!(db.get_project_scripts(path).is_none());
        let stale_id = review(&reviews, &db, &root);
        let id = review(&reviews, &db, &root);
        assert!(reviews.save(&db, path, &stale_id, &scripts, true).is_err());
        let mut edited = scripts.clone();
        edited.setup = vec!["reviewed setup".into()];
        reviews.save(&db, path, &id, &edited, true).unwrap();
        assert_eq!(db.get_project_scripts(path), Some(edited.clone()));
        assert!(reviews.save(&db, path, &id, &scripts, true).is_err());

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
        reviews.save(&db, path, &preview.review_id, &edited, true).unwrap();
        assert_eq!(db.get_project_scripts(path), Some(edited));
    }

    #[cfg(unix)]
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
        assert_eq!(reviews.save(&db, path, &id, &scripts, true).unwrap_err(), REVIEWED_PROJECT_CHANGED);
        assert!(db.get_project_scripts(path).is_none());

        let fresh = review(&reviews, &db, &root);
        fs::remove_dir(&root).unwrap();
        std::os::unix::fs::symlink(&old, &root).unwrap();
        assert_eq!(reviews.save(&db, path, &fresh, &scripts, true).unwrap_err(), REVIEWED_PROJECT_CHANGED);
        assert!(db.get_project_scripts(old.to_str().unwrap()).is_none());
        assert!(db.get_setting(&format!("project.scripts:{path}")).is_none());
    }

    #[test]
    fn ordinary_settings_save_binds_the_first_selection() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("project");
        fs::create_dir(&root).unwrap();
        let root = dunce::canonicalize(root).unwrap();
        let path = root.to_str().unwrap();
        let db = crate::database::init_test_database();
        let reviews = ProjectScriptsReviews::default();
        let scripts = ProjectScripts {
            setup: vec!["setup fixture".into()],
            teardown: vec!["teardown fixture".into()],
            ..Default::default()
        };

        let empty = reviews.read(&db, path).unwrap();
        assert!(empty.review_root.is_none());
        fs::rename(&root, fixture.path().join("before-save")).unwrap();
        fs::create_dir(&root).unwrap();
        assert_eq!(
            reviews.save(&db, path, &empty.review_id, &scripts, false).unwrap_err(),
            REVIEWED_PROJECT_CHANGED,
        );
        assert!(db.get_project_scripts(path).is_none());

        let selected = reviews.read(&db, path).unwrap();
        reviews.save(&db, path, &selected.review_id, &scripts, false).unwrap();
        assert_eq!(db.get_project_scripts(path), Some(scripts.clone()));
        fs::rename(&root, fixture.path().join("after-save")).unwrap();
        fs::create_dir(&root).unwrap();
        assert!(db.get_project_scripts(path).is_none());
        let draft = reviews.read(&db, path).unwrap();
        assert_eq!(draft.review_root.as_deref(), Some(root.as_path()));
        assert_eq!(draft.scripts, scripts);
    }

    #[test]
    fn saved_scripts_without_identity_require_explicit_local_review() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        let path = root.to_str().unwrap();
        let db = crate::database::init_test_database();
        let scripts = ProjectScripts {
            setup: vec!["setup fixture".into()],
            ..Default::default()
        };
        db.set_setting(
            &format!("project.scripts:{path}"),
            &serde_json::to_string(&scripts).unwrap(),
        )
        .unwrap();
        assert!(db.get_project_scripts(path).is_none());
        let reviews = ProjectScriptsReviews::default();
        let preview = reviews.read(&db, path).unwrap();
        assert_eq!(preview.review_root.as_deref(), Some(root.as_path()));
        assert_eq!(preview.scripts, scripts);
        assert!(reviews.save(&db, path, &preview.review_id, &scripts, false).is_err());
        reviews.save(&db, path, &preview.review_id, &scripts, true).unwrap();
        assert_eq!(db.get_project_scripts(path), Some(scripts));
    }
}
