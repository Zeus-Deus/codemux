//! Agent-chat provider processes that run on a device over `ssh -T`.
//!
//! Two jobs:
//!
//! - Install the bundled Claude runtime (`codemux-claude-sidecar`) on a
//!   device on first use. It is ~100 MB, so it is uploaded lazily when a
//!   Claude thread first starts there rather than during device setup.
//!   After a desktop update, a device that already has an older runtime
//!   gets the new one in the background ([`refresh_claude_sidecar`]).
//! - Build the SSH spawn for a provider and turn startup failures into
//!   errors that name the device ("not installed on …", "couldn't reach …")
//!   instead of a raw "child process exited".

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use crate::agent_provider::types::RemoteSpawnTarget;
use crate::agent_provider::{ProviderError, ProviderKind};
use crate::json_rpc_child::{RpcChildError, SpawnConfig};
use crate::ssh::exec::{run_remote, sh_path, sh_quote, stdio_spawn_config};

/// Where the Claude runtime lives on a device, next to `codemux-remote`.
const REMOTE_CLAUDE_SIDECAR: &str = "~/.local/bin/codemux-claude-sidecar";

/// Records which local build the device's runtime came from, because the
/// sidecar has no version command of its own.
const REMOTE_CLAUDE_SIDECAR_STAMP: &str = "~/.local/bin/.codemux-claude-sidecar.stamp";

/// Launch the Claude runtime on a device; `$1` is the `claude` CLI as a
/// [`DeviceProgram`]. The SDK only spawns `claude` on the first turn, so
/// check for the CLI up front: a missing install then fails at start with
/// exit 127 instead of mid-conversation.
const CLAUDE_LAUNCH_SCRIPT: &str = "command -v \"$1\" >/dev/null 2>&1 || \
     { echo 'claude: command not found' >&2; exit 127; }; \
     case $1 in */*) PATH=\"${1%/*}:$PATH\"; export PATH;; esac; \
     exec \"$HOME/.local/bin/codemux-claude-sidecar\"";

/// Run the absolute program in `$1` with its own folder first on PATH, so an
/// npm-installed CLI's `#!/usr/bin/env node` finds the `node` beside it.
const DEVICE_EXEC_SCRIPT: &str = "PATH=\"${1%/*}:$PATH\"; export PATH; exec \"$@\"";

/// Budget for the stamp check and stamp write round trips.
const CHECK_BUDGET: Duration = Duration::from_secs(30);

/// Per-target install state: `true` once the device's runtime matched this
/// build. The async mutex serializes concurrent first starts on one device
/// so they don't upload twice.
fn verified_targets() -> &'static Mutex<HashMap<String, Arc<tokio::sync::Mutex<bool>>>> {
    static TARGETS: OnceLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<bool>>>>> =
        OnceLock::new();
    TARGETS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn target_slot(ssh_target: &str) -> Arc<tokio::sync::Mutex<bool>> {
    let mut targets = verified_targets().lock().unwrap_or_else(|e| e.into_inner());
    Arc::clone(targets.entry(ssh_target.to_string()).or_default())
}

/// Devices the background refresh leaves alone for the rest of this run:
/// they never had a runtime, or theirs came from a newer build or another
/// OS or CPU type. A thread start still installs this build's.
fn left_alone() -> &'static Mutex<HashSet<String>> {
    static LEFT_ALONE: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    LEFT_ALONE.get_or_init(Default::default)
}

fn is_left_alone(ssh_target: &str) -> bool {
    left_alone()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains(ssh_target)
}

/// Make sure the device has this build's Claude runtime, uploading it when
/// it is missing or stale. Checked once per target per app run; a
/// background refresh already uploading it is waited for, not repeated.
pub async fn ensure_claude_sidecar(ssh_target: &str) -> Result<(), String> {
    let slot = target_slot(ssh_target);
    let mut verified = slot.lock().await;
    if *verified {
        return Ok(());
    }
    install_claude_sidecar_if_needed(ssh_target).await?;
    *verified = true;
    Ok(())
}

/// Drop the cached check so the next start re-verifies (and reinstalls)
/// the runtime, e.g. after it turned out to be missing at spawn.
pub fn forget_claude_sidecar(ssh_target: &str) {
    let mut targets = verified_targets().lock().unwrap_or_else(|e| e.into_inner());
    targets.remove(ssh_target);
}

