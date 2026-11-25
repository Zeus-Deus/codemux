use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// Stealth Chromium flags that reduce bot detection fingerprinting.
/// Passed via the AGENT_BROWSER_ARGS env var (comma-separated).
const STEALTH_CHROMIUM_ARGS: &str = "\
--disable-blink-features=AutomationControlled,\
--disable-features=AutomationControlled,\
--disable-infobars,\
--no-first-run,\
--no-default-browser-check,\
--disable-background-timer-throttling,\
--disable-backgrounding-occluded-windows,\
--disable-renderer-backgrounding,\
--disable-component-update,\
--disable-hang-monitor,\
--disable-prompt-on-repost,\
--metrics-recording-only,\
--password-store=basic";

/// Detect installed Chrome/Chromium version and return a realistic user-agent string.
fn stealth_user_agent() -> &'static str {
    // The browser version is stable for the lifetime of this process. Probing
    // it for every DOM action adds a subprocess round-trip to every command.
    static USER_AGENT: OnceLock<String> = OnceLock::new();
    USER_AGENT.get_or_init(detect_stealth_user_agent)
}

fn detect_stealth_user_agent() -> String {
    let candidates = ["chromium", "chromium-browser", "google-chrome-stable", "google-chrome"];
    for bin in candidates {
        let mut cmd = std::process::Command::new(bin);
        cmd.arg("--version");
        if let Ok(output) = output_capture_with_timeout(cmd, PROC_CONTROL_TIMEOUT) {
            if output.status.success() {
                let version_str = String::from_utf8_lossy(&output.stdout);
                // Parse version like "Chromium 131.0.6778.204" or "Google Chrome 131.0.6778.204"
                if let Some(ver) = version_str.split_whitespace().last() {
                    return format!(
                        "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{} Safari/537.36",
                        ver.trim()
                    );
                }
            }
        }
    }
    // Fallback to a reasonable default
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36".to_string()
}

fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::new();
    
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = chunk.get(1).copied().unwrap_or(0) as usize;
        let b2 = chunk.get(2).copied().unwrap_or(0) as usize;
        
        result.push(CHARS[b0 >> 2] as char);
        result.push(CHARS[((b0 & 0x03) << 4) | (b1 >> 4)] as char);
        
        if chunk.len() > 1 {
            result.push(CHARS[((b1 & 0x0f) << 2) | (b2 >> 6)] as char);
        } else {
            result.push('=');
        }
        
        if chunk.len() > 2 {
            result.push(CHARS[b2 & 0x3f] as char);
        } else {
            result.push('=');
        }
    }
    
    result
}

// ── External-process timeouts ───────────────────────────────────────────
//
// Every agent-browser CLI / process-control invocation in this module runs
// through `output_capture_with_timeout` or `launch_status_with_timeout` so a
// wedged child can never block the calling task — or the async executor —
// forever. Before this guard a hung `agent-browser` invocation hung
// `cargo test` on the Windows CI image until the 30-minute job timeout
// (issue #96); the production `close()` / `start_stream()` / automation
// paths had the same unbounded-wait failure mode. The async callers also
// run the blocking work through `tokio::task::spawn_blocking` so a slow CLI
// can't stall the runtime's worker threads.

/// Quick process-control commands (kill, probe, `which`, `--version`). These
/// return in well under a second in the normal case; the ceiling only exists
/// so a stuck syscall can't wedge startup/shutdown.
const PROC_CONTROL_TIMEOUT: Duration = Duration::from_secs(5);

/// Graceful daemon shutdown via `agent-browser close`. The issue calls for
/// ~10s here: long enough for a healthy daemon to flush Chromium state,
/// short enough that a wedged one is reaped quickly.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(10);

/// Daemon launch / page-load actions (`open`, `wait`). These legitimately
/// wait on a real page load, so they get a much larger ceiling than the
/// snappy DOM actions.
const OPEN_TIMEOUT: Duration = Duration::from_secs(60);

/// Interactive DOM actions (click, fill, screenshot, snapshot, eval, …).
/// Bounded well above their normal sub-second latency.
const ACTION_TIMEOUT: Duration = Duration::from_secs(30);

/// Per-action timeout for the agent-browser CLI. `open`/`wait` can block on a
/// real page load, so they get [`OPEN_TIMEOUT`]; everything else is a fast,
/// already-connected daemon round-trip bounded by [`ACTION_TIMEOUT`].
fn action_timeout(action: &str) -> Duration {
    match action {
        "open_url" | "open" | "wait" => OPEN_TIMEOUT,
        _ => ACTION_TIMEOUT,
    }
}

/// Drain a child's stdout/stderr pipe on a background thread so a process
/// that fills the OS pipe buffer can't deadlock our wait. Returns the bytes
/// read once the write end closes (the child exits or is killed).
fn drain_stream<R: std::io::Read + Send + 'static>(mut reader: R) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = reader.read_to_end(&mut buf);
        buf
    })
}

/// Forcibly terminate a child that blew its deadline, then reap it.
///
/// On Unix the timed-out child is started in its own process group (via
/// `process_group(0)` in [`spawn_wait_collect`]), so we signal the entire
/// group: this kills `sh -c '<bin> … && <bin> wait …'` *and* the
/// agent-browser process it spawned, not just the `sh` wrapper that would
/// otherwise leave the real CLI orphaned and still wedged. A direct
/// `child.kill()` follows as a backstop (and is the whole story on Windows,
/// where the child is the agent-browser executable itself, not a shell).
fn kill_timed_out_child(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        let pid = child.id() as i32;
        if pid > 0 {
            // Negative pid → deliver SIGKILL to the whole process group.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
    let _ = child.kill();
}

/// Block until `child` exits or `timeout` elapses. On timeout the child (and,
/// on Unix, its process group) is killed and reaped, and an `io::Error` of
/// kind `TimedOut` is returned so callers can treat a wedged CLI exactly like
/// a spawn failure. Polls with `try_wait` rather than taking a new crate
/// dependency; the 20ms cadence is irrelevant next to multi-second CLI
/// latencies.
fn wait_with_timeout(
    child: &mut std::process::Child,
    timeout: Duration,
) -> std::io::Result<std::process::ExitStatus> {
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if start.elapsed() >= timeout {
            kill_timed_out_child(child);
            // Reap the (now-killed) child so it doesn't linger as a zombie.
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("process exceeded {timeout:?} timeout and was killed"),
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Core timeout runner: spawns an already-stdio-configured command, drains
/// whichever of stdout/stderr the caller piped (so a full pipe can't
/// deadlock the wait), and enforces `timeout` via [`wait_with_timeout`].
///
/// On the timeout path the reader threads are deliberately NOT joined: if a
/// detached daemon inherited a pipe the read would never see EOF. They end on
/// their own once the write end finally closes (immediately for the
/// group-killed common case). Joining only on the success path is safe
/// because the child has fully exited there.
fn spawn_wait_collect(
    command: std::process::Command,
    timeout: Duration,
) -> std::io::Result<std::process::Output> {
    spawn_wait_collect_with_child(command, timeout, |_| Ok(()))
}

fn spawn_wait_collect_with_child(
    mut command: std::process::Command,
    timeout: Duration,
    on_spawn: impl FnOnce(&std::process::Child) -> std::io::Result<()>,
) -> std::io::Result<std::process::Output> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Own process group so a timeout can SIGKILL the whole `sh -c …`
        // subtree, not just the shell wrapper.
        command.process_group(0);
    }
    let mut child = command.spawn()?;
    if let Err(error) = on_spawn(&child) {
        kill_timed_out_child(&mut child);
        let _ = child.wait();
        return Err(error);
    }
    let out_reader = child.stdout.take().map(drain_stream);
    let err_reader = child.stderr.take().map(drain_stream);

    let status = wait_with_timeout(&mut child, timeout)?;

    let stdout = out_reader
        .map(|h| h.join().unwrap_or_default())
        .unwrap_or_default();
    let stderr = err_reader
        .map(|h| h.join().unwrap_or_default())
        .unwrap_or_default();
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

/// `Command::output()` with a hard timeout. Captures stdout+stderr and kills
/// the child on overrun. Drop-in replacement for the unbounded `.output()`
/// calls that previously hung the browser teardown / automation paths.
fn output_capture_with_timeout(
    mut command: std::process::Command,
    timeout: Duration,
) -> std::io::Result<std::process::Output> {
    command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    spawn_wait_collect(command, timeout)
}

/// Timeout-bounded spawn for daemon-launch commands (`agent-browser open …`).
/// Unlike [`output_capture_with_timeout`] this discards stdout (`/dev/null`)
/// and leaves stderr as the caller configured it (a per-session log file, or
/// inherited) — the launcher forks a long-lived daemon, and piping its std
/// handles risks a reader that never sees EOF. Returns the launcher's exit
/// status, or `TimedOut` if it wedged.
fn launch_status_with_timeout(
    mut command: std::process::Command,
    timeout: Duration,
) -> std::io::Result<std::process::ExitStatus> {
    command.stdout(std::process::Stdio::null());
    // stderr intentionally left untouched so a caller-set log file / inherited
    // handle is preserved; it is a file or inherited fd, never a pipe we own.
    spawn_wait_collect(command, timeout).map(|o| o.status)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserAutomationResult {
    pub request_id: String,
    pub browser_id: String,
    pub data: serde_json::Value,
    pub message: Option<String>,
}

/// Default stream port for the browser screencast WebSocket.
pub const DEFAULT_STREAM_PORT: u16 = 9223;
/// Maximum stream port (inclusive). Ports 9223–9299 are reserved.
const MAX_STREAM_PORT: u16 = 9299;

/// Stop only browser daemons launched by this app process. A matching process
/// name or listening port does not establish ownership.
pub fn kill_stream_daemons() {
    for dir in browser_runtime_dirs() {
        kill_owned_browser_daemons_in(&dir);
    }
}

fn kill_owned_browser_daemons_in(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let bin = resolve_binary();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(session) = session_from_pid_path(&path) else { continue };
        let Some(record) = read_browser_pid_record(&path) else { continue };
        if record.token != browser_owner_token() || !browser_process_matches(&record, session) {
            continue;
        }
        if let Ok(bin) = &bin {
            stop_owned_browser_daemon(session, &record, bin);
        } else {
            kill_verified_browser_pid(&record, session);
        }
        if !pid_alive(record.pid) {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Directory where codemux stores per-session runtime files (PID + log
/// files for each agent-browser daemon). Unix uses a private directory under
/// XDG_RUNTIME_DIR when a session's home-based socket path would be too long.
/// Windows uses a
/// new private directory because older releases created `~/.codemux/run`
/// with an inherited ACL.
///
/// The directory is created lazily on first use. Returns `None` when neither
/// a usable runtime root nor a home directory is available.
pub fn run_dir() -> Option<PathBuf> {
    run_dir_for_session("default")
}

fn run_dir_for_session(session: &str) -> Option<PathBuf> {
    validate_session_name(session).ok()?;
    #[cfg(unix)]
    {
        let runtime_root = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
        let home = dirs::home_dir();
        return run_dir_for_roots(home.as_deref(), runtime_root.as_deref(), session);
    }
    #[cfg(not(unix))]
    {
        let dir = browser_run_dir_for(&dirs::home_dir()?);
        prepare_run_dir(&dir)?;
        Some(dir)
    }
}

#[cfg(windows)]
fn browser_run_dir_for(home: &std::path::Path) -> PathBuf {
    home.join(".codemux-browser-run")
}

#[cfg(not(windows))]
fn browser_run_dir_for(home: &std::path::Path) -> PathBuf {
    home.join(".codemux").join("run")
}

#[cfg(unix)]
fn private_runtime_run_dir(runtime_root: &std::path::Path) -> Option<PathBuf> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    if !runtime_root.is_absolute() {
        return None;
    }
    let meta = std::fs::symlink_metadata(runtime_root).ok()?;
    if !meta.is_dir()
        || meta.file_type().is_symlink()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.permissions().mode() & 0o077 != 0
    {
        return None;
    }
    // Keep the path shorter than agent-browser's former default subdirectory
    // so a 103-byte control socket still fits at the boundary.
    Some(runtime_root.join("cmx-browser"))
}

#[cfg(unix)]
fn run_dir_for_roots(home: Option<&std::path::Path>, runtime_root: Option<&std::path::Path>, session: &str) -> Option<PathBuf> {
    let home_dir = home.map(browser_run_dir_for);
    if let Some(home_dir) = &home_dir {
        if browser_socket_path_fits(home_dir, session) && prepare_run_dir(home_dir).is_some() {
            return Some(home_dir.clone());
        }
    }
    if let Some(short_dir) = runtime_root.and_then(private_runtime_run_dir) {
        if browser_socket_path_fits(&short_dir, session) && prepare_run_dir(&short_dir).is_some() {
            return Some(short_dir);
        }
    }
    let home_dir = home_dir?;
    prepare_run_dir(&home_dir)?;
    Some(home_dir)
}

fn browser_runtime_dirs() -> Vec<PathBuf> {
    #[cfg(unix)]
    {
        let mut dirs = Vec::new();
        if let Some(home) = dirs::home_dir() {
            let home_dir = browser_run_dir_for(&home);
            if prepare_run_dir(&home_dir).is_some() {
                dirs.push(home_dir);
            }
        }
        if let Some(short_dir) = std::env::var_os("XDG_RUNTIME_DIR")
            .as_deref()
            .map(std::path::Path::new)
            .and_then(private_runtime_run_dir)
        {
            if prepare_run_dir(&short_dir).is_some() && !dirs.contains(&short_dir) {
                dirs.push(short_dir);
            }
        }
        dirs
    }
    #[cfg(not(unix))]
    {
        run_dir().into_iter().collect()
    }
}

#[cfg(unix)]
fn browser_socket_path_fits(dir: &std::path::Path, session: &str) -> bool {
    use std::os::unix::ffi::OsStrExt;
    dir.join(format!("{session}.sock")).as_os_str().as_bytes().len() <= 103
}

fn prepare_run_dir(dir: &std::path::Path) -> Option<()> {
    prepare_browser_directory(dir.parent()?, false)?;
    prepare_browser_directory(dir, true)
}

fn prepare_browser_directory(dir: &std::path::Path, private: bool) -> Option<()> {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    #[cfg(windows)]
    let create_result = if private { clack_private_fs::create_dir(dir) } else { builder.create(dir) };
    #[cfg(not(windows))]
    let create_result = builder.create(dir);
    match create_result {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return None,
    }
    let meta = std::fs::symlink_metadata(dir).ok()?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if meta.uid() != unsafe { libc::geteuid() } {
            return None;
        }
        if !private && meta.permissions().mode() & 0o022 != 0 {
            return None;
        }
        if private && meta.permissions().mode() & 0o777 != 0o700 {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).ok()?;
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // Junctions are reparse points, even when FileType::is_symlink()
        // does not recognize them as symbolic links.
        if meta.file_attributes() & 0x400 != 0 {
            return None;
        }
        if private {
            clack_private_fs::require_private_directory(dir).ok()?;
        }
    }
    #[cfg(not(unix))]
    let _ = private;
    Some(())
}

fn open_session_log(path: &std::path::Path) -> Option<std::fs::File> {
    #[cfg(windows)]
    {
        // Existing logs may predate private-file creation. Only append when
        // their ACL still grants access solely to this account.
        return match clack_private_fs::create_new_file(path) {
            Ok(file) => Some(file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                clack_private_fs::require_private_file(path).ok()?;
                std::fs::OpenOptions::new().append(true).open(path).ok()
            }
            Err(_) => None,
        };
    }

    #[cfg(not(windows))]
    {
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600)).ok()?;
    }
    Some(file)
    }
}

/// Path to the per-session PID file written by codemux when it spawns
/// the agent-browser daemon. Used by `close()` for targeted PID-based
/// teardown and by `new_with_adoption()` for safe restart adoption.
pub fn session_pid_path(session_name: &str) -> Option<PathBuf> {
    validate_session_name(session_name).ok()?;
    Some(run_dir_for_session(session_name)?.join(format!("agent-browser-{session_name}.pid")))
}

/// Path to the per-session stderr log file. Captured during daemon
/// spawn so the next investigation has a paper trail.
fn session_log_path(session_name: &str) -> Option<PathBuf> {
    validate_session_name(session_name).ok()?;
    Some(run_dir_for_session(session_name)?.join(format!("agent-browser-{session_name}.log")))
}

/// Read the agent-browser daemon's own PID file in our private run directory.
/// We set AGENT_BROWSER_SOCKET_DIR on every CLI invocation so the control
/// socket and PID file cannot silently move to XDG_RUNTIME_DIR or HOME.
fn agent_browser_daemon_pid_path(session_name: &str) -> Option<PathBuf> {
    validate_session_name(session_name).ok()?;
    Some(run_dir_for_session(session_name)?.join(format!("{session_name}.pid")))
}

fn read_agent_browser_daemon_pid(session_name: &str) -> Option<u32> {
    let path = agent_browser_daemon_pid_path(session_name)?;
    read_daemon_pid_file(&path)
}

fn read_daemon_pid_file(path: &std::path::Path) -> Option<u32> {
    let meta = std::fs::symlink_metadata(&path).ok()?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return None;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return None;
        }
    }
    let content = std::fs::read_to_string(path).ok()?;
    content.trim().parse::<u32>().ok()
}

#[cfg(windows)]
fn cleanup_stale_browser_lock_files(session: &str, pid: u32) {
    let Some(dir) = run_dir_for_session(session) else { return };
    cleanup_stale_browser_lock_files_in(&dir, session, pid);
}

fn cleanup_stale_browser_lock_files_in(dir: &std::path::Path, session: &str, pid: u32) {
    if validate_session_name(session).is_err() || pid_alive(pid) {
        return;
    }
    let pid_path = dir.join(format!("{session}.pid"));
    for ext in ["port", "engine", "stream", "version", "config", "lock", "pid"] {
        // The daemon may have restarted under this session name. Recheck
        // before touching each entry and remove the PID file last.
        if pid_alive(pid) || read_daemon_pid_file(&pid_path) != Some(pid) {
            break;
        }
        let _ = std::fs::remove_file(dir.join(format!("{session}.{ext}")));
    }
}

#[cfg(windows)]
fn cleanup_owned_browser_lock_files(session: &str, pid: u32) {
    if validate_session_name(session).is_err() {
        return;
    }
    for _ in 0..10 {
        if !pid_alive(pid) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    cleanup_stale_browser_lock_files(session, pid);
}

#[derive(Clone)]
struct BrowserPidRecord {
    pid: u32,
    start_time: u64,
    token: String,
}

fn browser_owner_token() -> &'static str {
    static TOKEN: OnceLock<String> = OnceLock::new();
    TOKEN.get_or_init(|| uuid::Uuid::new_v4().simple().to_string())
}

fn set_browser_owner_env(command: &mut std::process::Command, session: &str) -> Result<(), String> {
    let socket_dir = run_dir_for_session(session).ok_or("Cannot prepare private browser control directory")?;
    #[cfg(unix)]
    if !browser_socket_path_fits(&socket_dir, session) {
        return Err(format!(
            "Browser session {session} exceeds the 103-byte Unix control socket path limit"
        ));
    }
    command
        .env("CODEMUX_BROWSER_OWNER", browser_owner_token())
        .env("CODEMUX_BROWSER_SESSION", session)
        .env("AGENT_BROWSER_SOCKET_DIR", socket_dir)
        .env_remove("AGENT_BROWSER_NAMESPACE");
    Ok(())
}

fn session_from_pid_path(path: &std::path::Path) -> Option<&str> {
    let name = path.file_name()?.to_str()?;
    let session = name.strip_prefix("agent-browser-")?.strip_suffix(".pid")?;
    validate_session_name(session).ok()?;
    Some(session)
}

fn read_browser_pid_record(path: &std::path::Path) -> Option<BrowserPidRecord> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if meta.uid() != unsafe { libc::geteuid() } || meta.permissions().mode() & 0o077 != 0 {
            return None;
        }
    }
    #[cfg(windows)]
    clack_private_fs::require_private_file(path).ok()?;
    let content = std::fs::read_to_string(path).ok()?;
    let mut fields = content.split_whitespace();
    let pid = fields.next()?.parse::<u32>().ok()?;
    let start_time = fields.next()?.parse::<u64>().ok()?;
    let token = fields.next()?;
    if pid == 0
        || start_time == 0
        || token.len() != 32
        || !token.bytes().all(|byte| byte.is_ascii_hexdigit())
        || fields.next().is_some()
    {
        return None;
    }
    Some(BrowserPidRecord { pid, start_time, token: token.to_owned() })
}

fn browser_process_start(pid: u32, session: &str, token: &str) -> Option<u64> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

    let process_id = Pid::from_u32(pid);
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[process_id]),
        true,
        ProcessRefreshKind::nothing()
            .with_exe(UpdateKind::Always)
            .with_environ(UpdateKind::Always),
    );
    let process = system.process(process_id)?;
    let executable = process.exe()?.file_name()?.to_string_lossy().to_ascii_lowercase();
    if !executable.starts_with("agent-browser") {
        return None;
    }
    let expected_owner = format!("CODEMUX_BROWSER_OWNER={token}");
    let expected_session = format!("CODEMUX_BROWSER_SESSION={session}");
    let has_owner = process.environ().iter().any(|entry| entry.to_string_lossy() == expected_owner);
    let has_session = process.environ().iter().any(|entry| entry.to_string_lossy() == expected_session);
    (has_owner && has_session && process.start_time() > 0).then_some(process.start_time())
}

fn browser_process_matches(record: &BrowserPidRecord, session: &str) -> bool {
    browser_process_start(record.pid, session, &record.token) == Some(record.start_time)
}

fn owned_browser_pid(session: &str, pid: u32) -> Option<BrowserPidRecord> {
    if let Some(start_time) = browser_process_start(pid, session, browser_owner_token()) {
        return Some(BrowserPidRecord {
            pid,
            start_time,
            token: browser_owner_token().to_owned(),
        });
    }
    let path = session_pid_path(session)?;
    let record = read_browser_pid_record(&path)?;
    (record.pid == pid && browser_process_matches(&record, session)).then_some(record)
}

fn browser_control_endpoint_ready(session: &str, dir: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        std::os::unix::net::UnixStream::connect(dir.join(format!("{session}.sock"))).is_ok()
    }
    #[cfg(windows)]
    {
        // v0.44 only connects to the published .port entry. A missing file
        // leaves no reachable control endpoint, even if the old session hash
        // happens to collide with an unrelated listener.
        let Some(port) = browser_control_port(session, dir) else { return false };
        std::net::TcpStream::connect_timeout(
            &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
            Duration::from_millis(50),
        ).is_ok()
    }
}

#[cfg(windows)]
fn browser_control_port(session: &str, dir: &std::path::Path) -> Option<u16> {
    let path = dir.join(format!("{session}.port"));
    std::fs::read_to_string(path)
        .ok()
        .and_then(|value| value.trim().parse::<u16>().ok())
}

#[cfg(windows)]
fn browser_control_listener_owned_by_pid(session: &str, dir: &std::path::Path, pid: u32) -> bool {
    browser_control_port(session, dir).is_some_and(|port| listener_owned_by_pid(port, pid))
}

fn browser_daemon_version(session: &str, dir: &std::path::Path) -> Option<semver::Version> {
    let path = dir.join(format!("{session}.version"));
    let meta = std::fs::symlink_metadata(&path).ok()?;
    if !meta.is_file() || meta.file_type().is_symlink() {
        return None;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return None;
        }
    }
    semver::Version::parse(std::fs::read_to_string(path).ok()?.trim()).ok()
}

fn browser_daemon_has_protected_stream(session: &str, dir: &std::path::Path) -> bool {
    browser_daemon_version(session, dir)
        .is_some_and(|version| version >= semver::Version::new(0, 44, 0))
}

