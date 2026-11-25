use crate::config::workspace_config::{
    read_effective_config, read_workspace_config, WorkspaceConfig,
};
use crate::database::DatabaseStore;
use crate::workspace_service::Checkpoint;
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, File, OpenOptions};
use serde::Serialize;
use std::io::{self, ErrorKind, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Output, Stdio};
use tauri::{AppHandle, Emitter, Runtime};

/// Default worktree include patterns when no `.codemuxinclude` file or project
/// setting is configured.
pub const DEFAULT_WORKTREE_INCLUDES: &[&str] = &[".env", ".env.*", ".env.local"];

#[derive(Debug, Clone, Serialize)]
struct SetupProgress {
    workspace_id: String,
    command: String,
    index: usize,
    total: usize,
}

#[derive(Debug, Clone, Serialize)]
struct SetupFailed {
    workspace_id: String,
    command: String,
    stdout: String,
    stderr: String,
    exit_code: Option<i32>,
}

#[derive(Debug, Clone, Serialize)]
struct SetupComplete {
    workspace_id: String,
}

/// Build the common environment variables for setup/teardown/run scripts
/// and terminal PTY sessions.
pub fn script_env(
    workspace_path: &Path,
    root_path: &Path,
    branch: Option<&str>,
    port: Option<u16>,
) -> Vec<(&'static str, String)> {
    let mut vars = vec![
        ("CODEMUX_ROOT_PATH", root_path.to_string_lossy().to_string()),
        (
            "CODEMUX_WORKSPACE_PATH",
            workspace_path.to_string_lossy().to_string(),
        ),
    ];
    if let Some(branch) = branch {
        vars.push(("CODEMUX_BRANCH", branch.to_string()));
    }
    if let Some(port) = port {
        vars.push(("CODEMUX_PORT", port.to_string()));
    }
    vars
}

/// Allocate a stable port for a workspace based on a hash of its ID.
/// Uses a range of 3100–6499 (340 slots of 10 ports each).
/// The port is deterministic for a given workspace ID and will not change
/// if other workspaces are created or deleted.
pub fn allocate_workspace_port(workspace_id: &str) -> u16 {
    const BASE_PORT: u16 = 3100;
    const PORT_STRIDE: u16 = 10;
    const SLOT_COUNT: u16 = 340; // (65535 - 3100) / 10, capped to stay in user range

    // Simple FNV-1a-inspired hash for deterministic port assignment
    let mut hash: u32 = 2166136261;
    for byte in workspace_id.as_bytes() {
        hash ^= *byte as u32;
        hash = hash.wrapping_mul(16777619);
    }

    let slot = (hash % SLOT_COUNT as u32) as u16;
    BASE_PORT + slot * PORT_STRIDE
}

/// Which source provided the worktree include patterns.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum IncludeSource {
    File,
    Setting,
    Defaults,
}

#[derive(Debug, Clone, Serialize)]
pub struct IncludeResult {
    pub copied: Vec<String>,
    pub skipped_nested: Vec<String>,
    pub source: IncludeSource,
}

/// Process worktree includes: copy gitignored files matching patterns from the
/// main worktree into the new worktree.
///
/// Pattern source priority:
///   1. `.codemuxinclude` file in project root
///   2. `worktree_includes` from project settings
///   3. Hardcoded defaults (`DEFAULT_WORKTREE_INCLUDES`)
///
/// Automatically copy top-level files into a distinct registered checkout.
pub fn process_worktree_includes(
    root_path: &Path,
    worktree_path: &Path,
    setting_patterns: &[String],
) -> Result<IncludeResult, String> {
    process_registered_worktree_includes(root_path, worktree_path, setting_patterns, false)
}

fn process_registered_worktree_includes(
    root_path: &Path,
    worktree_path: &Path,
    setting_patterns: &[String],
    include_nested: bool,
) -> Result<IncludeResult, String> {
    if let Some((source, destination)) =
        open_registered_include_dirs(root_path, worktree_path, include_nested)
    {
        return copy_worktree_includes(&source, &destination, setting_patterns, include_nested);
    }
    Ok(IncludeResult {
        copied: Vec::new(),
        skipped_nested: Vec::new(),
        source: IncludeSource::Defaults,
    })
}

fn open_registered_include_dirs(
    root: &Path,
    workspace: &Path,
    include_nested: bool,
) -> Option<(IncludeSourceDir, Dir)> {
    let root = root.canonicalize().ok()?;
    let workspace = workspace.canonicalize().ok()?;
    let checkout_path = nearest_checkout(&workspace)?;
    if checkout_path == root {
        return None;
    }
    let relative = workspace.strip_prefix(checkout_path).ok()?;
    if !include_nested && !relative.as_os_str().is_empty() {
        return None;
    }
    let source = IncludeSourceDir::open(&root).ok()?;
    let checkout = Dir::open_ambient_dir(checkout_path, cap_std::ambient_authority()).ok()?;
    let destination = open_include_parent(&checkout, relative, false).ok()?;
    verify_include_registration(&source, &checkout, checkout_path)?;
    Some((source, destination))
}

/// A worktree's gitfile and the source's worktree registration must point
/// to each other. Read the records through the directories used for I/O.
fn verify_include_registration(
    source: &IncludeSourceDir,
    checkout: &Dir,
    path: &Path,
) -> Option<()> {
    let gitdir = read_registration_path(checkout, ".git", b"gitdir: ")?;
    let common = source.dir.open_dir_nofollow(".git").ok()?;
    let admin = common
        .open_dir_nofollow("worktrees")
        .ok()?
        .open_dir_nofollow(gitdir.file_name()?)
        .ok()?;

    let admin_path = path.join(gitdir);
    if !directory_matches(&admin, &admin_path) {
        return None;
    }
    let commondir = read_registration_path(&admin, "commondir", b"")?;
    if !directory_matches(&common, &admin_path.join(commondir)) {
        return None;
    }
    let backlink = admin_path.join(read_registration_path(&admin, "gitdir", b"")?);
    let registered_path = backlink.parent()?.canonicalize().ok()?;
    (backlink.file_name()? == ".git"
        && registered_path == path
        && directory_matches(checkout, &registered_path))
    .then_some(())
}

fn read_registration_path(dir: &Dir, name: &str, prefix: &[u8]) -> Option<PathBuf> {
    const MAX_GIT_PATH_RECORD: u64 = 1024 * 1024;
    let mut bytes = Vec::new();
    open_regular_include(dir, Path::new(name))
        .ok()??
        .take(MAX_GIT_PATH_RECORD + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_GIT_PATH_RECORD {
        return None;
    }
    crate::git::git_registration_path(&bytes, prefix)
}

fn directory_matches(dir: &Dir, path: &Path) -> bool {
    let Ok(actual) = Dir::open_ambient_dir(path, cap_std::ambient_authority()) else {
        return false;
    };
    let id = |dir: &Dir| {
        #[cfg(unix)]
        {
            use cap_std::fs::MetadataExt;
            dir.dir_metadata()
                .map(|metadata| (metadata.dev(), metadata.ino()))
        }
        #[cfg(windows)]
        crate::windows_file_id::file_id(&dir.try_clone()?.into_std_file())
    };
    id(dir).is_ok_and(|id_expected| id(&actual).is_ok_and(|id_actual| id_actual == id_expected))
}

struct IncludeSourceDir {
    path: PathBuf,
    dir: Dir,
    #[cfg(windows)]
    _ancestors: Vec<Dir>,
}

impl IncludeSourceDir {
    fn open(path: &Path) -> io::Result<Self> {
        let authority = cap_std::ambient_authority();
        #[cfg(unix)]
        let dir = Dir::open_ambient_dir(path, authority)?;
        #[cfg(windows)]
        let (dir, ancestors) = {
            // Git's working directory takes a path on Windows. Hold each
            // ancestor open without delete sharing so that path stays pinned.
            let chain: Vec<_> = path.ancestors().collect();
            let mut chain = chain.into_iter().rev();
            let mut dir = Dir::open_ambient_dir(chain.next().unwrap_or(path), authority)?;
            let mut ancestors = Vec::new();
            for child in chain {
                let next =
                    dir.open_dir_nofollow(child.file_name().ok_or(ErrorKind::InvalidInput)?)?;
                ancestors.push(dir);
                dir = next;
            }
            (dir, ancestors)
        };
        Ok(Self {
            path: path.to_path_buf(),
            dir,
            #[cfg(windows)]
            _ancestors: ancestors,
        })
    }

    fn git_command(&self) -> io::Result<std::process::Command> {
        let mut command = crate::git::git_command();
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            use std::os::unix::process::CommandExt;
            let dir = self.dir.try_clone()?;
            // SAFETY: the cloned handle stays open in the child, and fchdir
            // is async-signal-safe. The parent's working directory is untouched.
            unsafe {
                command.pre_exec(move || {
                    if libc::fchdir(dir.as_raw_fd()) == 0 {
                        Ok(())
                    } else {
                        Err(io::Error::last_os_error())
                    }
                });
            }
        }
        #[cfg(windows)]
        command.current_dir(&self.path);
        Ok(command)
    }
}

