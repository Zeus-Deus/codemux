use crate::database::DatabaseStore;
use cap_std::{ambient_authority, fs::Dir};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};

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

/// The working directory and project root used to select local lifecycle
/// commands. Keep the directory handles alive until the runner finishes so
/// replacements cannot be mistaken for the directories we inspected.
pub(crate) struct LifecyclePaths {
    cwd: PathBuf,
    root: PathBuf,
    checkout: PathBuf,
    cwd_dir: File,
    root_dir: File,
}

impl LifecyclePaths {
    pub(crate) fn resolve(workspace_path: &Path) -> Result<Self, String> {
        let cwd = dunce::canonicalize(workspace_path)
            .map_err(|error| format!("Could not resolve workspace directory: {error}"))?;
        let cwd_dir = open_directory(&cwd)
            .map_err(|error| format!("Could not open workspace directory: {error}"))?;
        let (checkout, root) = resolve_project_paths(&cwd)
            .map_err(|error| format!("Could not resolve project directory: {error}"))?;
        let root_dir = open_directory(&root)
            .map_err(|error| format!("Could not open project directory: {error}"))?;
        let paths = Self { cwd, root, checkout, cwd_dir, root_dir };
        paths.check()?;
        Ok(paths)
    }

    pub(crate) fn workspace(&self) -> &Path {
        &self.cwd
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn check(&self) -> Result<(), String> {
        if !directory_unchanged(&self.cwd, &self.cwd_dir) {
            return Err("Workspace directory changed after lifecycle commands were selected".into());
        }
        if !directory_unchanged(&self.root, &self.root_dir)
            || !resolve_project_paths(&self.cwd).is_ok_and(|(checkout, root)| {
                checkout == self.checkout && root == self.root
            })
        {
            return Err("Project directory changed after lifecycle commands were selected".into());
        }
        Ok(())
    }

    #[cfg(unix)]
    pub(crate) fn spawn(&self, cmd: &mut Command) -> std::io::Result<Child> {
        use std::os::fd::AsRawFd;
        use std::os::unix::process::CommandExt;

        let cwd = self.cwd_dir.try_clone()?;
        // SAFETY: The descriptor stays open through the callback. The child
        // only calls async-signal-safe fchdir and reads errno on failure.
        unsafe {
            cmd.pre_exec(move || {
                if libc::fchdir(cwd.as_raw_fd()) == 0 {
                    Ok(())
                } else {
                    Err(std::io::Error::last_os_error())
                }
            });
        }
        cmd.spawn()
    }

    #[cfg(windows)]
    pub(crate) fn spawn(&self, cmd: &mut Command) -> std::io::Result<Child> {
        let _directories = lock_workspace_path(&self.cwd, &self.cwd_dir)?;
        cmd.current_dir(&self.cwd).spawn()
    }

    pub(crate) fn saved_config(&self, db: &DatabaseStore) -> Option<WorkspaceConfig> {
        read_saved_config_at(&self.checkout, &self.root, Some(&self.root_dir), db)
    }

    pub(crate) fn permits_include_copy(&self) -> bool {
        self.checkout == self.root || is_registered_checkout(&self.checkout, &self.root)
    }

    /// Repository configuration still supplies include patterns. Only
    /// commands explicitly saved in Settings can run automatically.
    pub(crate) fn automatic_config(&self, db: &DatabaseStore) -> WorkspaceConfig {
        let mut config = self.saved_config(db).unwrap_or_default();
        if let Some(file) = read_workspace_config(&self.cwd) {
            config.worktree_includes = file.worktree_includes;
        }
        config
    }
}

fn open_directory(path: &Path) -> std::io::Result<File> {
    Dir::open_ambient_dir(path, ambient_authority()).map(Dir::into_std_file)
}

fn directory_unchanged(path: &Path, held: &File) -> bool {
    let current = (|| {
        if dunce::canonicalize(path)? != path {
            return Ok(false);
        }
        same_directory(held, &open_directory(path)?)
    })();
    current.unwrap_or(false)
}

#[cfg(unix)]
fn same_directory(left: &File, right: &File) -> std::io::Result<bool> {
    use std::os::unix::fs::MetadataExt;

    let left = left.metadata()?;
    let right = right.metadata()?;
    Ok((left.dev(), left.ino()) == (right.dev(), right.ino()))
}

#[cfg(windows)]
fn same_directory(left: &File, right: &File) -> std::io::Result<bool> {
    use crate::windows_file_id::file_id;

    Ok(file_id(left)? == file_id(right)?)
}

#[cfg(windows)]
fn lock_workspace_path(path: &Path, expected: &File) -> std::io::Result<Vec<File>> {
    use std::fs::OpenOptions;
    use std::io::{Error, ErrorKind};
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_READ,
    };