/// What [`refresh_claude_sidecar`] did.
#[derive(Debug, PartialEq, Eq)]
pub enum RuntimeRefresh {
    /// The device's runtime already matches this build.
    Current,
    /// An older build's runtime was replaced with this build's.
    Refreshed,
    /// Nothing to do: the device never had a runtime, has a newer build's
    /// or one for another OS or CPU type, this build has none to send, or a
    /// thread start is installing it right now.
    LeftAlone,
}

/// Bring a device's Claude runtime up to this build ahead of its next
/// Claude thread, but only where an older build's runtime is installed.
/// Devices that never ran a Claude thread don't get ~100 MB they may never
/// use, and a newer desktop's runtime isn't downgraded. Once a device has
/// been checked this run, later calls return without SSH.
pub async fn refresh_claude_sidecar(ssh_target: &str) -> Result<RuntimeRefresh, String> {
    if is_left_alone(ssh_target) {
        return Ok(RuntimeRefresh::LeftAlone);
    }
    let slot = target_slot(ssh_target);
    // A thread start holding the slot is already checking or installing it.
    let Ok(mut verified) = slot.try_lock() else {
        return Ok(RuntimeRefresh::LeftAlone);
    };
    if *verified {
        return Ok(RuntimeRefresh::Current);
    }
    let Ok(local) = local_runtime().await else {
        return Ok(RuntimeRefresh::LeftAlone);
    };
    let state = check_device(ssh_target).await?;
    match background_refresh(
        &state,
        &local.stamp,
        crate::agent_provider::claude::sidecar_path::target_triple(),
    ) {
        BackgroundRefresh::Current => {
            *verified = true;
            Ok(RuntimeRefresh::Current)
        }
        BackgroundRefresh::Skip => {
            left_alone()
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(ssh_target.to_string());
            Ok(RuntimeRefresh::LeftAlone)
        }
        BackgroundRefresh::Replace => {
            install_runtime(ssh_target, &local, &state).await?;
            *verified = true;
            Ok(RuntimeRefresh::Refreshed)
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum BackgroundRefresh {
    Current,
    Replace,
    Skip,
}

/// Whether a background refresh should replace the device's runtime. Only
/// one an older (or same-version, differently built) Codemux installed:
/// no stamp means the device never ran a Claude thread, a newer build's
/// stamp means a newer desktop shares the device, and a runtime for
/// another OS or CPU type came from a different kind of computer.
fn background_refresh(
    state: &RemoteSidecarState,
    stamp: &str,
    local_triple: &str,
) -> BackgroundRefresh {
    if state.is_current(stamp) {
        return BackgroundRefresh::Current;
    }
    if state.stamp.is_empty()
        || crate::ssh::bootstrap::target_for_uname(&state.uname) != Some(local_triple)
    {
        return BackgroundRefresh::Skip;
    }
    match (stamp_version(&state.stamp), stamp_version(stamp)) {
        (Some(device), Some(ours)) if device > ours => BackgroundRefresh::Skip,
        _ => BackgroundRefresh::Replace,
    }
}

/// The build version in a stamp from [`stamp_for`].
fn stamp_version(stamp: &str) -> Option<semver::Version> {
    let (version, len) = stamp.rsplit_once('-')?;
    len.parse::<u64>().ok()?;
    semver::Version::parse(version).ok()
}

async fn install_claude_sidecar_if_needed(ssh_target: &str) -> Result<(), String> {
    let local = local_runtime().await?;
    let state = check_device(ssh_target).await?;
    if state.is_current(&local.stamp) {
        return Ok(());
    }
    install_runtime(ssh_target, &local, &state).await
}

/// This build's runtime and the stamp a device gets with it.
struct LocalRuntime {
    path: PathBuf,
    len: u64,
    stamp: String,
}

async fn local_runtime() -> Result<LocalRuntime, String> {
    let path = crate::agent_provider::claude::sidecar_path::resolve_sidecar_path()
        .map_err(|_| "This build doesn't include the Claude runtime".to_string())?;
    let len = tokio::fs::metadata(&path)
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    // Dev builds stage empty placeholders; never ship one to a device.
    if len < crate::ssh::bootstrap::MIN_PLAUSIBLE_REMOTE_BINARY_BYTES {
        return Err("This build doesn't include the Claude runtime".into());
    }
    Ok(LocalRuntime {
        path,
        len,
        stamp: stamp_for(env!("CARGO_PKG_VERSION"), len),
    })
}

async fn check_device(ssh_target: &str) -> Result<RemoteSidecarState, String> {
    let out = run_remote(ssh_target, &check_script(), CHECK_BUDGET)
        .await
        .map_err(|e| format!("Couldn't reach the device over SSH: {e}"))?;
    Ok(parse_check(&out))
}

async fn install_runtime(
    ssh_target: &str,
    local: &LocalRuntime,
    state: &RemoteSidecarState,
) -> Result<(), String> {
    let local_triple = crate::agent_provider::claude::sidecar_path::target_triple();
    if crate::ssh::bootstrap::target_for_uname(&state.uname) != Some(local_triple) {
        return Err(format!(
            "Claude can only run on devices with the same OS and CPU type as this computer ({local_triple})"
        ));
    }

    eprintln!(
        "[codemux::ssh::sidecar] installing the Claude runtime on {ssh_target} ({} bytes)",
        local.len
    );
    crate::ssh::bootstrap::ssh_upload_executable(
        ssh_target,
        REMOTE_CLAUDE_SIDECAR,
        &local.path,
        crate::ssh::bootstrap::UPLOAD_DEADLINE,
    )
    .await
    .map_err(|e| format!("Couldn't install the Claude runtime on the device: {e}"))?;
    let write_stamp = format!(
        "printf '%s\\n' {} > {}",
        sh_quote(&local.stamp),
        sh_path(REMOTE_CLAUDE_SIDECAR_STAMP)
    );
    run_remote(ssh_target, &write_stamp, CHECK_BUDGET)
        .await
        .map_err(|e| format!("Couldn't install the Claude runtime on the device: {e}"))?;
    Ok(())
}

fn stamp_for(version: &str, len: u64) -> String {
    format!("{version}-{len}")
}

/// One round trip: the device's `uname -sm`, its stamp, and whether the
/// runtime is executable.
fn check_script() -> String {
    format!(
        "printf 'UNAME:%s\\n' \"$(uname -sm)\"; \
         printf 'STAMP:%s\\n' \"$(cat {stamp} 2>/dev/null)\"; \
         if [ -x {bin} ]; then echo 'BIN:yes'; else echo 'BIN:no'; fi",
        stamp = sh_path(REMOTE_CLAUDE_SIDECAR_STAMP),
        bin = sh_path(REMOTE_CLAUDE_SIDECAR),
    )
}

#[derive(Debug, Default, PartialEq)]
struct RemoteSidecarState {
    uname: String,
    stamp: String,
    installed: bool,
}

impl RemoteSidecarState {
    fn is_current(&self, stamp: &str) -> bool {
        self.installed && self.stamp == stamp
    }
}

/// Read the check output. Login shells may print banners first, so only the
/// prefixed lines count.
fn parse_check(stdout: &str) -> RemoteSidecarState {
    let mut state = RemoteSidecarState::default();
    for line in stdout.lines().map(str::trim) {
        if let Some(uname) = line.strip_prefix("UNAME:") {
            state.uname = uname.trim().to_string();
        } else if let Some(stamp) = line.strip_prefix("STAMP:") {
            state.stamp = stamp.trim().to_string();
        } else if let Some(bin) = line.strip_prefix("BIN:") {
            state.installed = bin == "yes";
        }
    }
    state
}

/// A provider CLI as named on a device: its absolute path there, or its bare
/// name for the launch PATH to find. Only [`device_program`] makes one, so a
/// path from this computer never reaches a device.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceProgram(String);

impl DeviceProgram {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Name `program` for a device. Only its bare name is used, since a local
/// path means nothing there. The user's shell on the device resolves it to
/// an absolute path when it can; otherwise the launch PATH (with the usual
/// per-user install dirs added) gets the bare name.
pub async fn device_program(
    remote: &RemoteSpawnTarget,
    program: &Path,
) -> Result<DeviceProgram, ProviderError> {
    let name = program
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| ProviderError::ValidationError {
            message: format!("can't run {} on another device", program.display()),
        })?;
    let resolved = crate::ssh::exec::resolve_remote_program(&remote.ssh_target, name).await;
    Ok(DeviceProgram(resolved.unwrap_or_else(|| name.to_string())))
}

/// Spawn the Claude runtime on a device in `cwd`, driving `claude`.
pub fn claude_spawn_config(
    remote: &RemoteSpawnTarget,
    claude: &DeviceProgram,
    env: &HashMap<String, String>,
    cwd: &Path,
    default_timeout: Duration,
) -> Result<SpawnConfig, ProviderError> {
    spawn_on_device(
        remote,
        "sh",
        &[
            "-c".into(),
            CLAUDE_LAUNCH_SCRIPT.into(),
            "sh".into(),
            claude.0.clone(),
        ],
        env,
        cwd,
        default_timeout,
    )
}

/// Spawn a provider CLI on a device in `cwd`.
pub fn device_spawn_config(
    remote: &RemoteSpawnTarget,
    program: &DeviceProgram,
    args: &[String],
    env: &HashMap<String, String>,
    cwd: &Path,
    default_timeout: Duration,
) -> Result<SpawnConfig, ProviderError> {
    if !program.0.contains('/') {
        return spawn_on_device(remote, &program.0, args, env, cwd, default_timeout);
    }
    let mut wrapped = vec![
        "-c".to_string(),
        DEVICE_EXEC_SCRIPT.to_string(),
        "sh".to_string(),
        program.0.clone(),
    ];
    wrapped.extend_from_slice(args);
    spawn_on_device(remote, "sh", &wrapped, env, cwd, default_timeout)
}

fn spawn_on_device(
    remote: &RemoteSpawnTarget,
    program: &str,
    args: &[String],
    env: &HashMap<String, String>,
    cwd: &Path,
    default_timeout: Duration,
) -> Result<SpawnConfig, ProviderError> {
    let cwd = cwd.to_str().ok_or_else(|| ProviderError::ValidationError {
        message: format!("{} isn't a valid folder on the device", cwd.display()),
    })?;
    stdio_spawn_config(
        &remote.ssh_target,
        program,
        args,
        env,
        Some(cwd),
        default_timeout,
    )
    .map_err(|message| ProviderError::ValidationError { message })
}

/// Map a failure to launch the local `ssh` client.
pub fn ssh_spawn_error(error: RpcChildError) -> ProviderError {
    match error {
        RpcChildError::SpawnFailed(ref io) if io.kind() == std::io::ErrorKind::NotFound => {
            ProviderError::ProcessError {
                message: "ssh isn't installed on this computer".into(),
                source: None,
            }
        }
        other => ProviderError::ProcessError {
            message: "Couldn't start ssh".into(),
            source: Some(other.to_string()),
        },
    }
}

/// Why a provider exited while starting on a device.
#[derive(Debug, PartialEq)]
enum DeviceStartFailure {
    /// ssh itself failed (exit 255): unreachable, auth refused, bad host key.
    Unreachable { detail: Option<String> },
    /// The program isn't on the device's PATH (exit 127).
    CommandNotFound { stderr: String },
    /// The thread's folder doesn't exist on the device.
    MissingFolder { detail: Option<String> },
}

/// Exit code and stderr of a child that exited during startup. Requests in
/// flight when the child exits fail with an `RpcError` carrying both in its
/// message (see `JsonRpcChild`'s watchdog), so read them back from there.
fn startup_exit(error: &RpcChildError) -> Option<(Option<i32>, String)> {
    match error {
        RpcChildError::ChildExited { code, stderr_tail } => Some((*code, stderr_tail.clone())),
        RpcChildError::RpcError(rpc) => {
            let rest = rpc.message.strip_prefix("child process exited (code=")?;
            let (code, stderr) = rest.split_once("); stderr: ")?;
            let code = code
                .strip_prefix("Some(")
                .and_then(|c| c.strip_suffix(')'))
                .and_then(|c| c.parse().ok());
            Some((code, stderr.to_string()))
        }
        _ => None,
    }
}

fn last_line(stderr: &str) -> Option<String> {
    stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .last()
        .map(str::to_string)
}

fn device_start_failure(error: &RpcChildError) -> Option<DeviceStartFailure> {
    // The first write raced an ssh client that had already given up
    // (unresolvable host, refused connection); nothing ran on the device.
    if let RpcChildError::IoError(io) = error {
        if io.kind() == std::io::ErrorKind::BrokenPipe {
            return Some(DeviceStartFailure::Unreachable { detail: None });
        }
    }
    let (code, stderr) = startup_exit(error)?;
    match code {
        Some(255) => Some(DeviceStartFailure::Unreachable {
            detail: last_line(&stderr),
        }),
        Some(127) => Some(DeviceStartFailure::CommandNotFound { stderr }),
        _ if stderr.contains("command not found") => {
            Some(DeviceStartFailure::CommandNotFound { stderr })
        }
        // `cd -- <cwd> && exec …` failed before the program ran.
        Some(1 | 2) if stderr.contains("cd: ") => Some(DeviceStartFailure::MissingFolder {
            detail: last_line(&stderr),
        }),
        _ => None,
    }
}

fn unreachable_error(remote: &RemoteSpawnTarget, detail: Option<String>) -> ProviderError {
    ProviderError::ProcessError {
        message: format!("Couldn't reach {} over SSH", remote.host_name),
        source: detail,
    }
}

/// Map a provider that failed while starting on a device to an error that
/// names the device. `None` leaves the caller's usual mapping in charge.
pub fn device_start_error(
    error: &RpcChildError,
    provider: ProviderKind,
    remote: &RemoteSpawnTarget,
    install_hint: String,
) -> Option<ProviderError> {
    let failure = device_start_failure(error)?;
    // The CLI may have moved or been installed since its path was resolved,
    // and an unreachable host never answered the lookup: ask again next time.
    if matches!(
        failure,
        DeviceStartFailure::Unreachable { .. } | DeviceStartFailure::CommandNotFound { .. }
    ) {
        crate::ssh::exec::forget_remote_programs(&remote.ssh_target);
    }
    Some(match failure {
        DeviceStartFailure::Unreachable { detail } => unreachable_error(remote, detail),
        DeviceStartFailure::CommandNotFound { .. } => ProviderError::NotInstalled {
            provider,
            hint: install_hint,
        },
        DeviceStartFailure::MissingFolder { detail } => ProviderError::ProcessError {
            message: format!("This thread's folder doesn't exist on {}", remote.host_name),
            source: detail,
        },
    })
}

/// [`device_start_error`] for Claude, which also needs Codemux's runtime on
/// the device. A runtime that vanished since it was checked is forgotten so
/// the next start reinstalls it.
pub fn claude_start_error(
    error: &RpcChildError,
    remote: &RemoteSpawnTarget,
) -> Option<ProviderError> {
    if let Some(DeviceStartFailure::CommandNotFound { stderr }) = device_start_failure(error) {
        if stderr.contains("codemux-claude-sidecar") {
            forget_claude_sidecar(&remote.ssh_target);
            return Some(ProviderError::ProcessError {
                message: format!(
                    "Codemux's Claude runtime is missing on {}. Try again to reinstall it.",
                    remote.host_name
                ),
                source: None,
            });
        }
    }
    device_start_error(
        error,
        ProviderKind::Claude,
        remote,
        format!(
            "Install Claude Code on {} so `claude` is on your PATH there, then sign in (run `claude` once)",
            remote.host_name
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json_rpc_child::{JsonRpcChild, RpcError};

    fn device() -> RemoteSpawnTarget {
        RemoteSpawnTarget {
            host_id: 3,
            ssh_target: "deus@ai-node".into(),
            host_name: "ai-node".into(),
        }
    }

    fn exited(code: i32, stderr: &str) -> RpcChildError {
        RpcChildError::ChildExited {
            code: Some(code),
            stderr_tail: stderr.into(),
        }
    }

    #[test]
    fn parses_check_output_after_a_banner() {
        let out = "Welcome to ai-node\nUNAME:Linux x86_64\nSTAMP:0.23.1-104857600\nBIN:yes\n";
        let state = parse_check(out);
        assert_eq!(state.uname, "Linux x86_64");
        assert!(state.is_current(&stamp_for("0.23.1", 104_857_600)));
        assert!(!state.is_current(&stamp_for("0.23.2", 104_857_600)));
        assert!(!state.is_current(&stamp_for("0.23.1", 1)));
    }

    fn device_runtime(stamp: &str, installed: bool) -> RemoteSidecarState {
        RemoteSidecarState {
            uname: "Linux x86_64".into(),
            stamp: stamp.into(),
            installed,
        }
    }

    #[test]
    fn background_refresh_replaces_only_an_older_builds_runtime() {
        let ours = stamp_for("0.23.1", 100);
        let triple = "x86_64-unknown-linux-gnu";
        let decide = |state: &RemoteSidecarState| background_refresh(state, &ours, triple);

        assert_eq!(
            decide(&device_runtime(&ours, true)),
            BackgroundRefresh::Current
        );
        assert_eq!(
            decide(&device_runtime("0.22.0-90", true)),
            BackgroundRefresh::Replace
        );
        assert_eq!(
            decide(&device_runtime("0.23.1-99", true)),
            BackgroundRefresh::Replace,
            "same version, different build"
        );
        assert_eq!(
            decide(&device_runtime("0.22.0-90", false)),
            BackgroundRefresh::Replace,
            "it had one before, so it gets it back"
        );
        assert_eq!(
            decide(&device_runtime("", false)),
            BackgroundRefresh::Skip,
            "never upload to a device that never had it"
        );
        assert_eq!(
            decide(&device_runtime("", true)),
            BackgroundRefresh::Skip,
            "not one Codemux installed"
        );
        assert_eq!(
            decide(&device_runtime("0.24.0-120", true)),
            BackgroundRefresh::Skip,
            "never downgrade a newer desktop's runtime"
        );
        assert_eq!(
            decide(&device_runtime("garbage", true)),
            BackgroundRefresh::Replace
        );
        let mac = RemoteSidecarState {
            uname: "Darwin arm64".into(),
            ..device_runtime("0.22.0-90", true)
        };
        assert_eq!(
            decide(&mac),
            BackgroundRefresh::Skip,
            "another OS or CPU type"
        );
    }

    #[test]
    fn stamp_version_reads_pre_releases() {
        assert_eq!(
            stamp_version("0.23.1-dev-5"),
            Some(semver::Version::parse("0.23.1-dev").unwrap())
        );
        assert_eq!(stamp_version("0.23.1"), None, "no length");
        assert_eq!(stamp_version(""), None);
    }

    #[tokio::test]
    async fn refresh_skips_a_device_a_thread_start_is_installing() {
        let target = "refresh-busy@ai-node";
        let slot = target_slot(target);
        let _installing = slot.lock().await;
        assert_eq!(
            refresh_claude_sidecar(target).await,
            Ok(RuntimeRefresh::LeftAlone)
        );
    }

    #[tokio::test]
    async fn refresh_is_free_once_verified_or_left_alone() {
        let verified = "refresh-verified@ai-node";
        *target_slot(verified).lock().await = true;
        assert_eq!(
            refresh_claude_sidecar(verified).await,
            Ok(RuntimeRefresh::Current)
        );

        let skipped = "refresh-skipped@ai-node";
        left_alone().lock().unwrap().insert(skipped.to_string());
        assert_eq!(
            refresh_claude_sidecar(skipped).await,
            Ok(RuntimeRefresh::LeftAlone)
        );
    }

    #[test]
    fn missing_runtime_is_never_current() {
        let state = parse_check("UNAME:Linux x86_64\nSTAMP:0.23.1-5\nBIN:no\n");
        assert!(!state.is_current("0.23.1-5"));
        assert!(!parse_check("").is_current(""));
    }

    #[test]
    fn check_script_reads_stamp_and_binary_under_home() {
        let script = check_script();
        assert!(script.contains("uname -sm"));
        assert!(script.contains("\"$HOME\"/'.local/bin/.codemux-claude-sidecar.stamp'"));
        assert!(script.contains("[ -x \"$HOME\"/'.local/bin/codemux-claude-sidecar' ]"));
    }

    #[test]
    fn claude_spawn_checks_for_the_cli_before_the_runtime() {
        let cfg = claude_spawn_config(
            &device(),
            &DeviceProgram("claude".into()),
            &HashMap::from([("CODEMUX".to_string(), "1".to_string())]),
            Path::new("/home/deus/.codemux/worktrees/repo/feat"),
            Duration::from_secs(20),
        )
        .unwrap();
        assert_eq!(cfg.program, std::path::PathBuf::from("ssh"));
        let script = cfg.args.last().unwrap();
        assert!(script.contains("command -v"));
        assert!(script.contains("codemux-claude-sidecar"));
        assert!(script.contains("/home/deus/.codemux/worktrees/repo/feat"));
    }

    /// Run `script` (the remote launch's `sh -c` body) with `args`, the way
    /// the device's `sh` would.
    fn run_launch(script: &str, args: &[&str]) -> std::process::Output {
        std::process::Command::new("sh")
            .arg("-c")
            .arg(script)
            .arg("sh")
            .args(args)
            .env("HOME", "/nonexistent-codemux-home")
            .output()
            .unwrap()
    }

    #[test]
    fn claude_launch_fails_with_127_when_the_cli_is_missing() {
        for claude in ["codemux-no-such-claude", "/nonexistent/bin/claude"] {
            let out = run_launch(CLAUDE_LAUNCH_SCRIPT, &[claude]);
            assert_eq!(out.status.code(), Some(127), "{claude}");
            assert!(String::from_utf8_lossy(&out.stderr).contains("claude: command not found"));
        }
        // A resolved CLI that exists gets past the check to the runtime.
        let out = run_launch(CLAUDE_LAUNCH_SCRIPT, &["/bin/sh"]);
        assert!(String::from_utf8_lossy(&out.stderr).contains("codemux-claude-sidecar"));
    }

    #[test]
    fn device_spawn_runs_a_bare_name_as_before() {
        let cfg = device_spawn_config(
            &device(),
            &DeviceProgram("codex".into()),
            &["app-server".into()],
            &HashMap::new(),
            Path::new("/srv/repo"),
            Duration::from_secs(20),
        )
        .unwrap();
        let script = cfg.args.last().unwrap();
        // The program is quoted inside the single-quoted `sh -lc` script.
        assert!(
            script.contains(r"'\''codex'\'' '\''app-server'\''"),
            "{script}"
        );
        assert!(!script.contains("${1%/*}"), "{script}");
    }

    #[test]
    fn device_spawn_puts_a_resolved_cli_folder_first_on_path() {
        let cfg = device_spawn_config(
            &device(),
            &DeviceProgram("/home/deus/.nvm/versions/node/v22/bin/codex".into()),
            &["app-server".into()],
            &HashMap::new(),
            Path::new("/srv/repo"),
            Duration::from_secs(20),
        )
        .unwrap();
        let script = cfg.args.last().unwrap();
        assert!(
            script.contains("/home/deus/.nvm/versions/node/v22/bin/codex"),
            "{script}"
        );
        assert!(script.contains("app-server"), "{script}");

        let out = run_launch(
            DEVICE_EXEC_SCRIPT,
            &["/bin/sh", "-c", "printf %s \"$PATH\""],
        );
        assert!(String::from_utf8_lossy(&out.stdout).starts_with("/bin:"));
        let out = run_launch(
            DEVICE_EXEC_SCRIPT,
            &["/nonexistent/bin/codex", "app-server"],
        );
        assert_eq!(out.status.code(), Some(127));
    }

    #[tokio::test]
    async fn device_program_never_sends_a_local_path() {
        let remote = RemoteSpawnTarget {
            ssh_target: "program-test@ai-node".into(),
            ..device()
        };
        // Unsafe names skip the device lookup and keep the bare name.
        let program = device_program(&remote, Path::new("/usr/local/bin/my tool"))
            .await
            .unwrap();
        assert_eq!(program.as_str(), "my tool");
    }

    #[test]
    fn maps_missing_cli_to_not_installed() {
        let err = device_start_error(
            &exited(127, "sh: 1: exec: codex: not found"),
            ProviderKind::Codex,
            &device(),
            "Install Codex on ai-node".into(),
        )
        .unwrap();
        assert!(matches!(
            err,
            ProviderError::NotInstalled { provider: ProviderKind::Codex, ref hint } if hint == "Install Codex on ai-node"
        ));
    }

    #[tokio::test]
    async fn a_missing_cli_is_looked_up_again_on_the_next_start() {
        use crate::ssh::exec::{cache_remote_program, is_remote_program_cached};
        let remote = RemoteSpawnTarget {
            ssh_target: "relookup-test@ai-node".into(),
            ..device()
        };
        cache_remote_program(&remote.ssh_target, "codex", Some("/old/bin/codex"));
        assert_eq!(
            device_program(&remote, Path::new("codex"))
                .await
                .unwrap()
                .as_str(),
            "/old/bin/codex"
        );
        assert!(matches!(
            device_start_error(
                &exited(127, "sh: 1: exec: /old/bin/codex: not found"),
                ProviderKind::Codex,
                &remote,
                "hint".into(),
            ),
            Some(ProviderError::NotInstalled { .. })
        ));
        assert!(!is_remote_program_cached(&remote.ssh_target, "codex"));

        // A folder that's missing says nothing about the CLI.
        cache_remote_program(&remote.ssh_target, "codex", Some("/old/bin/codex"));
        let _ = device_start_error(
            &exited(2, "sh: 1: cd: can't cd to /srv/gone"),
            ProviderKind::Codex,
            &remote,
            String::new(),
        );
        assert!(is_remote_program_cached(&remote.ssh_target, "codex"));
    }

    #[test]
    fn maps_ssh_failure_to_unreachable_with_the_last_stderr_line() {
        let err = device_start_error(
            &exited(
                255,
                "\nssh: connect to host ai-node port 22: Connection refused\n",
            ),
            ProviderKind::Codex,
            &device(),
            String::new(),
        )
        .unwrap();
        match err {
            ProviderError::ProcessError { message, source } => {
                assert_eq!(message, "Couldn't reach ai-node over SSH");
                assert_eq!(
                    source.as_deref(),
                    Some("ssh: connect to host ai-node port 22: Connection refused")
                );
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn maps_missing_folder() {
        let err = device_start_error(
            &exited(2, "sh: 1: cd: can't cd to /srv/gone"),
            ProviderKind::Cursor,
            &device(),
            String::new(),
        )
        .unwrap();
        assert!(
            matches!(err, ProviderError::ProcessError { ref message, .. } if message.contains("folder"))
        );
    }

    #[test]
    fn leaves_other_failures_to_the_caller() {
        assert!(device_start_error(
            &exited(1, "panic: boom"),
            ProviderKind::Codex,
            &device(),
            String::new()
        )
        .is_none());
        let timeout = RpcChildError::Timeout {
            method: "initialize".into(),
            elapsed: Duration::from_secs(20),
        };
        assert!(
            device_start_error(&timeout, ProviderKind::Codex, &device(), String::new()).is_none()
        );
    }

    #[test]
    fn reads_exit_details_from_an_in_flight_request_error() {
        let err = RpcChildError::RpcError(RpcError {
            code: -32000,
            message: "child process exited (code=Some(127)); stderr: claude: command not found"
                .into(),
            data: None,
        });
        assert_eq!(
            startup_exit(&err),
            Some((Some(127), "claude: command not found".to_string()))
        );
        assert!(matches!(
            claude_start_error(&err, &device()),
            Some(ProviderError::NotInstalled {
                provider: ProviderKind::Claude,
                ..
            })
        ));
    }

    #[test]
    fn missing_claude_runtime_is_forgotten_for_reinstall() {
        let remote = RemoteSpawnTarget {
            ssh_target: "forget-test@ai-node".into(),
            ..device()
        };
        let _ = target_slot(&remote.ssh_target);
        let err = claude_start_error(
            &exited(
                127,
                "sh: 1: exec: /home/deus/.local/bin/codemux-claude-sidecar: not found",
            ),
            &remote,
        )
        .unwrap();
        assert!(
            matches!(err, ProviderError::ProcessError { ref message, .. } if message.contains("runtime is missing"))
        );
        assert!(!verified_targets()
            .lock()
            .unwrap()
            .contains_key(&remote.ssh_target));
    }

    #[test]
    fn missing_local_ssh_says_so() {
        let err = ssh_spawn_error(RpcChildError::SpawnFailed(std::io::Error::from(
            std::io::ErrorKind::NotFound,
        )));
        assert!(
            matches!(err, ProviderError::ProcessError { ref message, .. } if message == "ssh isn't installed on this computer")
        );
    }

    /// Pins the in-flight error format `startup_exit` parses against the
    /// real `JsonRpcChild`, so a wording change there fails here.
    #[tokio::test]
    async fn real_child_exit_maps_to_not_installed() {
        let child = JsonRpcChild::spawn(SpawnConfig {
            program: "sh".into(),
            args: vec![
                "-c".into(),
                "sleep 0.3; echo 'sh: 1: exec: cursor-agent: not found' >&2; exit 127".into(),
            ],
            env: HashMap::new(),
            cwd: None,
            default_timeout: Duration::from_secs(10),
        })
        .await
        .unwrap();
        let err = child
            .request("initialize", serde_json::json!({}))
            .await
            .unwrap_err();
        let mapped = device_start_error(
            &err,
            ProviderKind::Cursor,
            &device(),
            "Install Cursor Agent on ai-node".into(),
        );
        assert!(
            matches!(mapped, Some(ProviderError::NotInstalled { .. })),
            "{err:?} mapped to {mapped:?}"
        );
    }
}