fn copy_worktree_includes(
    root: &IncludeSourceDir,
    destination: &Dir,
    setting_patterns: &[String],
    include_nested: bool,
) -> Result<IncludeResult, String> {
    let include_file = Path::new(".codemuxinclude");
    let mut patterns = tempfile::NamedTempFile::new()
        .map_err(|error| format!("Create worktree include patterns file: {error}"))?;
    let source = match root.dir.symlink_metadata(include_file) {
        Ok(_) => {
            let mut input = open_regular_include(&root.dir, include_file)
                .map_err(|error| format!("Open .codemuxinclude: {error}"))?
                .ok_or("Expected a regular .codemuxinclude file")?;
            io::copy(&mut input, &mut patterns)
                .map_err(|error| format!("Read .codemuxinclude: {error}"))?;
            IncludeSource::File
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            if setting_patterns.is_empty() {
                write_include_patterns(&mut patterns, DEFAULT_WORKTREE_INCLUDES.iter().copied())?;
                IncludeSource::Defaults
            } else {
                write_include_patterns(&mut patterns, setting_patterns.iter().map(String::as_str))?;
                IncludeSource::Setting
            }
        }
        Err(error) => return Err(format!("Inspect .codemuxinclude: {error}")),
    };
    patterns
        .flush()
        .map_err(|error| format!("Flush worktree include patterns: {error}"))?;

    eprintln!(
        "[codemux::scripts] worktree includes source={:?} for {}",
        source,
        root.path.display()
    );

    let (copied, skipped_nested) =
        copy_matching_files(root, destination, patterns.path(), include_nested)?;

    if !copied.is_empty() {
        eprintln!(
            "[codemux::scripts] worktree includes: copied {} file(s) from {}",
            copied.len(),
            root.path.display()
        );
    }

    Ok(IncludeResult {
        copied,
        skipped_nested,
        source,
    })
}

fn write_include_patterns<'a>(
    output: &mut impl Write,
    patterns: impl IntoIterator<Item = &'a str>,
) -> Result<(), String> {
    for pattern in patterns {
        writeln!(output, "{pattern}").map_err(|error| format!("Write include pattern: {error}"))?;
    }
    Ok(())
}

/// Use `git ls-files` to find gitignored files matching the exclude patterns,
/// then return the copied paths and any nested paths skipped during automatic setup.
fn copy_matching_files(
    root: &IncludeSourceDir,
    destination: &Dir,
    exclude_file: &Path,
    include_nested: bool,
) -> Result<(Vec<String>, Vec<String>), String> {
    let output = root
        .git_command()
        .map_err(|error| format!("Pin Git working directory: {error}"))?
        .args([
            "--git-dir=.git",
            "--work-tree=.",
            "ls-files",
            "-z",
            "--others",
            "--ignored",
            "--exclude-from",
        ])
        .arg(exclude_file)
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .map_err(|e| format!("Failed to run git ls-files for worktree includes: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git ls-files failed: {stderr}"));
    }

    let mut copied = Vec::new();
    let mut skipped_nested = Vec::new();

    // NUL records preserve Unicode and embedded newlines without Git's
    // C-style quoting. Unix paths may also contain non-UTF-8 bytes.
    for raw_path in output.stdout.split(|byte| *byte == 0) {
        if raw_path.is_empty() {
            continue;
        }
        #[cfg(unix)]
        let relative_path = {
            use std::os::unix::ffi::OsStrExt;
            Path::new(std::ffi::OsStr::from_bytes(raw_path))
        };
        #[cfg(not(unix))]
        let relative_path = Path::new(
            std::str::from_utf8(raw_path)
                .map_err(|e| format!("Invalid worktree include path: {e}"))?,
        );

        // On Unix an open nested directory can still be moved out of the
        // checkout. Automatic copies stay at the registered checkout root.
        if !include_nested && relative_path.components().count() != 1 {
            skipped_nested.push(relative_path.to_string_lossy().into_owned());
            continue;
        }
        if copy_confined_include(&root.dir, destination, relative_path).map_err(|error| {
            format!("Copy worktree include {}: {error}", relative_path.display())
        })? {
            copied.push(relative_path.to_string_lossy().into_owned());
        }
    }

    Ok((copied, skipped_nested))
}

fn open_include_parent(root: &Dir, path: &Path, create: bool) -> io::Result<Dir> {
    let mut directory = root.try_clone()?;
    for component in path.components() {
        let Component::Normal(name) = component else {
            return Err(ErrorKind::InvalidInput.into());
        };
        if create {
            match directory.create_dir(name) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        directory = directory.open_dir_nofollow(name)?;
        #[cfg(windows)]
        {
            use cap_std::fs::MetadataExt;
            use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
            if directory.dir_metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Err(io::Error::other("Worktree include crosses a reparse point"));
            }
        }
    }
    Ok(directory)
}

fn copy_confined_include(source: &Dir, destination: &Dir, path: &Path) -> io::Result<bool> {
    if !path
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "Invalid worktree include path",
        ));
    }
    let parent = path.parent().ok_or(ErrorKind::InvalidInput)?;
    let name = Path::new(path.file_name().ok_or(ErrorKind::InvalidInput)?);
    let Some(mut input) = open_regular_include(source, path)? else {
        return Ok(false);
    };
    let destination = open_include_parent(destination, parent, true)?;

    match destination.symlink_metadata(name) {
        Ok(entry) if !is_regular_include(&entry) => return Ok(false),
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    stage_include(&mut input, &destination)?.publish(name)?;
    Ok(true)
}

struct StagedInclude<'a> {
    parent: &'a Dir,
    name: String,
    published: bool,
}

impl StagedInclude<'_> {
    fn publish(mut self, name: &Path) -> io::Result<()> {
        self.parent.rename(&self.name, self.parent, name)?;
        self.published = true;
        Ok(())
    }
}

impl Drop for StagedInclude<'_> {
    fn drop(&mut self) {
        if !self.published {
            let _ = self.parent.remove_file(&self.name);
        }
    }
}

fn stage_include<'a>(input: &mut File, parent: &'a Dir) -> io::Result<StagedInclude<'a>> {
    // A new file never truncates an existing inode that another path could
    // share. Use the same parent for the temporary file, rename and cleanup.
    let permissions = input.metadata()?.permissions();
    let name = format!(".codemux-include-{}.tmp", uuid::Uuid::new_v4());
    let mut write = OpenOptions::new();
    write.write(true).create_new(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        write.mode(0o600);
    }
    let mut output = parent.open_with(&name, &write)?;
    let stage = StagedInclude { parent, name, published: false };
    let staged = io::copy(input, &mut output).and_then(|_| output.set_permissions(permissions));
    drop(output);
    staged?;
    Ok(stage)
}