    // CreateProcess takes a pathname. Hold each component from the volume
    // root down, excluding writers and renames, until that call returns.
    let mut ancestors: Vec<_> = path.ancestors().collect();
    let mut held = Vec::with_capacity(ancestors.len());
    let changed = || Error::other("Workspace directory changed before starting the command");
    while let Some(directory) = ancestors.pop() {
        let handle = OpenOptions::new()
            .access_mode(FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(directory)?;
        let metadata = handle.metadata()?;
        if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(changed());
        }
        held.push(handle);
    }
    let leaf = held.last().ok_or_else(|| Error::new(ErrorKind::InvalidInput, "Missing workspace directory"))?;
    if !same_directory(leaf, expected)? {
        return Err(changed());
    }
    Ok(held)
}

fn resolve_project_paths(cwd: &Path) -> std::io::Result<(PathBuf, PathBuf)> {
    let checkout = find_git_checkout(cwd).unwrap_or(cwd);
    let root = match resolve_worktree_git_file(&checkout.join(".git")) {
        Some(root) => dunce::canonicalize(root)?,
        None => checkout.to_path_buf(),
    };
    Ok((checkout.to_path_buf(), root))
}

pub(crate) fn read_saved_config(workspace_path: &Path, db: &DatabaseStore) -> Option<WorkspaceConfig> {
    let cwd = dunce::canonicalize(workspace_path).ok()?;
    if !cwd.is_dir() {
        return None;
    }
    let (checkout, root) = resolve_project_paths(&cwd).ok()?;
    read_saved_config_at(&checkout, &root, None, db)
}

fn read_saved_config_at(
    checkout: &Path,
    root: &Path,
    held_root: Option<&File>,
    db: &DatabaseStore,
) -> Option<WorkspaceConfig> {
    let scripts = db.get_project_scripts_at(root, held_root)?;
    if checkout != root && !is_registered_checkout(checkout, root) {
        return None;
    }
    Some(saved_workspace_config(scripts))
}

fn is_registered_checkout(checkout: &Path, root: &Path) -> bool {
    // A .git file alone cannot authorize access to a different project.
    crate::git::git_list_worktrees(root).is_ok_and(|worktrees| {
        worktrees.into_iter().any(|worktree| {
            !worktree.is_bare
                && dunce::canonicalize(&worktree.path).is_ok_and(|path| path == checkout)
        })
    })
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

    #[cfg(windows)]
    #[test]
    fn windows_workspace_launch_locks_ancestors_and_refuses_a_replaced_directory() {
        let fixture = tempfile::tempdir().unwrap();
        let ancestor = fixture.path().join("project");
        let cwd = ancestor.join("workspace");
        let moved = fixture.path().join("previous");
        fs::create_dir_all(&cwd).unwrap();
        let paths = LifecyclePaths::resolve(&cwd).unwrap();
        let held = lock_workspace_path(&paths.cwd, &paths.cwd_dir).unwrap();
        assert!(fs::rename(&ancestor, &moved).is_err());
        drop(held);

        fs::rename(&ancestor, &moved).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        assert!(lock_workspace_path(&paths.cwd, &paths.cwd_dir).is_err());
    }

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
        let missing = dir.path().join("missing");
        fs::write(
            dir.path().join(".git"),
            format!("gitdir: {}", missing.join(".git/worktrees/feat").display()),
        )
        .unwrap();

        // Should parse the path even if the target doesn't exist
        let result = find_git_root(dir.path());
        assert_eq!(result, Some(missing));
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

        let automatic = LifecyclePaths::resolve(dir.path()).unwrap().automatic_config(&db);
        assert!(automatic.setup.is_empty());
        assert!(automatic.teardown.is_empty());
        assert_eq!(automatic.worktree_includes, ["file-pattern"]);

        db.set_project_scripts(&dir.path().to_string_lossy(), &crate::database::ProjectScripts {
            setup: vec!["local-setup".into()],
            teardown: vec!["local-teardown".into()],
            ..Default::default()
        }).unwrap();
        let automatic = LifecyclePaths::resolve(dir.path()).unwrap().automatic_config(&db);
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
