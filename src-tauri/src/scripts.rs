use crate::config::workspace_config::{find_git_root, LifecyclePaths, WorkspaceConfig};
use crate::workspace_service::Checkpoint;
use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, File, OpenOptions};
use serde::Serialize;
use std::io::{self, ErrorKind, Write};
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
    pub source: IncludeSource,
}

use crate::workspace_paths::paths_equal;

/// Process worktree includes: copy gitignored files matching patterns from the
/// main worktree into the new worktree.
///
/// Pattern source priority:
///   1. `.codemuxinclude` file in project root
///   2. `worktree_includes` from project settings
///   3. Hardcoded defaults (`DEFAULT_WORKTREE_INCLUDES`)
///
/// If `root_path` and `worktree_path` resolve to the same directory (i.e., the
/// workspace is the project root itself, not a separate git worktree), this is
/// a no-op. Without that guard, the copy step would call
/// `std::fs::copy(src, src)`, which on Linux truncates the file to zero bytes
/// because Rust opens the destination with `O_TRUNC` *before* reading the
/// source. See `test_worktree_includes_same_root_does_not_truncate`.
pub fn process_worktree_includes(
    root_path: &Path,
    worktree_path: &Path,
    setting_patterns: &[String],
) -> Result<IncludeResult, String> {
    if paths_equal(root_path, worktree_path) {
        eprintln!(
            "[codemux::scripts] worktree includes skipped: workspace path == project root ({})",
            root_path.display()
        );
        return Ok(IncludeResult {
            copied: Vec::new(),
            source: IncludeSource::Defaults,
        });
    }

    let source = Dir::open_ambient_dir(root_path, cap_std::ambient_authority())
        .map_err(|error| format!("Open include source: {error}"))?;
    let destination = Dir::open_ambient_dir(worktree_path, cap_std::ambient_authority())
        .map_err(|error| format!("Open include destination: {error}"))?;
    copy_worktree_includes(&source, &destination, setting_patterns, true, |git| {
        git.current_dir(root_path).output()
    })
}

fn copy_worktree_includes(
    source: &Dir,
    destination: &Dir,
    setting_patterns: &[String],
    include_nested: bool,
    list_files: impl FnOnce(&mut std::process::Command) -> io::Result<Output>,
) -> Result<IncludeResult, String> {
    let mut patterns = tempfile::NamedTempFile::new()
        .map_err(|error| format!("Create worktree include patterns file: {error}"))?;
    let include_file = Path::new(".codemuxinclude");
    let pattern_source = match source.symlink_metadata(include_file) {
        Ok(_) => {
            let mut input = open_regular_include(source, include_file)
                .map_err(|error| format!("Open .codemuxinclude: {error}"))?
                .ok_or("Expected a regular .codemuxinclude file")?;
            io::copy(&mut input, &mut patterns)
                .map_err(|error| format!("Read .codemuxinclude: {error}"))?;
            IncludeSource::File
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            if setting_patterns.is_empty() {
                write_patterns(&mut patterns, DEFAULT_WORKTREE_INCLUDES.iter().copied())?;
                IncludeSource::Defaults
            } else {
                write_patterns(&mut patterns, setting_patterns.iter().map(String::as_str))?;
                IncludeSource::Setting
            }
        }
        Err(error) => return Err(format!("Inspect .codemuxinclude: {error}")),
    };
    patterns
        .flush()
        .map_err(|error| format!("Flush worktree include patterns: {error}"))?;

    let mut git = crate::git::git_command();
    git.args([
        "--git-dir=.git",
        "--work-tree=.",
        "ls-files",
        "-z",
        "--others",
        "--ignored",
        "--exclude-from",
    ])
    .arg(patterns.path())
    .env_remove("GIT_COMMON_DIR")
    .env_remove("GIT_INDEX_FILE");
    let output = list_files(&mut git)
        .map_err(|error| format!("Failed to run git ls-files for worktree includes: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git ls-files failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let mut copied = Vec::new();
    for raw_path in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        #[cfg(unix)]
        let path = {
            use std::os::unix::ffi::OsStrExt;
            Path::new(std::ffi::OsStr::from_bytes(raw_path))
        };
        #[cfg(not(unix))]
        let path = Path::new(
            std::str::from_utf8(raw_path)
                .map_err(|error| format!("Invalid worktree include path: {error}"))?,
        );
        // A nested destination directory can move outside the checkout even
        // while its handle is open. Automatic copies stay at the held root.
        if !include_nested && path.components().count() != 1 {
            continue;
        }
        if copy_include(source, destination, path)
            .map_err(|error| format!("Copy worktree include {}: {error}", path.display()))?
        {
            copied.push(path.to_string_lossy().into_owned());
        }
    }
    Ok(IncludeResult {
        copied,
        source: pattern_source,
    })
}

fn write_patterns<'a>(
    output: &mut impl Write,
    patterns: impl IntoIterator<Item = &'a str>,
) -> Result<(), String> {
    for pattern in patterns {
        writeln!(output, "{pattern}").map_err(|error| format!("Write include pattern: {error}"))?;
    }
    Ok(())
}