fn open_regular_include(dir: &Dir, path: &Path) -> io::Result<Option<File>> {
    match dir.symlink_metadata(path) {
        Ok(entry) if is_regular_include(&entry) => {}
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    }
    let mut options = OpenOptions::new();
    options.read(true).follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = dir.open_with(path, &options)?;
    Ok(is_regular_include(&file.metadata()?).then_some(file))
}

fn is_regular_include(metadata: &cap_std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use cap_std::fs::MetadataExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return false;
        }
    }
    metadata.is_file()
}

/// Nearest checkout, before following the metadata of a linked worktree.
fn nearest_checkout(workspace: &Path) -> Option<&Path> {
    workspace.ancestors().find(|parent| {
        let marker = parent.join(".git");
        marker.is_dir() || marker.is_file()
    })
}

/// Resolve the project root for an ordinary subdirectory or a registered
/// linked worktree. A linked checkout's pointer alone grants no access.
pub fn resolve_root_path(workspace_path: &Path) -> PathBuf {
    let fallback = || workspace_path.to_path_buf();
    let Ok(workspace) = workspace_path.canonicalize() else {
        return fallback();
    };
    let Some(checkout) = nearest_checkout(&workspace) else {
        return fallback();
    };
    if checkout.join(".git").is_dir() {
        return checkout.to_path_buf();
    }
    registered_project_root(checkout).unwrap_or_else(fallback)
}

fn registered_project_root(checkout: &Path) -> Option<PathBuf> {
    if !checkout.join(".git").is_file() {
        return None;
    }
    // Ask Git for the actual common directory without interpreting the
    // checkout's pointer as UTF-8 or assuming the pointer is absolute.
    let common = crate::git::git_path(checkout, "--git-common-dir")?;
    if common.file_name()? != ".git" {
        return None;
    }
    let root = common.parent()?.canonicalize().ok()?;
    (root != checkout && crate::git::is_registered_worktree(&root, checkout)).then_some(root)
}

#[derive(Debug, Clone, Serialize)]
struct WorktreeIncludesApplied {
    workspace_id: String,
    source: IncludeSource,
    copied: Vec<String>,
    skipped_nested: Vec<String>,
}

/// Run the full setup pipeline: worktree includes copy + setup scripts.
/// Resolves config from file/DB.
pub fn run_setup_scripts<R: Runtime>(
    workspace_path: &Path,
    workspace_name: &str,
    workspace_id: &str,
    app_handle: &AppHandle<R>,
    db: Option<&DatabaseStore>,
    branch: Option<&str>,
    port: Option<u16>,
) -> Result<(), String> {
    eprintln!(
        "[codemux::scripts] Looking for setup config at workspace_path={}",
        workspace_path.display()
    );

    let root_path = resolve_root_path(workspace_path);

    // Read config to get worktree_includes setting for fallback
    let config = if let Some(db) = db {
        read_effective_config(workspace_path, db)
    } else {
        read_workspace_config(workspace_path)
    };

    let setting_patterns = config
        .as_ref()
        .map(|c| c.worktree_includes.clone())
        .unwrap_or_default();

    // Step 1: Process worktree includes (copy gitignored files from main worktree)
    match process_registered_worktree_includes(&root_path, workspace_path, &setting_patterns, true) {
        Ok(result) => {
            let _ = app_handle.emit(
                "worktree-includes-applied",
                WorktreeIncludesApplied {
                    workspace_id: workspace_id.to_string(),
                    source: result.source,
                    copied: result.copied,
                    skipped_nested: result.skipped_nested,
                },
            );
        }
        Err(e) => {
            eprintln!("[codemux::scripts] worktree includes error: {e}");
        }
    }

    // Step 2: Run setup commands

    let config = match config {
        Some(c) if !c.setup.is_empty() => c,
        _ => {
            eprintln!("[codemux::scripts] No setup config found for workspace {workspace_id}");
            return Ok(());
        }
    };

    run_setup_scripts_with_config(
        workspace_path,
        workspace_name,
        workspace_id,
        app_handle,
        &config,
        &root_path,
        branch,
        port,
        &Checkpoint::default(),
    )
}

/// Progress notifications from the shared setup-command runner core.
///
/// The desktop (`run_setup_scripts_with_config`) maps these to the Tauri
/// events the frontend listens for (`workspace-setup-progress` /
/// `workspace-setup-failed` / `workspace-setup-complete`); the headless
/// daemon (`remote::tools::worktree_create`) logs them to stderr because
/// there is no frontend to notify. Fields are borrowed: events are
/// consumed synchronously by the notifier.
pub enum SetupEvent<'a> {
    Progress {
        command: &'a str,
        index: usize,
        total: usize,
    },
    Failed {
        command: &'a str,
        stdout: &'a str,
        stderr: &'a str,
        exit_code: Option<i32>,
    },
    Complete,
}

/// UI-free core of the setup-command runner — shared by the desktop and
/// the headless daemon's `worktree_create` tool so both paths run
/// identical commands with identical env (`CODEMUX_ROOT_PATH` /
/// `CODEMUX_WORKSPACE_PATH` / `CODEMUX_BRANCH` / `CODEMUX_PORT` plus
/// `CODEMUX_WORKSPACE_NAME` / `CODEMUX_WORKSPACE_ID`). Commands run
/// sequentially via `sh -c` in `workspace_path`, as agent processes (see
/// [`run_script_command`]); the first failure stops the pipeline and is
/// returned as `Err`.
#[allow(clippy::too_many_arguments)]
pub fn run_setup_commands(
    workspace_path: &Path,
    workspace_name: &str,
    workspace_id: &str,
    config: &WorkspaceConfig,
    root_path: &Path,
    branch: Option<&str>,
    port: Option<u16>,
    notify: &mut dyn FnMut(SetupEvent<'_>),
) -> Result<(), String> {
    run_checked_setup_commands(
        workspace_path,
        workspace_name,
        workspace_id,
        config,
        root_path,
        branch,
        port,
        &Checkpoint::default(),
        notify,
    )
}

/// [`run_setup_commands`], asking `checkpoint` before each command starts.
/// Setup runs in the background after a create returned, and a command can
/// take long: once whoever asked for it lost their authority, no further
/// command starts, and setup fails as [`stop_setup`] reports.
#[allow(clippy::too_many_arguments)]
fn run_checked_setup_commands(
    workspace_path: &Path,
    workspace_name: &str,
    workspace_id: &str,
    config: &WorkspaceConfig,
    root_path: &Path,
    branch: Option<&str>,
    port: Option<u16>,
    checkpoint: &Checkpoint,
    notify: &mut dyn FnMut(SetupEvent<'_>),
) -> Result<(), String> {
    if config.setup.is_empty() {
        return Ok(());
    }

    eprintln!(
        "[codemux::scripts] Running {} setup command(s) for workspace {workspace_id} (root={})",
        config.setup.len(),
        root_path.display()
    );

    let mut env_vars = script_env(workspace_path, root_path, branch, port);
    env_vars.push(("CODEMUX_WORKSPACE_NAME", workspace_name.to_string()));
    env_vars.push(("CODEMUX_WORKSPACE_ID", workspace_id.to_string()));
    let total = config.setup.len();

    for (index, command) in config.setup.iter().enumerate() {
        if let Err(refusal) = checkpoint.check() {
            return Err(stop_setup(command, &refusal, notify));
        }
        notify(SetupEvent::Progress {
            command,
            index,
            total,
        });

        let output = run_script_command(command, workspace_path, &env_vars, SETUP_SCRIPT)
            .map_err(|e| format!("Failed to run setup command `{command}`: {e}"))?;

        if !output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let exit_code = output.status.code();

            notify(SetupEvent::Failed {
                command,
                stdout: &stdout,
                stderr: &stderr,
                exit_code,
            });

            return Err(format!(
                "Setup command `{command}` failed (exit {}): {}",
                exit_code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "signal".into()),
                if stderr.is_empty() { &stdout } else { &stderr }
            ));
        }
    }

    notify(SetupEvent::Complete);

    Ok(())
}

