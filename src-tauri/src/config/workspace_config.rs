use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkspaceConfig {
    #[serde(default)]
    pub setup: Vec<String>,
    #[serde(default)]
    pub teardown: Vec<String>,
}

/// Find the git repository root by walking up from `path`.
/// For worktrees (where `.git` is a file pointing to the main repo),
/// this follows the pointer to return the actual repo root.
pub fn find_git_root(path: &Path) -> Option<PathBuf> {
    let mut current = path.to_path_buf();
    loop {
        let git_path = current.join(".git");
        if git_path.is_dir() {
            // Real repo root
            return Some(current);
        }
        if git_path.is_file() {
            // Worktree: .git file contains "gitdir: /path/to/main/.git/worktrees/<name>"
            if let Some(repo_root) = resolve_worktree_git_file(&git_path) {
                return Some(repo_root);
            }
            // If we can't parse it, treat this directory as the root
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

/// Parse a worktree `.git` file and resolve to the main repo root.
fn resolve_worktree_git_file(git_file: &Path) -> Option<PathBuf> {
    let content = std::fs::read_to_string(git_file).ok()?;
    let gitdir = content.strip_prefix("gitdir: ")?.trim();
    // gitdir is like "/path/to/main/.git/worktrees/<name>"
    // Walk up: worktrees/ -> .git/ -> repo root
    PathBuf::from(gitdir)
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