/// Walk one directory component at a time so no ancestor can be a symlink
/// or Windows reparse point even when its target is inside the checkout.
fn include_parent(root: &Dir, path: &Path, create: bool) -> io::Result<Dir> {
    let mut directory = root.try_clone()?;
    for component in path.components() {
        let Component::Normal(name) = component else {
            return Err(ErrorKind::InvalidInput.into());
        };
        let child = Path::new(name);
        if create {
            match directory.create_dir(child) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        let next = directory.open_dir_nofollow(child)?;
        #[cfg(windows)]
        {
            use cap_std::fs::MetadataExt;
            use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
            if next.dir_metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Err(io::Error::other("Worktree include crosses a reparse point"));
            }
        }
        directory = next;
    }
    Ok(directory)
}

fn open_regular_include(dir: &Dir, name: &Path) -> io::Result<Option<File>> {
    match dir.symlink_metadata(name) {
        Ok(metadata) if regular_include(&metadata) => {}
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
    let file = dir.open_with(name, &options)?;
    Ok(regular_include(&file.metadata()?).then_some(file))
}

fn regular_include(metadata: &cap_std::fs::Metadata) -> bool {
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

fn copy_include(source: &Dir, destination: &Dir, path: &Path) -> io::Result<bool> {
    let parent = path.parent().ok_or(ErrorKind::InvalidInput)?;
    let name = Path::new(path.file_name().ok_or(ErrorKind::InvalidInput)?);
    let source = match include_parent(source, parent, false) {
        Ok(directory) => directory,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let Some(mut input) = open_regular_include(&source, name)? else {
        return Ok(false);
    };
    let destination = include_parent(destination, parent, true)?;
    match destination.symlink_metadata(name) {
        Ok(metadata) if !regular_include(&metadata) => return Ok(false),
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    // Replace an entry atomically without truncating any existing inode.
    let temporary = format!(".codemux-include-{}.tmp", uuid::Uuid::new_v4());
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let permissions = input.metadata()?.permissions();
    let mut output = destination.open_with(&temporary, &options)?;
    let staged =
        io::copy(&mut input, &mut output).and_then(|_| output.set_permissions(permissions));
    drop(output);
    match staged.and_then(|_| destination.rename(&temporary, &destination, name)) {
        Ok(()) => Ok(true),
        Err(error) => {
            let _ = destination.remove_file(&temporary);
            Err(error)
        }
    }
}

/// Resolve the git root for a workspace path, with fallback.
pub fn resolve_root_path(workspace_path: &Path) -> PathBuf {
    find_git_root(workspace_path).unwrap_or_else(|| workspace_path.to_path_buf())
}

#[derive(Debug, Clone, Serialize)]
struct WorktreeIncludesApplied {
    workspace_id: String,
    source: IncludeSource,
    copied: Vec<String>,
}

/// Run the full setup pipeline: worktree includes copy + setup scripts.
/// The caller selects the configuration according to its authority.
pub fn run_setup_scripts<R: Runtime>(
    workspace_path: &Path,
    workspace_name: &str,
    workspace_id: &str,
    app_handle: &AppHandle<R>,
    config: Option<WorkspaceConfig>,
    branch: Option<&str>,
    port: Option<u16>,
) -> Result<(), String> {
    eprintln!(
        "[codemux::scripts] Looking for setup config at workspace_path={}",
        workspace_path.display()
    );

    run_setup_pipeline(
        ScriptLocation::Explicit {
            workspace: workspace_path,
            root: &resolve_root_path(workspace_path),
        },
        workspace_name,
        workspace_id,
        app_handle,
        config.as_ref(),
        branch,
        port,
        &Checkpoint::default(),
    )
}

pub(crate) enum ScriptLocation<'a> {
    Explicit { workspace: &'a Path, root: &'a Path },
    Checked(&'a LifecyclePaths),
}

impl ScriptLocation<'_> {
    fn workspace(&self) -> &Path {
        match self {
            Self::Explicit { workspace, .. } => workspace,
            Self::Checked(paths) => paths.workspace(),
        }
    }

    fn root(&self) -> &Path {
        match self {
            Self::Explicit { root, .. } => root,
            Self::Checked(paths) => paths.root(),
        }
    }

    fn check(&self, checkpoint: &Checkpoint) -> Result<(), String> {
        checkpoint.check()?;
        if let Self::Checked(paths) = self {
            paths.check()?;
        }
        Ok(())
    }

    pub(crate) fn copy_includes(&self, patterns: &[String]) -> Result<Option<IncludeResult>, String> {
        if let Self::Checked(paths) = self {
            if !paths.permits_include_copy() {
                return Ok(None);
            }
            // The Git lookup may have waited. Confirm the directories still
            // match before reading source files or applying the copy.
            paths.check()?;
            let directories = paths
                .include_directories()
                .map_err(|error| format!("Hold worktree include directories: {error}"))?;
            return copy_worktree_includes(
                &directories.source,
                &directories.destination,
                patterns,
                false,
                |git| {
                    git.stdout(Stdio::piped())
                        .stderr(Stdio::piped())
                        .stdin(Stdio::null());
                    paths
                        .spawn_in_include_source(git, &directories)?
                        .wait_with_output()
                },
            )
            .map(Some);
        }
        process_worktree_includes(self.root(), self.workspace(), patterns).map(Some)
    }
}

/// Apply includes and run the selected commands. The guard is checked before
/// the include copy and again before starting each command.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_setup_pipeline<R: Runtime>(
    location: ScriptLocation<'_>,
    workspace_name: &str,
    workspace_id: &str,
    app_handle: &AppHandle<R>,
    config: Option<&WorkspaceConfig>,
    branch: Option<&str>,
    port: Option<u16>,
    checkpoint: &Checkpoint,
) -> Result<(), String> {
    location.check(checkpoint).map_err(|refusal| {
        report_setup_stopped(app_handle, workspace_id, config, &refusal)
    })?;
    let setting_patterns = config.map_or(&[][..], |config| config.worktree_includes.as_slice());

    // Step 1: Process worktree includes (copy gitignored files from main worktree)
    match location.copy_includes(setting_patterns) {
        Ok(Some(result)) => {
            let _ = app_handle.emit(
                "worktree-includes-applied",
                WorktreeIncludesApplied {
                    workspace_id: workspace_id.to_string(),
                    source: result.source,
                    copied: result.copied,
                },
            );
        }
        Ok(None) => {}
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

    run_checked_setup_commands(
        location,
        workspace_name,
        workspace_id,
        config,
        branch,
        port,
        checkpoint,
        &mut frontend_events(app_handle, workspace_id),
    )
}

/// Progress notifications from the shared setup-command runner core.
///
/// The desktop (`run_setup_pipeline`) maps these to the Tauri
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
        ScriptLocation::Explicit { workspace: workspace_path, root: root_path },
        workspace_name,
        workspace_id,
        config,
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
    location: ScriptLocation<'_>,
    workspace_name: &str,
    workspace_id: &str,
    config: &WorkspaceConfig,
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
        location.root().display()
    );

    let mut env_vars = script_env(location.workspace(), location.root(), branch, port);
    env_vars.push(("CODEMUX_WORKSPACE_NAME", workspace_name.to_string()));
    env_vars.push(("CODEMUX_WORKSPACE_ID", workspace_id.to_string()));
    let total = config.setup.len();

    for (index, command) in config.setup.iter().enumerate() {
        if let Err(refusal) = location.check(checkpoint) {
            return Err(stop_setup(command, &refusal, notify));
        }
        notify(SetupEvent::Progress {
            command,
            index,
            total,
        });

        let output = run_script_command(command, &location, &env_vars, SETUP_SCRIPT)
            .map_err(|e| stop_setup(command, &e.to_string(), notify))?;

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

/// Report why the setup guard stopped the pipeline before it began.
fn report_setup_stopped<R: Runtime>(
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

/// Runs locally saved `commands` in order, stopping at the
/// first failure. It blocks until they finish, so an async caller runs it on
/// the blocking pool. A command can take long while the close waits: once
/// whoever asked for the close lost their authority (`checkpoint`), no
/// further command starts.
pub(crate) fn run_teardown_scripts(
    paths: &LifecyclePaths,
    workspace_name: &str,
    workspace_id: &str,
    commands: &[String],
    checkpoint: &Checkpoint,
) -> Result<(), String> {
    if commands.is_empty() {
        return Ok(());
    }

    let location = ScriptLocation::Checked(paths);
    let mut env_vars = script_env(location.workspace(), location.root(), None, None);
    env_vars.push(("CODEMUX_WORKSPACE_NAME", workspace_name.to_string()));
    env_vars.push(("CODEMUX_WORKSPACE_ID", workspace_id.to_string()));

    for command in commands {
        location
            .check(checkpoint)
            .map_err(|refusal| format!("{refusal}; teardown stopped before `{command}`"))?;
        let output = run_script_command(command, &location, &env_vars, TEARDOWN_SCRIPT)
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
/// These commands can execute repository content, so each one runs as an
/// agent process. The env marker and the ancestry
/// registration make a `codemux` it starts an agent on the control socket,
/// refused admin commands, never the owner.
///
/// The `run` command is not marked. It runs only in the person's own
/// "Workspace Run" terminal when they press Run (`run_project_dev_command`),
/// and a person's terminal is the owner, as for anything they type there.
/// Agents cannot start it: no socket command or tool does, and typing into a
/// terminal is admin.
fn run_script_command(
    command: &str,
    location: &ScriptLocation<'_>,
    env: &[(&str, String)],
    kind: &str,
) -> std::io::Result<Output> {
    let mut cmd = crate::execution::host_command("sh");
    cmd.arg("-c")
        .arg(command)
        .envs(env.iter().map(|(key, value)| (*key, value)))
        .env(crate::control_identity::AGENT_PROCESS_ENV, kind)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = match location {
        ScriptLocation::Explicit { workspace, .. } => cmd.current_dir(workspace).spawn(),
        ScriptLocation::Checked(paths) => paths.spawn(&mut cmd),
    }?;
    crate::control_identity::register_agent_process(child.id());
    child.wait_with_output()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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
                _ => run_teardown_scripts(
                    &LifecyclePaths::resolve(&folder).unwrap(), "ws", "ws-1",
                    &config.teardown, &Checkpoint::default(),
                ),
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
        let paths = LifecyclePaths::resolve(dir.path()).unwrap();
        let mut failed = Vec::new();
        let error = run_checked_setup_commands(ScriptLocation::Checked(&paths), "ws", "ws-1", &config, None, None, &once(), &mut |event| {
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
        let error = run_teardown_scripts(&paths, "ws", "ws-1", &commands, &once()).unwrap_err();
        assert_eq!(error, "grant_revoked: withdrawn; teardown stopped before `touch two`");
        assert!(dir.path().join("one").exists());
        assert!(!dir.path().join("two").exists());
    }

    #[cfg(unix)]
    #[test]
    fn lifecycle_commands_stop_after_workspace_or_project_directory_changes() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        for check_workspace in [true, false] {
            let fixture = tempfile::tempdir().unwrap();
            let project = fixture.path().join("project");
            let workspace = fixture.path().join("workspace");
            fs::create_dir_all(project.join(".git/worktrees/fixture")).unwrap();
            fs::create_dir(&workspace).unwrap();
            fs::write(
                workspace.join(".git"),
                format!("gitdir: {}", project.join(".git/worktrees/fixture").display()),
            )
            .unwrap();
            let paths = LifecyclePaths::resolve(&workspace).unwrap();
            let replace = if check_workspace { workspace.clone() } else { project };
            let backup = fixture.path().join("previous");
            let checks = AtomicUsize::new(0);
            let checkpoint = Checkpoint::new(move || {
                if checks.fetch_add(1, Ordering::SeqCst) == 1 {
                    fs::rename(&replace, &backup).unwrap();
                    fs::create_dir(&replace).unwrap();
                }
                Ok(())
            });
            let commands = vec!["touch first".to_string(), "touch second".to_string()];

            let error = if check_workspace {
                let config = WorkspaceConfig { setup: commands, ..Default::default() };
                run_checked_setup_commands(
                    ScriptLocation::Checked(&paths), "ws", "ws-1", &config,
                    None, None, &checkpoint, &mut |_| {},
                ).unwrap_err()
            } else {
                run_teardown_scripts(
                    &paths, "ws", "ws-1", &commands, &checkpoint,
                ).unwrap_err()
            };

            let changed = if check_workspace { "Workspace" } else { "Project" };
            assert!(error.contains(&format!("{changed} directory changed")), "{error}");
            assert!(workspace.join("first").exists() || fixture.path().join("previous/first").exists());
            assert!(!workspace.join("second").exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn automatic_setup_child_uses_the_held_directory_after_progress() {
        let fixture = tempfile::tempdir().unwrap();
        let workspace = fixture.path().join("workspace");
        let previous = fixture.path().join("previous");
        fs::create_dir(&workspace).unwrap();
        let paths = LifecyclePaths::resolve(&workspace).unwrap();
        let config = WorkspaceConfig {
            setup: vec!["touch marker".into()],
            ..Default::default()
        };
        run_checked_setup_commands(
            ScriptLocation::Checked(&paths), "ws", "ws-1", &config, None, None,
            &Checkpoint::default(), &mut |event| {
                if matches!(event, SetupEvent::Progress { .. }) {
                    fs::rename(&workspace, &previous).unwrap();
                    fs::create_dir(&workspace).unwrap();
                }
            },
        )
        .unwrap();
        assert!(previous.join("marker").exists());
        assert!(!workspace.join("marker").exists());
    }

    fn create_linked_include_fixture(project: &Path, registered: &Path) {
        fs::create_dir(project).unwrap();
        let run_git = |args: &[&str]| {
            let result = crate::git::git_command()
                .current_dir(project)
                .args(args)
                .output()
                .unwrap();
            assert!(result.status.success(), "{result:?}");
        };
        run_git(&["init"]);
        run_git(&[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-m",
            "initial",
        ]);
        run_git(&["worktree", "add", "--detach", registered.to_str().unwrap()]);
    }

    #[test]
    fn automatic_includes_independently_require_a_registered_linked_checkout() {
        let fixture = tempfile::tempdir().unwrap();
        let project = fixture.path().join("project");
        let registered = fixture.path().join("registered");
        let unrelated = fixture.path().join("unrelated");
        create_linked_include_fixture(&project, &registered);
        fs::create_dir(&unrelated).unwrap();
        fs::copy(registered.join(".git"), unrelated.join(".git")).unwrap();
        fs::write(project.join(".env"), "FIXTURE=sample").unwrap();
        fs::write(project.join(".codemuxinclude"), ".env\nsettings/private.env\n").unwrap();
        fs::create_dir(project.join("settings")).unwrap();
        fs::write(project.join("settings/private.env"), "FIXTURE=nested").unwrap();

        let db = crate::database::init_test_database();
        for (checkout, allowed) in [(&registered, true), (&unrelated, false)] {
            let mut paths = LifecyclePaths::resolve(checkout).unwrap();
            assert_eq!(paths.bind_to_workspace(&db, "workspace").is_ok(), allowed);
            assert!(paths.saved_config(&db).is_none());
            let copy = ScriptLocation::Checked(&paths).copy_includes(&[]).unwrap();
            assert_eq!(copy.is_some(), allowed);
            assert_eq!(checkout.join(".env").exists(), allowed);
            assert!(!checkout.join("settings").exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn automatic_include_reads_and_writes_stay_on_the_held_directories() {
        let fixture = tempfile::tempdir().unwrap();
        let project = fixture.path().join("project");
        let checkout = fixture.path().join("checkout");
        let moved_project = fixture.path().join("moved-project");
        let moved_checkout = fixture.path().join("moved-checkout");
        create_linked_include_fixture(&project, &checkout);
        fs::write(project.join(".codemuxinclude"), "fixture.env\n").unwrap();
        fs::write(project.join("fixture.env"), "FIXTURE=sample").unwrap();

        let db = crate::database::init_test_database();
        let mut paths = LifecyclePaths::resolve(&checkout).unwrap();
        paths.bind_to_workspace(&db, "workspace").unwrap();
        assert!(paths.permits_include_copy());
        let directories = paths.include_directories().unwrap();
        fs::rename(&project, &moved_project).unwrap();
        fs::rename(&checkout, &moved_checkout).unwrap();
        fs::create_dir(&project).unwrap();
        fs::create_dir(&checkout).unwrap();
        fs::write(project.join(".codemuxinclude"), "replacement.env\n").unwrap();

        let result = copy_worktree_includes(
            &directories.source,
            &directories.destination,
            &[],
            false,
            |git| {
                git.stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
                paths
                    .spawn_in_include_source(git, &directories)?
                    .wait_with_output()
            },
        )
        .unwrap();
        assert_eq!(result.source, IncludeSource::File);
        assert_eq!(result.copied, ["fixture.env"]);
        assert_eq!(
            fs::read_to_string(moved_checkout.join("fixture.env")).unwrap(),
            "FIXTURE=sample"
        );
        assert!(!checkout.join("fixture.env").exists());
    }

    #[cfg(unix)]
    #[test]
    fn worktree_includes_skip_symlink_files_and_refuse_symlink_parents() {
        use std::os::unix::fs::symlink;

        let from = tempfile::tempdir().unwrap();
        let to = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("fixture.env"), "unchanged").unwrap();
        fs::write(from.path().join("fixture.env"), "fixture").unwrap();
        fs::create_dir(from.path().join("settings")).unwrap();
        fs::write(from.path().join("settings/fixture.env"), "fixture").unwrap();
        symlink(outside.path(), to.path().join("settings")).unwrap();
        symlink(
            outside.path().join("fixture.env"),
            to.path().join("fixture.env"),
        )
        .unwrap();
        symlink(
            outside.path().join("fixture.env"),
            from.path().join("source-link.env"),
        )
        .unwrap();

        let authority = cap_std::ambient_authority();
        let source = Dir::open_ambient_dir(from.path(), authority).unwrap();
        let destination = Dir::open_ambient_dir(to.path(), authority).unwrap();
        assert!(!copy_include(&source, &destination, Path::new("fixture.env")).unwrap());
        assert!(!copy_include(&source, &destination, Path::new("source-link.env")).unwrap());
        assert!(copy_include(&source, &destination, Path::new("settings/fixture.env")).is_err());
        assert_eq!(
            fs::read_to_string(outside.path().join("fixture.env")).unwrap(),
            "unchanged"
        );
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
        let result = process_worktree_includes(root.path(), worktree.path(), &[]);
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
        let result = process_worktree_includes(root.path(), worktree.path(), &patterns).unwrap();

        assert_eq!(result.source, IncludeSource::Setting);
        assert!(result.copied.contains(&".env".to_string()));
        assert_eq!(
            fs::read_to_string(worktree.path().join(".env")).unwrap(),
            "SECRET=abc"
        );
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
        let result = process_worktree_includes(root.path(), worktree.path(), &patterns).unwrap();

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

        let result = process_worktree_includes(root.path(), worktree.path(), &[]).unwrap();
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

        let result = process_worktree_includes(root.path(), worktree.path(), &[]).unwrap();
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
        let result = process_worktree_includes(root.path(), worktree.path(), &[]).unwrap();
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

        let result = process_worktree_includes(root.path(), worktree.path(), &[]).unwrap();
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

        let result = process_worktree_includes(root.path(), worktree.path(), &[]).unwrap();
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
        let result = process_worktree_includes(root.path(), worktree.path(), &[]).unwrap();
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