/// Ends setup before `command`, the first one that did not start, because
/// whoever asked for it lost their authority. It is reported as a failed
/// setup is, as that command's failure, so the person sees which commands
/// never ran. Returns the error.
fn stop_setup(command: &str, refusal: &str, notify: &mut dyn FnMut(SetupEvent<'_>)) -> String {
    let stderr = format!("{refusal}\nSetup stopped before this command started.");
    notify(SetupEvent::Failed {
        command,
        stdout: "",
        stderr: &stderr,
        exit_code: None,
    });
    format!("Setup stopped before `{command}`: {refusal}")
}

/// Run setup scripts with a pre-resolved config and root path.
/// The `root_path` should be resolved on the calling thread (not inside a spawned thread)
/// to avoid race conditions with worktree `.git` file resolution.
///
/// Thin Tauri wrapper over [`run_setup_commands`]: forwards each
/// [`SetupEvent`] to the frontend as the events it has always listened
/// for, with unchanged payload shapes. `checkpoint` is asked before each
/// command starts.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_setup_scripts_with_config<R: Runtime>(
    workspace_path: &Path,
    workspace_name: &str,
    workspace_id: &str,
    app_handle: &AppHandle<R>,
    config: &WorkspaceConfig,
    root_path: &Path,
    branch: Option<&str>,
    port: Option<u16>,
    checkpoint: &Checkpoint,
) -> Result<(), String> {
    run_checked_setup_commands(
        workspace_path,
        workspace_name,
        workspace_id,
        config,
        root_path,
        branch,
        port,
        checkpoint,
        &mut frontend_events(app_handle, workspace_id),
    )
}

/// Setup that stopped before it began, as [`run_setup_scripts_with_config`]
/// reports one that stopped before a command: whoever asked for it lost
/// their authority while it waited to start. Returns the error.
pub fn report_setup_stopped<R: Runtime>(
    app_handle: &AppHandle<R>,
    workspace_id: &str,
    config: Option<&WorkspaceConfig>,
    refusal: &str,
) -> String {
    match config.and_then(|config| config.setup.first()) {
        Some(command) => stop_setup(command, refusal, &mut frontend_events(app_handle, workspace_id)),
        None => format!("Setup stopped before it started: {refusal}"),
    }
}

/// Tells the frontend about a workspace's setup.
fn frontend_events<'a, R: Runtime>(
    app_handle: &'a AppHandle<R>,
    workspace_id: &'a str,
) -> impl FnMut(SetupEvent<'_>) + 'a {
    move |event: SetupEvent<'_>| match event {
        SetupEvent::Progress {
            command,
            index,
            total,
        } => {
            let _ = app_handle.emit(
                "workspace-setup-progress",
                SetupProgress {
                    workspace_id: workspace_id.to_string(),
                    command: command.to_string(),
                    index,
                    total,
                },
            );
        }
        SetupEvent::Failed {
            command,
            stdout,
            stderr,
            exit_code,
        } => {
            let _ = app_handle.emit(
                "workspace-setup-failed",
                SetupFailed {
                    workspace_id: workspace_id.to_string(),
                    command: command.to_string(),
                    stdout: stdout.to_string(),
                    stderr: stderr.to_string(),
                    exit_code,
                },
            );
        }
        SetupEvent::Complete => {
            let _ = app_handle.emit(
                "workspace-setup-complete",
                SetupComplete {
                    workspace_id: workspace_id.to_string(),
                },
            );
        }
    }
}

/// The folder's teardown commands: its `.codemux/config.json`, else the
/// project's saved scripts when `db` is given.
pub fn teardown_commands(workspace_path: &Path, db: Option<&DatabaseStore>) -> Vec<String> {
    let config = if let Some(db) = db {
        read_effective_config(workspace_path, db)
    } else {
        read_workspace_config(workspace_path)
    };
    config.map(|config| config.teardown).unwrap_or_default()
}

/// Runs `commands` (see [`teardown_commands`]) in order, stopping at the
/// first failure. It blocks until they finish, so an async caller runs it on
/// the blocking pool. A command can take long while the close waits: once
/// whoever asked for the close lost their authority (`checkpoint`), no
/// further command starts.
pub(crate) fn run_teardown_scripts(
    workspace_path: &Path,
    workspace_name: &str,
    workspace_id: &str,
    commands: &[String],
    checkpoint: &Checkpoint,
) -> Result<(), String> {
    if commands.is_empty() {
        return Ok(());
    }

    let root_path = resolve_root_path(workspace_path);
    let mut env_vars = script_env(workspace_path, &root_path, None, None);
    env_vars.push(("CODEMUX_WORKSPACE_NAME", workspace_name.to_string()));
    env_vars.push(("CODEMUX_WORKSPACE_ID", workspace_id.to_string()));

    for command in commands {
        checkpoint
            .check()
            .map_err(|refusal| format!("{refusal}; teardown stopped before `{command}`"))?;
        let output = run_script_command(command, workspace_path, &env_vars, TEARDOWN_SCRIPT)
            .map_err(|e| format!("Failed to run teardown command `{command}`: {e}"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let exit_code = output.status.code();

            return Err(format!(
                "Teardown command `{command}` failed (exit {}): {}",
                exit_code
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| "signal".into()),
                if stderr.is_empty() { &stdout } else { &stderr }
            ));
        }
    }

    Ok(())
}

/// The agent-process kinds of setup and teardown commands. Diagnostic only,
/// like every `CODEMUX_AGENT_PROCESS` value.
const SETUP_SCRIPT: &str = "setup-script";
const TEARDOWN_SCRIPT: &str = "teardown-script";

