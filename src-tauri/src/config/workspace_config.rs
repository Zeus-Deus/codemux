use crate::database::DatabaseStore;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkspaceConfig {
    #[serde(default)]
    pub setup: Vec<String>,
    #[serde(default)]
    pub teardown: Vec<String>,
    #[serde(default)]
    pub run: Option<String>,
    #[serde(default)]
    pub worktree_includes: Vec<String>,
}

/// Find the git repository root by walking up from `path`.
/// For worktrees (where `.git` is a file pointing to the main repo),
/// this follows the pointer to return the actual repo root.
pub fn find_git_root(path: &Path) -> Option<PathBuf> {
    let checkout = find_git_checkout(path)?;
    Some(resolve_worktree_git_file(&checkout.join(".git")).unwrap_or_else(|| checkout.to_path_buf()))
}

/// The nearest checkout's own directory, before resolving a linked worktree.
fn find_git_checkout(path: &Path) -> Option<&Path> {
    path.ancestors().find(|dir| {
        let marker = dir.join(".git");
        marker.is_dir() || marker.is_file()
    })
}

/// Parse a worktree `.git` file and resolve to the main repo root.
fn resolve_worktree_git_file(git_file: &Path) -> Option<PathBuf> {
    if !git_file.is_file() {
        return None;
    }
    let content = std::fs::read_to_string(git_file).ok()?;
    let gitdir = content.strip_prefix("gitdir: ")?.trim();
    if gitdir.is_empty() {
        return None;
    }
    // gitdir is like "/path/to/main/.git/worktrees/<name>"
    // Walk up: worktrees/ -> .git/ -> repo root
    git_file.parent()?.join(gitdir)
        .parent() // worktrees/
        .and_then(|p| p.parent()) // .git/
        .and_then(|p| p.parent()) // repo root
        .map(|p| p.to_path_buf())
}

/// Read `.codemux/config.json` from the workspace directory or git repo root.
pub fn read_workspace_config(workspace_path: &Path) -> Option<WorkspaceConfig> {
    // Try workspace directory first
    let config_path = workspace_path.join(".codemux/config.json");
    if let Some(config) = try_read_config(&config_path) {
        return Some(config);
    }

    // Fall back to git repo root
    if let Some(repo_root) = find_git_root(workspace_path) {
        if repo_root != workspace_path {
            let root_config_path = repo_root.join(".codemux/config.json");
            if let Some(config) = try_read_config(&root_config_path) {
                return Some(config);
            }
        }
    }

    None
}

/// Read config with DB fallback: file config takes precedence over DB-stored scripts.
pub fn read_effective_config(
    workspace_path: &Path,
    db: &DatabaseStore,
) -> Option<WorkspaceConfig> {
    // File config wins
    if let Some(config) = read_workspace_config(workspace_path) {
        return Some(config);
    }

    read_saved_config(workspace_path, db)
}

/// Lifecycle events may be triggered by opening an untrusted checkout.
/// Only commands explicitly saved in Settings can run automatically.
/// The file still supplies include patterns when present.
pub fn read_automatic_lifecycle_config(
    workspace_path: &Path,
    db: &DatabaseStore,
) -> WorkspaceConfig {
    let mut config = read_saved_config(workspace_path, db).unwrap_or_default();
    if let Some(file) = read_workspace_config(workspace_path) {
        config.worktree_includes = file.worktree_includes;
    }
    config
}

pub(crate) fn read_saved_config(workspace_path: &Path, db: &DatabaseStore) -> Option<WorkspaceConfig> {
    let cwd = dunce::canonicalize(workspace_path).ok()?;
    if !cwd.is_dir() {
        return None;
    }
    let checkout = find_git_checkout(&cwd).unwrap_or(&cwd);
    let Some(root) = resolve_worktree_git_file(&checkout.join(".git")) else {
        return db.get_project_scripts_at(checkout).map(saved_workspace_config);
    };
    let root = dunce::canonicalize(root).ok()?;
    let scripts = db.get_project_scripts_at(&root)?;
    // A .git file is untrusted. Even a linked directory inside the main
    // tree must be registered as this project's worktree to inherit consent.
    if checkout != root
        && !crate::git::git_list_worktrees(&root)
            .ok()?
            .into_iter()
            .filter(|worktree| !worktree.is_bare)
            .any(|worktree| dunce::canonicalize(&worktree.path).is_ok_and(|path| path == checkout))
    {
        return None;
    }
    Some(saved_workspace_config(scripts))
}

