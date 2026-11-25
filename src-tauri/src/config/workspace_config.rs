use crate::database::{DatabaseStore, ProjectDirectoryId};
#[cfg(unix)]
use cap_std::ambient_authority;
use cap_std::fs::Dir;
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
    checkout_dir: Option<File>,
    worktree_id: Option<LifecycleWorktreeId>,
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct LifecycleWorktreeId {
    workspace: ProjectDirectoryId,
    checkout: ProjectDirectoryId,
    project: ProjectDirectoryId,
    registration: ProjectDirectoryId,
}

const UNVERIFIED_WORKTREE: &str =
    "Worktree identity could not be verified for automatic workspace actions";

pub(crate) struct LifecycleIncludeDirectories {
    pub(crate) source: Dir,
    pub(crate) destination: Dir,
    #[cfg(windows)]
    _source_path_lock: WorkspacePathLock,
    #[cfg(windows)]
    _destination_path_lock: WorkspacePathLock,
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
        let checkout_dir = (checkout != cwd && checkout != root)
            .then(|| open_directory(&checkout))
            .transpose()
            .map_err(|error| format!("Could not open linked checkout directory: {error}"))?;
        let paths = Self { cwd, root, checkout, cwd_dir, root_dir, checkout_dir, worktree_id: None };
        paths.check()?;
        Ok(paths)
    }

    pub(crate) fn resolve_linked_checkout(workspace_path: &Path) -> Result<Self, String> {
        let paths = Self::resolve(workspace_path)?;
        if paths.checkout == paths.root {
            return Err(UNVERIFIED_WORKTREE.into());
        }
        Ok(paths)
    }

    pub(crate) fn resolve_fresh_worktree(
        workspace_path: &Path,
        expected_project: &Path,
    ) -> Result<Self, String> {
        let mut paths = Self::resolve_linked_checkout(workspace_path)?;
        let expected_root = dunce::canonicalize(expected_project)
            .ok()
            .and_then(|path| resolve_project_paths(&path).ok())
            .map(|(_, root)| root);
        if expected_root.as_deref() != Some(paths.root()) {
            return Err(UNVERIFIED_WORKTREE.into());
        }
        paths.worktree_id = Some(paths.verified_worktree_id().ok_or(UNVERIFIED_WORKTREE)?);
        paths.check()?;
        Ok(paths)
    }

    pub(crate) fn resolve_for_workspace(
        workspace_path: &Path,
        db: &DatabaseStore,
        workspace_id: &str,
        expects_worktree: bool,
    ) -> Result<Self, String> {
        let mut paths = Self::resolve(workspace_path)?;
        paths.worktree_id = db.lifecycle_worktree_id(workspace_id)?;
        if paths.worktree_id.is_none() && (expects_worktree || paths.checkout != paths.root) {
            return Err(UNVERIFIED_WORKTREE.into());
        }
        paths.check()?;
        Ok(paths)
    }

    /// Called only for a newly created local workspace, including one which
    /// adopts an existing linked checkout. A persisted binding is never replaced.
    pub(crate) fn bind_to_workspace(&mut self, db: &DatabaseStore, workspace_id: &str) -> Result<(), String> {
        self.check()?;
        if self.worktree_id.is_none() && self.checkout != self.root {
            self.worktree_id = Some(self.verified_worktree_id().ok_or(UNVERIFIED_WORKTREE)?);
        }
        if let Some(identity) = &self.worktree_id {
            db.bind_lifecycle_worktree(workspace_id, identity)?;
        }
        self.worktree_id = db.lifecycle_worktree_id(workspace_id)?;
        self.check()
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
        if let Some(expected) = &self.worktree_id {
            if self.verified_worktree_id().as_ref() != Some(expected) {
                return Err(UNVERIFIED_WORKTREE.into());
            }
        }
        Ok(())
    }

    fn verified_worktree_id(&self) -> Option<LifecycleWorktreeId> {
        if self.checkout == self.root {
            return None;
        }
        let checkout_dir = self.checkout_dir.as_ref().unwrap_or(&self.cwd_dir);
        if !directory_unchanged(&self.checkout, checkout_dir) {
            return None;
        }
        let registration = registered_checkout(&self.checkout, &self.root)?;
        Some(LifecycleWorktreeId {
            workspace: ProjectDirectoryId::from_file(&self.cwd_dir)?,
            checkout: ProjectDirectoryId::from_file(checkout_dir)?,
            project: ProjectDirectoryId::from_file(&self.root_dir)?,
            registration: ProjectDirectoryId::from_file(&open_directory(&registration).ok()?)?,
        })
    }

    #[cfg(unix)]
    pub(crate) fn spawn(&self, cmd: &mut Command) -> std::io::Result<Child> {
        spawn_in_directory(cmd, self.cwd_dir.try_clone()?)
    }

    #[cfg(windows)]
    pub(crate) fn spawn(&self, cmd: &mut Command) -> std::io::Result<Child> {
        let _directories = lock_workspace_path(&self.cwd, &self.cwd_dir)?;
        cmd.current_dir(&self.cwd).spawn()
    }

    pub(crate) fn saved_config(&self, db: &DatabaseStore) -> Option<WorkspaceConfig> {
        if self.checkout != self.root && self.worktree_id.is_none() {
            return None;
        }
        self.check().ok()?;
        db.get_project_scripts_at(&self.root, Some(&self.root_dir)).map(saved_workspace_config)
    }

    pub(crate) fn permits_include_copy(&self) -> bool {
        self.cwd == self.checkout && self.worktree_id.is_some() && self.check().is_ok()
    }

    #[cfg(unix)]
    pub(crate) fn include_directories(&self) -> std::io::Result<LifecycleIncludeDirectories> {
        Ok(LifecycleIncludeDirectories {
            source: Dir::from_std_file(self.root_dir.try_clone()?),
            destination: Dir::from_std_file(self.cwd_dir.try_clone()?),
        })
    }

    #[cfg(windows)]
    pub(crate) fn include_directories(&self) -> std::io::Result<LifecycleIncludeDirectories> {
        use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

        // Windows capability filesystem operations use paths internally.
        // Keep names locked through the copy. Publishing a file also needs
        // write sharing on the destination directory.
        let source_path_lock = lock_workspace_path(&self.root, &self.root_dir)?;
        let destination_path_lock = lock_workspace_path_with_leaf_sharing(
            &self.cwd, &self.cwd_dir, FILE_SHARE_READ | FILE_SHARE_WRITE,
        )?;
        Ok(LifecycleIncludeDirectories {
            source: Dir::from_std_file(source_path_lock.leaf.try_clone()?),
            destination: Dir::from_std_file(destination_path_lock.leaf.try_clone()?),
            _source_path_lock: source_path_lock,
            _destination_path_lock: destination_path_lock,
        })
    }

    pub(crate) fn spawn_in_include_source(
        &self,
        cmd: &mut Command,
        directories: &LifecycleIncludeDirectories,
    ) -> std::io::Result<Child> {
        #[cfg(unix)]
        return spawn_in_directory(cmd, directories.source.try_clone()?.into_std_file());

        #[cfg(windows)]
        {
            let _ = directories;
            cmd.current_dir(&self.root).spawn()
        }
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

#[cfg(unix)]
fn spawn_in_directory(cmd: &mut Command, directory: File) -> std::io::Result<Child> {
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;

    // SAFETY: The descriptor stays open through the callback. The child only
    // calls async-signal-safe fchdir and reads errno on failure.
    unsafe {
        cmd.pre_exec(move || {
            if libc::fchdir(directory.as_raw_fd()) == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
    cmd.spawn()
}

#[cfg(unix)]
fn open_directory(path: &Path) -> std::io::Result<File> {
    Dir::open_ambient_dir(path, ambient_authority()).map(Dir::into_std_file)
}

#[cfg(windows)]
fn open_directory(path: &Path) -> std::io::Result<File> {
    use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE};

    // Lifecycle paths are captured before background setup starts. These
    // handles track identity without blocking a concurrent workspace close.
    open_windows_directory(path, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
}

#[cfg(windows)]
fn open_windows_directory(path: &Path, share_mode: u32) -> std::io::Result<File> {
    use std::fs::OpenOptions;
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES,
    };

    let handle = OpenOptions::new()
        .access_mode(FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES)
        .share_mode(share_mode)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let metadata = handle.metadata()?;
    if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(std::io::Error::other("Expected a directory without a reparse point"));
    }
    Ok(handle)
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
struct WorkspacePathLock {
    _ancestors: Vec<File>,
    leaf: File,
}

#[cfg(windows)]
fn lock_workspace_path(path: &Path, expected: &File) -> std::io::Result<WorkspacePathLock> {
    use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

    lock_workspace_path_with_leaf_sharing(path, expected, FILE_SHARE_READ)
}

#[cfg(windows)]
fn lock_workspace_path_with_leaf_sharing(
    path: &Path,
    expected: &File,
    leaf_sharing: u32,
) -> std::io::Result<WorkspacePathLock> {
    use std::io::{Error, ErrorKind};
    use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

    // CreateProcess takes a pathname. Hold each component from the volume
    // root down, excluding writers and renames, until that call returns.
    let mut ancestors: Vec<_> = path.ancestors().collect();
    let mut held = Vec::with_capacity(ancestors.len());
    let changed = || Error::other("Workspace directory changed before starting the command");
    while let Some(directory) = ancestors.pop() {
        let sharing = if ancestors.is_empty() { leaf_sharing } else { FILE_SHARE_READ };
        held.push(open_windows_directory(directory, sharing)?);
    }
    let leaf = held.pop().ok_or_else(|| Error::new(ErrorKind::InvalidInput, "Missing workspace directory"))?;
    if !same_directory(&leaf, expected)? {
        return Err(changed());
    }
    Ok(WorkspacePathLock { _ancestors: held, leaf })
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
    if checkout != root {
        registered_checkout(&checkout, &root)?;
    }
    db.get_project_scripts_at(&root, None).map(saved_workspace_config)
}

fn read_git_link(path: &Path, prefix: &str) -> Option<PathBuf> {
    if !std::fs::symlink_metadata(path).ok()?.is_file() {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let target = text.strip_prefix(prefix)?.trim_end_matches(['\r', '\n']);
    (!target.is_empty()).then(|| PathBuf::from(target))
}

fn registered_checkout(checkout: &Path, root: &Path) -> Option<PathBuf> {
    let marker = checkout.join(".git");
    let registration = dunce::canonicalize(checkout.join(read_git_link(&marker, "gitdir: ")?)).ok()?;
    let git_dir = root.join(".git");
    let registrations = git_dir.join("worktrees");
    if registration.parent()? != registrations
        || dunce::canonicalize(&registrations).ok()? != registrations
    {
        return None;
    }
    let back_link = read_git_link(&registration.join("gitdir"), "")?;
    let common = read_git_link(&registration.join("commondir"), "")?;
    if dunce::canonicalize(registration.join(back_link)).ok()? != marker
        || dunce::canonicalize(registration.join(common)).ok()? != git_dir
        || !crate::git::lists_active_worktree(&git_dir, checkout)
    {
        return None;
    }
    Some(registration)
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
        let path = paths.cwd.clone();
        drop(paths);

        fs::rename(&ancestor, &moved).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        let original = open_directory(&moved.join("workspace")).unwrap();
        assert!(lock_workspace_path(&path, &original).is_err());
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
            assert!(db.get_project_scripts(&git_style).is_none());
            assert!(read_saved_config(&nested, &db).is_none());
            let (draft, _, needs_review) = db.project_scripts_for_settings(&git_style).unwrap();
            assert_eq!(draft, scripts);
            assert!(needs_review);

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

        assert!(LifecyclePaths::resolve_for_workspace(&nested, &db, "workspace", true).is_err());
        LifecyclePaths::resolve(&nested)
            .unwrap()
            .bind_to_workspace(&db, "workspace")
            .unwrap();
        let restored = LifecyclePaths::resolve_for_workspace(&nested, &db, "workspace", true)
            .unwrap()
            .saved_config(&db)
            .unwrap();
        assert_eq!(restored.setup, scripts.setup);
        assert_eq!(restored.teardown, scripts.teardown);

        // Another valid entry in the same repo cannot lend its Git pointer.
        let marker = fs::read(linked.join(".git")).unwrap();
        let other = project.join("other");
        run_git(&["worktree", "add", "--detach", other.to_str().unwrap()]);
        fs::copy(other.join(".git"), linked.join(".git")).unwrap();
        assert!(read_saved_config(&nested, &db).is_none());
        assert!(LifecyclePaths::resolve_for_workspace(&nested, &db, "workspace", true).is_err());
        fs::write(linked.join(".git"), &marker).unwrap();

        // Git still accepts this path when a directory returns with the
        // original Git pointer. Automatic commands also require its identity.
        let moved = project.join("relocated");
        fs::rename(&linked, &moved).unwrap();
        assert!(read_saved_config(&moved.join("src/feature"), &db).is_none());
        fs::create_dir_all(&nested).unwrap();
        fs::write(linked.join(".git"), marker).unwrap();
        assert!(read_saved_config(&nested, &db).is_some());
        assert!(LifecyclePaths::resolve_for_workspace(&nested, &db, "workspace", true).is_err());
        assert!(LifecyclePaths::resolve(&nested)
            .unwrap()
            .bind_to_workspace(&db, "workspace")
            .is_err());
        assert!(LifecyclePaths::resolve_for_workspace(&nested, &db, "workspace", true).is_err());
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