/// Runs one setup or teardown command with `sh -c` in `folder`, its output
/// captured, and waits for it.
///
/// These commands are repository content: `.codemux/config.json`, which any
/// agent working in the folder can write, overrides the project's saved
/// scripts, and agents can make them run (`rerun_setup`, workspace and
/// worktree creation, closing a worktree's workspace). So each one runs as an
/// agent process, whoever triggered it: the env marker and the ancestry
/// registration make a `codemux` it starts an agent on the control socket,
/// refused admin commands, never the owner.
///
/// The `run` command is not marked. It runs only in the person's own
/// "Workspace Run" terminal when they press Run (`run_project_dev_command`),
/// and a person's terminal is the owner, as for anything they type there.
/// Agents cannot start it: no socket command or tool does, and typing into a
/// terminal is admin.
fn run_script_command(command: &str, folder: &Path, env: &[(&str, String)], kind: &str) -> std::io::Result<Output> {
    let mut cmd = crate::execution::host_command("sh");
    cmd.arg("-c")
        .arg(command)
        .current_dir(folder)
        .envs(env.iter().map(|(key, value)| (*key, value)))
        .env(crate::control_identity::AGENT_PROCESS_ENV, kind)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = cmd.spawn()?;
    crate::control_identity::register_agent_process(child.id());
    child.wait_with_output()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn copy_worktree_includes(
        root: &Path,
        destination: &Path,
        patterns: &[String],
    ) -> Result<IncludeResult, String> {
        let root = IncludeSourceDir::open(&root.canonicalize().unwrap()).unwrap();
        let destination = Dir::open_ambient_dir(destination, cap_std::ambient_authority()).unwrap();
        super::copy_worktree_includes(&root, &destination, patterns, true)
    }

    // Bash-subprocess round-trip: spawn `sh -c 'echo -n "$VAR" > file'`, then
    // read the file back. Unix-only because (1) `sh` is not universally
    // available on Windows, (2) on windows-latest Git Bash *is* on PATH but
    // Windows-style tempdir paths contain backslashes that bash interprets as
    // escape sequences, silently breaking the `>` redirect. The sibling test
    // `test_env_vars_do_not_include_compose_project_name` covers the same
    // `script_env()` function with a pure-Rust in-memory check that runs on
    // both platforms.
    #[cfg(unix)]
    #[test]
    fn test_env_vars_include_workspace_path() {
        let workspace_dir = tempfile::tempdir().unwrap();
        let output_file = workspace_dir.path().join("ws_path.txt");

        let cmd_str = format!(
            "echo -n \"$CODEMUX_WORKSPACE_PATH\" > {}",
            output_file.display()
        );

        let root_path = workspace_dir.path().to_path_buf();
        let env_vars = script_env(workspace_dir.path(), &root_path, None, None);

        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c")
            .arg(&cmd_str)
            .current_dir(workspace_dir.path());
        for (k, v) in &env_vars {
            cmd.env(k, v);
        }

        let output = cmd.output().expect("Failed to run test command");
        assert!(output.status.success(), "Script failed: {:?}", output);

        let written = fs::read_to_string(&output_file).expect("Output file should exist");
        assert_eq!(
            written,
            workspace_dir.path().to_string_lossy(),
            "CODEMUX_WORKSPACE_PATH should match workspace directory"
        );
    }

    /// Setup and teardown commands come from the repository, which agents can
    /// write, so a `codemux` they start must reach the control socket as an
    /// agent process: the env marker on every platform, and on Linux the
    /// registration that still holds once the marker is cleared.
    #[cfg(unix)]
    #[test]
    fn setup_and_teardown_commands_run_as_agent_processes() {
        use std::time::{Duration, Instant};
        let dir = tempfile::tempdir().unwrap();
        // Each probe reports its pid and env, then holds until `<name>.go`
        // so the test can check the live process (bounded at 30 s).
        let probe = |name: &str| {
            format!(
                "{{ echo $$; env; }} > {name}.part && mv {name}.part {name}.out; \
                 i=0; while [ ! -e {name}.go ] && [ $i -lt 600 ]; do sleep 0.05; i=$((i+1)); done"
            )
        };
        let config = WorkspaceConfig { setup: vec![probe("setup")], teardown: vec![probe("teardown")], ..Default::default() };
        fs::create_dir(dir.path().join(".codemux")).unwrap();
        fs::write(dir.path().join(".codemux/config.json"), serde_json::to_string(&config).unwrap()).unwrap();

        for (name, kind) in [("setup", "setup-script"), ("teardown", "teardown-script")] {
            let (folder, config) = (dir.path().to_path_buf(), config.clone());
            let run = std::thread::spawn(move || match name {
                "setup" => run_setup_commands(&folder, "ws", "ws-1", &config, &folder, None, None, &mut |_| {}),
                _ => run_teardown_scripts(&folder, "ws", "ws-1", &teardown_commands(&folder, None), &Checkpoint::default()),
            });
            let out = dir.path().join(format!("{name}.out"));
            let deadline = Instant::now() + Duration::from_secs(20);
            while !out.exists() {
                assert!(Instant::now() < deadline && !run.is_finished(), "the {name} command never reported");
                std::thread::sleep(Duration::from_millis(20));
            }
            let report = fs::read_to_string(&out).unwrap();
            let pid: u32 = report.lines().next().unwrap().trim().parse().unwrap();
            // Exact: a test run from an agent's shell inherits another value.
            let marker = format!("{}={kind}", crate::control_identity::AGENT_PROCESS_ENV);
            assert!(report.lines().any(|line| line == marker), "the {name} command is not marked `{marker}`:\n{report}");
            assert!(report.lines().any(|line| line == "CODEMUX_WORKSPACE_ID=ws-1"), "its own env stays:\n{report}");
            #[cfg(target_os = "linux")]
            while !crate::control_identity::is_agent_descendant(pid) {
                assert!(Instant::now() < deadline, "the {name} command ({pid}) was never registered as an agent process");
                std::thread::sleep(Duration::from_millis(20));
            }
            let _ = pid;
            fs::write(dir.path().join(format!("{name}.go")), "").unwrap();
            run.join().unwrap().unwrap_or_else(|error| panic!("{name}: {error}"));
        }
    }

    /// Once the checkpoint refuses, no further setup or teardown command
    /// starts, and setup reports the first one it did not start as failed.
    #[cfg(unix)]
    #[test]
    fn setup_and_teardown_commands_stop_where_their_checkpoint_refuses() {
        let dir = tempfile::tempdir().unwrap();
        let commands = vec!["touch one".to_string(), "touch two".to_string()];
        // Passes once, then refuses as a revoked grant does.
        let once = || {
            let calls = std::sync::atomic::AtomicUsize::new(0);
            Checkpoint::new(move || match calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) {
                0 => Ok(()),
                _ => Err("grant_revoked: withdrawn".into()),
            })
        };

        let config = WorkspaceConfig { setup: commands.clone(), ..Default::default() };
        let mut failed = Vec::new();
        let error = run_checked_setup_commands(dir.path(), "ws", "ws-1", &config, dir.path(), None, None, &once(), &mut |event| {
            if let SetupEvent::Failed { command, stderr, exit_code, .. } = event {
                failed.push((command.to_string(), stderr.to_string(), exit_code));
            }
        })
        .unwrap_err();
        assert_eq!(error, "Setup stopped before `touch two`: grant_revoked: withdrawn");
        assert_eq!(failed, [("touch two".to_string(), "grant_revoked: withdrawn\nSetup stopped before this command started.".to_string(), None)]);
        assert!(dir.path().join("one").exists());
        assert!(!dir.path().join("two").exists());

        fs::remove_file(dir.path().join("one")).unwrap();
        let error = run_teardown_scripts(dir.path(), "ws", "ws-1", &commands, &once()).unwrap_err();
        assert_eq!(error, "grant_revoked: withdrawn; teardown stopped before `touch two`");
        assert!(dir.path().join("one").exists());
        assert!(!dir.path().join("two").exists());
    }

    #[test]
    fn test_env_vars_do_not_include_compose_project_name() {
        let workspace_dir = tempfile::tempdir().unwrap();
        let project_root = workspace_dir.path().join("my-cool-project");
        fs::create_dir_all(&project_root).unwrap();

        let env_vars = script_env(workspace_dir.path(), &project_root, None, None);
        assert!(
            env_vars.iter().all(|(k, _)| *k != "COMPOSE_PROJECT_NAME"),
            "COMPOSE_PROJECT_NAME should not be set automatically"
        );
    }

    #[test]
    fn test_env_vars_include_branch_and_port() {
        let workspace_dir = tempfile::tempdir().unwrap();
        let root_path = workspace_dir.path().to_path_buf();

        let env_vars = script_env(
            workspace_dir.path(),
            &root_path,
            Some("feature/login"),
            Some(3110),
        );

        let branch_var = env_vars.iter().find(|(k, _)| *k == "CODEMUX_BRANCH");
        let port_var = env_vars.iter().find(|(k, _)| *k == "CODEMUX_PORT");

        assert_eq!(branch_var.unwrap().1, "feature/login");
        assert_eq!(port_var.unwrap().1, "3110");
    }

    #[test]
    fn test_env_vars_omit_branch_and_port_when_none() {
        let workspace_dir = tempfile::tempdir().unwrap();
        let root_path = workspace_dir.path().to_path_buf();

        let env_vars = script_env(workspace_dir.path(), &root_path, None, None);

        assert!(env_vars.iter().all(|(k, _)| *k != "CODEMUX_BRANCH"));
        assert!(env_vars.iter().all(|(k, _)| *k != "CODEMUX_PORT"));
    }

    #[test]
    fn test_allocate_workspace_port_deterministic() {
        // Same ID always produces the same port
        let port1 = allocate_workspace_port("ws-abc-123");
        let port2 = allocate_workspace_port("ws-abc-123");
        assert_eq!(port1, port2);

        // Port is in valid range
        assert!(port1 >= 3100);
        assert!(port1 < 6500);
        // Port is 10-aligned
        assert_eq!(port1 % 10, 0);

        // Different IDs produce different ports (probabilistically)
        let port_a = allocate_workspace_port("workspace-alpha");
        let port_b = allocate_workspace_port("workspace-beta");
        // Not guaranteed unique, but extremely unlikely to collide for different inputs
        assert_ne!(port_a, port_b);
    }

    #[test]
    fn test_worktree_includes_no_file_uses_defaults() {
        let root = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();

        // Set up a git repo so git ls-files works
        std::process::Command::new("git")
            .args(["init"])
            .current_dir(root.path())
            .output()
            .unwrap();

        // Create .gitignore so default patterns can match
        fs::write(root.path().join(".gitignore"), ".env\n").unwrap();

        // No .codemuxinclude, no setting patterns — should use defaults
        let result = copy_worktree_includes(root.path(), worktree.path(), &[]);
        let result = result.unwrap();
        assert_eq!(result.source, IncludeSource::Defaults);
    }

    #[test]
    fn test_worktree_includes_setting_fallback() {
        let root = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();

        std::process::Command::new("git")
            .args(["init"])
            .current_dir(root.path())
            .output()
            .unwrap();

        fs::write(root.path().join(".gitignore"), ".env\n").unwrap();
        fs::write(root.path().join(".env"), "SECRET=abc").unwrap();

        // No .codemuxinclude file, but setting patterns provided
        let patterns = vec![".env".to_string()];
        let result = copy_worktree_includes(root.path(), worktree.path(), &patterns).unwrap();

        assert_eq!(result.source, IncludeSource::Setting);
        assert!(result.copied.contains(&".env".to_string()));
        assert_eq!(
            fs::read_to_string(worktree.path().join(".env")).unwrap(),
            "SECRET=abc"
        );
    }

    #[cfg(unix)]
    #[test]
    fn worktree_includes_keep_file_links_outside_the_copy() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let external = outside.path().join("external.txt");
        fs::write(&external, "unchanged").unwrap();
        fs::write(root.path().join(".env"), "value=local").unwrap();
        symlink(&external, root.path().join(".env.link")).unwrap();
        symlink(&external, worktree.path().join(".env")).unwrap();

        let authority = cap_std::ambient_authority();
        let source = Dir::open_ambient_dir(root.path(), authority).unwrap();
        let destination = Dir::open_ambient_dir(worktree.path(), authority).unwrap();
        assert!(!copy_confined_include(&source, &destination, Path::new(".env")).unwrap());
        assert!(!copy_confined_include(&source, &destination, Path::new(".env.link")).unwrap());
        assert_eq!(fs::read_to_string(&external).unwrap(), "unchanged");
        assert!(!worktree.path().join(".env.link").exists());
    }

    #[cfg(unix)]
    #[test]
    fn worktree_includes_confine_destination_parents() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("settings")).unwrap();
        fs::write(root.path().join("settings/.env"), "value=local").unwrap();
        symlink(outside.path(), worktree.path().join("settings")).unwrap();

        let authority = cap_std::ambient_authority();
        let source = Dir::open_ambient_dir(root.path(), authority).unwrap();
        let destination = Dir::open_ambient_dir(worktree.path(), authority).unwrap();
        assert!(copy_confined_include(&source, &destination, Path::new("settings/.env")).is_err());
        assert!(!outside.path().join(".env").exists());
    }

    #[test]
    fn worktree_includes_replace_only_the_destination_entry() {
        let root = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();
        let existing = root.path().join("previous.txt");
        fs::write(&existing, "old").unwrap();
        fs::write(root.path().join(".env"), "new").unwrap();
        fs::hard_link(&existing, worktree.path().join(".env")).unwrap();

        let authority = cap_std::ambient_authority();
        let source = Dir::open_ambient_dir(root.path(), authority).unwrap();
        let destination = Dir::open_ambient_dir(worktree.path(), authority).unwrap();
        assert!(copy_confined_include(&source, &destination, Path::new(".env")).unwrap());
        assert_eq!(fs::read_to_string(&existing).unwrap(), "old");
        assert_eq!(fs::read_to_string(worktree.path().join(".env")).unwrap(), "new");
        assert_eq!(fs::read_dir(worktree.path()).unwrap().count(), 1);
    }

    fn include_source_project(root: &Path) {
        fs::create_dir(root).unwrap();
        for args in [
            &["init", "-b", "main"][..],
            &[
                "-c", "user.name=Fixture", "-c", "user.email=fixture@example.test",
                "-c", "commit.gpgsign=false", "commit", "--allow-empty", "-m", "Fixture",
            ],
        ] {
            let output = crate::git::git_command()
                .current_dir(root)
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
        }
        fs::write(root.join(".env"), "INCLUDE_FIXTURE=local").unwrap();
    }

    fn register_include_fixture(root: &Path, checkout: &Path) {
        let output = crate::git::git_command()
            .current_dir(root)
            .args(["worktree", "add", "--detach"])
            .arg(checkout)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }

    #[test]
    fn worktree_includes_require_registration_for_external_and_nested_checkouts() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("project");
        let registered = fixture.path().join("registered");
        let nested_registered = root.join("linked");
        let subdirectory = root.join("subdirectory");
        include_source_project(&root);
        fs::write(root.join(".codemuxinclude"), ".env\nsettings/private.env\n").unwrap();
        fs::create_dir(root.join("settings")).unwrap();
        fs::write(root.join("settings/private.env"), "INCLUDE_FIXTURE=nested").unwrap();
        fs::create_dir(&subdirectory).unwrap();
        register_include_fixture(&root, &registered);
        register_include_fixture(&root, &nested_registered);

        let canonical_root = root.canonicalize().unwrap();
        assert_eq!(resolve_root_path(&subdirectory), canonical_root);
        assert!(process_worktree_includes(&root, &subdirectory, &[]).unwrap().copied.is_empty());
        assert!(!subdirectory.join(".env").exists());

        for checkout in [&registered, &nested_registered] {
            assert_eq!(resolve_root_path(checkout), canonical_root);
            let includes = process_worktree_includes(&root, checkout, &[]).unwrap();
            assert_eq!(includes.copied, [".env"]);
            assert_eq!(includes.skipped_nested, ["settings/private.env"]);
            assert!(!checkout.join("settings").exists());
            let subdirectory = checkout.join("subdirectory");
            fs::create_dir(&subdirectory).unwrap();
            let includes = process_worktree_includes(&root, &subdirectory, &[]).unwrap();
            assert!(includes.copied.is_empty());
            assert!(includes.skipped_nested.is_empty());
            assert!(!subdirectory.join(".env").exists());
        }
        for checkout in [fixture.path().join("unregistered"), root.join("nested")] {
            fs::create_dir(&checkout).unwrap();
            fs::copy(registered.join(".git"), checkout.join(".git")).unwrap();
            assert_eq!(resolve_root_path(&checkout), checkout);

            // The copy entry point verifies membership even if an earlier
            // setup step passed the parent repository directly.
            let includes = process_worktree_includes(&root, &checkout, &[]).unwrap();
            assert!(includes.copied.is_empty());
            assert!(includes.skipped_nested.is_empty());
            assert!(!checkout.join(".env").exists());
        }

        let includes = process_registered_worktree_includes(&root, &registered, &[], true).unwrap();
        assert!(includes.copied.iter().any(|path| path == "settings/private.env"));
        assert!(includes.skipped_nested.is_empty());
        assert_eq!(
            fs::read_to_string(registered.join("settings/private.env")).unwrap(),
            "INCLUDE_FIXTURE=nested"
        );
        assert_eq!(fs::read_to_string(root.join(".env")).unwrap(), "INCLUDE_FIXTURE=local");
    }

    #[cfg(unix)]
    #[test]
    fn worktree_includes_preserve_native_paths_and_pattern_file_priority() {
        use std::os::unix::ffi::OsStrExt;

        let fixture = tempfile::tempdir().unwrap();
        let root = fixture
            .path()
            .join(std::ffi::OsStr::from_bytes(b"project-\xfe"));
        let checkout = fixture
            .path()
            .join(std::ffi::OsStr::from_bytes(b"linked\nworkspace-\xff"));
        include_source_project(&root);
        fs::write(root.join("local.settings"), "INCLUDE_FIXTURE=custom").unwrap();
        fs::write(root.join(".codemuxinclude"), "local.settings\n").unwrap();
        register_include_fixture(&root, &checkout);

        let subdirectory = checkout.join("nested");
        fs::create_dir(&subdirectory).unwrap();
        let source = resolve_root_path(&subdirectory);
        assert_eq!(source, root.canonicalize().unwrap());
        let includes =
            process_registered_worktree_includes(&source, &subdirectory, &[".env".into()], true)
                .unwrap();
        assert_eq!(includes.source, IncludeSource::File);
        assert_eq!(includes.copied, ["local.settings"]);
        assert_eq!(
            fs::read_to_string(subdirectory.join("local.settings")).unwrap(),
            "INCLUDE_FIXTURE=custom"
        );
        assert!(!subdirectory.join(".env").exists());
    }

    #[cfg(unix)]
    #[test]
    fn worktree_includes_accept_crlf_registration_records() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("project");
        let checkout = fixture.path().join("linked\r\ncheckout");
        include_source_project(&root);
        register_include_fixture(&root, &checkout);

        let admin = crate::git::git_path(&checkout, "--git-dir").unwrap();
        for path in [checkout.join(".git"), admin.join("commondir"), admin.join("gitdir")] {
            let mut record = fs::read(&path).unwrap();
            assert_eq!(record.pop(), Some(b'\n'));
            record.extend_from_slice(b"\r\n");
            fs::write(path, record).unwrap();
        }

        assert_eq!(resolve_root_path(&checkout), root.canonicalize().unwrap());
        let includes = process_worktree_includes(&root, &checkout, &[]).unwrap();
        assert_eq!(includes.copied, [".env"]);
        assert_eq!(
            fs::read_to_string(checkout.join(".env")).unwrap(),
            "INCLUDE_FIXTURE=local"
        );
    }

    #[cfg(unix)]
    #[test]
    fn failed_include_publication_cleans_up_after_the_parent_moves() {
        let fixture = tempfile::tempdir().unwrap();
        let checkout = fixture.path().join("checkout");
        let original = checkout.join("settings");
        let moved = fixture.path().join("moved-settings");
        fs::create_dir_all(original.join(".env")).unwrap();
        fs::write(fixture.path().join("source.env"), "INCLUDE_FIXTURE=private").unwrap();

        let authority = cap_std::ambient_authority();
        let source = Dir::open_ambient_dir(fixture.path(), authority).unwrap();
        let checkout = Dir::open_ambient_dir(&checkout, authority).unwrap();
        let parent = open_include_parent(&checkout, Path::new("settings"), false).unwrap();
        let mut input = source.open("source.env").unwrap();
        let stage = stage_include(&mut input, &parent).unwrap();
        assert!(original.join(&stage.name).is_file());

        fs::rename(&original, &moved).unwrap();
        fs::create_dir(&original).unwrap();
        assert!(stage.publish(Path::new(".env")).is_err());
        assert_eq!(fs::read_dir(&moved).unwrap().count(), 1);
        assert!(moved.join(".env").is_dir());
        assert_eq!(fs::read_dir(&original).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn worktree_includes_use_the_opened_directories_after_authorization() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("project");
        let checkout = fixture.path().join("checkout");
        let moved_root = fixture.path().join("moved-project");
        let moved_checkout = fixture.path().join("moved-checkout");
        include_source_project(&root);
        register_include_fixture(&root, &checkout);
        fs::write(root.join(".codemuxinclude"), ".env\n").unwrap();
        let (source, destination) = open_registered_include_dirs(&root, &checkout, false).unwrap();

        fs::rename(&root, &moved_root).unwrap();
        fs::rename(&checkout, &moved_checkout).unwrap();
        include_source_project(&root);
        fs::remove_file(root.join(".env")).unwrap();
        fs::write(root.join(".codemuxinclude"), "different\n").unwrap();
        fs::create_dir(&checkout).unwrap();

        let includes = super::copy_worktree_includes(&source, &destination, &[], false).unwrap();
        assert_eq!(includes.source, IncludeSource::File);
        assert_eq!(includes.copied, [".env"]);
        assert_eq!(
            fs::read_to_string(moved_checkout.join(".env")).unwrap(),
            "INCLUDE_FIXTURE=local"
        );
        assert!(!checkout.join(".env").exists());
    }

    #[cfg(unix)]
    #[test]
    fn worktree_includes_reject_linked_pattern_files_without_falling_back() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().join("project");
        let checkout = fixture.path().join("checkout");
        include_source_project(&root);
        register_include_fixture(&root, &checkout);
        let patterns = fixture.path().join("patterns");
        fs::write(&patterns, "local.settings\n").unwrap();
        std::os::unix::fs::symlink(&patterns, root.join(".codemuxinclude")).unwrap();

        assert!(process_worktree_includes(&root, &checkout, &[]).is_err());
        assert!(!checkout.join(".env").exists());
    }

    #[test]
    fn test_worktree_includes_file_takes_priority() {
        let root = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();

        std::process::Command::new("git")
            .args(["init"])
            .current_dir(root.path())
            .output()
            .unwrap();

        fs::write(root.path().join(".gitignore"), ".env\nconfig/\n").unwrap();

        // Create .codemuxinclude file
        fs::write(root.path().join(".codemuxinclude"), ".env\nconfig/master.key\n").unwrap();

        // Create the gitignored files
        fs::write(root.path().join(".env"), "SECRET=abc").unwrap();
        fs::create_dir_all(root.path().join("config")).unwrap();
        fs::write(root.path().join("config/master.key"), "key123").unwrap();

        // Even though setting patterns are provided, file should win
        let patterns = vec!["something-else".to_string()];
        let result = copy_worktree_includes(root.path(), worktree.path(), &patterns).unwrap();

        assert_eq!(result.source, IncludeSource::File);
        assert!(result.copied.contains(&".env".to_string()));
        assert!(result.copied.contains(&"config/master.key".to_string()));

        assert_eq!(
            fs::read_to_string(worktree.path().join(".env")).unwrap(),
            "SECRET=abc"
        );
        assert_eq!(
            fs::read_to_string(worktree.path().join("config/master.key")).unwrap(),
            "key123"
        );
    }

    #[test]
    fn test_worktree_includes_patterns_match_nothing() {
        let root = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();

        std::process::Command::new("git")
            .args(["init"])
            .current_dir(root.path())
            .output()
            .unwrap();

        fs::write(root.path().join(".gitignore"), ".env\n").unwrap();
        // .codemuxinclude asks for files that don't exist
        fs::write(root.path().join(".codemuxinclude"), "nonexistent.txt\n").unwrap();

        let result = copy_worktree_includes(root.path(), worktree.path(), &[]).unwrap();
        assert_eq!(result.source, IncludeSource::File);
        assert!(result.copied.is_empty());
    }

    #[test]
    fn test_worktree_includes_source_file_deleted() {
        let root = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();

        std::process::Command::new("git")
            .args(["init"])
            .current_dir(root.path())
            .output()
            .unwrap();

        fs::write(root.path().join(".gitignore"), ".env\nsecrets/\n").unwrap();
        fs::write(root.path().join(".codemuxinclude"), ".env\nsecrets/key.pem\n").unwrap();

        // Create .env but NOT secrets/key.pem — simulates a file that was
        // listed by git ls-files but deleted before the copy step
        fs::write(root.path().join(".env"), "DB_URL=postgres://...").unwrap();

        let result = copy_worktree_includes(root.path(), worktree.path(), &[]).unwrap();
        assert_eq!(result.source, IncludeSource::File);
        // Only .env should be copied; the missing file is silently skipped
        assert_eq!(result.copied, vec![".env"]);
        assert!(worktree.path().join(".env").exists());
        assert!(!worktree.path().join("secrets/key.pem").exists());
    }

    #[test]
    fn test_worktree_includes_empty_patterns_uses_defaults() {
        let root = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();

        std::process::Command::new("git")
            .args(["init"])
            .current_dir(root.path())
            .output()
            .unwrap();

        fs::write(root.path().join(".gitignore"), ".env\n").unwrap();

        // Empty setting patterns should fall through to defaults
        let result = copy_worktree_includes(root.path(), worktree.path(), &[]).unwrap();
        assert_eq!(result.source, IncludeSource::Defaults);
    }

    /// Regression test for the bug where opening an existing project's main
    /// branch as a workspace (workspace path == project root) would silently
    /// truncate `.env` files to zero bytes. The copy step internally called
    /// `std::fs::copy(src, src)`, and Rust opens the destination with
    /// `O_TRUNC` before reading the source, so the file was destroyed.
    ///
    /// The fix: `process_worktree_includes` short-circuits when root and
    /// worktree paths resolve to the same directory.
    #[test]
    fn test_worktree_includes_same_root_does_not_truncate() {
        let root = tempfile::tempdir().unwrap();

        std::process::Command::new("git")
            .args(["init"])
            .current_dir(root.path())
            .output()
            .unwrap();

        fs::write(root.path().join(".gitignore"), ".env\n.env.*\n").unwrap();

        let env_content = "DATABASE_URL=postgres://localhost/prod\nSECRET_KEY=do-not-lose-this\n";
        fs::write(root.path().join(".env"), env_content).unwrap();
        fs::write(root.path().join(".env.local"), "LOCAL=keep-me").unwrap();

        // Workspace path == project root (the buggy scenario).
        let result = process_worktree_includes(root.path(), root.path(), &[]).unwrap();

        // Nothing was copied — there's no separate destination to copy into.
        assert!(
            result.copied.is_empty(),
            "expected no-op when workspace == root, got copied={:?}",
            result.copied
        );

        // Crucially, the original files must still have their content.
        assert_eq!(
            fs::read_to_string(root.path().join(".env")).unwrap(),
            env_content,
            ".env was truncated by self-copy — regression!"
        );
        assert_eq!(
            fs::read_to_string(root.path().join(".env.local")).unwrap(),
            "LOCAL=keep-me",
            ".env.local was truncated by self-copy — regression!"
        );
    }

    /// Same-root guard must hold even when an explicit `.codemuxinclude` file
    /// is present (file source, not defaults).
    #[test]
    fn test_worktree_includes_same_root_with_codemuxinclude_does_not_truncate() {
        let root = tempfile::tempdir().unwrap();

        std::process::Command::new("git")
            .args(["init"])
            .current_dir(root.path())
            .output()
            .unwrap();

        fs::write(root.path().join(".gitignore"), ".env\nsecrets/\n").unwrap();
        fs::write(
            root.path().join(".codemuxinclude"),
            ".env\nsecrets/key.pem\n",
        )
        .unwrap();

        fs::write(root.path().join(".env"), "ENV=preserved").unwrap();
        fs::create_dir_all(root.path().join("secrets")).unwrap();
        fs::write(root.path().join("secrets/key.pem"), "PEM=preserved").unwrap();

        let result = process_worktree_includes(root.path(), root.path(), &[]).unwrap();
        assert!(result.copied.is_empty());

        assert_eq!(
            fs::read_to_string(root.path().join(".env")).unwrap(),
            "ENV=preserved"
        );
        assert_eq!(
            fs::read_to_string(root.path().join("secrets/key.pem")).unwrap(),
            "PEM=preserved"
        );
    }

    /// Same-root detection must work through symlinks. If `worktree_path` is a
    /// symlink to `root_path` (or vice versa), the canonicalized check should
    /// catch it and skip the copy.
    #[cfg(unix)]
    #[test]
    fn test_worktree_includes_same_root_via_symlink_does_not_truncate() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("real");
        let alias = parent.path().join("alias");
        fs::create_dir(&root).unwrap();
        std::os::unix::fs::symlink(&root, &alias).unwrap();

        std::process::Command::new("git")
            .args(["init"])
            .current_dir(&root)
            .output()
            .unwrap();

        fs::write(root.join(".gitignore"), ".env\n").unwrap();
        let content = "SYMLINKED=preserved";
        fs::write(root.join(".env"), content).unwrap();

        // root and alias resolve to the same directory after canonicalization.
        let result = process_worktree_includes(&root, &alias, &[]).unwrap();
        assert!(result.copied.is_empty());

        assert_eq!(fs::read_to_string(root.join(".env")).unwrap(), content);
    }

    /// Sanity check: with distinct directories, the copy still works
    /// (the fix must not break the legitimate worktree case).
    #[test]
    fn test_worktree_includes_distinct_paths_still_copies() {
        let root = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();

        std::process::Command::new("git")
            .args(["init"])
            .current_dir(root.path())
            .output()
            .unwrap();

        fs::write(root.path().join(".gitignore"), ".env\n").unwrap();
        fs::write(root.path().join(".env"), "FROM_ROOT=ok").unwrap();

        let result = copy_worktree_includes(root.path(), worktree.path(), &[]).unwrap();
        assert!(result.copied.contains(&".env".to_string()));
        assert_eq!(
            fs::read_to_string(worktree.path().join(".env")).unwrap(),
            "FROM_ROOT=ok"
        );
        // Source must be untouched too.
        assert_eq!(
            fs::read_to_string(root.path().join(".env")).unwrap(),
            "FROM_ROOT=ok"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_worktree_includes_preserves_unusual_file_names() {
        let root = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();
        let status = std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(root.path())
            .status()
            .unwrap();
        assert!(status.success());

        let names = [".env.über", ".env.\"quoted\"", ".env.line\nbreak"];
        for name in names {
            fs::write(root.path().join(name), format!("SOURCE={name}"))
                .unwrap();
        }

        let result = copy_worktree_includes(root.path(), worktree.path(), &[]).unwrap();
        assert_eq!(result.copied.len(), names.len());
        for name in names {
            assert!(result.copied.iter().any(|copied| copied == name));
            let expected = format!("SOURCE={name}");
            assert_eq!(fs::read_to_string(worktree.path().join(name)).unwrap(), expected);
            assert_eq!(fs::read_to_string(root.path().join(name)).unwrap(), expected);
        }
    }

    #[test]
    fn test_worktree_includes_default_patterns_copy_env() {
        let root = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();

        std::process::Command::new("git")
            .args(["init"])
            .current_dir(root.path())
            .output()
            .unwrap();

        fs::write(root.path().join(".gitignore"), ".env\n.env.*\n").unwrap();
        fs::write(root.path().join(".env"), "SECRET=val").unwrap();
        fs::write(root.path().join(".env.local"), "LOCAL=val").unwrap();

        // No file, no settings — defaults should pick up .env and .env.local
        let result = copy_worktree_includes(root.path(), worktree.path(), &[]).unwrap();
        assert_eq!(result.source, IncludeSource::Defaults);
        assert!(result.copied.contains(&".env".to_string()));
        assert!(result.copied.contains(&".env.local".to_string()));
        assert_eq!(
            fs::read_to_string(worktree.path().join(".env")).unwrap(),
            "SECRET=val"
        );
        assert_eq!(
            fs::read_to_string(worktree.path().join(".env.local")).unwrap(),
            "LOCAL=val"
        );
    }
}