fn saved_workspace_config(scripts: crate::database::ProjectScripts) -> WorkspaceConfig {
    WorkspaceConfig {
        setup: scripts.setup,
        teardown: scripts.teardown,
        run: scripts.run,
        worktree_includes: scripts.worktree_includes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_read_config_from_workspace_dir() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join(".codemux");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_dir.join("config.json"),
            r#"{"setup": ["npm install"], "teardown": ["echo bye"]}"#,
        )
        .unwrap();

        let config = read_workspace_config(dir.path()).unwrap();
        assert_eq!(config.setup, vec!["npm install"]);
        assert_eq!(config.teardown, vec!["echo bye"]);
    }

    #[test]
    fn test_read_config_returns_none_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_workspace_config(dir.path()).is_none());
    }

    #[test]
    fn test_read_config_fallback_to_repo_root() {
        // Simulate a worktree: create a main repo with .git dir + config,
        // and a worktree dir with .git file pointing to the main repo.
        let main_repo = tempfile::tempdir().unwrap();
        let worktree_dir = tempfile::tempdir().unwrap();

        // Main repo: real .git directory
        fs::create_dir_all(main_repo.path().join(".git/worktrees/my-branch")).unwrap();

        // Main repo: .codemux/config.json
        let config_dir = main_repo.path().join(".codemux");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_dir.join("config.json"),
            r#"{"setup": ["cargo build"]}"#,
        )
        .unwrap();

        // Worktree: .git file pointing to main repo
        let gitdir = main_repo.path().join(".git/worktrees/my-branch");
        fs::write(
            worktree_dir.path().join(".git"),
            format!("gitdir: {}", gitdir.display()),
        )
        .unwrap();

        let config = read_workspace_config(worktree_dir.path()).unwrap();
        assert_eq!(config.setup, vec!["cargo build"]);
    }

    #[test]
    fn test_find_git_root_regular_repo() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("a/b/c");
        fs::create_dir_all(&sub).unwrap();
        fs::create_dir_all(dir.path().join(".git")).unwrap();

        assert_eq!(find_git_root(&sub), Some(dir.path().to_path_buf()));
    }

    #[test]
    fn test_find_git_root_worktree() {
        let main_repo = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();

        fs::create_dir_all(main_repo.path().join(".git/worktrees/feat")).unwrap();
        let gitdir = main_repo.path().join(".git/worktrees/feat");
        fs::write(
            worktree.path().join(".git"),
            format!("gitdir: {}", gitdir.display()),
        )
        .unwrap();

        assert_eq!(find_git_root(worktree.path()), Some(main_repo.path().to_path_buf()));
    }

    #[test]
    fn test_find_git_root_corrupted_git_file() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".git"), "this is garbage data").unwrap();

        // Should not crash, should fall back to treating directory as root
        let result = find_git_root(dir.path());
        assert_eq!(result, Some(dir.path().to_path_buf()));
    }

    #[test]
    fn test_find_git_root_missing_target() {
        let dir = tempfile::tempdir().unwrap();
        // .git file points to a path that doesn't exist on disk
        fs::write(
            dir.path().join(".git"),
            "gitdir: /nonexistent/path/.git/worktrees/feat",
        )
        .unwrap();

        // Should parse the path even if the target doesn't exist
        let result = find_git_root(dir.path());
        assert_eq!(result, Some(PathBuf::from("/nonexistent/path")));
    }

    #[test]
    fn test_find_git_root_no_git_at_all() {
        let dir = tempfile::tempdir().unwrap();
        // No .git file or directory anywhere

        let result = find_git_root(dir.path());
        assert_eq!(result, None);
    }

    #[test]
    fn test_read_config_with_run_field() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join(".codemux");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_dir.join("config.json"),
            r#"{"setup": ["npm install"], "teardown": [], "run": "npm run dev"}"#,
        )
        .unwrap();

        let config = read_workspace_config(dir.path()).unwrap();
        assert_eq!(config.setup, vec!["npm install"]);
        assert_eq!(config.run, Some("npm run dev".to_string()));
    }

    #[test]
    fn test_read_config_without_run_field() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join(".codemux");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_dir.join("config.json"),
            r#"{"setup": ["npm install"], "teardown": ["echo bye"]}"#,
        )
        .unwrap();

        let config = read_workspace_config(dir.path()).unwrap();
        assert_eq!(config.run, None);
    }

    #[test]
    fn test_config_file_precedence_over_db() {
        let db = crate::database::init_test_database();

        // Save scripts to DB
        let db_scripts = crate::database::ProjectScripts {
            setup: vec!["db-setup-cmd".into()],
            teardown: vec!["db-teardown-cmd".into()],
            run: Some("db-run-cmd".into()),
            ..Default::default()
        };
        let dir = tempfile::tempdir().unwrap();
        db.set_project_scripts(&dir.path().to_string_lossy(), &db_scripts)
            .unwrap();

        // Also create .codemux/config.json with different values
        let config_dir = dir.path().join(".codemux");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_dir.join("config.json"),
            r#"{"setup": ["file-setup-cmd"], "teardown": ["file-teardown-cmd"], "run": "file-run-cmd"}"#,
        )
        .unwrap();

        // read_effective_config should return the file config, not DB
        let config = read_effective_config(dir.path(), &db).unwrap();
        assert_eq!(config.setup, vec!["file-setup-cmd"]);
        assert_eq!(config.teardown, vec!["file-teardown-cmd"]);
        assert_eq!(config.run, Some("file-run-cmd".into()));
    }

    #[test]
    fn automatic_lifecycle_requires_commands_saved_on_this_device() {
        let db = crate::database::init_test_database();
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join(".codemux");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_dir.join("config.json"),
            r#"{"setup":["repo-setup"],"teardown":["repo-teardown"],"worktree_includes":["file-pattern"]}"#,
        )
        .unwrap();

        let automatic = read_automatic_lifecycle_config(dir.path(), &db);
        assert!(automatic.setup.is_empty());
        assert!(automatic.teardown.is_empty());
        assert_eq!(automatic.worktree_includes, ["file-pattern"]);

        db.set_project_scripts(&dir.path().to_string_lossy(), &crate::database::ProjectScripts {
            setup: vec!["local-setup".into()],
            teardown: vec!["local-teardown".into()],
            ..Default::default()
        }).unwrap();
        let automatic = read_automatic_lifecycle_config(dir.path(), &db);
        assert_eq!(automatic.setup, ["local-setup"]);
        assert_eq!(automatic.teardown, ["local-teardown"]);
        assert_eq!(automatic.worktree_includes, ["file-pattern"]);

        let explicit = read_effective_config(dir.path(), &db).unwrap();
        assert_eq!(explicit.setup, ["repo-setup"]);
    }

    #[test]
    fn saved_commands_follow_registered_worktree_subdirectories() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        fs::create_dir(&project).unwrap();
        let project = dunce::canonicalize(project).unwrap();
        let run_git = |args: &[&str]| {
            let output = crate::git::git_command()
                .arg("-C")
                .arg(&project)
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
        };
        run_git(&["init", "--initial-branch=main"]);
        run_git(&[
            "-c", "user.name=Fixture", "-c", "user.email=fixture@example.test", "-c", "commit.gpgsign=false",
            "commit", "--allow-empty", "-m", "initial",
        ]);
        // Put the linked checkout inside the main tree so registration, not
        // lexical containment, decides whether it can inherit local scripts.
        let linked = project.join("linked");
        run_git(&["worktree", "add", "-b", "fixture", linked.to_str().unwrap()]);
        let nested = linked.join("src").join("feature");
        fs::create_dir_all(&nested).unwrap();

        let db = crate::database::init_test_database();
        let scripts = crate::database::ProjectScripts {
            setup: vec!["fixture-setup".into()],
            teardown: vec!["fixture-teardown".into()],
            ..Default::default()
        };
        #[cfg(windows)]
        {
            let native = project.to_str().unwrap();
            let git_style = native.replace('\\', "/");
            let verbatim = project.canonicalize().unwrap();
            let verbatim = verbatim.to_str().unwrap();
            let legacy = serde_json::to_string(&scripts).unwrap();
            db.set_setting(&format!("project.scripts:{verbatim}"), &legacy).unwrap();
            assert_eq!(db.get_project_scripts(&git_style), Some(scripts.clone()));
            assert_eq!(read_saved_config(&nested, &db).unwrap().setup, scripts.setup);

            let conflicting = serde_json::to_string(&crate::database::ProjectScripts::default()).unwrap();
            db.set_setting(&format!("project.scripts:{git_style}"), &conflicting).unwrap();
            assert!(read_saved_config(&nested, &db).is_none());
            db.set_project_scripts(verbatim, &scripts).unwrap();
            assert_eq!(db.get_project_scripts(&git_style), Some(scripts.clone()));
            assert_eq!(db.get_project_scripts(native), Some(scripts.clone()));
        }
        #[cfg(not(windows))]
        db.set_project_scripts(project.to_str().unwrap(), &scripts).unwrap();

        let inherited = read_saved_config(&nested, &db).unwrap();
        assert_eq!(inherited.setup, scripts.setup);
        assert_eq!(inherited.teardown, scripts.teardown);

        // Moving the checkout by hand leaves Git's registration at its old
        // path, so it cannot use the main project's automatic commands.
        let moved = project.join("relocated");
        fs::rename(&linked, &moved).unwrap();
        assert!(read_saved_config(&moved.join("src/feature"), &db).is_none());
    }
}

fn try_read_config(path: &Path) -> Option<WorkspaceConfig> {
    let contents = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return None,
    };

    match serde_json::from_str::<WorkspaceConfig>(&contents) {
        Ok(config) => Some(config),
        Err(e) => {
            eprintln!(
                "[codemux::config] Failed to parse {}: {e}",
                path.display()
            );
            None
        }
    }
}