fn require_owned_browser_session(session: &str) -> Result<(), String> {
    validate_session_name(session)?;
    let dir = run_dir_for_session(session).ok_or("Cannot prepare private browser control directory")?;
    require_owned_browser_session_in(session, &dir)
}

fn require_owned_browser_session_in(session: &str, dir: &std::path::Path) -> Result<(), String> {
    let path = dir.join(format!("{session}.pid"));
    match std::fs::symlink_metadata(&path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if browser_control_endpoint_ready(session, &dir) {
                return Err(format!("Browser session {session} has a live control endpoint without an owner PID"));
            }
            return Ok(());
        }
        Err(error) => return Err(format!("Cannot inspect browser session {session}: {error}")),
    }
    let pid = read_daemon_pid_file(&path)
        .ok_or_else(|| format!("Browser session {session} has an invalid daemon PID file"))?;
    if !pid_alive(pid) {
        if browser_control_endpoint_ready(session, &dir) {
            return Err(format!("Browser session {session} has a live control endpoint with a stale owner PID"));
        }
        cleanup_stale_browser_lock_files_in(dir, session, pid);
        if read_daemon_pid_file(&path) == Some(pid) {
            return Err(format!("Could not remove stale browser session {session}"));
        }
        return Ok(());
    }
    if owned_browser_pid(session, pid).is_none() {
        return Err(format!("Browser session {session} is already used by another process"));
    }
    #[cfg(windows)]
    if !browser_control_listener_owned_by_pid(session, dir, pid) {
        return Err(format!("Browser session {session} has a control port not owned by its daemon"));
    }
    Ok(())
}

fn stop_owned_browser_daemon(session: &str, record: &BrowserPidRecord, bin: &str) {
    if !browser_process_matches(record, session) {
        return;
    }
    // Windows stops through a retained, identity-checked process handle. A
    // missing or replaced PID file must not prevent cleanup of that process.
    #[cfg(not(windows))]
    if read_agent_browser_daemon_pid(session) != Some(record.pid) {
        return;
    }
    // On Windows the CLI resolves its control port from a separate file. It
    // could send `close` to an unrelated listener after the PID check, so
    // only terminate the repeatedly verified process tree below.
    #[cfg(windows)]
    let _ = bin;
    #[cfg(not(windows))]
    {
        let mut cmd = std::process::Command::new("sh");
        if set_browser_owner_env(&mut cmd, session).is_ok() {
            cmd.args([
                "-c",
                &format!("{} close --session {} 2>/dev/null", shell_binary(bin), shell_quote(session)),
            ]);
            let _ = output_capture_with_timeout(cmd, CLOSE_TIMEOUT);
        }
    }
    #[cfg(windows)]
    if let Some(handle) = open_verified_browser_handle(record, session) {
        kill_verified_browser_handle(&handle, record.pid);
        cleanup_owned_browser_lock_files(session, record.pid);
    }
    #[cfg(not(windows))]
    if browser_process_matches(record, session) {
        kill_verified_browser_pid(record, session);
    }
}

#[cfg(windows)]
fn open_verified_browser_handle(record: &BrowserPidRecord, session: &str) -> Option<std::os::windows::io::OwnedHandle> {
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, PROCESS_SYNCHRONIZE};
    use windows_sys::Win32::Foundation::WAIT_TIMEOUT;

    let raw = unsafe {
        OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, 0, record.pid)
    };
    if raw.is_null() {
        return None;
    }
    let handle = unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(raw) };
    if process_creation_seconds(raw)? != record.start_time
        || unsafe { WaitForSingleObject(raw, 0) } != WAIT_TIMEOUT
        || !browser_process_matches(record, session)
    {
        return None;
    }
    // Keeping this handle open prevents Windows from recycling the PID even
    // if the daemon exits before its descendants are torn down.
    Some(handle)
}

#[cfg(windows)]
fn process_creation_seconds(handle: windows_sys::Win32::Foundation::HANDLE) -> Option<u64> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::GetProcessTimes;

    let mut created = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
    let mut exited = created;
    let mut kernel = created;
    let mut user = created;
    if unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) } == 0 {
        return None;
    }
    let ticks = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
    (ticks / 10_000_000).checked_sub(11_644_473_600)
}

#[cfg(windows)]
fn kill_verified_browser_handle(handle: &std::os::windows::io::OwnedHandle, pid: u32) {
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::{TerminateProcess, WaitForSingleObject};
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    // The retained process handle keeps `pid` reserved while taskkill walks
    // its Chromium children. Terminate the verified process through that same
    // handle even if taskkill fails or the daemon outlives its children.
    let mut cmd = std::process::Command::new("taskkill");
    cmd.args(["/PID", &pid.to_string(), "/T", "/F"])
        .creation_flags(CREATE_NO_WINDOW);
    let _ = output_capture_with_timeout(cmd, PROC_CONTROL_TIMEOUT);
    let raw = handle.as_raw_handle();
    unsafe {
        TerminateProcess(raw, 1);
        WaitForSingleObject(raw, 1_000);
    }
}

#[cfg(not(windows))]
fn kill_verified_browser_pid(record: &BrowserPidRecord, session: &str) {
    #[cfg(unix)]
    {
        let Ok(pid) = i32::try_from(record.pid) else { return };
        if browser_process_matches(record, session) {
            // SAFETY: the PID is positive and was checked against the live
            // process identity immediately before signalling.
            unsafe { libc::kill(pid, libc::SIGTERM) };
        }
        std::thread::sleep(Duration::from_millis(200));
        if browser_process_matches(record, session) {
            // Recheck after the grace period so a recycled PID is never sent
            // the escalation signal.
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
    }
}

/// Write the tracked PID to our own per-session PID file under
/// `~/.codemux/run/`. Callers launching a daemon must handle a write failure
/// before sending the first command, or they could leave it untracked.
fn write_session_pid(session_name: &str, record: &BrowserPidRecord) -> std::io::Result<()> {
    use std::io::Write;

    let path = session_pid_path(session_name)
        .ok_or_else(|| std::io::Error::other("Cannot prepare private browser PID path"))?;
    let content = format!("{} {} {}\n", record.pid, record.start_time, record.token);
    #[cfg(windows)]
    let result = (|| -> std::io::Result<()> {
        let mut file = match clack_private_fs::create_new_file(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                clack_private_fs::require_private_file(&path)?;
                std::fs::OpenOptions::new().write(true).truncate(true).open(&path)?
            }
            Err(error) => return Err(error),
        };
        file.write_all(content.as_bytes())?;
        file.sync_all()
    })();
    #[cfg(not(windows))]
    let result = (|| -> std::io::Result<()> {
        let parent = path.parent().ok_or_else(|| std::io::Error::other("PID path has no parent"))?;
        let mut staged = tempfile::NamedTempFile::new_in(parent)?;
        staged.write_all(content.as_bytes())?;
        staged.as_file().sync_all()?;
        staged.persist(&path).map_err(|error| error.error)?;
        Ok(())
    })();
    if let Err(error) = &result {
        eprintln!(
            "[codemux::browser] failed to write PID file {}: {error}",
            path.display()
        );
    }
    result
}

fn track_current_browser_daemon(session: &str) -> Option<BrowserPidRecord> {
    let pid = read_agent_browser_daemon_pid(session)?;
    let record = owned_browser_pid(session, pid)?;
    if record.token == browser_owner_token() {
        let _ = write_session_pid(session, &record);
    }
    Some(record)
}

#[cfg(not(windows))]
fn failed_open_daemon_started_here(
    current: &BrowserPidRecord,
    before_open: Option<&BrowserPidRecord>,
) -> bool {
    current.token == browser_owner_token()
        && before_open.is_none_or(|previous| {
            previous.pid != current.pid || previous.start_time != current.start_time
        })
}

#[cfg(not(windows))]
fn rollback_failed_unix_open(session: &str, before_open: Option<&BrowserPidRecord>) {
    if let Some(record) = track_current_browser_daemon(session)
        .filter(|record| failed_open_daemon_started_here(record, before_open))
    {
        // Signal this exact process. A session-wide CLI `close` could race a
        // second app that starts a replacement at the same control socket.
        kill_verified_browser_pid(&record, session);
    }
}

fn live_owned_browser_pid(session: &str) -> Option<u32> {
    let pid = read_agent_browser_daemon_pid(session)?;
    owned_browser_pid(session, pid).map(|record| record.pid)
}

/// Remove our PID file once the daemon is gone. Best-effort.
fn clear_session_pid(session_name: &str) {
    if let Some(path) = session_pid_path(session_name) {
        let _ = std::fs::remove_file(path);
    }
}

/// Process-tree helper for other managed Windows children. Browser daemons
/// use `kill_verified_browser_handle` so their PID stays reserved.
#[cfg(windows)]
pub(crate) fn kill_pid(pid: u32) {
    if pid == 0 {
        return;
    }
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut cmd = std::process::Command::new("taskkill");
    cmd.args(["/PID", &pid.to_string(), "/T", "/F"])
        .creation_flags(CREATE_NO_WINDOW);
    let _ = output_capture_with_timeout(cmd, PROC_CONTROL_TIMEOUT);
}

/// Probe whether a stream port is actually accepting connections.
/// Used as a health check after spawning a daemon — `running = true`
/// only gets set once we've seen the port respond, instead of trusting
/// a 1-second sleep.
///
/// Tries up to `max_attempts` connections with a short delay between
/// each. Returns true on the first successful TCP handshake. Connection
/// is immediately dropped — the real WebSocket connection happens later
/// from the BrowserPane.
fn probe_stream_port(port: u16, max_attempts: u32, delay: Duration) -> bool {
    for _ in 0..max_attempts {
        if std::net::TcpStream::connect_timeout(
            &([127, 0, 0, 1], port).into(),
            Duration::from_millis(500),
        )
        .is_ok()
        {
            return true;
        }
        std::thread::sleep(delay);
    }
    false
}

/// Require the IPv4 loopback stream listener to belong to the verified
/// daemon. A successful TCP probe alone could be an unrelated process that
/// claimed the port between allocation and browser startup.
fn listener_owned_by_pid(port: u16, pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        let Ok(table) = std::fs::read_to_string("/proc/net/tcp") else { return false };
        let address = format!("0100007F:{port:04X}");
        let inodes: std::collections::HashSet<&str> = table
            .lines()
            .filter_map(|line| {
                let fields: Vec<_> = line.split_whitespace().collect();
                (fields.len() > 9 && fields[1] == address && fields[3] == "0A")
                    .then_some(fields[9])
            })
            .collect();
        if inodes.is_empty() {
            return false;
        }
        let Ok(fds) = std::fs::read_dir(format!("/proc/{pid}/fd")) else { return false };
        fds.flatten().any(|fd| {
            let Ok(target) = std::fs::read_link(fd.path()) else { return false };
            let target = target.to_string_lossy();
            target.strip_prefix("socket:[")
                .and_then(|inode| inode.strip_suffix(']'))
                .is_some_and(|inode| inodes.contains(inode))
        })
    }
    #[cfg(target_os = "macos")]
    {
        let mut command = std::process::Command::new("lsof");
        command.args([
            "-nP", "-a", "-p", &pid.to_string(),
            &format!("-iTCP:{port}"), "-sTCP:LISTEN", "-F", "n",
        ]);
        output_capture_with_timeout(command, PROC_CONTROL_TIMEOUT)
            .ok()
            .filter(|output| output.status.success())
            .is_some_and(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .any(|line| line == format!("n127.0.0.1:{port}"))
            })
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut command = std::process::Command::new("netstat");
        command.args(["-ano", "-p", "TCP"]).creation_flags(CREATE_NO_WINDOW);
        output_capture_with_timeout(command, PROC_CONTROL_TIMEOUT)
            .ok()
            .filter(|output| output.status.success())
            .is_some_and(|output| {
                String::from_utf8_lossy(&output.stdout).lines().any(|line| {
                    let fields: Vec<_> = line.split_whitespace().collect();
                    fields.len() >= 5
                        && fields[0] == "TCP"
                        && fields[1] == format!("127.0.0.1:{port}")
                        && fields[3] == "LISTENING"
                        && fields[4].parse::<u32>() == Ok(pid)
                })
            })
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = port;
        false
    }
}

/// Open the stream socket and verify the process on the server side of that
/// exact connection before either proxy sends an HTTP or WebSocket request.
pub(crate) async fn connect_verified_stream_peer(
    identity: BrowserStreamIdentity,
) -> std::io::Result<tokio::net::TcpStream> {
    let stream = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::net::TcpStream::connect(("127.0.0.1", identity.port)),
    )
    .await
    .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "Browser stream connection timed out"))??;
    let local = stream.local_addr()?;
    let peer = stream.peer_addr()?;
    let owned = tokio::task::spawn_blocking(move || {
        // On Unix the accepted socket may not appear in the daemon's file
        // descriptors until its accept loop runs. Bound the retry and fail
        // closed if the established peer never belongs to the daemon.
        for _ in 0..10 {
            if connected_stream_peer_owned_by_identity(local, peer, identity) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    })
    .await
    .map_err(std::io::Error::other)?;
    if !owned {
        return Err(std::io::Error::other("Browser stream connection belongs to another process"));
    }
    Ok(stream)
}

fn connected_stream_peer_owned_by_identity(
    local: std::net::SocketAddr,
    peer: std::net::SocketAddr,
    identity: BrowserStreamIdentity,
) -> bool {
    let (std::net::SocketAddr::V4(local), std::net::SocketAddr::V4(peer)) = (local, peer) else {
        return false;
    };
    if !local.ip().is_loopback()
        || !peer.ip().is_loopback()
        || peer.port() != identity.port
        || identity.pid == 0
        || process_start_time(identity.pid) != Some(identity.start_time)
    {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        let Ok(table) = std::fs::read_to_string("/proc/net/tcp") else { return false };
        let server = format!("0100007F:{:04X}", peer.port());
        let client = format!("0100007F:{:04X}", local.port());
        let inodes: std::collections::HashSet<&str> = table
            .lines()
            .filter_map(|line| {
                let fields: Vec<_> = line.split_whitespace().collect();
                (fields.len() > 9
                    && fields[1] == server
                    && fields[2] == client
                    && fields[3] == "01")
                    .then_some(fields[9])
            })
            .collect();
        if inodes.is_empty() {
            return false;
        }
        let Ok(fds) = std::fs::read_dir(format!("/proc/{}/fd", identity.pid)) else { return false };
        fds.flatten().any(|fd| {
            let Ok(target) = std::fs::read_link(fd.path()) else { return false };
            target.to_string_lossy()
                .strip_prefix("socket:[")
                .and_then(|inode| inode.strip_suffix(']'))
                .is_some_and(|inode| inodes.contains(inode))
        })
    }
    #[cfg(target_os = "macos")]
    {
        let mut command = std::process::Command::new("lsof");
        command.args([
            "-nP", "-a", "-p", &identity.pid.to_string(),
            &format!("-iTCP:{}", peer.port()), "-sTCP:ESTABLISHED", "-F", "n",
        ]);
        let expected = format!("n127.0.0.1:{}->127.0.0.1:{}", peer.port(), local.port());
        output_capture_with_timeout(command, PROC_CONTROL_TIMEOUT)
            .ok()
            .filter(|output| output.status.success())
            .is_some_and(|output| String::from_utf8_lossy(&output.stdout).lines().any(|line| line == expected))
    }
    #[cfg(windows)]
    {
        connected_tcp_peer_pid_for_addrs(
            std::net::SocketAddr::V4(local),
            std::net::SocketAddr::V4(peer),
        ) == Some(identity.pid)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        false
    }
}

/// Check whether a PID is still alive without sending a real signal.
/// Used by safe-adoption to decide whether a stale PID file points at a
/// dead daemon (reap) or a still-running one (adopt).
fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        // `kill -0 PID` returns 0 (success) while the process is alive,
        // ESRCH otherwise. Cheap, no actual signal delivered.
        let mut cmd = std::process::Command::new("kill");
        cmd.args(["-0", &pid.to_string()]);
        output_capture_with_timeout(cmd, PROC_CONTROL_TIMEOUT)
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        // tasklist exits 0 even when no match — we have to check stdout.
        let cmd = {
            let mut c = std::process::Command::new("tasklist");
            c.args(["/FI", &format!("PID eq {pid}"), "/NH"])
                .creation_flags(CREATE_NO_WINDOW);
            c
        };
        let Ok(output) = output_capture_with_timeout(cmd, PROC_CONTROL_TIMEOUT) else {
            return false;
        };
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Real rows contain the PID; "INFO: No tasks..." rows do not.
        stdout.contains(&pid.to_string())
    }
}

/// Per-session stream state. The session is keyed externally by the
/// session id (workspace_id, cli_session_name, or browser_id depending
/// on caller); inside we cache the things we actually need to reason
/// about lifecycle: the port, the daemon PID (when known), and a
/// freshness timestamp for the optional reaper.
struct StreamSession {
    /// Stream port allocated for this session.
    port: u16,
    /// Whether the daemon is believed to be live. Only set true after a
    /// successful TCP probe of `port`.
    running: bool,
    /// PID of the spawned agent-browser daemon, when known. None when
    /// the session was adopted from a stale PID file or when the
    /// agent-browser CLI didn't write its lock file in time.
    pid: Option<u32>,
    /// Start time disambiguates PID reuse after an automation command
    /// restarts the daemon without going through start_stream.
    pid_start_time: Option<u64>,
    /// Rotates after every successful daemon launch, even if the OS reuses
    /// the same PID and stream port immediately.
    stream_generation: u128,
    /// Last time we saw signs of life on this session (TCP probe success
    /// or first-frame timestamp passed in by the frontend). Used by the
    /// optional reaper background task.
    #[allow(dead_code)]
    last_seen_at: Option<Instant>,
}

#[derive(Clone)]
pub struct AgentBrowserManager {
    /// Atomic counter for the next port to try allocating.
    next_port: Arc<AtomicU16>,
    /// Per-session state keyed by session identifier (workspace_id or cli_session_name).
    sessions: Arc<Mutex<HashMap<String, StreamSession>>>,
    /// Serializes start_stream calls to prevent concurrent daemon launches
    /// from racing (e.g., React StrictMode double-mount, pane remount + agent action).
    start_lock: Arc<Mutex<()>>,
    #[cfg(test)]
    test_stream_ports: Arc<Mutex<HashMap<u16, BrowserStreamIdentity>>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct BrowserStreamIdentity {
    pub port: u16,
    pub pid: u32,
    pub start_time: u64,
    pub generation: u128,
}

fn refresh_stream_session_identity(
    stream: &mut StreamSession,
    record: &BrowserPidRecord,
) -> BrowserStreamIdentity {
    let identity_changed = stream.pid != Some(record.pid)
        || stream.pid_start_time.is_some_and(|start| start != record.start_time);
    if identity_changed {
        stream.stream_generation = uuid::Uuid::new_v4().as_u128();
    }
    stream.pid = Some(record.pid);
    stream.pid_start_time = Some(record.start_time);
    BrowserStreamIdentity {
        port: stream.port,
        pid: record.pid,
        start_time: record.start_time,
        generation: stream.stream_generation,
    }
}

fn process_start_time(pid: u32) -> Option<u64> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

    let process_id = Pid::from_u32(pid);
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[process_id]),
        true,
        ProcessRefreshKind::nothing(),
    );
    system.process(process_id).map(|process| process.start_time())
}

#[cfg(test)]
pub(crate) fn current_process_stream_identity(port: u16) -> BrowserStreamIdentity {
    let pid = std::process::id();
    BrowserStreamIdentity {
        port,
        pid,
        start_time: process_start_time(pid).expect("test process start time"),
        generation: 1,
    }
}

fn session_name(browser_id: &str) -> &str {
    if browser_id.is_empty() { "default" } else { browser_id }
}

/// One lifecycle operation at a time for a daemon session, including its
/// first command and any rollback. The gate is shared by manager instances
/// so a second caller cannot use a daemon while its starter may stop it.
fn browser_lifecycle_gate(session: &str) -> Arc<Mutex<()>> {
    static GATES: OnceLock<std::sync::Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();
    let gates = GATES.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    let mut gates = gates.lock().unwrap();
    gates.entry(session.to_owned()).or_insert_with(|| Arc::new(Mutex::new(()))).clone()
}

/// Keep the session gate in a worker that survives cancellation of its
/// caller. A blocking CLI job cannot be cancelled once started, so releasing
/// the gate with the waiting caller would let a second operation race its
/// rollback and state reconciliation.
async fn run_detached_browser_lifecycle<T, F>(session: String, work: F) -> Result<T, String>
where
    T: Send + 'static,
    F: std::future::Future<Output = Result<T, String>> + Send + 'static,
{
    tokio::spawn(async move {
        let _lifecycle = browser_lifecycle_gate(&session).lock_owned().await;
        work.await
    })
    .await
    .map_err(|error| format!("Browser lifecycle task failed: {error}"))?
}

fn validate_session_name(name: &str) -> Result<(), String> {
    // Match the v0.44 CLI grammar and keep PID/socket paths to one segment.
    if name.is_empty()
        || name.len() > 64
        || !name.chars().all(|ch| ch.is_alphanumeric() || matches!(ch, '-' | '_'))
    {
        return Err("Invalid browser session name".into());
    }
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(not(target_os = "windows"))]
fn shell_binary(binary: &str) -> String {
    shell_quote(binary)
}

#[cfg(not(target_os = "windows"))]
fn browser_open_command(bin: &str, session: &str, port: u16) -> Result<std::process::Command, String> {
    let mut cmd = if bin == "npx agent-browser" {
        let mut cmd = std::process::Command::new("npx");
        cmd.arg("agent-browser");
        cmd
    } else {
        std::process::Command::new(bin)
    };
    set_browser_owner_env(&mut cmd, session)?;
    cmd.args(["open", "about:blank", "--session", session])
        .env("AGENT_BROWSER_STREAM_PORT", port.to_string())
        .env("AGENT_BROWSER_ARGS", STEALTH_CHROMIUM_ARGS)
        .env("AGENT_BROWSER_USER_AGENT", stealth_user_agent());
    Ok(cmd)
}

/// JavaScript that queries the DOM for all interactive elements.
/// Used as a fallback when the ARIA snapshot returns nothing useful.
pub const DOM_SNAPSHOT_SCRIPT: &str = r###"(() => {
  const sel = "a[href], button, input, select, textarea, [role='button'], [role='link'], [role='tab'], [role='checkbox'], [role='radio'], [role='combobox'], [role='menuitem'], [role='searchbox'], [tabindex]:not([tabindex='-1'])";
  const els = document.querySelectorAll(sel);
  const results = [];
  const seen = new Set();
  for (const el of els) {
    if (el.offsetParent === null && el.tagName !== "INPUT" && el.getAttribute("type") !== "hidden") continue;
    try { if (getComputedStyle(el).display === "none" || getComputedStyle(el).visibility === "hidden") continue; } catch(e) { continue; }
    const tag = el.tagName.toLowerCase();
    const role = el.getAttribute("role") || tag;
    const text = (el.getAttribute("aria-label") || el.textContent || el.getAttribute("placeholder") || el.getAttribute("value") || el.getAttribute("title") || "").substring(0, 80).trim().replace(/\s+/g, " ");
    let selector = "";
    if (el.id) selector = "#" + el.id;
    else if (el.getAttribute("name")) selector = tag + "[name='" + el.getAttribute("name") + "']";
    else if (el.getAttribute("aria-label")) selector = tag + "[aria-label='" + el.getAttribute("aria-label") + "']";
    else if (tag === "a" && el.getAttribute("href")) {
      const href = el.getAttribute("href");
      if (href.length < 60) selector = "a[href='" + href + "']";
    }
    if (!selector) {
      let cur = el; const parts = [];
      while (cur && cur !== document.body && cur !== document.documentElement) {
        let seg = cur.tagName.toLowerCase();
        const parent = cur.parentElement;
        if (parent) {
          const siblings = Array.from(parent.children).filter(c => c.tagName === cur.tagName);
          if (siblings.length > 1) seg += ":nth-of-type(" + (siblings.indexOf(cur) + 1) + ")";
        }
        parts.unshift(seg);
        cur = parent;
        if (parts.length > 4) break;
      }
      selector = parts.join(" > ");
    }
    const key = role + "|" + text + "|" + selector;
    if (seen.has(key)) continue;
    seen.add(key);
    const entry = text ? "- [" + role + "] \"" + text + "\" \u2192 " + selector : "- [" + role + "] " + selector;
    results.push(entry);
  }
  return results.length > 0 ? results.join("\n") : "(no elements found)";
})()"###;

fn needs_dom_fallback(stdout: &str) -> bool {
    let trimmed = stdout.trim();
    trimmed.contains("(no interactive elements)")
        || trimmed == "- document"
        || trimmed.is_empty()
        || trimmed == "(empty)"
}

fn extract_eval_result(stdout: &str) -> String {
    // agent-browser eval returns JSON: {"success":true,"data":{"result":"...","origin":"..."}}
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(stdout) {
        if let Some(result) = json.pointer("/data/result").and_then(|v| v.as_str()) {
            return result.to_string();
        }
    }
    // Native binary may wrap eval string results in quotes — strip them
    let trimmed = stdout.trim();
    if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() > 1 {
        // Unescape the JSON string
        if let Ok(serde_json::Value::String(s)) = serde_json::from_str::<serde_json::Value>(trimmed) {
            return s;
        }
    }
    trimmed.to_string()
}

/// Resolve only a native binary with the origin-protected stream API.
/// Bundled sidecars take precedence over the npm development package; PATH
/// and npx are deliberately excluded so an older installation cannot reopen
/// the unauthenticated v0.24 stream endpoint.
fn resolve_binary() -> Result<String, String> {
    static BINARY: OnceLock<Result<String, String>> = OnceLock::new();
    BINARY.get_or_init(resolve_binary_uncached).clone()
}

fn resolve_binary_uncached() -> Result<String, String> {
    let npm_relative = if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "node_modules/@agent-browser/cli-linux-x64-gnu/bin/agent-browser"
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        "node_modules/@agent-browser/cli-linux-arm64-gnu/bin/agent-browser"
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "node_modules/@agent-browser/cli-darwin-x64/bin/agent-browser"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "node_modules/@agent-browser/cli-darwin-arm64/bin/agent-browser"
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "node_modules/@agent-browser/cli-win32-x64-msvc/bin/agent-browser.exe"
    } else {
        return Err("No secure agent-browser sidecar is available for this platform".into());
    };

    let exe = std::env::current_exe().ok();
    #[cfg(debug_assertions)]
    let dev_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent();
    #[cfg(not(debug_assertions))]
    let dev_root: Option<&std::path::Path> = None;
    let candidates = native_binary_candidates(exe.as_deref().and_then(std::path::Path::parent), dev_root, bundled_browser_binary_name(), npm_relative);
    resolve_verified_candidate(&candidates)
}

fn bundled_browser_binary_name() -> &'static str {
    if cfg!(windows) { "agent-browser.exe" } else { "agent-browser" }
}

fn native_binary_candidates(
    exe_dir: Option<&std::path::Path>,
    dev_root: Option<&std::path::Path>,
    sidecar_name: &str,
    npm_relative: &str,
) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(dir) = exe_dir {
        candidates.push(dir.join(sidecar_name));
    }
    if let Some(root) = dev_root {
        candidates.push(root.join(npm_relative));
    }
    candidates
}

fn resolve_verified_candidate(candidates: &[PathBuf]) -> Result<String, String> {
    for candidate in candidates {
        let Ok(meta) = std::fs::symlink_metadata(candidate) else { continue };
        if !meta.is_file() || meta.file_type().is_symlink() {
            continue;
        }
        if native_browser_version(candidate, PROC_CONTROL_TIMEOUT)
            .is_some_and(|version| version >= semver::Version::new(0, 44, 0)) {
            return Ok(candidate.to_string_lossy().into_owned());
        }
    }
    Err("Secure agent-browser v0.44 or newer is unavailable; reinstall Codemux's browser sidecar".into())
}

fn native_browser_version(path: &std::path::Path, timeout: Duration) -> Option<semver::Version> {
    let mut command = std::process::Command::new(path);
    command.arg("--version");
    let output = output_capture_with_timeout(command, timeout).ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .find_map(|word| semver::Version::parse(word.trim_start_matches('v')).ok())
}

/// Separate the legacy npx fallback from a native path so the verified
/// Windows runner can reject wrappers before starting a command.
#[cfg(target_os = "windows")]
fn split_resolved_binary(bin: &str) -> (String, Vec<String>) {
    let trimmed = bin.trim();
    if let Some(program) = trimmed.strip_suffix(" agent-browser") {
        (program.to_string(), vec!["agent-browser".to_string()])
    } else {
        (trimmed.to_string(), Vec::new())
    }
}

#[cfg(target_os = "windows")]
fn native_browser_program(bin: &str) -> std::io::Result<String> {
    let (program, prefix_args) = split_resolved_binary(bin);
    if !prefix_args.is_empty() || !program.to_ascii_lowercase().ends_with(".exe") {
        return Err(std::io::Error::other("A native agent-browser binary is required for verified Windows control"));
    }
    Ok(program)
}

#[cfg(target_os = "windows")]
fn relay_cli_argv(argv: &[String], session: &str, alias: &str) -> std::io::Result<Vec<String>> {
    let mut cli_argv = argv.to_vec();
    let session_arg = cli_argv.iter().enumerate().rposition(|(index, arg)| {
        arg == "--session" && cli_argv.get(index + 1).is_some_and(|value| value == session)
    }).ok_or_else(|| std::io::Error::other("Browser CLI command has no session argument"))?;
    cli_argv[session_arg + 1] = alias.to_owned();
    Ok(cli_argv)
}

/// Locate a Chromium-based browser executable on Windows. agent-browser
/// auto-detects "system Chrome" but doesn't probe Brave / Edge / per-user
/// Chrome installs — and codemux on Windows can't ship its own Chromium
/// without a 150 MB download per machine. This helper makes the common
/// case (user has Brave/Chrome/Edge already installed) Just Work without
/// `agent-browser install` being a setup gotcha.
///
/// Order: Brave, Chrome (system + per-user), Edge. Edge always exists on
/// Windows 10+, so it's the safety net — if everything else is missing
/// the user still gets a working browser pane out of the box.
///
/// Override: respects `CODEMUX_BROWSER_EXECUTABLE` env var if set, so
/// power users can point at a custom build (Vivaldi, Chromium nightly,
/// portable installs, etc.) without editing settings.
#[cfg(target_os = "windows")]
fn find_chromium_browser_path() -> Option<String> {
    use std::path::PathBuf;

    if let Ok(custom) = std::env::var("CODEMUX_BROWSER_EXECUTABLE") {
        let path = PathBuf::from(&custom);
        if path.exists() {
            return Some(custom);
        }
    }

    let local_appdata = std::env::var("LOCALAPPDATA").ok();
    let program_files =
        std::env::var("ProgramFiles").unwrap_or_else(|_| "C:\\Program Files".into());
    let program_files_x86 = std::env::var("ProgramFiles(x86)")
        .unwrap_or_else(|_| "C:\\Program Files (x86)".into());

    let mut candidates: Vec<PathBuf> = Vec::new();

    // Brave (per-user first — that's where the .exe Installer puts it for
    // a single-user install; Brave's own default).
    if let Some(ref local) = local_appdata {
        candidates.push(
            format!("{local}\\BraveSoftware\\Brave-Browser\\Application\\brave.exe").into(),
        );
    }
    candidates.push(
        format!("{program_files}\\BraveSoftware\\Brave-Browser\\Application\\brave.exe").into(),
    );

    // Google Chrome.
    candidates.push(format!("{program_files}\\Google\\Chrome\\Application\\chrome.exe").into());
    candidates
        .push(format!("{program_files_x86}\\Google\\Chrome\\Application\\chrome.exe").into());
    if let Some(ref local) = local_appdata {
        candidates.push(format!("{local}\\Google\\Chrome\\Application\\chrome.exe").into());
    }

    // Microsoft Edge — preinstalled on every Windows 10+ install. Last
    // resort but guaranteed to exist if all else fails.
    candidates
        .push(format!("{program_files_x86}\\Microsoft\\Edge\\Application\\msedge.exe").into());
    candidates.push(format!("{program_files}\\Microsoft\\Edge\\Application\\msedge.exe").into());

    candidates
        .into_iter()
        .find(|p| p.exists())
        .map(|p| p.to_string_lossy().into_owned())
}

/// Build the `--executable-path <path>` arg pair to splice into an
/// agent-browser argv when launching a session, or an empty Vec if
/// auto-detection found nothing (in which case agent-browser will run
/// its own probe and emit the "Chrome not found" error to surface the
/// gap to the user). Windows-only because the override is only needed
/// where agent-browser's own probe misses common installs.
#[cfg(target_os = "windows")]
fn windows_executable_path_args() -> Vec<String> {
    match find_chromium_browser_path() {
        Some(path) => vec!["--executable-path".into(), path],
        None => Vec::new(),
    }
}

/// Resolve the JSON params passed to a `viewport` action into a concrete
/// `ViewportSpec`. Accepts (in priority order):
///
/// 1. `preset` — a named entry from `browser_viewport::PRESETS` or the
///    literal `"reset"`. Highest priority because it's what the CLI and
///    MCP tool send; explicit width/height/scale beneath are only used
///    as fallbacks for legacy callers (e.g. `BrowserPane.tsx`'s
///    `ResizeObserver` that auto-syncs the pane size on every resize).
/// 2. `width` + `height` (+ optional `scale` for DPR). This is the
///    pre-preset shape the frontend already emits, so it stays a
///    first-class input. Missing fields fall back to 1280×720×1.0.
///
/// An unknown preset name falls through to the (1280, 720, 1.0)
/// fallback because the calling layer (CLI / MCP) is expected to have
/// already validated the input via `browser_viewport::parse_spec`. The
/// fallback prevents a typo from breaking the live browser pane.
fn resolve_viewport_params(params: &serde_json::Value) -> crate::browser_viewport::ViewportSpec {
    // Preset path (CLI / MCP path).
    if let Some(name) = params.get("preset").and_then(|v| v.as_str()) {
        let dpr_override = params.get("scale").and_then(|v| v.as_f64());
        if let Ok(spec) = crate::browser_viewport::parse_spec(name, dpr_override) {
            return spec;
        }
        // Fall through to width/height path on unknown preset — see doc
        // comment for the rationale.
    }

    let width = params.get("width").and_then(|v| v.as_u64()).unwrap_or(1280) as u32;
    let height = params.get("height").and_then(|v| v.as_u64()).unwrap_or(720) as u32;
    let dpr = params.get("scale").and_then(|v| v.as_f64()).unwrap_or(1.0);
    crate::browser_viewport::ViewportSpec::new(width, height, dpr)
}

/// Format a device-pixel-ratio float for the `agent-browser set viewport
/// W H [scale]` CLI argument. Trims trailing zeros so `2.0` becomes
/// `"2"` (cleaner shell echo, smaller argv) but preserves precision when
/// users pass odd values like `1.5`.
fn format_dpr(dpr: f64) -> String {
    if (dpr - dpr.round()).abs() < f64::EPSILON {
        format!("{}", dpr as u64)
    } else {
        // Two decimal places is more than enough — DPR > 3.0 is
        // already exotic and nobody cares about the third decimal.
        format!("{dpr:.2}")
    }
}

/// Argv-form sibling of `build_agent_browser_command`. Used only on Windows
/// to spawn agent-browser.exe without depending on Git Bash being installed.
#[cfg(target_os = "windows")]
fn build_agent_browser_argv_groups(
    session: &str,
    action: &str,
    params: &serde_json::Value,
) -> Result<Vec<Vec<String>>, String> {
    let s = session.to_string();
    let groups: Vec<Vec<String>> = match action {
        "open_url" | "open" => {
            let url = params
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or("about:blank")
                .to_string();
            // `--executable-path` is a TOP-LEVEL agent-browser flag, so it
            // must precede the subcommand (`open`). Putting it after
            // `open` makes clap reject the argv as an unknown flag for
            // the `open` subcommand and the binary exits silently —
            // exactly the "no log activity, browser never loads" mode
            // the user hit.
            let mut open_argv: Vec<String> = windows_executable_path_args();
            open_argv.extend(["open".into(), url, "--session".into(), s.clone()]);
            // agent-browser open already waits for load. A second wait
            // subscribes after that event and can stall until its timeout.
            vec![open_argv]
        }
        "screenshot" => vec![vec!["screenshot".into(), "--session".into(), s]],
        "snapshot" | "accessibility_snapshot" => vec![vec![
            "snapshot".into(),
            "-i".into(),
            "--session".into(),
            s,
        ]],
        "click" => {
            let selector = params
                .get("selector")
                .and_then(|v| v.as_str())
                .unwrap_or("body")
                .to_string();
            vec![vec!["click".into(), selector, "--session".into(), s]]
        }
        "fill" => {
            let selector = params
                .get("selector")
                .and_then(|v| v.as_str())
                .unwrap_or("body")
                .to_string();
            let value = params
                .get("value")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            vec![vec!["fill".into(), selector, value, "--session".into(), s]]
        }
        "type_text" => {
            let text = params
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            vec![vec![
                "type".into(),
                "body".into(),
                text,
                "--session".into(),
                s,
            ]]
        }
        "console_logs" | "console" => vec![vec!["console".into(), "--session".into(), s]],
        "evaluate" | "eval" => {
            let script = params
                .get("script")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            vec![vec!["eval".into(), script, "--session".into(), s]]
        }
        "back" => vec![vec!["back".into(), "--session".into(), s]],
        "forward" => vec![vec!["forward".into(), "--session".into(), s]],
        "reload" => vec![vec!["reload".into(), "--session".into(), s]],
        "viewport" => {
            let spec = resolve_viewport_params(params);
            let mut argv = vec![
                "set".into(),
                "viewport".into(),
                spec.width.to_string(),
                spec.height.to_string(),
            ];
            // Only pass the scale argument when it differs from 1.0 so
            // legacy callers that didn't set DPR keep producing the same
            // exact `set viewport W H` argv. agent-browser treats a
            // missing 3rd arg as 1.0 anyway, so this is a pure
            // backwards-compat preservation.
            if (spec.dpr - 1.0).abs() > f64::EPSILON {
                argv.push(format_dpr(spec.dpr));
            }
            argv.push("--session".into());
            argv.push(s);
            vec![argv]
        }
        "get_styles" => {
            let selector = params
                .get("selector")
                .and_then(|v| v.as_str())
                .unwrap_or("body")
                .to_string();
            vec![vec![
                "get".into(),
                "styles".into(),
                selector,
                "--json".into(),
                "--session".into(),
                s,
            ]]
        }
        "wait" => {
            let selector = params
                .get("selector")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let text = params.get("text").and_then(|v| v.as_str()).map(String::from);
            if let Some(text) = text {
                vec![vec![
                    "wait".into(),
                    "--text".into(),
                    text,
                    "--session".into(),
                    s,
                ]]
            } else {
                vec![vec!["wait".into(), selector, "--session".into(), s]]
            }
        }
        "get_text" => {
            let selector = params
                .get("selector")
                .and_then(|v| v.as_str())
                .unwrap_or("body")
                .to_string();
            vec![vec![
                "get".into(),
                "text".into(),
                selector,
                "--session".into(),
                s,
            ]]
        }
        "get_box" => {
            let selector = params
                .get("selector")
                .and_then(|v| v.as_str())
                .unwrap_or("body")
                .to_string();
            vec![vec![
                "get".into(),
                "box".into(),
                selector,
                "--json".into(),
                "--session".into(),
                s,
            ]]
        }
        _ => return Err(format!("Unknown action: {}", action)),
    };
    Ok(groups)
}

#[cfg(target_os = "windows")]
static BROWSER_DAEMON_START_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(target_os = "windows")]
fn ensure_owned_browser_daemon(bin: &str, session: &str, stream_port: u16, timeout: Duration) -> std::io::Result<(BrowserPidRecord, bool)> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    // Serialize the missing-PID check and direct spawn across callers. A
    // second daemon for the same session would race on both lock files. The
    // lock wait consumes the caller's startup budget just like readiness.
    let started = Instant::now();
    let _start_guard = loop {
        if started.elapsed() >= timeout {
            return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "Browser daemon startup timed out"));
        }
        match BROWSER_DAEMON_START_LOCK.try_lock() {
            Ok(guard) => break guard,
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return Err(std::io::Error::other("Browser daemon start lock was poisoned"));
            }
            Err(std::sync::TryLockError::WouldBlock) => {
                std::thread::sleep(Duration::from_millis(10).min(timeout.saturating_sub(started.elapsed())));
            }
        }
    };
    let program = native_browser_program(bin)?;
    let expected_version = native_browser_version(
        std::path::Path::new(&program),
        timeout.saturating_sub(started.elapsed()).min(PROC_CONTROL_TIMEOUT),
    )
    .filter(|version| *version >= semver::Version::new(0, 44, 0))
    .ok_or_else(|| std::io::Error::other("Cannot verify browser CLI version"))?;
    require_owned_browser_session(session).map_err(std::io::Error::other)?;
    if let Some(pid) = read_agent_browser_daemon_pid(session) {
        let record = owned_browser_pid(session, pid)
            .ok_or_else(|| std::io::Error::other("Browser daemon identity changed"))?;
        let dir = run_dir_for_session(session)
            .ok_or_else(|| std::io::Error::other("Cannot prepare private browser control directory"))?;
        if browser_daemon_version(session, &dir).as_ref() == Some(&expected_version) {
            return Ok((record, false));
        }
        // A v0.24 daemon still has an unprotected stream endpoint. Terminate
        // only its verified process before starting the secure native daemon.
        stop_owned_browser_daemon(session, &record, bin);
        if pid_alive(pid) {
            return Err(std::io::Error::other("Could not stop outdated browser daemon"));
        }
    }

    // Starting daemon mode directly avoids sending the first `open` command
    // through the CLI's unauthenticated port lookup. The relay below handles
    // that command only after the daemon is ready and identified.
    let mut cmd = std::process::Command::new(program);
    set_browser_owner_env(&mut cmd, session).map_err(std::io::Error::other)?;
    cmd.env("AGENT_BROWSER_DAEMON", "1")
        .env("AGENT_BROWSER_SESSION", session)
        .env("AGENT_BROWSER_STREAM_PORT", stream_port.to_string())
        .env("AGENT_BROWSER_ARGS", STEALTH_CHROMIUM_ARGS)
        .env("AGENT_BROWSER_USER_AGENT", stealth_user_agent())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW);
    if let Some(path) = find_chromium_browser_path() {
        cmd.env("AGENT_BROWSER_EXECUTABLE_PATH", path);
    }
    if let Some(log) = session_log_path(session).as_deref().and_then(open_session_log) {
        cmd.stderr(log);
    } else {
        cmd.stderr(std::process::Stdio::null());
    }
    let mut child = cmd.spawn()?;
    loop {
        if let Some(pid) = read_agent_browser_daemon_pid(session) {
            if pid == child.id() {
                if let Some(record) = owned_browser_pid(session, pid) {
                    if connect_verified_browser_control(session, &record).is_ok() {
                        return Ok((record, true));
                    }
                }
            }
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                if let Some(dir) = run_dir_for_session(session) {
                    cleanup_stale_browser_lock_files_in(&dir, session, child.id());
                }
                return Err(std::io::Error::other(format!("Browser daemon exited before control startup: {status}")));
            }
            Ok(None) => {}
            Err(error) => {
                let pid = child.id();
                let _ = child.kill();
                let _ = child.wait();
                if let Some(dir) = run_dir_for_session(session) {
                    cleanup_stale_browser_lock_files_in(&dir, session, pid);
                }
                return Err(error);
            }
        }
        if started.elapsed() >= timeout {
            let pid = child.id();
            let _ = child.kill();
            let _ = child.wait();
            if let Some(dir) = run_dir_for_session(session) {
                cleanup_stale_browser_lock_files_in(&dir, session, pid);
            }
            return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "Browser daemon control startup timed out"));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Run the CLI against a private relay. The CLI resolves its control port a
/// second time when it sends a command, so a listener check made before
/// spawning it cannot authorize the connection it actually uses.
#[cfg(target_os = "windows")]
fn run_agent_browser_native(
    bin: &str,
    argv: &[String],
    session: &str,
    stream_port: u16,
    discard_stderr: bool,
    timeout: Duration,
) -> std::io::Result<std::process::Output> {
    let started = Instant::now();
    let (record, started_here) = ensure_owned_browser_daemon(bin, session, stream_port, timeout)?;
    track_browser_daemon_before_command(session, bin, &record, started_here)?;
    let remaining = timeout.saturating_sub(started.elapsed());
    let result = run_agent_browser_native_with_record(
        bin,
        argv,
        session,
        stream_port,
        discard_stderr,
        remaining,
        record.clone(),
    );
    finish_browser_command(result, session, bin, &record, started_here)
}

#[cfg(windows)]
fn track_browser_daemon_before_command(
    session: &str,
    bin: &str,
    record: &BrowserPidRecord,
    started_here: bool,
) -> std::io::Result<()> {
    if record.token == browser_owner_token() {
        if let Err(error) = write_session_pid(session, record) {
            if started_here {
                stop_owned_browser_daemon(session, record, bin);
            }
            return Err(error);
        }
    }
    Ok(())
}

#[cfg(windows)]
fn finish_browser_command(
    result: std::io::Result<std::process::Output>,
    session: &str,
    bin: &str,
    record: &BrowserPidRecord,
    started_here: bool,
) -> std::io::Result<std::process::Output> {
    if started_here && result.as_ref().map_or(true, |output| !output.status.success()) {
        stop_owned_browser_daemon(session, record, bin);
    }
    result
}

#[cfg(target_os = "windows")]
fn run_agent_browser_native_with_record(
    bin: &str,
    argv: &[String],
    session: &str,
    stream_port: u16,
    discard_stderr: bool,
    remaining: Duration,
    record: BrowserPidRecord,
) -> std::io::Result<std::process::Output> {
    if remaining.is_zero() {
        return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "Browser command startup timed out"));
    }
    let mut relay = VerifiedBrowserRelay::new(bin, session, record, remaining)?;
    run_agent_browser_native_raw(bin, argv, session, stream_port, discard_stderr, remaining, &mut relay)
}

/// Spawn the native CLI using the relay's private control directory.
#[cfg(target_os = "windows")]
fn run_agent_browser_native_raw(
    bin: &str,
    argv: &[String],
    session: &str,
    stream_port: u16,
    discard_stderr: bool,
    timeout: Duration,
    relay: &mut VerifiedBrowserRelay,
) -> std::io::Result<std::process::Output> {
    let program = native_browser_program(bin)?;
    let mut cmd = std::process::Command::new(&program);
    let cli_argv = relay_cli_argv(argv, session, &relay.cli_session)?;
    for a in &cli_argv {
        cmd.arg(a);
    }
    set_browser_owner_env(&mut cmd, session).map_err(std::io::Error::other)?;
    cmd.env("AGENT_BROWSER_SOCKET_DIR", &relay.directory.path);
    cmd.env("AGENT_BROWSER_STREAM_PORT", stream_port.to_string())
        .env("AGENT_BROWSER_ARGS", STEALTH_CHROMIUM_ARGS)
        .env("AGENT_BROWSER_USER_AGENT", stealth_user_agent())
        .env_remove("AGENT_BROWSER_DAEMON")
        .env_remove("AGENT_BROWSER_DEBUG");
    cmd.stdout(std::process::Stdio::piped());
    if discard_stderr {
        cmd.stderr(std::process::Stdio::null());
    } else {
        cmd.stderr(std::process::Stdio::piped());
    }
    // Hard timeout + kill so a wedged agent-browser.exe can't block the
    // calling task forever (issue #96).
    spawn_wait_collect_with_child(cmd, timeout, |child| relay.register_cli(child))
}

#[cfg(windows)]
fn require_successful_browser_stream_open(
    result: std::io::Result<std::process::Output>,
    log_file: Option<std::fs::File>,
) -> Result<(), String> {
    use std::io::Write;

    let output = result.map_err(|error| format!("Browser stream open failed: {error}"))?;
    if let Some(mut log) = log_file {
        let _ = log.write_all(&output.stderr);
    }
    if !output.status.success() {
        return Err(format!("Browser stream open failed with status {}", output.status));
    }
    Ok(())
}

#[cfg(not(windows))]
fn require_successful_browser_stream_open_status(
    result: std::io::Result<std::process::ExitStatus>,
) -> Result<(), String> {
    let status = result.map_err(|error| format!("Browser stream open failed: {error}"))?;
    if !status.success() {
        return Err(format!("Browser stream open failed with status {status}"));
    }
    Ok(())
}

#[cfg(windows)]
fn finish_browser_stream_open(
    result: std::io::Result<std::process::Output>,
    log_file: Option<std::fs::File>,
    session: &str,
    bin: &str,
    record: &BrowserPidRecord,
    started_here: bool,
) -> Result<(), String> {
    let result = require_successful_browser_stream_open(result, log_file);
    if result.is_err() && started_here {
        stop_owned_browser_daemon(session, record, bin);
    }
    result
}

#[cfg(windows)]
fn connect_verified_browser_control(
    session: &str,
    record: &BrowserPidRecord,
) -> std::io::Result<(std::net::TcpStream, std::os::windows::io::OwnedHandle)> {
    let process = open_verified_browser_handle(record, session)
        .ok_or_else(|| std::io::Error::other("Browser daemon identity changed"))?;
    let dir = run_dir_for_session(session).ok_or_else(|| std::io::Error::other("Cannot prepare private browser control directory"))?;
    let port = browser_control_port(session, &dir)
        .ok_or_else(|| std::io::Error::other("Browser control port is missing or invalid"))?;
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let stream = std::net::TcpStream::connect_timeout(&address, Duration::from_millis(500))?;
    if !connected_control_peer_owned_by_pid(&stream, record.pid) {
        return Err(std::io::Error::other("Browser control connection belongs to another process"));
    }
    Ok((stream, process))
}

/// Query the server side of this established TCP connection. Checking its
/// listener before connect would authorize a different peer after a restart.
#[cfg(windows)]
fn connected_control_peer_owned_by_pid(stream: &std::net::TcpStream, pid: u32) -> bool {
    connected_tcp_peer_pid(stream) == Some(pid)
}

#[cfg(windows)]
fn connected_tcp_peer_pid(stream: &std::net::TcpStream) -> Option<u32> {
    connected_tcp_peer_pid_for_addrs(stream.local_addr().ok()?, stream.peer_addr().ok()?)
}

#[cfg(windows)]
fn connected_tcp_peer_pid_for_addrs(
    local: std::net::SocketAddr,
    peer: std::net::SocketAddr,
) -> Option<u32> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, MIB_TCP_STATE_ESTAB, TCP_TABLE_OWNER_PID_CONNECTIONS,
    };
    use windows_sys::Win32::Networking::WinSock::AF_INET;

    let (std::net::SocketAddr::V4(peer), std::net::SocketAddr::V4(local)) =
        (peer, local)
    else {
        return None;
    };
    let row_matches = |row: MIB_TCPROW_OWNER_PID| {
        row.dwState == MIB_TCP_STATE_ESTAB as u32
            && row.dwLocalAddr == u32::from_ne_bytes(peer.ip().octets())
            && u16::from_be(row.dwLocalPort as u16) == peer.port()
            && row.dwRemoteAddr == u32::from_ne_bytes(local.ip().octets())
            && u16::from_be(row.dwRemotePort as u16) == local.port()
    };

    for _ in 0..3 {
        let mut size = 0u32;
        unsafe {
            GetExtendedTcpTable(std::ptr::null_mut(), &mut size, 0, u32::from(AF_INET), TCP_TABLE_OWNER_PID_CONNECTIONS, 0);
        }
        if size < std::mem::size_of::<u32>() as u32 {
            return None;
        }
        let mut table = vec![0u8; size as usize];
        let status = unsafe {
            GetExtendedTcpTable(table.as_mut_ptr().cast(), &mut size, 0, u32::from(AF_INET), TCP_TABLE_OWNER_PID_CONNECTIONS, 0)
        };
        if status == 0 && table.len() >= std::mem::size_of::<u32>() {
            let entries = unsafe { std::ptr::read_unaligned(table.as_ptr().cast::<u32>()) } as usize;
            let row_size = std::mem::size_of::<MIB_TCPROW_OWNER_PID>();
            let available = (table.len() - std::mem::size_of::<u32>()) / row_size;
            for index in 0..entries.min(available) {
                let row = unsafe {
                    std::ptr::read_unaligned(table.as_ptr().add(4 + index * row_size).cast::<MIB_TCPROW_OWNER_PID>())
                };
                if row_matches(row) {
                    return Some(row.dwOwningPid);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    None
}

#[cfg(windows)]
struct RelayDirectory {
    path: PathBuf,
    port_file: PathBuf,
    version_file: PathBuf,
    config_file: PathBuf,
    lock_file: PathBuf,
}

#[cfg(windows)]
impl Drop for RelayDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.lock_file);
        let _ = std::fs::remove_file(&self.config_file);
        let _ = std::fs::remove_file(&self.version_file);
        let _ = std::fs::remove_file(&self.port_file);
        let _ = std::fs::remove_dir(&self.path);
    }
}

#[cfg(windows)]
fn write_private_relay_file(path: &std::path::Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    let mut file = clack_private_fs::create_new_file(path)?;
    file.write_all(contents)?;
    file.sync_all()?;
    drop(file);
    clack_private_fs::require_private_file(path)?;
    Ok(())
}

#[cfg(windows)]
struct VerifiedBrowserRelay {
    directory: RelayDirectory,
    cli_session: String,
    port: u16,
    _listener_guard: std::net::TcpListener,
    port_guard: Option<std::fs::File>,
    directory_guard: Option<clack_private_fs::PrivateDirectory>,
    expected_cli_pid: std::sync::Arc<std::sync::atomic::AtomicU32>,
    _cli_process: Option<std::os::windows::io::OwnedHandle>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

#[cfg(windows)]
fn bind_exclusive_browser_relay_listener(port: u16) -> std::io::Result<std::net::TcpListener> {
    use std::os::windows::io::AsRawSocket;
    use windows_sys::Win32::Networking::WinSock::{setsockopt, SO_EXCLUSIVEADDRUSE, SOL_SOCKET};

    let socket = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, Some(socket2::Protocol::TCP))?;
    let enabled = 1i32;
    let status = unsafe {
        setsockopt(
            socket.as_raw_socket() as usize,
            SOL_SOCKET,
            SO_EXCLUSIVEADDRUSE,
            (&enabled as *const i32).cast(),
            std::mem::size_of_val(&enabled) as i32,
        )
    };
    if status != 0 {
        return Err(std::io::Error::last_os_error());
    }
    socket.bind(&socket2::SockAddr::from(std::net::SocketAddr::from(([127, 0, 0, 1], port))))?;
    socket.listen(128)?;
    Ok(socket.into())
}

#[cfg(windows)]
impl VerifiedBrowserRelay {
    fn new(bin: &str, session: &str, record: BrowserPidRecord, timeout: Duration) -> std::io::Result<Self> {
        use std::io::{Read, Write};
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

        let program = native_browser_program(bin)?;
        let version = native_browser_version(std::path::Path::new(&program), timeout.min(PROC_CONTROL_TIMEOUT))
            .filter(|version| *version >= semver::Version::new(0, 44, 0))
            .ok_or_else(|| std::io::Error::other("Cannot verify browser CLI version for private control relay"))?;
        let base = run_dir_for_session(session).ok_or_else(|| std::io::Error::other("Cannot prepare private browser control directory"))?;
        let cli_session = format!("relay-{}", uuid::Uuid::new_v4().simple());
        let listener = bind_exclusive_browser_relay_listener(0)?;
        let path = base.join(format!("relay-{}", uuid::Uuid::new_v4().simple()));
        clack_private_fs::create_dir(&path)?;
        let directory = RelayDirectory {
            port_file: path.join(format!("{cli_session}.port")),
            version_file: path.join(format!("{cli_session}.version")),
            config_file: path.join(format!("{cli_session}.config")),
            lock_file: path.join(format!("{cli_session}.lock")),
            path,
        };
        let directory_guard = clack_private_fs::require_private_directory(&directory.path)?;
        // Keep the port bound until the CLI has exited even if the worker
        // thread fails. v0.44 uses only the published `.port` file.
        let listener_guard = listener.try_clone()?;
        let port = listener.local_addr()?.port();
        let mut file = clack_private_fs::create_new_file(&directory.port_file)?;
        write!(file, "{port}")?;
        file.sync_all()?;
        drop(file);
        clack_private_fs::require_private_file(&directory.port_file)?;
        // Deny write/delete sharing for the CLI's entire lifetime. A changed
        // valid `.port` file would otherwise redirect its next connection.
        let mut port_guard = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&directory.port_file)?;
        let mut contents = String::new();
        port_guard.read_to_string(&mut contents)?;
        if contents.trim().parse::<u16>() != Ok(port) {
            return Err(std::io::Error::other("Browser relay port file changed during setup"));
        }
        // The CLI checks these sidecars before reusing the relay endpoint.
        // Without them it would try to stop and replace the relay as a daemon.
        write_private_relay_file(&directory.version_file, version.to_string().as_bytes())?;
        write_private_relay_file(&directory.config_file, br#"{"debug":false}"#)?;
        listener.set_nonblocking(true)?;

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let expected_cli_pid = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let worker_stop = stop.clone();
        let worker_cli_pid = expected_cli_pid.clone();
        let session = session.to_owned();
        let worker = std::thread::spawn(move || {
            while !worker_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut client, _)) => {
                        let expected_pid = (0..20).find_map(|_| {
                            let pid = worker_cli_pid.load(Ordering::Acquire);
                            if pid != 0 { Some(pid) } else {
                                std::thread::sleep(Duration::from_millis(5));
                                None
                            }
                        });
                        let Some(expected_pid) = expected_pid else { continue };
                        if connected_tcp_peer_pid(&client) != Some(expected_pid) {
                            continue;
                        }
                        let session = session.clone();
                        let record = record.clone();
                        std::thread::spawn(move || {
                            if relay_browser_command(&mut client, &session, &record, expected_pid, timeout).is_err() {
                                let _ = client.set_write_timeout(Some(Duration::from_secs(1)));
                                let _ = client.write_all(b"{\"success\":false,\"error\":\"Browser control peer verification failed\"}\n");
                            }
                        });
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self { directory, cli_session, port, _listener_guard: listener_guard, port_guard: Some(port_guard), directory_guard: Some(directory_guard), expected_cli_pid, _cli_process: None, stop, worker: Some(worker) })
    }

    fn register_cli(&mut self, child: &std::process::Child) -> std::io::Result<()> {
        use std::os::windows::io::{AsRawHandle, BorrowedHandle};

        // Duplicate the Child's process handle before its wait completes.
        // Keeping this handle in the relay prevents its PID from being reused
        // while the listener is accepting command connections.
        let borrowed = unsafe { BorrowedHandle::borrow_raw(child.as_raw_handle()) };
        let process = borrowed.try_clone_to_owned()?;
        if process_creation_seconds(process.as_raw_handle()).is_none() {
            return Err(std::io::Error::other("Cannot identify browser CLI process"));
        }
        self._cli_process = Some(process);
        self.expected_cli_pid.store(child.id(), Ordering::Release);
        Ok(())
    }
}

#[cfg(windows)]
impl Drop for VerifiedBrowserRelay {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = std::net::TcpStream::connect(("127.0.0.1", self.port));
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        self.port_guard.take();
        self.directory_guard.take();
    }
}

#[cfg(windows)]
fn write_browser_control_with_deadline(
    stream: &mut std::net::TcpStream,
    mut bytes: &[u8],
    deadline: Instant,
) -> std::io::Result<()> {
    use std::io::Write;

    while !bytes.is_empty() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "Browser control write timed out"));
        }
        stream.set_write_timeout(Some(remaining.min(Duration::from_secs(5))))?;
        let written = stream.write(bytes)?;
        if written == 0 {
            return Err(std::io::Error::from(std::io::ErrorKind::WriteZero));
        }
        bytes = &bytes[written..];
    }
    Ok(())
}

#[cfg(windows)]
fn relay_browser_command(
    client: &mut std::net::TcpStream,
    session: &str,
    record: &BrowserPidRecord,
    expected_cli_pid: u32,
    timeout: Duration,
) -> std::io::Result<()> {
    use std::io::{BufRead, BufReader, Read};
    const MAX_REQUEST_BYTES: usize = 4 * 1024 * 1024;
    const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;

    if connected_tcp_peer_pid(client) != Some(expected_cli_pid) {
        return Err(std::io::Error::other("Browser CLI connection changed owner"));
    }
    let request_deadline = Instant::now() + Duration::from_secs(5);
    let mut request = Vec::new();
    loop {
        let remaining = request_deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(std::io::Error::other("Browser command request timed out"));
        }
        client.set_read_timeout(Some(remaining))?;
        let mut chunk = [0u8; 4096];
        let read = client.read(&mut chunk)?;
        if read == 0 {
            if request.is_empty() {
                return Ok(()); // CLI readiness probe
            }
            return Err(std::io::Error::other("Incomplete browser command"));
        }
        let end = chunk[..read].iter().position(|byte| *byte == b'\n').map(|index| index + 1);
        let take = end.unwrap_or(read);
        if request.len() + take > MAX_REQUEST_BYTES {
            return Err(std::io::Error::other("Browser command exceeds request limit"));
        }
        request.extend_from_slice(&chunk[..take]);
        if end.is_some() {
            break;
        }
    }
    let (mut upstream, _process) = connect_verified_browser_control(session, record)?;
    write_browser_control_with_deadline(&mut upstream, &request, Instant::now() + Duration::from_secs(5))?;
    let response_deadline = Instant::now() + timeout;
    let mut reader = BufReader::new(upstream);
    let mut response_bytes = 0usize;
    loop {
        let remaining = response_deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(std::io::Error::other("Browser command response timed out"));
        }
        reader.get_ref().set_read_timeout(Some(remaining))?;
        let chunk = reader.fill_buf()?;
        if chunk.is_empty() {
            return Err(std::io::Error::other("Browser daemon closed without a complete response"));
        }
        let end = chunk.iter().position(|byte| *byte == b'\n').map(|index| index + 1);
        let take = end.unwrap_or(chunk.len());
        response_bytes = response_bytes.checked_add(take)
            .filter(|bytes| *bytes <= MAX_RESPONSE_BYTES)
            .ok_or_else(|| std::io::Error::other("Browser response exceeds size limit"))?;
        write_browser_control_with_deadline(client, &chunk[..take], response_deadline)?;
        reader.consume(take);
        if end.is_some() {
            return Ok(());
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn build_agent_browser_command(binary: &str, session: &str, action: &str, params: &serde_json::Value) -> Result<String, String> {
    let bin = shell_binary(binary);
    let session = shell_quote(session);
    let command = match action {
        "open_url" | "open" => {
            let url = params.get("url").and_then(|v| v.as_str()).unwrap_or("about:blank");
            format!(
                "{bin} open {url} --session {s}",
                bin = bin,
                url = shell_quote(url),
                s = session,
            )
        }
        "screenshot" => format!("{} screenshot --session {}", bin, session),
        "snapshot" | "accessibility_snapshot" => {
            format!("{} snapshot -i --session {}", bin, session)
        }
        "click" => {
            let selector = params.get("selector").and_then(|v| v.as_str()).unwrap_or("body");
            format!("{} click {} --session {}", bin, shell_quote(selector), session)
        }
        "fill" => {
            let selector = params.get("selector").and_then(|v| v.as_str()).unwrap_or("body");
            let value = params.get("value").and_then(|v| v.as_str()).unwrap_or("");
            format!(
                "{} fill {} {} --session {}",
                bin,
                shell_quote(selector),
                shell_quote(value),
                session
            )
        }
        "type_text" => {
            let text = params.get("text").and_then(|v| v.as_str()).unwrap_or("");
            format!("{} type body {} --session {}", bin, shell_quote(text), session)
        }
        "console_logs" | "console" => format!("{} console --session {}", bin, session),
        "evaluate" | "eval" => {
            let script = params.get("script").and_then(|v| v.as_str()).unwrap_or("");
            format!("{} eval {} --session {}", bin, shell_quote(script), session)
        }
        "back" => format!("{} back --session {}", bin, session),
        "forward" => format!("{} forward --session {}", bin, session),
        "reload" => format!("{} reload --session {}", bin, session),
        "viewport" => {
            let spec = resolve_viewport_params(params);
            // Match the argv builder: only emit the scale arg when it
            // differs from 1.0 so existing call sites that pass plain
            // {width, height} produce the same exact shell string.
            if (spec.dpr - 1.0).abs() > f64::EPSILON {
                format!(
                    "{} set viewport {} {} {} --session {}",
                    bin,
                    spec.width,
                    spec.height,
                    format_dpr(spec.dpr),
                    session
                )
            } else {
                format!(
                    "{} set viewport {} {} --session {}",
                    bin, spec.width, spec.height, session
                )
            }
        }
        // New v0.24.0 commands
        "get_styles" => {
            let selector = params.get("selector").and_then(|v| v.as_str()).unwrap_or("body");
            format!("{} get styles {} --json --session {}", bin, shell_quote(selector), session)
        }
        "wait" => {
            let selector = params.get("selector").and_then(|v| v.as_str()).unwrap_or("");
            let text = params.get("text").and_then(|v| v.as_str());
            if let Some(text) = text {
                format!("{} wait --text {} --session {}", bin, shell_quote(text), session)
            } else {
                format!("{} wait {} --session {}", bin, shell_quote(selector), session)
            }
        }
        "get_text" => {
            let selector = params.get("selector").and_then(|v| v.as_str()).unwrap_or("body");
            format!("{} get text {} --session {}", bin, shell_quote(selector), session)
        }
        "get_box" => {
            let selector = params.get("selector").and_then(|v| v.as_str()).unwrap_or("body");
            format!("{} get box {} --json --session {}", bin, shell_quote(selector), session)
        }
        _ => return Err(format!("Unknown action: {}", action)),
    };

    Ok(command)
}

fn make_request_id() -> String {
    format!(
        "req-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    )
}

fn execute_agent_browser_action(browser_id: &str, action: &str, params: serde_json::Value, stream_port: u16) -> Result<BrowserAutomationResult, String> {
    let session = session_name(browser_id);
    validate_session_name(session)?;
    require_owned_browser_session(session)?;
    let bin = resolve_binary()?;

    #[cfg(target_os = "windows")]
    let (stdout, stderr, status_success) = {
        let groups = build_agent_browser_argv_groups(session, action, &params)?;
        let mut combined_stdout = String::new();
        let mut combined_stderr = String::new();
        let mut all_ok = true;
        for argv in &groups {
            let out = run_agent_browser_native(&bin, argv, session, stream_port, false, action_timeout(action))
                .map_err(|e| format!("Failed to run agent-browser: {}", e))?;
            combined_stdout.push_str(&String::from_utf8_lossy(&out.stdout));
            combined_stderr.push_str(&String::from_utf8_lossy(&out.stderr));
            if !out.status.success() {
                all_ok = false;
                break; // matches `cmd1 && cmd2` short-circuit semantics
            }
        }
        (combined_stdout, combined_stderr, all_ok)
    };

    #[cfg(not(target_os = "windows"))]
    let (stdout, stderr, status_success) = {
        let shell_cmd = build_agent_browser_command(&bin, session, action, &params)?;
        let mut cmd = std::process::Command::new("sh");
        set_browser_owner_env(&mut cmd, session)?;
        cmd.args(["-c", &shell_cmd])
            .env("AGENT_BROWSER_STREAM_PORT", stream_port.to_string())
            .env("AGENT_BROWSER_ARGS", STEALTH_CHROMIUM_ARGS)
            .env("AGENT_BROWSER_USER_AGENT", stealth_user_agent());
        let output = output_capture_with_timeout(cmd, action_timeout(action))
            .map_err(|error| format!("Failed to run agent-browser: {}", error))?;
        (
            String::from_utf8_lossy(&output.stdout).to_string(),
            String::from_utf8_lossy(&output.stderr).to_string(),
            output.status.success(),
        )
    };

    if !status_success && !stdout.contains("✓") && !stdout.contains("{") && !stdout.contains("- ") {
        return Err(format!("agent-browser failed: {} {}", stdout, stderr));
    }
    if action == "close" {
        clear_session_pid(session);
    } else {
        let _ = track_current_browser_daemon(session);
    }

    // For snapshot: detect useless ARIA result and fall back to DOM-based query
    if (action == "snapshot" || action == "accessibility_snapshot") && needs_dom_fallback(&stdout) {
        eprintln!("[codemux::browser] ARIA snapshot empty, falling back to DOM query");
        let dom_params = serde_json::json!({ "script": DOM_SNAPSHOT_SCRIPT });

        #[cfg(target_os = "windows")]
        let dom_output_opt = {
            match build_agent_browser_argv_groups(session, "eval", &dom_params) {
                Ok(groups) if !groups.is_empty() => {
                    run_agent_browser_native(&bin, &groups[0], session, stream_port, false, ACTION_TIMEOUT).ok()
                }
                _ => None,
            }
        };
        #[cfg(not(target_os = "windows"))]
        let dom_output_opt = match build_agent_browser_command(&bin, session, "eval", &dom_params) {
            Ok(dom_cmd) => {
                let mut cmd = std::process::Command::new("sh");
                if set_browser_owner_env(&mut cmd, session).is_err() {
                    None
                } else {
                    cmd.args(["-c", &dom_cmd]);
                    output_capture_with_timeout(cmd, ACTION_TIMEOUT).ok()
                }
            }
            Err(_) => None,
        };

        if let Some(dom_output) = dom_output_opt {
            let dom_stdout = String::from_utf8_lossy(&dom_output.stdout).to_string();
            let dom_tree = extract_eval_result(&dom_stdout);
            if !dom_tree.is_empty() && dom_tree != "(no elements found)" {
                let combined = format!(
                    "{}\n\n--- Interactive Elements (DOM) ---\n{}",
                    stdout.trim(),
                    dom_tree
                );
                return Ok(BrowserAutomationResult {
                    request_id: make_request_id(),
                    browser_id: browser_id.to_string(),
                    data: serde_json::json!({ "tree": combined }),
                    message: None,
                });
            }
        }
    }

    let data: serde_json::Value = if stdout.contains("{") {
        serde_json::from_str(&stdout).unwrap_or(serde_json::json!({ "raw": stdout }))
    } else if action == "screenshot" {
        if stdout.trim().is_empty() {
            serde_json::json!({ "error": "No screenshot" })
        } else {
            serde_json::json!({ "raw": stdout })
        }
    } else if action == "snapshot" || action == "accessibility_snapshot" {
        serde_json::json!({ "tree": stdout })
    } else {
        // For eval results, the native binary may wrap string values in quotes — strip them
        let result_str = if (action == "evaluate" || action == "eval") && stdout.trim().starts_with('"') {
            extract_eval_result(&stdout)
        } else {
            stdout.clone()
        };
        serde_json::json!({ "result": result_str, "success": status_success })
    };

    Ok(BrowserAutomationResult {
        request_id: make_request_id(),
        browser_id: browser_id.to_string(),
        data,
        message: None,
    })
}

pub fn run_cli_action(browser_id: &str, action: &str, params: serde_json::Value, stream_port: u16) -> Result<BrowserAutomationResult, String> {
    execute_agent_browser_action(browser_id, action, params, stream_port)
}

impl Default for AgentBrowserManager {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentBrowserManager {
    pub fn new() -> Self {
        Self {
            next_port: Arc::new(AtomicU16::new(DEFAULT_STREAM_PORT)),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            start_lock: Arc::new(Mutex::new(())),
            #[cfg(test)]
            test_stream_ports: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Create a manager and inspect persisted daemon records. Legacy numeric
    /// PID files cannot establish ownership and never trigger process kills.
    pub fn new_with_cleanup() -> Self {
        Self::new_with_adoption()
    }

    /// Public name for the new behavior. `new_with_cleanup` remains an alias.
    pub fn new_with_adoption() -> Self {
        let mgr = Self::new();
        Self::reconcile_runtime_files();
        mgr
    }

    /// Keep live records and discard only records whose PID no longer exists.
    /// Ownership is checked again before any later action or kill.
    fn reconcile_runtime_files() {
        for dir in browser_runtime_dirs() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                let Some(_session) = session_from_pid_path(&path) else { continue };
                let Some(record) = read_browser_pid_record(&path) else { continue };
                if !pid_alive(record.pid) {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
    }

    /// Allocate a unique stream port for the given session key.
    /// Returns the existing port if already allocated, or assigns the next
    /// available port in the range [DEFAULT_STREAM_PORT, MAX_STREAM_PORT].
    ///
    /// The candidate port is filtered through three checks before being
    /// handed out:
    ///   1. **In-memory session map** — no other live session may own it.
    ///   2. **Bind-test** (`TcpListener::bind` on `127.0.0.1`) — skips
    ///      ports occupied by ghost daemons that we don't track. This used
    ///      to be Windows-only because `kill_session_tree` is a no-op
    ///      there, but the same ghost-port problem can hit Linux/macOS too
    ///      (orphaned processes reparented to PID 1 after a worktree shell
    ///      exits without closing the manager's session). Running it on
    ///      every OS makes allocation symmetric and removes a class of
    ///      "frontend connects to dead port" failures.
    ///   3. **Counter advance** — `next_port` is monotonic across the
    ///      9223–9299 range and wraps back to the start when it exhausts.
    ///      This is intentional: immediately reusing a freshly closed port
    ///      can race against the daemon's still-flushing socket on
    ///      TIME_WAIT, so we prefer giving the OS a moment by advancing.
    pub async fn allocate_port(&self, session_key: &str) -> Result<u16, String> {
        validate_session_name(session_name(session_key))?;
        let mut sessions = self.sessions.lock().await;
        if let Some(s) = sessions.get(session_key) {
            return Ok(s.port);
        }

        // Assign the next port from the counter, skipping ports owned by
        // other sessions or by ghost listeners we can't see.
        let range_size = MAX_STREAM_PORT - DEFAULT_STREAM_PORT + 1;
        for _ in 0..range_size {
            let port = self.next_port.fetch_add(1, Ordering::Relaxed);
            // Wrap around if we've gone past the range.
            if port > MAX_STREAM_PORT {
                self.next_port.store(DEFAULT_STREAM_PORT, Ordering::Relaxed);
                continue;
            }
            // Check no other session already owns this port.
            if sessions.values().any(|s| s.port == port) {
                continue;
            }

            // Symmetric bind test.
            // We bind `127.0.0.1:<port>` and immediately drop the listener;
            // if the bind fails some other process (often a dead-PID leak
            // from a previous codemux run, or a separate codemux instance,
            // or a tool the user is running) is on it. Skipping it is
            // cheaper and safer than returning the port and watching the
            // frontend's WebSocket fail to connect.
            if std::net::TcpListener::bind(("127.0.0.1", port)).is_err() {
                continue;
            }

            sessions.insert(
                session_key.to_string(),
                StreamSession {
                    port,
                    running: false,
                    pid: None,
                    pid_start_time: None,
                    stream_generation: 0,
                    last_seen_at: None,
                },
            );
            return Ok(port);
        }
        Err("All browser stream ports (9223-9299) are in use".to_string())
    }

    /// Return the port allocated for a session, if any.
    pub async fn get_port(&self, session_key: &str) -> Option<u16> {
        self.sessions.lock().await.get(session_key).map(|s| s.port)
    }

    async fn clear_failed_stream_allocation(&self, session_key: &str, port: u16) {
        let mut sessions = self.sessions.lock().await;
        if sessions.get(session_key).is_some_and(|s| s.port == port && !s.running) {
            sessions.remove(session_key);
        }
    }

    /// Authorize a stream bridge only for a live session tracked by this
    /// manager. The identity is rechecked for every bridged request so a
    /// closed browser cannot leave access to a later port occupant.
    pub(crate) async fn verified_stream_identity(&self, browser_id: &str) -> Result<BrowserStreamIdentity, String> {
        let session = session_name(browser_id).to_owned();
        validate_session_name(&session)?;
        let _start_guard = self.start_lock.lock().await;
        let (port, generation) = {
            let sessions = self.sessions.lock().await;
            let stream = sessions.get(browser_id)
                .ok_or_else(|| format!("Browser session {session} is not streaming"))?;
            if !stream.running {
                return Err(format!("Browser session {session} is not streaming"));
            }
            (stream.port, stream.stream_generation)
        };
        let record = tokio::task::spawn_blocking(move || {
            require_owned_browser_session(&session)?;
            let dir = run_dir_for_session(&session)
                .ok_or_else(|| format!("Cannot verify browser session {session} runtime directory"))?;
            if !browser_daemon_has_protected_stream(&session, &dir) {
                return Err(format!("Browser session {session} uses an outdated stream daemon"));
            }
            let pid = read_agent_browser_daemon_pid(&session)
                .ok_or_else(|| format!("Cannot verify browser session {session} PID"))?;
            let record = owned_browser_pid(&session, pid)
                .ok_or_else(|| format!("Cannot verify browser session {session} owner"))?;
            if !listener_owned_by_pid(port, pid) {
                return Err(format!("Browser session {session} does not own stream port {port}"));
            }
            Ok(record)
        })
        .await
        .map_err(|error| format!("Browser ownership check failed: {error}"))??;
        let mut sessions = self.sessions.lock().await;
        let stream = sessions.get_mut(browser_id)
            .ok_or_else(|| "Browser session closed during ownership check".to_string())?;
        if !stream.running || stream.port != port || stream.stream_generation != generation {
            return Err("Browser session changed during ownership check".into());
        }
        Ok(refresh_stream_session_identity(stream, &record))
    }

    pub(crate) async fn verified_stream_identity_for_port(&self, port: u16) -> Option<BrowserStreamIdentity> {
        #[cfg(test)]
        if let Some(identity) = self.test_stream_ports.lock().await.get(&port).copied() {
            return Some(identity);
        }
        let browser_id = {
            let sessions = self.sessions.lock().await;
            sessions.iter()
                .find(|(_, session)| session.port == port && session.running)
                .map(|(browser_id, _)| browser_id.clone())
        };
        let browser_id = browser_id?;
        self.verified_stream_identity(&browser_id).await
            .ok()
            .filter(|identity| identity.port == port)
    }

    #[cfg(test)]
    pub(crate) async fn owns_stream_port(&self, port: u16) -> bool {
        self.verified_stream_identity_for_port(port).await.is_some()
    }

    #[cfg(test)]
    pub(crate) async fn allow_test_stream_port(&self, port: u16) {
        self.test_stream_ports.lock().await.insert(port, current_process_stream_identity(port));
    }

    async fn clear_stale_running_stream_allocation(&self, session_key: &str, port: u16) {
        let mut sessions = self.sessions.lock().await;
        if sessions.get(session_key).is_some_and(|s| s.port == port && s.running) {
            sessions.remove(session_key);
        }
    }

    /// Reconcile a provisional `running` flag with the verified daemon after
    /// a first or failed command. The lifecycle gate remains held by callers.
    async fn refresh_running_session(&self, browser_id: &str, session: &str) -> bool {
        let session = session.to_owned();
        let pid = tokio::task::spawn_blocking(move || live_owned_browser_pid(&session))
            .await
            .ok()
            .flatten();
        if let Some(entry) = self.sessions.lock().await.get_mut(browser_id) {
            entry.running = pid.is_some();
            entry.pid = pid;
        }
        pid.is_some()
    }

    /// **Deprecated.** No-op kept for binary/source compatibility with any
    /// external caller that linked against the old aliasing-based API.
    ///
    /// The previous design registered the same port under multiple keys
    /// (workspace_id and cli_session_name) so different code paths could
    /// look up the session by whichever id they had. That dual-keying was
    /// the root cause of the leaked-alias bug: `close()` only
    /// cleaned the key it was passed, leaving the alias entry pinning the
    /// port forever.
    ///
    /// All in-tree call sites now allocate by `cli_session_name` directly
    /// (see `control.rs::browser_automation`), so the alias is never
    /// needed. This function is kept as a no-op rather than removed so
    /// downstream forks or stale call sites compile cleanly while we
    /// migrate.
    #[deprecated(
        note = "Allocate ports by cli_session_name directly; no aliasing required."
    )]
    pub async fn ensure_port(&self, _session_key: &str, _port: u16) {
        // Intentionally empty. See doc comment.
    }

    pub async fn spawn(&self, browser_id: &str) -> Result<(), String> {
        let session = session_name(browser_id).to_string();
        validate_session_name(&session)?;
        let manager = self.clone();
        let browser_id = browser_id.to_owned();
        run_detached_browser_lifecycle(session, async move {
            manager.spawn_locked(&browser_id).await
        }).await
    }

    async fn spawn_locked(&self, browser_id: &str) -> Result<(), String> {
        let session = session_name(browser_id).to_string();
        let port = self.allocate_port(browser_id).await?;

        let was_running = {
            let sessions = self.sessions.lock().await;
            sessions.get(browser_id).is_some_and(|s| s.running)
        };
        if was_running && self.refresh_running_session(browser_id, &session).await {
            return Ok(());
        }

        // The launch shells out to agent-browser (which forks the daemon) and
        // can block on a real page load — run it on a blocking thread with a
        // hard timeout so it can never stall the async runtime (issue #96).
        let pid = tokio::task::spawn_blocking(move || -> Result<u32, String> {
            require_owned_browser_session(&session)?;
            let bin = resolve_binary()?;

            #[cfg(target_os = "windows")]
            let output = {
                // `--executable-path` is a top-level flag — must come before
                // the `open` subcommand or clap rejects it.
                let mut argv: Vec<String> = windows_executable_path_args();
                argv.extend([
                    "open".into(),
                    "about:blank".into(),
                    "--session".into(),
                    session.clone(),
                ]);
                run_agent_browser_native(&bin, &argv, &session, port, false, OPEN_TIMEOUT)
                    .map_err(|e| format!("Failed to start agent-browser: {}", e))?
            };
            #[cfg(not(target_os = "windows"))]
            let output = {
                let cmd = browser_open_command(&bin, &session, port)?;
                output_capture_with_timeout(cmd, OPEN_TIMEOUT)
                    .map_err(|e| format!("Failed to start agent-browser: {}", e))?
            };

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(format!("Failed to start browser: {}", stderr));
            }
            track_current_browser_daemon(&session)
                .map(|record| record.pid)
                .ok_or_else(|| "Could not verify the started browser daemon".to_string())
        })
        .await
        .map_err(|e| format!("agent-browser spawn task failed: {e}"))??;

        self.sessions.lock().await.entry(browser_id.to_string()).and_modify(|s| {
            s.running = true;
            s.pid = Some(pid);
            s.pid_start_time = None;
            s.stream_generation = uuid::Uuid::new_v4().as_u128();
        });
        Ok(())
    }

    /// Record that a command is about to run and return whether the daemon
    /// was already considered running.
    ///
    /// For every action except "close" this marks the session running BEFORE
    /// the blocking CLI call. The CLI auto-starts the daemon on first
    /// invocation, so it's effectively "running" as soon as we call it.
    /// Setting this early prevents a concurrent start_stream (from
    /// BrowserPane mounting) from killing the daemon mid-command. The prior
    /// flag is returned so the fresh-launch viewport default in run_command
    /// fires exactly once per daemon lifetime.
    ///
    /// "close" only reads the flag: the early-mark exists to protect a
    /// daemon we want alive, which is pointless when we're about to kill it
    /// ourselves — and marking it running would leave a stale `true` behind.
    async fn note_action_start(&self, browser_id: &str, action: &str) -> bool {
        let mut sessions = self.sessions.lock().await;
        match sessions.get_mut(browser_id) {
            Some(s) => {
                if action == "close" {
                    s.running
                } else {
                    let was_running = std::mem::replace(&mut s.running, true);
                    if !was_running {
                        s.stream_generation = uuid::Uuid::new_v4().as_u128();
                    }
                    was_running
                }
            }
            None => false,
        }
    }

    /// Reset session state after a "close" action so the next command sees a
    /// fresh daemon (and re-applies the default viewport). Called
    /// unconditionally — even if the close reported an error the daemon
    /// state is unknown at worst, and both consumers of `running` tolerate a
    /// stale `false`: start_stream re-probes the socket, and a redundant
    /// viewport apply on a still-live daemon is harmless right after the
    /// agent chose to close it. The port allocation / map entry stays intact.
    async fn note_close_finished(&self, browser_id: &str) {
        let mut sessions = self.sessions.lock().await;
        if let Some(s) = sessions.get_mut(browser_id) {
            s.running = false;
            s.pid = None;
            s.pid_start_time = None;
            s.stream_generation = 0;
        }
    }

    pub async fn run_command(&self, browser_id: &str, action: &str, params: serde_json::Value) -> Result<BrowserAutomationResult, String> {
        let session = session_name(browser_id).to_string();
        validate_session_name(&session)?;
        let manager = self.clone();
        let browser_id = browser_id.to_owned();
        let action = action.to_owned();
        run_detached_browser_lifecycle(session, async move {
            manager.run_command_locked(&browser_id, &action, params).await
        }).await
    }

    async fn run_command_locked(&self, browser_id: &str, action: &str, params: serde_json::Value) -> Result<BrowserAutomationResult, String> {
        let session = session_name(browser_id).to_string();
        let port = self.allocate_port(browser_id).await?;
        let was_running = self.note_action_start(browser_id, action).await;

        // User-configured default viewport (`browser.default_viewport`,
        // synced settings): apply it to a freshly launched daemon BEFORE the
        // requested action, so even a first-command screenshot renders at
        // the user's preferred size (e.g. 2560×1440 to match their monitor)
        // instead of the agent-browser built-in. Only when the setting is
        // present — unset keeps today's behavior byte-for-byte. Skipped for:
        // - "viewport": the caller is setting an explicit size anyway;
        // - "close": don't boot a daemon just to resize-then-kill it.
        // Never re-applied to a running daemon, so an agent's own viewport
        // choice survives subsequent commands. Best-effort: a failure here
        // must not block the real action.
        if !was_running && action != "viewport" && action != "close" {
            if let Some(spec) = crate::browser_viewport::configured_default_viewport() {
                let bid = browser_id.to_string();
                let vp_params = crate::browser_viewport::socket_action(spec);
                let _ = tokio::task::spawn_blocking(move || {
                    execute_agent_browser_action(&bid, "viewport", vp_params, port)
                })
                .await;
            }
        }

        // Run the (timeout-bounded) CLI work on a blocking thread so a slow
        // agent-browser round-trip never stalls the async executor.
        let owned_browser_id = browser_id.to_string();
        let owned_action = action.to_string();
        let result = tokio::task::spawn_blocking(move || {
            execute_agent_browser_action(&owned_browser_id, &owned_action, params, port)
        })
        .await
        .map_err(|e| format!("agent-browser action task failed: {e}"))
        .and_then(|result| result);

        // The agent-facing "close" goes through this path (not close()), so
        // reset the state here — otherwise the entry keeps running = true and
        // a later open on the fresh daemon would skip the default viewport.
        if action == "close" {
            self.note_close_finished(browser_id).await;
        } else if !was_running || result.is_err() {
            self.refresh_running_session(browser_id, &session).await;
        }

        result
    }

    pub async fn get_screenshot(&self, browser_id: &str) -> Result<String, String> {
        let session = session_name(browser_id).to_string();
        validate_session_name(&session)?;
        let manager = self.clone();
        let browser_id = browser_id.to_owned();
        run_detached_browser_lifecycle(session, async move {
            manager.get_screenshot_locked(&browser_id).await
        }).await
    }

    async fn get_screenshot_locked(&self, browser_id: &str) -> Result<String, String> {
        let session = session_name(browser_id).to_string();
        let port = self.allocate_port(browser_id).await?;

        // The CLI call + file read are blocking; run them off the executor
        // under a hard timeout so a wedged screenshot can't hang the runtime.
        tokio::task::spawn_blocking(move || -> Result<String, String> {
            require_owned_browser_session(&session)?;
            let bin = resolve_binary()?;

            #[cfg(target_os = "windows")]
            let output = run_agent_browser_native(
                &bin,
                &[
                    "screenshot".into(),
                    "--session".into(),
                    session.clone(),
                ],
                &session,
                port,
                false,
                ACTION_TIMEOUT,
            )
            .map_err(|e| format!("Failed to get screenshot: {}", e))?;
            #[cfg(not(target_os = "windows"))]
            let output = {
                let mut cmd = std::process::Command::new("sh");
                set_browser_owner_env(&mut cmd, &session)?;
                cmd.args(["-c", &format!("{} screenshot --session {}", shell_binary(&bin), shell_quote(&session))])
                    .env("AGENT_BROWSER_STREAM_PORT", port.to_string())
                    .env("AGENT_BROWSER_ARGS", STEALTH_CHROMIUM_ARGS)
                    .env("AGENT_BROWSER_USER_AGENT", stealth_user_agent());
                output_capture_with_timeout(cmd, ACTION_TIMEOUT)
                    .map_err(|e| format!("Failed to get screenshot: {}", e))?
            };

            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            if output.status.success() {
                let _ = track_current_browser_daemon(&session);
            }

            // Parse the screenshot path from output like "Screenshot saved to /path/to/file.png"
            // Strip ANSI codes (escape sequences like \x1b[32m)
            let mut clean = String::new();
            let mut in_ansi = false;
            for c in stdout.chars() {
                if c == '\x1b' {
                    in_ansi = true;
                } else if in_ansi && c == 'm' {
                    in_ansi = false;
                } else if !in_ansi {
                    clean.push(c);
                }
            }

            if let Some(path_start) = clean.find("Screenshot saved to ") {
                let path = clean[path_start + 19..].trim();

                // Read the file and convert to base64
                if let Ok(data) = std::fs::read(path) {
                    let base64 = base64_encode(&data);
                    return Ok(format!("data:image/png;base64,{}", base64));
                }
            }

            Ok(clean)
        })
        .await
        .map_err(|e| format!("agent-browser screenshot task failed: {e}"))?
    }

    /// Atomic teardown for a session. Ordered so each step's failure
    /// leaves the system in a recoverable state:
    ///
    ///   1. Pop the session from the map under the lock — no other caller
    ///      can find it after this point, even if subsequent steps stall.
    ///   2. Verify the daemon's process marker, session, and start time.
    ///   3. Issue a graceful CLI close, then signal that same daemon only if
    ///      it is still running and still matches the ownership record.
    ///   4. Clean up our PID file under `~/.codemux/run/`.
    ///
    /// Returns Ok even if individual steps fail — close is idempotent and
    /// callers should not be blocked from retrying or moving on.
    pub async fn close(&self, browser_id: &str) -> Result<(), String> {
        let session = session_name(browser_id).to_string();
        validate_session_name(&session)?;
        let manager = self.clone();
        let browser_id = browser_id.to_owned();
        run_detached_browser_lifecycle(session, async move {
            manager.close_locked(&browser_id).await
        }).await
    }

    async fn close_locked(&self, browser_id: &str) -> Result<(), String> {
        let session = session_name(browser_id).to_string();

        // Step 1: pop the session under the lock so nothing else races us.
        let removed = self.sessions.lock().await.remove(browser_id);
        let had_session = removed.is_some();

        // Steps 2–3 spawn external processes for a verified daemon. They are compiled
        // out of unit-test builds: the close() tests assert session-map and
        // port-allocator semantics and must not depend on a real
        // agent-browser binary on the test host. Every CLI/kill invocation
        // now carries a hard timeout, and the whole teardown runs on a
        // blocking thread so a wedged daemon can't hang close() or stall the
        // async executor — the failure mode that hung `cargo test` to the
        // 30-minute CI job timeout on Windows runners (issue #96).
        #[cfg(not(test))]
        {
            let session_for_shutdown = session.clone();
            let _ = tokio::task::spawn_blocking(move || {
                Self::shutdown_session_processes(&session_for_shutdown, removed);
            })
            .await;
        }
        #[cfg(test)]
        let _ = removed;

        // Keep the ownership record if verified teardown failed so the
        // process-exit sweep can retry stopping the same daemon.
        let owned_daemon_still_running = read_agent_browser_daemon_pid(&session)
            .and_then(|pid| owned_browser_pid(&session, pid))
            .is_some_and(|record| browser_process_matches(&record, &session));
        if had_session && !owned_daemon_still_running {
            clear_session_pid(&session);
        }

        Ok(())
    }

    /// Best-effort teardown of the daemon behind a closed session:
    /// graceful CLI `close` on Unix, then a kill of the verified daemon. Spawns
    /// external processes — which is exactly why
    /// `close()` only calls this in non-test builds.
    #[cfg(not(test))]
    fn shutdown_session_processes(session: &str, removed: Option<StreamSession>) {
        let Some(removed) = removed else { return };
        let Some(pid) = removed.pid.or_else(|| read_agent_browser_daemon_pid(session)) else { return };
        let Some(record) = owned_browser_pid(session, pid) else { return };
        if let Ok(bin) = resolve_binary() {
            stop_owned_browser_daemon(session, &record, &bin);
        } else {
            kill_verified_browser_pid(&record, session);
        }
    }

    /// Start the browser session and return the WebSocket stream URL.
    ///
    /// With agent-browser v0.24.0+, the Rust daemon auto-starts on first
    /// command and streaming is enabled by default. The flow is:
    ///   1. Allocate a unique port for this session.
    ///   2. Set `AGENT_BROWSER_STREAM_PORT` so the daemon binds to our port.
    ///   3. Run `agent-browser open` to trigger daemon + browser launch,
    ///      with stderr redirected to `~/.codemux/run/agent-browser-{name}.log`
    ///      so the next investigation has a paper trail.
    ///   4. Probe the port with `probe_stream_port()` — only mark
    ///      `running = true` when we've actually seen a live socket. The
    ///      old "sleep(1s) and trust" approach lied to the frontend when
    ///      the daemon crashed mid-startup, leaving the BrowserPane to
    ///      retry-loop a dead URL forever.
    ///   5. Capture the daemon PID from `~/.codemux/run/{name}.pid` and
    ///      mirror it into our own `~/.codemux/run/{name}.pid` so the next
    ///      process-tree teardown can target it directly.
    ///   6. Return the WebSocket URL.
    pub async fn start_stream(&self, browser_id: &str) -> Result<String, String> {
        let session = session_name(browser_id).to_string();
        validate_session_name(&session)?;
        let manager = self.clone();
        let browser_id = browser_id.to_owned();
        run_detached_browser_lifecycle(session, async move {
            manager.start_stream_locked(&browser_id).await
        }).await
    }

    async fn start_stream_locked(&self, browser_id: &str) -> Result<String, String> {
        let session = session_name(browser_id).to_string();

        // Serialize start_stream calls. The old code held a single Mutex<bool>
        // across the entire operation (including sleeps). Without this, concurrent
        // callers (React StrictMode double-mount, pane remount + agent action)
        // both see running=false and race to start/kill the daemon.
        let _start_guard = self.start_lock.lock().await;

        // Re-check running under the start_lock — a prior call may have finished.
        // A previously-marked-running session might still be live; verify with a
        // quick TCP probe so we don't return a URL pointing at a dead daemon.
        if let Some(port) = self.get_port(browser_id).await {
            let already_running = {
                let sessions = self.sessions.lock().await;
                sessions.get(browser_id).map_or(false, |s| s.running)
            };
            if already_running && probe_stream_port(port, 1, Duration::from_millis(0)) {
                let owned = read_agent_browser_daemon_pid(&session)
                    .and_then(|pid| owned_browser_pid(&session, pid));
                let Some(record) = owned else {
                    self.clear_stale_running_stream_allocation(browser_id, port).await;
                    return Err(format!("Could not verify ownership of browser session {session}"));
                };
                if !listener_owned_by_pid(port, record.pid) {
                    self.clear_stale_running_stream_allocation(browser_id, port).await;
                    return Err(format!("Browser stream port {port} is not owned by session {session}"));
                }
                if run_dir_for_session(&session)
                    .is_some_and(|dir| browser_daemon_has_protected_stream(&session, &dir)) {
                    return Ok(format!("ws://127.0.0.1:{port}"));
                }
            }
            // running was true but the socket is dead — clear the flag so
            // we don't hand the caller a stale URL. The respawn path below
            // will set it again once the new daemon is verified.
            self.sessions
                .lock()
                .await
                .entry(browser_id.to_string())
                .and_modify(|s| {
                    s.running = false;
                    s.pid = None;
                    s.pid_start_time = None;
                    s.stream_generation = 0;
                });
        }

        // A previous daemon for this session must be stopped before we assign
        // a new stream port. Only a verified Codemux-owned daemon is eligible.
        let bin = {
            let session_for_close = session.clone();
            tokio::task::spawn_blocking(move || -> Result<String, String> {
                let bin = resolve_binary()?;
                require_owned_browser_session(&session_for_close)?;
                if let Some(pid) = read_agent_browser_daemon_pid(&session_for_close) {
                    if pid_alive(pid) {
                        let record = owned_browser_pid(&session_for_close, pid)
                            .ok_or_else(|| format!("Browser session {} is already used by another process", session_for_close))?;
                        stop_owned_browser_daemon(&session_for_close, &record, &bin);
                        if pid_alive(pid) {
                            return Err(format!("Could not stop browser session {session_for_close}"));
                        }
                    }
                }
                Ok(bin)
            })
            .await
            .map_err(|e| format!("agent-browser stale-close task failed: {e}"))??
        };

        let port = self.allocate_port(browser_id).await?;

        // Launch the daemon, health-probe it, and capture
        // its PID. Every step here shells out, sleeps, or blocks on a TCP
        // probe, so the whole sequence runs on a blocking thread under hard
        // timeouts so it can never stall the async runtime (issue #96).
        #[cfg(windows)]
        let session_after_launch = session.clone();
        let launch_result = tokio::task::spawn_blocking(move || -> Result<(bool, Option<u32>), String> {
            if std::net::TcpListener::bind(("127.0.0.1", port)).is_err() {
                return Err(format!("Browser stream port {port} was taken by another process"));
            }

            // Launch browser via CLI. The v0.24.0 Rust daemon auto-starts and
            // streaming is enabled by default when AGENT_BROWSER_STREAM_PORT is set.
            eprintln!(
                "[codemux::browser] Starting browser session={} port={}",
                session, port
            );

            // Send stderr to a per-session log file under ~/.codemux/run/.
            // Best-effort — if we can't open the log file we fall back to the
            // inherited stderr so behavior matches the pre-fix baseline.
            let log_path = session_log_path(&session);
            let log_file = log_path.as_deref().and_then(open_session_log);

            #[cfg(target_os = "windows")]
            {
                let started = Instant::now();
                let (record, started_here) = ensure_owned_browser_daemon(&bin, &session, port, OPEN_TIMEOUT)
                    .map_err(|error| format!("Browser daemon startup failed: {error}"))?;
                // The daemon exists before the CLI issues `open`. Persist
                // its identity now so failed opens remain visible to teardown.
                track_browser_daemon_before_command(&session, &bin, &record, started_here)
                    .map_err(|error| format!("Cannot track browser daemon: {error}"))?;
                let mut argv: Vec<String> = windows_executable_path_args();
                argv.extend([
                    "open".into(),
                    "about:blank".into(),
                    "--session".into(),
                    session.clone(),
                ]);
                finish_browser_stream_open(
                    run_agent_browser_native_with_record(
                        &bin,
                        &argv,
                        &session,
                        port,
                        false,
                        OPEN_TIMEOUT.saturating_sub(started.elapsed()),
                        record.clone(),
                    ),
                    log_file,
                    &session,
                    &bin,
                    &record,
                    started_here,
                )?;
            }
            #[cfg(not(target_os = "windows"))]
            {
                let before_open = track_current_browser_daemon(&session);
                let mut cmd = browser_open_command(&bin, &session, port)?;
                if let Some(log) = log_file {
                    cmd.stderr(log);
                }
                let opened = require_successful_browser_stream_open_status(
                    launch_status_with_timeout(cmd, OPEN_TIMEOUT),
                );
                if let Err(error) = opened {
                    rollback_failed_unix_open(&session, before_open.as_ref());
                    return Err(error);
                }
            }

            // Probe the port up to ~5s; only mark
            // running when we have actual evidence the daemon is alive.
            let port_alive = probe_stream_port(port, 25, Duration::from_millis(200));

            // Capture the daemon's PID. agent-browser writes its PID file
            // synchronously while initialising, so by the time the probe
            // succeeds the file should exist. We try once before and once
            // after to handle either ordering.
            let mut record = track_current_browser_daemon(&session);
            if record.is_none() {
                std::thread::sleep(Duration::from_millis(100));
                record = track_current_browser_daemon(&session);
            }

            if !port_alive {
                return Err(format!("Browser daemon did not open stream port {port}"));
            }
            let Some(record) = record else { return Err(format!("Could not verify ownership of browser session {session}")) };
            if !listener_owned_by_pid(port, record.pid) {
                return Err(format!("Browser stream port {port} is not owned by session {session}"));
            }

            Ok((port_alive, Some(record.pid)))
        })
        .await
        .map_err(|e| format!("agent-browser launch task failed: {e}"))
        .and_then(|result| result);
        let (port_alive, pid) = match launch_result {
            Ok(result) => result,
            Err(error) => {
                // A failed Windows `open` can leave a daemon alive if
                // verified teardown itself fails. Keep that PID attached to
                // the session so close() and process-exit cleanup can retry.
                #[cfg(windows)]
                let lingering = read_agent_browser_daemon_pid(&session_after_launch)
                    .and_then(|pid| owned_browser_pid(&session_after_launch, pid))
                    .filter(|record| browser_process_matches(record, &session_after_launch));
                #[cfg(windows)]
                if let Some(record) = lingering {
                    self.sessions.lock().await.entry(browser_id.to_string()).and_modify(|entry| {
                        entry.running = false;
                        entry.pid = Some(record.pid);
                    });
                    return Err(error);
                }
                // The next attempt must choose a fresh port if another
                // process claimed this allocation before the launch.
                self.clear_failed_stream_allocation(browser_id, port).await;
                return Err(error);
            }
        };

        let now = Some(Instant::now());
        self.sessions
            .lock()
            .await
            .entry(browser_id.to_string())
            .and_modify(|s| {
                s.running = port_alive;
                s.pid = pid;
                s.pid_start_time = None;
                s.stream_generation = uuid::Uuid::new_v4().as_u128();
                s.last_seen_at = now;
            });

        Ok(format!("ws://127.0.0.1:{port}"))
    }

    /// Update the freshness timestamp on a session. Called from the
    /// frontend whenever a frame is decoded so an external reaper task
    /// (or an admin debug command) can tell live sessions from stuck
    /// ones. No-op if the session was already removed.
    #[allow(dead_code)]
    pub async fn note_frame_seen(&self, session_key: &str) {
        let now = Some(Instant::now());
        self.sessions
            .lock()
            .await
            .entry(session_key.to_string())
            .and_modify(|s| s.last_seen_at = now);
    }

    /// Return the session keys currently tracked in the in-memory map.
    /// Used by the periodic orphan sweep (issue #126): the sweep only ever
    /// considers sessions THIS app instance spawned, so it can never kill
    /// a daemon belonging to another codemux instance.
    pub async fn session_keys(&self) -> Vec<String> {
        self.sessions.lock().await.keys().cloned().collect()
    }
}

/// Decide which tracked session keys are orphaned `ws-*` sessions.
/// Only workspace-scoped names (`ws-` prefix) are eligible — user
/// browser-pane sessions (`browser-NNN`) and the `default` session are
/// never swept. A key survives if any live workspace still maps to it.
///
/// Pure and synchronous on purpose: it takes plain data (no lock guards,
/// no I/O) so it's trivially unit-testable and callable from the lib.rs
/// sweep loop without holding either the manager's or the state store's
/// lock while deciding what to close.
pub(crate) fn orphaned_ws_sessions(
    tracked_keys: &[String],
    live_names: &std::collections::HashSet<String>,
) -> Vec<String> {
    tracked_keys
        .iter()
        .filter(|key| key.starts_with("ws-") && !live_names.contains(*key))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn browser_open_passes_session_as_one_argument_for_native_and_npx() {
        let session = "browser-'$(id);x";
        let native = browser_open_command("/tmp/my browser/agent-browser", session, 9223).unwrap();
        assert_eq!(native.get_program(), std::ffi::OsStr::new("/tmp/my browser/agent-browser"));
        let args: Vec<_> = native.get_args().map(|arg| arg.to_string_lossy().into_owned()).collect();
        assert_eq!(args, ["open", "about:blank", "--session", session]);

        let npx = browser_open_command("npx agent-browser", session, 9223).unwrap();
        assert_eq!(npx.get_program(), std::ffi::OsStr::new("npx"));
        let args: Vec<_> = npx.get_args().map(|arg| arg.to_string_lossy().into_owned()).collect();
        assert_eq!(args, ["agent-browser", "open", "about:blank", "--session", session]);
    }

    struct TestBrowserPidFile(PathBuf);

    impl TestBrowserPidFile {
        fn unowned(session: &str) -> Self {
            let dir = run_dir_for_session(session).unwrap();
            let path = dir.join(format!("{session}.pid"));
            std::fs::write(&path, std::process::id().to_string()).unwrap();
            Self(path)
        }
    }

    impl Drop for TestBrowserPidFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[tokio::test]
    async fn failed_first_command_does_not_make_explicit_spawn_succeed() {
        let session = format!("browser-failed-spawn-{}", uuid::Uuid::new_v4().simple());
        let _pid_file = TestBrowserPidFile::unowned(&session);
        let mgr = AgentBrowserManager::new();
        assert!(mgr.run_command(&session, "viewport", serde_json::json!({})).await.is_err());
        assert!(!mgr.sessions.lock().await.get(&session).unwrap().running);
        let spawn = mgr.spawn(&session).await;
        assert!(spawn.as_ref().is_err_and(|error| error.contains("already used by another process")), "{spawn:?}");
    }

    #[tokio::test]
    async fn stream_start_and_command_wait_for_the_same_session_lifecycle() {
        let session = format!("browser-overlap-{}", uuid::Uuid::new_v4().simple());
        let _pid_file = TestBrowserPidFile::unowned(&session);
        let mgr = Arc::new(AgentBrowserManager::new());
        let held = browser_lifecycle_gate(&session).lock_owned().await;
        let stream_mgr = mgr.clone();
        let stream_session = session.clone();
        let mut stream = tokio::spawn(async move { stream_mgr.start_stream(&stream_session).await });
        let command_mgr = mgr.clone();
        let command_session = session.clone();
        let mut command = tokio::spawn(async move {
            command_mgr.run_command(&command_session, "viewport", serde_json::json!({})).await
        });
        assert!(tokio::time::timeout(Duration::from_millis(50), &mut stream).await.is_err());
        assert!(tokio::time::timeout(Duration::from_millis(50), &mut command).await.is_err());
        drop(held);
        assert!(tokio::time::timeout(Duration::from_secs(10), stream).await.unwrap().unwrap().is_err());
        assert!(tokio::time::timeout(Duration::from_secs(10), command).await.unwrap().unwrap().is_err());
    }

    #[tokio::test]
    async fn cancelled_caller_does_not_release_a_running_lifecycle_worker() {
        let session = format!("browser-cancel-{}", uuid::Uuid::new_v4().simple());
        let (started, started_rx) = std::sync::mpsc::channel();
        let (release, release_rx) = std::sync::mpsc::channel();
        let caller = tokio::spawn(run_detached_browser_lifecycle(session.clone(), async move {
            tokio::task::spawn_blocking(move || {
                started.send(()).unwrap();
                release_rx.recv().unwrap();
            })
            .await
            .unwrap();
            Ok(())
        }));
        tokio::task::spawn_blocking(move || started_rx.recv_timeout(Duration::from_secs(5)))
            .await
            .unwrap()
            .unwrap();
        caller.abort();
        let gate = browser_lifecycle_gate(&session);
        assert!(tokio::time::timeout(Duration::from_millis(50), gate.clone().lock_owned()).await.is_err());
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), gate.lock_owned()).await.unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn failed_open_never_rolls_back_a_foreign_or_preexisting_daemon() {
        let own = BrowserPidRecord {
            pid: 10,
            start_time: 100,
            token: browser_owner_token().to_owned(),
        };
        let foreign = BrowserPidRecord {
            token: uuid::Uuid::new_v4().simple().to_string(),
            ..own.clone()
        };
        assert!(!failed_open_daemon_started_here(&foreign, None));
        assert!(!failed_open_daemon_started_here(&own, Some(&own)));
        assert!(failed_open_daemon_started_here(&own, None));
        let new_own = BrowserPidRecord { pid: 11, ..own.clone() };
        assert!(failed_open_daemon_started_here(&new_own, Some(&own)));
    }

    #[test]
    fn protected_stream_requires_a_versioned_browser_daemon() {
        let temp = tempfile::tempdir().unwrap();
        let version_file = temp.path().join("session.version");
        assert!(!browser_daemon_has_protected_stream("session", temp.path()));
        std::fs::write(&version_file, "0.24.0").unwrap();
        assert!(!browser_daemon_has_protected_stream("session", temp.path()));
        std::fs::write(&version_file, "0.44.0").unwrap();
        assert!(browser_daemon_has_protected_stream("session", temp.path()));
        std::fs::write(&version_file, "invalid").unwrap();
        assert!(!browser_daemon_has_protected_stream("session", temp.path()));
    }

    #[test]
    fn stream_listener_must_belong_to_expected_pid() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(listener_owned_by_pid(port, std::process::id()));
        assert!(!listener_owned_by_pid(port, u32::MAX));
    }

    #[cfg(windows)]
    #[test]
    fn browser_start_lock_obeys_caller_timeout() {
        let guard = BROWSER_DAEMON_START_LOCK.lock().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = ensure_owned_browser_daemon("unused.exe", "browser-lock-timeout", 0, Duration::from_millis(30))
                .map(|_| ())
                .map_err(|error| error.kind());
            let _ = sender.send(result);
        });
        let result = receiver.recv_timeout(Duration::from_secs(5));
        drop(guard);
        worker.join().unwrap();
        assert_eq!(result.unwrap(), Err(std::io::ErrorKind::TimedOut));
    }

    #[cfg(windows)]
    #[test]
    fn browser_relay_port_is_exclusive() {
        let listener = bind_exclusive_browser_relay_listener(0).unwrap();
        let address = listener.local_addr().unwrap();
        let contender = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, Some(socket2::Protocol::TCP)).unwrap();
        contender.set_reuse_address(true).unwrap();
        assert!(contender.bind(&socket2::SockAddr::from(address)).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn browser_stream_rejects_failed_open_with_owned_listener() {
        use std::os::windows::process::ExitStatusExt;

        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(listener_owned_by_pid(port, std::process::id()));

        let output = std::process::Output {
            status: std::process::ExitStatus::from_raw(1),
            stdout: Vec::new(),
            stderr: b"open failed".to_vec(),
        };
        assert!(require_successful_browser_stream_open(Ok(output), None).is_err());
        assert!(require_successful_browser_stream_open(Err(std::io::Error::other("relay failed")), None).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn failed_browser_stream_open_stops_its_owned_daemon() {
        use std::os::windows::process::ExitStatusExt;

        struct ChildGuard(std::process::Child);
        impl Drop for ChildGuard {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        let temp = tempfile::tempdir().unwrap();
        let binary = temp.path().join("agent-browser.exe");
        std::fs::copy(which::which("ping").unwrap(), &binary).unwrap();
        let session = format!("browser-failed-open-{}", uuid::Uuid::new_v4().simple());
        let token = browser_owner_token().to_owned();
        let child = std::process::Command::new(&binary)
            .args(["-t", "127.0.0.1"])
            .env("CODEMUX_BROWSER_OWNER", &token)
            .env("CODEMUX_BROWSER_SESSION", &session)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let mut child = ChildGuard(child);
        let pid = child.0.id();
        let started_waiting = Instant::now();
        let start_time = loop {
            if let Some(start) = browser_process_start(pid, &session, &token) {
                break start;
            }
            assert!(started_waiting.elapsed() < Duration::from_secs(3));
            std::thread::sleep(Duration::from_millis(25));
        };
        let record = BrowserPidRecord { pid, start_time, token };
        let dir = run_dir_for_session(&session).unwrap();
        let pid_path = dir.join(format!("{session}.pid"));
        let own_record_path = session_pid_path(&session).unwrap();
        std::fs::write(&pid_path, pid.to_string()).unwrap();
        write_session_pid(&session, &record).unwrap();

        let output = std::process::Output {
            status: std::process::ExitStatus::from_raw(1),
            stdout: Vec::new(),
            stderr: b"open failed".to_vec(),
        };
        assert!(finish_browser_stream_open(Ok(output), None, &session, "unused.exe", &record, true).is_err());
        let stopped_waiting = Instant::now();
        while child.0.try_wait().unwrap().is_none() {
            assert!(stopped_waiting.elapsed() < Duration::from_secs(3), "failed open left an owned daemon running");
            std::thread::sleep(Duration::from_millis(25));
        }
        let _ = std::fs::remove_file(pid_path);
        let _ = std::fs::remove_file(own_record_path);
    }

    #[cfg(windows)]
    #[test]
    fn failed_first_browser_command_stops_a_tracked_daemon() {
        struct ChildGuard(std::process::Child);
        impl Drop for ChildGuard {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }

        let temp = tempfile::tempdir().unwrap();
        let binary = temp.path().join("agent-browser.exe");
        std::fs::copy(which::which("ping").unwrap(), &binary).unwrap();
        let session = format!("browser-first-command-{}", uuid::Uuid::new_v4().simple());
        let token = browser_owner_token().to_owned();
        let child = std::process::Command::new(&binary)
            .args(["-t", "127.0.0.1"])
            .env("CODEMUX_BROWSER_OWNER", &token)
            .env("CODEMUX_BROWSER_SESSION", &session)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let mut child = ChildGuard(child);
        let pid = child.0.id();
        let started_waiting = Instant::now();
        let start_time = loop {
            if let Some(start) = browser_process_start(pid, &session, &token) {
                break start;
            }
            assert!(started_waiting.elapsed() < Duration::from_secs(3));
            std::thread::sleep(Duration::from_millis(25));
        };
        let record = BrowserPidRecord { pid, start_time, token };
        let dir = run_dir_for_session(&session).unwrap();
        let pid_path = dir.join(format!("{session}.pid"));
        let own_record_path = session_pid_path(&session).unwrap();
        std::fs::write(&pid_path, pid.to_string()).unwrap();
        track_browser_daemon_before_command(&session, "unused.exe", &record, true).unwrap();
        assert_eq!(read_browser_pid_record(&own_record_path).unwrap().pid, pid);

        let failed = finish_browser_command(
            Err(std::io::Error::other("relay failed")),
            &session,
            "unused.exe",
            &record,
            true,
        );
        assert!(failed.is_err());
        let stopped_waiting = Instant::now();
        while child.0.try_wait().unwrap().is_none() {
            assert!(stopped_waiting.elapsed() < Duration::from_secs(3), "failed command left its daemon running");
            std::thread::sleep(Duration::from_millis(25));
        }
        let _ = std::fs::remove_file(pid_path);
        let _ = std::fs::remove_file(own_record_path);
    }

    #[cfg(windows)]
    #[test]
    fn control_listener_must_belong_to_browser_daemon() {
        let temp = tempfile::tempdir().unwrap();
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        std::fs::write(temp.path().join("owned.port"), port.to_string()).unwrap();
        assert!(browser_control_listener_owned_by_pid("owned", temp.path(), std::process::id()));
        assert!(!browser_control_listener_owned_by_pid("owned", temp.path(), u32::MAX));

        std::fs::write(temp.path().join("owned.port"), "invalid").unwrap();
        assert!(!browser_control_listener_owned_by_pid("owned", temp.path(), std::process::id()));
        assert_eq!(browser_control_port("default", temp.path()), None);
        std::fs::write(temp.path().join("default.port"), "invalid").unwrap();
        assert_eq!(browser_control_port("default", temp.path()), None);
    }

    #[tokio::test]
    async fn unrelated_listener_in_browser_range_is_not_proxy_authorized() {
        let manager = AgentBrowserManager::new();
        let port = manager.allocate_port("unowned-listener-test").await.unwrap();
        let _listener = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
        assert!(!manager.owns_stream_port(port).await);
    }

    #[cfg(windows)]
    #[test]
    fn shutdown_does_not_send_close_to_a_stale_control_listener() {
        struct ChildGuard(std::process::Child);
        impl Drop for ChildGuard {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        struct RuntimeFiles([std::path::PathBuf; 2]);
        impl Drop for RuntimeFiles {
            fn drop(&mut self) {
                for path in &self.0 {
                    let _ = std::fs::remove_file(path);
                }
            }
        }

        let temp = tempfile::tempdir().unwrap();
        let binary = temp.path().join("agent-browser.exe");
        std::fs::copy(which::which("ping").unwrap(), &binary).unwrap();
        let session = format!("browser-stop-{}", uuid::Uuid::new_v4().simple());
        let token = browser_owner_token().to_owned();
        let child = std::process::Command::new(&binary)
            .args(["-t", "127.0.0.1"])
            .env("CODEMUX_BROWSER_OWNER", &token)
            .env("CODEMUX_BROWSER_SESSION", &session)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let mut child = ChildGuard(child);
        let pid = child.0.id();
        let started_waiting = Instant::now();
        let start_time = loop {
            if let Some(start) = browser_process_start(pid, &session, &token) {
                break start;
            }
            if started_waiting.elapsed() >= Duration::from_secs(3) {
                panic!("browser identity unavailable: pid={pid}, child_status={:?}", child.0.try_wait().unwrap());
            }
            std::thread::sleep(Duration::from_millis(25));
        };
        let record = BrowserPidRecord { pid, start_time, token };

        let dir = run_dir().unwrap();
        let pid_path = dir.join(format!("{session}.pid"));
        let port_path = dir.join(format!("{session}.port"));
        let _runtime_files = RuntimeFiles([pid_path.clone(), port_path.clone()]);
        std::fs::write(&pid_path, pid.to_string()).unwrap();
        let unrelated = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = unrelated.local_addr().unwrap().port();
        std::fs::write(&port_path, port.to_string()).unwrap();
        assert!(!browser_control_listener_owned_by_pid(&session, &dir, pid));
        assert!(connect_verified_browser_control(&session, &record).is_err());

        let marker = temp.path().join("close-called");
        let fake_cli = temp.path().join("close-marker.cmd");
        std::fs::write(&fake_cli, "@echo off\r\necho called>\"%~dp0close-called\"\r\n").unwrap();
        let bin = fake_cli.to_str().unwrap();
        let close_args = ["close".into(), "--session".into(), session.clone()];
        let mut fake_command = std::process::Command::new(bin);
        fake_command.args(&close_args);
        set_browser_owner_env(&mut fake_command, &session).unwrap();
        assert!(launch_status_with_timeout(fake_command, Duration::from_secs(2)).unwrap().success());
        assert!(marker.exists(), "test CLI must reveal a close invocation");
        std::fs::remove_file(&marker).unwrap();

        stop_owned_browser_daemon(&session, &record, bin);
        assert!(!marker.exists(), "shutdown must not invoke the control CLI");
        assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_ok());
        let stopped_waiting = Instant::now();
        while child.0.try_wait().unwrap().is_none() {
            assert!(stopped_waiting.elapsed() < Duration::from_secs(3), "verified daemon must stop");
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    #[cfg(windows)]
    #[test]
    fn identifies_the_peer_of_an_established_control_connection() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let script = format!(
            "$client = [Net.Sockets.TcpClient]::new('127.0.0.1', {port}); Start-Sleep -Seconds 10"
        );
        let mut child = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let child_pid = child.id();
        listener.set_nonblocking(true).unwrap();
        let started = Instant::now();
        let server = loop {
            match listener.accept() {
                Ok((server, _)) => break server,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if started.elapsed() >= Duration::from_secs(5) {
                        let status = child.try_wait().unwrap();
                        let _ = child.kill();
                        let _ = child.wait();
                        panic!("PowerShell TCP fixture did not connect: {status:?}");
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(error) => panic!("TCP fixture accept failed: {error}"),
            }
        };
        let peer_pid = connected_tcp_peer_pid(&server);
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(peer_pid, Some(child_pid));
    }

    #[cfg(unix)]
    #[test]
    fn live_control_socket_without_pid_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let session = "missing-pid-control-test";
        let socket = temp.path().join(format!("{session}.sock"));
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        assert!(require_owned_browser_session_in(session, temp.path()).is_err());
        drop(listener);
        assert!(require_owned_browser_session_in(session, temp.path()).is_ok());
    }

    // ── External-process timeout guard (issue #96) ───────────────────
    //
    // These exercise the hard-timeout wrappers with OS built-ins
    // (`sh`/`cmd`/`sleep`/`ping`) so they stay hermetic — no agent-browser
    // binary required. The whole point of the feature is that a wedged
    // child is killed, so a broken implementation fails fast (or trips the
    // assertion), it does not hang the suite.

    #[test]
    fn action_timeout_open_and_wait_get_long_ceiling() {
        assert_eq!(action_timeout("open"), OPEN_TIMEOUT);
        assert_eq!(action_timeout("open_url"), OPEN_TIMEOUT);
        assert_eq!(action_timeout("wait"), OPEN_TIMEOUT);
        // Snappy already-connected DOM actions get the shorter ceiling.
        assert_eq!(action_timeout("click"), ACTION_TIMEOUT);
        assert_eq!(action_timeout("screenshot"), ACTION_TIMEOUT);
        assert_eq!(action_timeout("snapshot"), ACTION_TIMEOUT);
        assert_eq!(action_timeout("eval"), ACTION_TIMEOUT);
    }

    #[cfg(unix)]
    #[test]
    fn output_capture_with_timeout_collects_stdout_on_fast_exit() {
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", "printf hello"]);
        let out = output_capture_with_timeout(cmd, Duration::from_secs(5))
            .expect("fast command should complete");
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout), "hello");
    }

    #[cfg(unix)]
    #[test]
    fn output_capture_with_timeout_kills_wedged_child_fast() {
        // A 30s sleeper bounded to 300ms must return promptly with
        // TimedOut — proving the child was killed, not waited out.
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", "sleep 30"]);
        let start = Instant::now();
        let result = output_capture_with_timeout(cmd, Duration::from_millis(300));
        let elapsed = start.elapsed();
        let err = result.expect_err("a 30s sleep must trip the 300ms timeout");
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        assert!(
            elapsed < Duration::from_secs(5),
            "timed-out child should be killed quickly, took {elapsed:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn timeout_kills_whole_process_group_not_just_the_shell() {
        // `sh -c 'sleep 30; true'` forces sh to FORK `sleep` as a
        // grandchild rather than exec it in place. A naive `child.kill()`
        // on the shell would orphan that sleep; the process-group SIGKILL
        // must reap it too.
        use std::os::unix::process::CommandExt;
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", "sleep 30; true"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        cmd.process_group(0);
        let mut child = cmd.spawn().expect("spawn sh");
        let pgid = child.id() as i32;

        let result = wait_with_timeout(&mut child, Duration::from_millis(300));
        assert!(result.is_err(), "wedged child should time out");

        // Poll until the process group is empty (init reaps the SIGKILL'd
        // grandchild). `kill(-pgid, 0)` returns 0 while any member is
        // alive, -1/ESRCH once the group is gone.
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut group_gone = false;
        while Instant::now() < deadline {
            let alive = unsafe { libc::kill(-pgid, 0) } == 0;
            if !alive {
                group_gone = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            group_gone,
            "process group {pgid} still has live members — grandchild sleep was not killed"
        );
    }

    #[cfg(unix)]
    #[test]
    fn launch_status_with_timeout_returns_status_on_fast_exit() {
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", "exit 0"]);
        let status = launch_status_with_timeout(cmd, Duration::from_secs(5))
            .expect("fast launch should complete");
        assert!(status.success());
    }

    #[cfg(unix)]
    #[test]
    fn failed_unix_open_is_rejected_even_with_a_live_stream_port() {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(probe_stream_port(port, 1, Duration::from_millis(0)));

        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", "exit 7"]);
        let result = launch_status_with_timeout(cmd, Duration::from_secs(5));
        assert!(require_successful_browser_stream_open_status(result).is_err());
        assert!(require_successful_browser_stream_open_status(Err(
            std::io::Error::new(std::io::ErrorKind::TimedOut, "open timed out")
        )).is_err());
        assert!(probe_stream_port(port, 1, Duration::from_millis(0)));
    }

    #[cfg(unix)]
    #[test]
    fn launch_status_with_timeout_kills_wedged_launch() {
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", "sleep 30"]);
        let start = Instant::now();
        let err = launch_status_with_timeout(cmd, Duration::from_millis(300))
            .expect_err("wedged launch must time out");
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[cfg(windows)]
    #[test]
    fn output_capture_with_timeout_collects_stdout_on_fast_exit_windows() {
        let mut cmd = std::process::Command::new("cmd");
        cmd.args(["/C", "echo hello"]);
        let out = output_capture_with_timeout(cmd, Duration::from_secs(5))
            .expect("fast command should complete");
        assert!(out.status.success());
        assert!(String::from_utf8_lossy(&out.stdout).contains("hello"));
    }

    #[cfg(windows)]
    #[test]
    fn output_capture_with_timeout_kills_wedged_child_fast_windows() {
        // `ping -n 30 127.0.0.1` sleeps ~29s; bound it to 500ms. This is
        // the exact failure shape that hung `cargo test` on the Windows CI
        // image before the timeout guard.
        let mut cmd = std::process::Command::new("cmd");
        cmd.args(["/C", "ping -n 30 127.0.0.1"]);
        let start = Instant::now();
        let err = output_capture_with_timeout(cmd, Duration::from_millis(500))
            .expect_err("a ~29s ping must trip the 500ms timeout");
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        assert!(start.elapsed() < Duration::from_secs(8));
    }

    #[test]
    fn resolver_fails_closed_without_a_secure_native_binary() {
        assert!(resolve_verified_candidate(&[]).is_err());
        if let Ok(path) = resolve_binary() {
            assert!(std::path::Path::new(&path).is_file());
        }
    }

    #[test]
    fn resolver_candidates_ignore_an_untrusted_working_directory() {
        let temp = tempfile::tempdir().unwrap();
        let untrusted = temp.path().join("workspace");
        let trusted = temp.path().join("app");
        let dev_root = temp.path().join("source");
        std::fs::create_dir_all(&untrusted).unwrap();
        let candidates = native_binary_candidates(
            Some(&trusted),
            Some(&dev_root),
            "agent-browser",
            "node_modules/@agent-browser/cli-linux-x64-gnu/bin/agent-browser",
        );
        assert!(candidates.iter().all(|path| !path.starts_with(&untrusted)));
        assert_eq!(candidates[0], trusted.join("agent-browser"));
        assert!(candidates[1].starts_with(&dev_root));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let planted = untrusted.join("node_modules/@agent-browser/cli-linux-x64-gnu/bin/agent-browser");
            std::fs::create_dir_all(planted.parent().unwrap()).unwrap();
            let marker = temp.path().join("executed");
            std::fs::write(&planted, format!("#!/bin/sh\ntouch '{}'\necho 'agent-browser 0.44.0'\n", marker.display())).unwrap();
            std::fs::set_permissions(&planted, std::fs::Permissions::from_mode(0o700)).unwrap();
            assert!(resolve_verified_candidate(&candidates).is_err());
            assert!(!marker.exists(), "untrusted cwd binary must never execute");
        }
    }

    #[cfg(unix)]
    #[test]
    fn resolver_finds_release_style_bundled_binary_without_a_dev_fallback() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let app_dir = temp.path().join("app");
        std::fs::create_dir(&app_dir).unwrap();
        let binary = app_dir.join("agent-browser");
        std::fs::write(&binary, b"#!/bin/sh\necho 'agent-browser 0.44.0'\n").unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(bundled_browser_binary_name(), "agent-browser");
        let candidates = native_binary_candidates(
            Some(&app_dir),
            None,
            bundled_browser_binary_name(),
            "node_modules/unavailable/agent-browser",
        );
        assert_eq!(candidates, vec![binary.clone()]);
        assert_eq!(resolve_verified_candidate(&candidates).unwrap(), binary.to_string_lossy());
    }

    #[cfg(unix)]
    #[test]
    fn resolver_rejects_old_native_binary_and_accepts_secure_version() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let binary = temp.path().join("agent-browser");
        std::fs::write(&binary, b"#!/bin/sh\necho 'agent-browser 0.24.0'\n").unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(resolve_verified_candidate(&[binary.clone()]).is_err());

        std::fs::write(&binary, b"#!/bin/sh\necho 'agent-browser 0.44.0'\n").unwrap();
        assert_eq!(resolve_verified_candidate(&[binary.clone()]).unwrap(), binary.to_string_lossy());
    }

    #[test]
    fn verified_replacement_rotates_stale_stream_capability() {
        let mut stream = StreamSession {
            port: 9223,
            running: true,
            pid: Some(41),
            pid_start_time: Some(100),
            stream_generation: 1,
            last_seen_at: None,
        };
        let replacement = BrowserPidRecord {
            pid: 42,
            start_time: 200,
            token: "a".repeat(32),
        };
        let identity = refresh_stream_session_identity(&mut stream, &replacement);
        assert_eq!(identity.pid, 42);
        assert_eq!(identity.start_time, 200);
        assert_ne!(identity.generation, 1);
        assert_eq!(refresh_stream_session_identity(&mut stream, &replacement), identity);

        // A replacement may reuse the same PID and port; its process start
        // still invalidates the old capability.
        let reused_pid = BrowserPidRecord {
            start_time: 201,
            ..replacement
        };
        let reused_identity = refresh_stream_session_identity(&mut stream, &reused_pid);
        assert_eq!(reused_identity.pid, identity.pid);
        assert_ne!(reused_identity.generation, identity.generation);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn build_command_open_uses_navigation_load_wait_only() {
        let cmd = build_agent_browser_command("agent-browser", "test-session", "open", &serde_json::json!({"url": "https://example.com"})).unwrap();
        assert!(cmd.starts_with("'agent-browser'"), "Command should start with the selected binary: {}", cmd);
        assert!(cmd.contains("--session 'test-session'"));
        assert!(cmd.contains("https://example.com"));
        assert!(!cmd.contains("wait --load"), "open already waits for load: {}", cmd);
        assert!(!cmd.contains("stream disable"), "Should NOT restart stream: {}", cmd);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn browser_command_treats_session_metacharacters_as_one_argument() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("injected");
        let session = format!("browser; touch {} #", marker.display());
        let cmd = build_agent_browser_command("agent-browser", &session, "screenshot", &serde_json::json!({})).unwrap();
        let session_arg = cmd.split_once("--session ").unwrap().1;
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("set -- {session_arg}; printf '%s' \"$1\""))
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout).unwrap(), session);
        assert!(!marker.exists());
    }

    #[tokio::test]
    async fn browser_session_names_cannot_escape_runtime_directory() {
        let manager = AgentBrowserManager::new();
        assert!(manager.allocate_port("../outside").await.is_err());
        assert!(manager.close("../outside").await.is_err());
        assert!(manager.start_stream("../outside").await.is_err());
        assert!(session_pid_path("../outside").is_none());
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn build_command_viewport_uses_set_viewport() {
        let cmd = build_agent_browser_command("agent-browser", "s", "viewport", &serde_json::json!({"width": 800, "height": 600})).unwrap();
        assert!(cmd.contains("set viewport 800 600"), "v0.24.0 uses 'set viewport', got: {}", cmd);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn build_command_viewport_omits_dpr_when_one() {
        // Backwards-compat with the pre-preset call sites (BrowserPane's
        // ResizeObserver, the old socket clients): a plain {width, height}
        // payload must produce the same exact `set viewport W H` shell
        // string with no trailing scale arg.
        let cmd = build_agent_browser_command(
            "agent-browser",
            "s",
            "viewport",
            &serde_json::json!({"width": 800, "height": 600, "scale": 1.0}),
        )
        .unwrap();
        assert!(
            cmd.contains("set viewport 800 600 --session"),
            "scale=1.0 should be omitted to keep argv tight: {}",
            cmd
        );
        assert!(!cmd.contains(" 1 --session"), "no stray '1' DPR arg: {}", cmd);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn build_command_viewport_emits_dpr_when_retina() {
        // Mobile presets always pass scale=2 or scale=3 — that must
        // surface as a 3rd positional arg so agent-browser actually
        // applies the retina factor (not just resizes the box).
        let cmd = build_agent_browser_command(
            "agent-browser",
            "s",
            "viewport",
            &serde_json::json!({"width": 390, "height": 844, "scale": 3.0}),
        )
        .unwrap();
        assert!(
            cmd.contains("set viewport 390 844 3 --session"),
            "DPR=3 must be 3rd positional arg: {}",
            cmd
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn build_command_viewport_resolves_preset_name() {
        // Preset-name path (CLI / MCP path) must resolve through
        // browser_viewport::parse_spec and produce the right argv —
        // catches a regression where someone forgets to wire preset
        // resolution into the legacy width/height path.
        let cmd = build_agent_browser_command(
            "agent-browser",
            "s",
            "viewport",
            &serde_json::json!({"preset": "mobile"}),
        )
        .unwrap();
        // mobile = 390x844 @ DPR 3
        assert!(
            cmd.contains("set viewport 390 844 3 --session"),
            "'mobile' preset → 390x844x3: {}",
            cmd
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn build_command_viewport_preset_unknown_falls_back() {
        // An unknown preset must fall back to width/height defaults
        // (1280×720) rather than fail mid-stream. The CLI / MCP layer is
        // expected to validate up front; this is the last-line defence.
        let cmd = build_agent_browser_command(
            "agent-browser",
            "s",
            "viewport",
            &serde_json::json!({"preset": "iphone-99-pro-max-ultra"}),
        )
        .unwrap();
        assert!(
            cmd.contains("set viewport 1280 720"),
            "unknown preset should fall back to defaults: {}",
            cmd
        );
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn build_command_viewport_preset_reset() {
        // "reset" preset = RESET_SPEC = 1280x800 @ DPR 1.0; DPR 1.0 must
        // be omitted from argv (same backwards-compat rule).
        let cmd = build_agent_browser_command(
            "agent-browser",
            "s",
            "viewport",
            &serde_json::json!({"preset": "reset"}),
        )
        .unwrap();
        assert!(
            cmd.contains("set viewport 1280 800 --session"),
            "'reset' preset → 1280x800 with no scale: {}",
            cmd
        );
    }

    #[test]
    fn format_dpr_integer_round_trips() {
        // DPRs are floats internally but the CLI argv looks cleaner with
        // integer DPRs as integers (no trailing ".0").
        assert_eq!(format_dpr(1.0), "1");
        assert_eq!(format_dpr(2.0), "2");
        assert_eq!(format_dpr(3.0), "3");
    }

    #[test]
    fn format_dpr_preserves_fractional() {
        // Odd values (1.5x, 2.75x — yes, some Android phones) must
        // round-trip to two decimals so we don't silently truncate to 1x.
        // Note: Rust's `{:.2}` uses banker's rounding (round half to even),
        // so 2.625 actually rounds to 2.62 — pick values that don't hit
        // the half-to-even ambiguity to keep this test stable across
        // platforms and rustc versions.
        assert_eq!(format_dpr(1.5), "1.50");
        assert_eq!(format_dpr(2.75), "2.75");
        assert_eq!(format_dpr(2.626), "2.63");
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn build_command_unknown_action_returns_error() {
        let result = build_agent_browser_command("agent-browser", "s", "nonexistent_action", &serde_json::json!({}));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unknown action"));
    }

    /// Windows-only sibling test that exercises the argv builder. The Linux
    /// tests above assert shell-string shape; the argv form has different
    /// semantics (no shell quoting, separate args), so this test asserts the
    /// argv shape directly instead of trying to share assertions.
    #[cfg(target_os = "windows")]
    #[test]
    fn build_argv_open_uses_navigation_load_wait_only() {
        let groups = build_agent_browser_argv_groups(
            "test-session",
            "open",
            &serde_json::json!({"url": "https://example.com"}),
        )
        .unwrap();
        assert_eq!(groups.len(), 1, "open already waits for load");

        // `windows_executable_path_args()` may prepend a global
        // `--executable-path <path>` pair when Chrome / Brave / Edge is
        // detected. The GitHub `windows-latest` runner ships Edge, so on
        // CI this is always the case; on a developer box without any
        // supported browser installed it's empty. The contract is: any
        // global flags must precede the `open` subcommand, otherwise clap
        // rejects them and agent-browser exits silently (the runtime bug
        // the argv-position fix originally addressed). Assert both that
        // `open` is present and that any `--executable-path` precedes it.
        let argv = &groups[0];
        let open_pos = argv
            .iter()
            .position(|a| a == "open")
            .expect("open subcommand must appear in argv");
        if let Some(exec_pos) = argv.iter().position(|a| a == "--executable-path") {
            assert!(
                exec_pos < open_pos,
                "--executable-path must precede the `open` subcommand (clap requires global flags first); argv: {:?}",
                argv
            );
        }
        assert!(argv.contains(&"https://example.com".to_string()));
        assert!(argv.contains(&"--session".to_string()));
        assert!(argv.contains(&"test-session".to_string()));
        assert!(!argv.contains(&"wait".to_string()));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn build_argv_viewport_uses_set_viewport() {
        let groups = build_agent_browser_argv_groups(
            "s",
            "viewport",
            &serde_json::json!({"width": 800, "height": 600}),
        )
        .unwrap();
        assert_eq!(groups.len(), 1);
        let argv = &groups[0];
        assert_eq!(argv[0], "set");
        assert_eq!(argv[1], "viewport");
        assert_eq!(argv[2], "800");
        assert_eq!(argv[3], "600");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn build_argv_unknown_action_returns_error() {
        let result =
            build_agent_browser_argv_groups("s", "nonexistent_action", &serde_json::json!({}));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Unknown action"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn split_resolved_binary_handles_npx_fallback() {
        let (program, prefix) = split_resolved_binary("npx agent-browser");
        assert_eq!(program, "npx");
        assert_eq!(prefix, vec!["agent-browser".to_string()]);
        assert!(native_browser_program("npx.cmd agent-browser").is_err());
        assert!(native_browser_program(r"C:\Tools\agent-browser.cmd").is_err());
        assert_eq!(native_browser_program(r"C:\Tools\agent-browser.exe").unwrap(), r"C:\Tools\agent-browser.exe");
    }

    #[cfg(windows)]
    #[test]
    fn relay_rewrites_only_the_cli_transport_session() {
        let args = ["eval".into(), "--session".into(), "real".into(), "--session".into(), "real".into()];
        let rewritten = relay_cli_argv(&args, "real", "relay-private").unwrap();
        assert_eq!(rewritten[2], "real");
        assert_eq!(rewritten[4], "relay-private");
        assert!(relay_cli_argv(&["open".into()], "real", "relay-private").is_err());
    }

    #[cfg(windows)]
    #[test]
    fn relay_metadata_is_private_and_removed_after_use() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("relay");
        clack_private_fs::create_dir(&path).unwrap();
        let directory = RelayDirectory {
            port_file: path.join("alias.port"),
            version_file: path.join("alias.version"),
            config_file: path.join("alias.config"),
            lock_file: path.join("alias.lock"),
            path: path.clone(),
        };
        write_private_relay_file(&directory.port_file, b"54321").unwrap();
        write_private_relay_file(&directory.version_file, b"0.44.0").unwrap();
        write_private_relay_file(&directory.config_file, br#"{"debug":false}"#).unwrap();
        write_private_relay_file(&directory.lock_file, b"").unwrap();

        assert_eq!(std::fs::read_to_string(&directory.version_file).unwrap(), "0.44.0");
        let config: serde_json::Value = serde_json::from_slice(&std::fs::read(&directory.config_file).unwrap()).unwrap();
        assert_eq!(config["debug"], false);
        drop(directory);
        assert!(!path.exists(), "relay metadata must be removed after the CLI exits");
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn split_resolved_binary_handles_plain_path() {
        let (program, prefix) =
            split_resolved_binary(r"C:\Users\Some User\node_modules\agent-browser\bin\agent-browser-win32-x64.exe");
        assert_eq!(program, r"C:\Users\Some User\node_modules\agent-browser\bin\agent-browser-win32-x64.exe");
        assert!(prefix.is_empty());

        let (program, prefix) = split_resolved_binary(r"C:\Program Files\nodejs\npx.cmd agent-browser");
        assert_eq!(program, r"C:\Program Files\nodejs\npx.cmd");
        assert_eq!(prefix, vec!["agent-browser".to_string()]);
    }

    #[tokio::test]
    async fn two_sessions_get_different_ports() {
        let mgr = AgentBrowserManager::new();
        let p1 = mgr.allocate_port("workspace-a").await.unwrap();
        let p2 = mgr.allocate_port("workspace-b").await.unwrap();
        assert_ne!(p1, p2, "Two workspaces must get different ports");
        assert!(p1 >= DEFAULT_STREAM_PORT && p1 <= MAX_STREAM_PORT);
        assert!(p2 >= DEFAULT_STREAM_PORT && p2 <= MAX_STREAM_PORT);
    }

    #[tokio::test]
    async fn allocate_port_is_idempotent() {
        let mgr = AgentBrowserManager::new();
        let p1 = mgr.allocate_port("workspace-x").await.unwrap();
        let p2 = mgr.allocate_port("workspace-x").await.unwrap();
        assert_eq!(p1, p2, "Same session key must return same port");
    }

    #[tokio::test]
    async fn failed_stream_launch_releases_taken_port_for_retry() {
        let mgr = AgentBrowserManager::new();
        let session = format!("browser-port-race-{}", uuid::Uuid::new_v4().simple());
        let mut blocked = None;
        for _ in 0..5 {
            let port = mgr.allocate_port(&session).await.unwrap();
            if let Ok(listener) = std::net::TcpListener::bind(("127.0.0.1", port)) {
                blocked = Some((port, listener));
                break;
            }
            mgr.clear_failed_stream_allocation(&session, port).await;
        }
        let (port, blocker) = blocked.expect("could not reserve an allocated stream port");
        let error = mgr.start_stream(&session).await.unwrap_err();
        assert!(error.contains("was taken by another process"), "{error}");
        assert!(mgr.get_port(&session).await.is_none());
        let retry_port = mgr.allocate_port(&session).await.unwrap();
        assert_ne!(retry_port, port);
        drop(blocker);
    }

    #[tokio::test]
    async fn stale_running_stream_allocation_retries_on_a_fresh_port() {
        let mgr = AgentBrowserManager::new();
        let session = format!("browser-stale-running-{}", uuid::Uuid::new_v4().simple());
        let mut blocked = None;
        for _ in 0..5 {
            let port = mgr.allocate_port(&session).await.unwrap();
            if let Ok(listener) = std::net::TcpListener::bind(("127.0.0.1", port)) {
                blocked = Some((port, listener));
                break;
            }
            mgr.clear_failed_stream_allocation(&session, port).await;
        }
        let (port, blocker) = blocked.expect("could not reserve an allocated stream port");
        mgr.sessions.lock().await.get_mut(&session).unwrap().running = true;

        let error = mgr.start_stream(&session).await.unwrap_err();
        assert!(error.contains("Could not verify ownership"), "{error}");
        assert!(mgr.get_port(&session).await.is_none());
        assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_ok());

        let retry_port = mgr.allocate_port(&session).await.unwrap();
        assert_ne!(retry_port, port);
        drop(blocker);
    }

    /// Regression test for the open→close→open gap: the agent-facing
    /// "close" runs through run_command (not close()), so the session entry
    /// used to keep running = true after a close. The second open then
    /// skipped the fresh-launch default-viewport apply even though it booted
    /// a brand-new daemon. Locks in the state transitions via the helpers
    /// run_command itself calls, without shelling out to agent-browser.
    #[tokio::test]
    async fn close_resets_running_so_reopen_reapplies_default_viewport() {
        let mgr = AgentBrowserManager::new();
        mgr.allocate_port("ws-reopen").await.unwrap();

        // First command on a fresh daemon: not yet running → default
        // viewport would be applied.
        assert!(!mgr.note_action_start("ws-reopen", "open").await);
        // Second command on the running daemon: no re-apply.
        assert!(mgr.note_action_start("ws-reopen", "open").await);
        // Close sees a running daemon and must NOT flip the flag itself.
        assert!(mgr.note_action_start("ws-reopen", "close").await);
        assert!(
            mgr.note_action_start("ws-reopen", "close").await,
            "close must only read the running flag, not modify it",
        );
        // After the close action completes, state resets...
        mgr.note_close_finished("ws-reopen").await;
        {
            let sessions = mgr.sessions.lock().await;
            let s = sessions.get("ws-reopen").expect("entry survives close");
            assert!(!s.running);
            assert!(s.pid.is_none());
        }
        // ...so the next open counts as a fresh daemon again and the
        // default viewport would be re-applied.
        assert!(!mgr.note_action_start("ws-reopen", "open").await);
    }

    #[tokio::test]
    async fn ports_never_collide() {
        let mgr = AgentBrowserManager::new();
        let mut ports = std::collections::HashSet::new();
        // Allocate 10 sessions — all should get unique ports
        for i in 0..10 {
            let port = mgr.allocate_port(&format!("ws-{}", i)).await.unwrap();
            assert!(
                ports.insert(port),
                "Port {} was already assigned to another session",
                port,
            );
        }
    }

    #[tokio::test]
    async fn close_releases_session() {
        let mgr = AgentBrowserManager::new();
        let p = mgr.allocate_port("ws-close-test").await.unwrap();
        assert!(mgr.get_port("ws-close-test").await.is_some());
        let _ = mgr.close("ws-close-test").await;
        assert!(mgr.get_port("ws-close-test").await.is_none(), "Session should be removed after close");
        // Port range still works after release
        let p2 = mgr.allocate_port("ws-close-test-2").await.unwrap();
        assert!(p2 >= DEFAULT_STREAM_PORT && p2 <= MAX_STREAM_PORT);
        assert_ne!(p, p2, "New allocation should skip the (possibly still in-use) released port");
    }

    // ── Browser stream lifecycle regression tests ──
    //
    // These tests target only behavior that lives entirely in this
    // module: pure helpers and the manager's public surface. The
    // daemon-spawning paths (start_stream/spawn) are exercised in the
    // existing manual-smoke flow and intentionally not unit-tested
    // here — they would require staging an agent-browser binary inside
    // CI's sandbox, which is out of scope.

    /// The Windows process-tree fallback must never target PID 0.
    #[cfg(windows)]
    #[test]
    fn kill_pid_zero_is_safe_noop() {
        kill_pid(0);
    }

    /// `pid_alive(0)` returns false. PID 0 is "any process in this
    /// group" on Unix and "Idle Process" on Windows; both should be
    /// treated as "not a tracked daemon".
    #[test]
    fn pid_alive_zero_returns_false() {
        assert!(!pid_alive(0));
    }

    /// `probe_stream_port` returns true when there is a live TCP
    /// listener on the port. We bind a `TcpListener` ourselves so the
    /// test is self-contained and does not depend on agent-browser
    /// running.
    #[test]
    fn probe_stream_port_succeeds_against_live_listener() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        // One attempt is enough — localhost handshake completes well
        // under the 500ms connect timeout baked into the helper.
        assert!(probe_stream_port(port, 1, Duration::from_millis(0)));
        drop(listener);
    }

    /// `probe_stream_port` returns false (not panic, not loop forever)
    /// when no one is listening. Use port 1 — IANA-reserved, root-only
    /// to bind on Unix, and free in CI containers, so we get a
    /// reliable ECONNREFUSED.
    #[test]
    fn probe_stream_port_fails_when_nothing_is_listening() {
        // Probe port 1 (IANA-reserved tcpmux, root-only to bind on Unix
        // and unused in CI containers) for a reliable ECONNREFUSED. The
        // earlier bind-an-ephemeral-port-then-drop-it approach was flaky
        // under parallel test load: a merely-bound-then-dropped listener
        // has no TIME_WAIT, so the OS could immediately hand the freed
        // port to a concurrent test binding `127.0.0.1:0`, making the
        // probe connect and this assertion fail. Port 1 is never bound
        // by any test, so nothing races us.
        assert!(!probe_stream_port(1, 2, Duration::from_millis(10)));
    }

    /// `allocate_port` must reject a port that something else is already
    /// binding. We bind 9223
    /// ourselves before the manager wakes up and assert the manager
    /// hands out 9224+ instead.
    #[tokio::test]
    async fn allocate_port_skips_bound_ports() {
        // Bind the lowest port in the manager's range so the manager
        // is forced to skip past it. If the bind-test regresses to
        // Windows-only, this test fails on Linux/macOS.
        let blocker = std::net::TcpListener::bind(("127.0.0.1", DEFAULT_STREAM_PORT));
        // If 9223 happens to be unavailable on the test host (another
        // codemux instance running), skip the assertion rather than
        // failing — the test is opportunistic.
        let Ok(blocker) = blocker else {
            eprintln!(
                "[test] could not bind {DEFAULT_STREAM_PORT} — skipping allocate_port_skips_bound_ports"
            );
            return;
        };
        let mgr = AgentBrowserManager::new();
        let port = mgr.allocate_port("ws-bind-test").await.unwrap();
        assert_ne!(
            port, DEFAULT_STREAM_PORT,
            "allocate_port handed out a port we are actively binding (bind-test regressed)"
        );
        drop(blocker);
    }

    /// `close()` must remove the session from the in-memory map even
    /// when the daemon was never actually spawned. This is the
    /// regression that fed the leaked-alias bug — `close()` returning
    /// without cleaning the map would let entries pin ports forever.
    #[tokio::test]
    async fn close_removes_session_entry_for_unstarted_session() {
        let mgr = AgentBrowserManager::new();
        let _port = mgr.allocate_port("ws-close-unstarted").await.unwrap();
        assert!(mgr.get_port("ws-close-unstarted").await.is_some());
        let _ = mgr.close("ws-close-unstarted").await;
        assert!(
            mgr.get_port("ws-close-unstarted").await.is_none(),
            "close() must drop the session entry even when no daemon was running"
        );
    }

    /// Allocate, close, allocate, close — repeated 30 times — must
    /// never exhaust the 9223–9299 range as long as no daemon
    /// genuinely holds the port. Guards against the leak where an
    /// alias was kept under a second key after `close()`.
    #[tokio::test]
    async fn many_allocate_close_cycles_do_not_exhaust_range() {
        let mgr = AgentBrowserManager::new();
        for i in 0..30 {
            let key = format!("ws-cycle-{i}");
            let port = mgr
                .allocate_port(&key)
                .await
                .expect("allocator exhausted: leaked entries on close");
            assert!(
                (DEFAULT_STREAM_PORT..=MAX_STREAM_PORT).contains(&port),
                "port {port} outside the reserved range"
            );
            let _ = mgr.close(&key).await;
            assert!(mgr.get_port(&key).await.is_none());
        }
    }

    /// `note_frame_seen` updates `last_seen_at` only for an existing
    /// session and is a no-op for unknown keys (so the frontend can
    /// fire-and-forget without races at teardown).
    #[tokio::test]
    async fn note_frame_seen_is_noop_for_unknown_session() {
        let mgr = AgentBrowserManager::new();
        // Should not panic, should not insert.
        mgr.note_frame_seen("ws-never-allocated").await;
        assert!(mgr.get_port("ws-never-allocated").await.is_none());
    }

    #[tokio::test]
    async fn note_frame_seen_updates_existing_session() {
        let mgr = AgentBrowserManager::new();
        let _ = mgr.allocate_port("ws-frame-tracked").await.unwrap();
        // Before: last_seen_at = None.
        let before = mgr
            .sessions
            .lock()
            .await
            .get("ws-frame-tracked")
            .and_then(|s| s.last_seen_at);
        assert!(before.is_none());
        mgr.note_frame_seen("ws-frame-tracked").await;
        let after = mgr
            .sessions
            .lock()
            .await
            .get("ws-frame-tracked")
            .and_then(|s| s.last_seen_at);
        assert!(
            after.is_some(),
            "note_frame_seen should set last_seen_at on a tracked session"
        );
    }

    /// `session_pid_path` returns a path under the runtime dir when
    /// `HOME` is set. Sanity-check the layout so the safe-adoption
    /// path can find files written here.
    #[test]
    fn session_pid_path_lives_under_run_dir() {
        // Skip the test in environments where dirs::home_dir() can't
        // resolve a home — CI sandboxes occasionally lack one.
        let Some(_home) = dirs::home_dir() else {
            eprintln!("[test] no home dir — skipping session_pid_path layout check");
            return;
        };
        let path = session_pid_path("ws-layout-test").expect("path resolves");
        let s = path.to_string_lossy();
        assert!(
            s.ends_with("agent-browser-ws-layout-test.pid"),
            "unexpected pid path filename: {s}"
        );
        assert!(path.starts_with(run_dir_for_session("ws-layout-test").unwrap()), "PID file must live in the browser runtime directory: {s}");
    }

    #[cfg(unix)]
    #[test]
    fn long_home_uses_short_private_runtime_for_browser_socket() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let runtime_root = temp.path().join("rt");
        std::fs::create_dir(&runtime_root).unwrap();
        std::fs::set_permissions(&runtime_root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let home = temp.path().join("long-home-segment-".repeat(8));
        let session = "browser-long-home";
        assert!(!browser_socket_path_fits(&browser_run_dir_for(&home), session));

        let dir = run_dir_for_roots(Some(&home), Some(&runtime_root), session).unwrap();
        assert_eq!(dir, runtime_root.join("cmx-browser"));
        assert_eq!(run_dir_for_roots(None, Some(&runtime_root), session), Some(dir.clone()));
        assert!(browser_socket_path_fits(&dir, session));
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);

        let fallback_home = temp.path().join("home");
        std::fs::create_dir(&fallback_home).unwrap();
        assert_eq!(
            run_dir_for_roots(Some(&fallback_home), Some(&runtime_root), session),
            Some(browser_run_dir_for(&fallback_home)),
            "existing sessions that fit under home must stay in their original runtime directory"
        );
        std::fs::set_permissions(&runtime_root, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            run_dir_for_roots(Some(&fallback_home), Some(&runtime_root), session),
            Some(browser_run_dir_for(&fallback_home))
        );
    }

    #[cfg(unix)]
    #[test]
    fn short_runtime_dir_preserves_former_socket_path_boundary() {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir_in("/tmp").unwrap();
        let root_name_len = 19usize.checked_sub(temp.path().as_os_str().as_bytes().len() + 1).unwrap();
        let runtime_root = temp.path().join("r".repeat(root_name_len));
        std::fs::create_dir(&runtime_root).unwrap();
        std::fs::set_permissions(&runtime_root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let session = "s".repeat(64);
        validate_session_name(&session).unwrap();
        assert_eq!(runtime_root.join("agent-browser").join(format!("{session}.sock")).as_os_str().as_bytes().len(), 103);
        let dir = private_runtime_run_dir(&runtime_root).unwrap();
        assert!(browser_socket_path_fits(&dir, &session));
    }

    #[cfg(windows)]
    #[test]
    fn legacy_inherited_run_acl_does_not_block_browser_after_upgrade() {
        let temp = tempfile::tempdir().unwrap();
        let legacy = temp.path().join(".codemux").join("run");
        std::fs::create_dir_all(&legacy).unwrap();
        let private = browser_run_dir_for(temp.path());
        assert_ne!(private, legacy);
        prepare_run_dir(&private).expect("new browser runtime directory is private");
        clack_private_fs::require_private_directory(&private).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn runtime_directory_and_browser_logs_are_owner_only() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let temp = tempfile::tempdir().unwrap();
        let run = temp.path().join(".codemux/run");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o755)).unwrap();
        prepare_run_dir(&run).unwrap();
        assert_eq!(std::fs::metadata(&run).unwrap().permissions().mode() & 0o777, 0o700);

        let app_dir = run.parent().unwrap();
        std::fs::set_permissions(app_dir, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(prepare_run_dir(&run).is_none());
        std::fs::set_permissions(app_dir, std::fs::Permissions::from_mode(0o700)).unwrap();

        let log = run.join("browser.log");
        std::fs::write(&log, b"existing log").unwrap();
        std::fs::set_permissions(&log, std::fs::Permissions::from_mode(0o644)).unwrap();
        open_session_log(&log).unwrap();
        assert_eq!(std::fs::metadata(&log).unwrap().permissions().mode() & 0o777, 0o600);

        let target = temp.path().join("outside.log");
        std::fs::write(&target, b"keep").unwrap();
        symlink(&target, run.join("linked.log")).unwrap();
        assert!(open_session_log(&run.join("linked.log")).is_none());
        assert_eq!(std::fs::read(target).unwrap(), b"keep");

        let outside = temp.path().join("outside-dir");
        std::fs::create_dir(&outside).unwrap();
        let linked_run = temp.path().join("linked-run");
        symlink(outside, &linked_run).unwrap();
        assert!(prepare_run_dir(&linked_run).is_none());

        let linked_app = temp.path().join("linked-app");
        symlink(temp.path().join(".codemux"), &linked_app).unwrap();
        assert!(prepare_run_dir(&linked_app.join("run")).is_none());
    }

    /// Round-trip a PID through `write_session_pid` + the read helper.
    /// Confirms the on-disk format matches what
    /// `read_agent_browser_daemon_pid` expects so the runtime
    /// reconciliation path can actually use what we wrote.
    #[test]
    fn pid_file_round_trip_via_write_session_pid() {
        // We write to ~/.codemux/run/agent-browser-{name}.pid and read
        // via session_pid_path (NOT the agent-browser-CLI's own PID
        // file). Use a unique name so parallel test runs don't collide.
        let name = format!("ws-roundtrip-{}", std::process::id());
        let first = BrowserPidRecord {
            pid: 12345,
            start_time: 100,
            token: "a".repeat(32),
        };
        write_session_pid(&name, &first).unwrap();
        let path = session_pid_path(&name).expect("path resolves");
        let on_disk = read_browser_pid_record(&path).unwrap();
        assert_eq!(on_disk.pid, 12345);
        assert_eq!(on_disk.start_time, 100);
        assert_eq!(on_disk.token, first.token);
        clear_session_pid(&name);
        assert!(
            !path.exists(),
            "clear_session_pid must remove the file we wrote"
        );

        std::fs::write(&path, b"12345\n").unwrap();
        assert!(read_browser_pid_record(&path).is_none(), "legacy PID-only files are untrusted");
        clear_session_pid(&name);

        #[cfg(unix)]
        {
            use std::os::unix::fs::{symlink, PermissionsExt};
            let temp = tempfile::tempdir().unwrap();
            let target = temp.path().join("outside.pid");
            std::fs::write(&target, b"keep").unwrap();
            symlink(&target, &path).unwrap();
            let second = BrowserPidRecord {
                pid: 54321,
                start_time: 200,
                token: "b".repeat(32),
            };
            write_session_pid(&name, &second).unwrap();
            assert_eq!(std::fs::read(&target).unwrap(), b"keep");
            assert_eq!(read_browser_pid_record(&path).unwrap().pid, 54321);
            assert!(!std::fs::symlink_metadata(&path).unwrap().file_type().is_symlink());
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
            clear_session_pid(&name);
        }
    }

    #[cfg(unix)]
    #[test]
    fn browser_process_identity_requires_session_token_and_start_time() {
        let temp = tempfile::tempdir().unwrap();
        let binary = temp.path().join("agent-browser");
        // Some coreutils binaries dispatch by argv[0] and exit when copied
        // under a different name, so keep a shell process alive instead.
        std::fs::copy(which::which("bash").unwrap(), &binary).unwrap();
        let session = "browser-identity-test";
        let token = "a".repeat(32);
        let mut command = std::process::Command::new(&binary);
        command.args(["-c", "while :; do sleep 1; done"])
            .env("CODEMUX_BROWSER_OWNER", &token)
            .env("CODEMUX_BROWSER_SESSION", session);
        let spawn_started = Instant::now();
        let mut child = loop {
            match command.spawn() {
                Ok(child) => break child,
                Err(error) if error.raw_os_error() == Some(libc::ETXTBSY)
                    && spawn_started.elapsed() < Duration::from_secs(3) =>
                {
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(error) => panic!("failed to start browser identity fixture: {error}"),
            }
        };
        let pid = child.id();
        let started_waiting = Instant::now();
        let start_time = loop {
            if let Some(start) = browser_process_start(pid, session, &token) {
                break start;
            }
            if started_waiting.elapsed() >= Duration::from_secs(3) {
                let status = child.try_wait().unwrap();
                let _ = child.kill();
                let _ = child.wait();
                panic!("browser process identity unavailable after 3s: pid={pid}, child_status={status:?}, executable={binary:?}");
            }
            std::thread::sleep(Duration::from_millis(25));
        };
        let record = BrowserPidRecord { pid, start_time, token: token.clone() };
        assert!(browser_process_matches(&record, session));
        assert!(!browser_process_matches(&record, "another-session"));
        assert!(!browser_process_matches(&BrowserPidRecord { token: "b".repeat(32), ..record.clone() }, session));
        assert!(!browser_process_matches(&BrowserPidRecord { start_time: start_time + 1, ..record }, session));
        assert!(browser_process_start(std::process::id(), session, &token).is_none());
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn failed_open_after_another_process_starts_does_not_stop_its_daemon() {
        let temp = tempfile::tempdir().unwrap();
        let binary = temp.path().join("agent-browser");
        std::fs::copy(which::which("bash").unwrap(), &binary).unwrap();
        let session = format!("browser-foreign-open-{}", uuid::Uuid::new_v4().simple());
        let token = uuid::Uuid::new_v4().simple().to_string();
        let mut command = std::process::Command::new(&binary);
        command.args(["-c", "while :; do sleep 1; done"])
            .env("CODEMUX_BROWSER_OWNER", &token)
            .env("CODEMUX_BROWSER_SESSION", &session);
        let spawn_started = Instant::now();
        let mut child = loop {
            match command.spawn() {
                Ok(child) => break child,
                Err(error) if error.raw_os_error() == Some(libc::ETXTBSY)
                    && spawn_started.elapsed() < Duration::from_secs(3) =>
                {
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(error) => panic!("failed to start foreign browser fixture: {error}"),
            }
        };
        let start = Instant::now();
        let start_time = loop {
            if let Some(start_time) = browser_process_start(child.id(), &session, &token) {
                break start_time;
            }
            assert!(start.elapsed() < Duration::from_secs(3));
            std::thread::sleep(Duration::from_millis(25));
        };
        let daemon_pid_path = run_dir_for_session(&session)
            .unwrap()
            .join(format!("{session}.pid"));
        std::fs::write(&daemon_pid_path, child.id().to_string()).unwrap();
        write_session_pid(&session, &BrowserPidRecord {
            pid: child.id(),
            start_time,
            token,
        }).unwrap();

        rollback_failed_unix_open(&session, None);
        let still_running = child.try_wait().unwrap().is_none();

        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_file(daemon_pid_path);
        clear_session_pid(&session);
        assert!(still_running, "failed open stopped another process's daemon");
    }

    #[cfg(unix)]
    #[test]
    fn browser_shutdown_preserves_unrelated_process_named_in_pid_file() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let mut child = std::process::Command::new(which::which("sleep").unwrap())
            .arg("30")
            .spawn()
            .unwrap();
        let session = "unrelated-process-test";
        let path = temp.path().join(format!("agent-browser-{session}.pid"));
        std::fs::write(&path, format!("{} 1 {}\n", child.id(), browser_owner_token())).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();

        kill_owned_browser_daemons_in(temp.path());
        assert!(child.try_wait().unwrap().is_none(), "unrelated process must remain alive");

        child.kill().unwrap();
        child.wait().unwrap();
    }

    // ── Periodic orphan sweep helper (issue #126) ─────────────────────
    //
    // `orphaned_ws_sessions` is the pure decision function behind the
    // background sweep in lib.rs. These tests pin its contract without
    // needing the async manager, a real daemon, or app state.

    #[test]
    fn orphaned_ws_sessions_returns_ws_session_with_no_live_workspace() {
        let tracked = vec!["ws-abc123".to_string()];
        let live: std::collections::HashSet<String> = std::collections::HashSet::new();
        assert_eq!(orphaned_ws_sessions(&tracked, &live), vec!["ws-abc123".to_string()]);
    }

    #[test]
    fn orphaned_ws_sessions_skips_ws_session_with_live_workspace() {
        let tracked = vec!["ws-abc123".to_string()];
        let live: std::collections::HashSet<String> =
            ["ws-abc123".to_string()].into_iter().collect();
        assert!(orphaned_ws_sessions(&tracked, &live).is_empty());
    }

    #[test]
    fn orphaned_ws_sessions_never_sweeps_browser_pane_or_default_sessions() {
        let tracked = vec!["browser-42".to_string(), "default".to_string()];
        // Neither name is in the live set, but neither is a `ws-*` name, so
        // both must survive — user browser panes and the default session
        // are never workspace-scoped and are never candidates for the
        // orphan sweep.
        let live: std::collections::HashSet<String> = std::collections::HashSet::new();
        assert!(orphaned_ws_sessions(&tracked, &live).is_empty());
    }

    #[test]
    fn orphaned_ws_sessions_handles_a_mixed_tracked_set() {
        let tracked = vec![
            "ws-live".to_string(),
            "ws-orphan".to_string(),
            "browser-7".to_string(),
            "default".to_string(),
        ];
        let live: std::collections::HashSet<String> =
            ["ws-live".to_string()].into_iter().collect();
        assert_eq!(orphaned_ws_sessions(&tracked, &live), vec!["ws-orphan".to_string()]);
    }
}
