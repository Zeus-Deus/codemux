use serde::{Deserialize, Serialize};
use std::collections::HashMap;
#[cfg(not(windows))]
use std::collections::HashSet;
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
/// Unlike [`output_capture_with_timeout`] this writes stdout to an anonymous
/// file and leaves stderr as the caller configured it. The launcher forks a
/// long-lived daemon that may inherit stdout; a pipe could wait forever for
/// EOF. Reading the file after the launcher exits also preserves pending
/// confirmation replies without waiting for the daemon to close its handle.
#[cfg(not(windows))]
fn launch_status_with_timeout(
    mut command: std::process::Command,
    timeout: Duration,
) -> std::io::Result<std::process::Output> {
    use std::io::{Read, Seek};

    let mut stdout_file = tempfile::tempfile()?;
    command.stdout(stdout_file.try_clone()?);
    // stderr intentionally left untouched so a caller-set log file / inherited
    // handle is preserved; it is a file or inherited fd, never a pipe we own.
    let mut output = spawn_wait_collect(command, timeout)?;
    stdout_file.rewind()?;
    stdout_file.take(64 * 1024).read_to_end(&mut output.stdout)?;
    Ok(output)
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
    // A detached Unix daemon may be alive before it publishes a PID file.
    // Its inherited owner token still lets us verify and stop it on exit.
    #[cfg(unix)]
    kill_untracked_browser_daemons(None);
}

#[cfg(unix)]
fn kill_untracked_browser_daemons(session_filter: Option<&str>) {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

    let owner = format!("CODEMUX_BROWSER_OWNER={}", browser_owner_token());
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_exe(UpdateKind::Always)
            .with_environ(UpdateKind::Always),
    );
    for process in system.processes().values() {
        let Some(session) = process
            .environ()
            .iter()
            .find_map(|entry| entry.to_string_lossy().strip_prefix("CODEMUX_BROWSER_SESSION=").map(str::to_owned))
        else {
            continue;
        };
        if session_filter.is_some_and(|expected| expected != session.as_str())
            || validate_session_name(&session).is_err()
            || !process.environ().iter().any(|entry| entry.to_string_lossy() == owner)
        {
            continue;
        }
        let pid = process.pid().as_u32();
        let Some(start_time) = process_start_time(pid) else { continue };
        let record = BrowserPidRecord {
            pid,
            start_time,
            token: browser_owner_token().to_owned(),
            owner_pid: 0,
            owner_start_time: 0,
            native_start: true,
        };
        if browser_process_matches(&record, &session) {
            kill_verified_browser_pid(&record, &session);
        }
    }
}

fn kill_owned_browser_daemons_in(dir: &std::path::Path) {
    let bin = resolve_binary();
    for path in browser_pid_record_paths_in(dir) {
        let Some(session) = session_from_pid_path(&path) else { continue };
        let Some(record) = read_browser_pid_record(&path) else { continue };
        if record.token != browser_owner_token() || !browser_process_matches(&record, session) {
            continue;
        }
        stop_owned_browser_daemon(session, &record, &bin);
        if path.parent() != Some(dir) && !pid_alive(record.pid) {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Root for Codemux's per-session browser records and logs. New agent-browser
/// daemons keep their PID, socket, and port files in a private `d/` child.
/// Unix uses a private directory under XDG_RUNTIME_DIR when a session's
/// home-based socket path would be too long.
/// Windows uses a
/// new private directory because older releases created `~/.codemux/run`
/// with an inherited ACL.
///
/// The directory is created lazily on first use. Returns `None` when neither
/// a usable runtime root nor a home directory is available.
pub fn run_dir() -> Option<PathBuf> {
    browser_run_root_for_session("default")
}

fn run_dir_for_session(session: &str) -> Option<PathBuf> {
    let root = browser_run_root_for_session(session)?;
    browser_daemon_dir_in(&root, session)
}

fn browser_run_root_for_session(session: &str) -> Option<PathBuf> {
    validate_session_name(session).ok()?;
    #[cfg(unix)]
    {
        let runtime_root = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
        let home = dirs::home_dir();
        return browser_run_root_for_roots(home.as_deref(), runtime_root.as_deref(), session);
    }
    #[cfg(not(unix))]
    {
        let dir = browser_run_dir_for(&dirs::home_dir()?);
        prepare_run_dir(&dir)?;
        Some(dir)
    }
}

#[cfg(unix)]
fn browser_run_root_for_roots(
    home: Option<&std::path::Path>,
    runtime_root: Option<&std::path::Path>,
    session: &str,
) -> Option<PathBuf> {
    // A live old daemon may use a root whose socket fits only without /d.
    // Check both former locations before choosing a root for new launches.
    let home_dir = home.map(browser_run_dir_for);
    let short_dir = runtime_root.and_then(private_runtime_run_dir);
    for dir in [home_dir.as_deref(), short_dir.as_deref()].into_iter().flatten() {
        if browser_socket_path_fits(dir, session) && prepare_run_dir(dir).is_some()
            && (verified_legacy_browser_pid_in(dir, session)
                || probe_legacy_browser_control_endpoint(session, dir, &dir.join(BROWSER_DAEMON_DIR))
                    != BrowserControlEndpointProbe::Absent)
        {
            return Some(dir.to_path_buf());
        }
    }
    run_dir_for_roots(home, runtime_root, session)
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
        if browser_socket_path_fits(&home_dir.join(BROWSER_DAEMON_DIR), session)
            && prepare_run_dir(home_dir).is_some() {
            return Some(home_dir.clone());
        }
    }
    if let Some(short_dir) = runtime_root.and_then(private_runtime_run_dir) {
        if browser_socket_path_fits(&short_dir.join(BROWSER_DAEMON_DIR), session)
            && prepare_run_dir(&short_dir).is_some() {
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
        browser_run_root_for_session("default").into_iter().collect()
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

const BROWSER_DAEMON_DIR: &str = "d";

fn verified_legacy_browser_pid_in(root: &std::path::Path, session: &str) -> bool {
    let path = root.join(format!("{session}.pid"));
    let Some(pid) = read_daemon_pid_file(&path) else { return false };
    session_pid_record_in(root, session)
        .is_some_and(|(_, record)| record.pid == pid && browser_process_uses_control_dir(&record, session, root))
}

fn browser_process_uses_control_dir(record: &BrowserPidRecord, session: &str, dir: &std::path::Path) -> bool {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

    if !browser_process_matches(record, session) {
        return false;
    }
    let pid = Pid::from_u32(record.pid);
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing().with_environ(UpdateKind::Always),
    );
    let expected = format!("AGENT_BROWSER_SOCKET_DIR={}", dir.display());
    system.process(pid).is_some_and(|process| {
        process.environ().iter().any(|entry| entry.to_string_lossy() == expected)
            && process_start_time(record.pid) == Some(record.start_time)
    })
}

#[cfg(not(windows))]
fn probe_legacy_browser_control_endpoint(session: &str, root: &std::path::Path, _current: &std::path::Path) -> BrowserControlEndpointProbe {
    probe_browser_control_endpoint(session, root)
}

#[cfg(windows)]
fn probe_legacy_browser_control_endpoint(session: &str, root: &std::path::Path, current: &std::path::Path) -> BrowserControlEndpointProbe {
    let root_pid = read_daemon_pid_file(&root.join(format!("{session}.pid")));
    let current_pid = read_daemon_pid_file(&current.join(format!("{session}.pid")));
    let current_verified = current_pid.is_some_and(|pid| {
        owned_browser_pid(session, pid).is_some_and(|record| {
            browser_process_uses_control_dir(&record, session, current)
                && browser_control_listener_owned_by_pid(session, current, pid)
        })
    });
    if current_verified && !verified_legacy_browser_pid_in(root, session) {
        return BrowserControlEndpointProbe::Absent;
    }
    let port_path = root.join(format!("{session}.port"));
    let (port, root_port_present) = match std::fs::symlink_metadata(&port_path) {
        Ok(meta) => {
            use std::os::windows::fs::MetadataExt;
            if !meta.is_file() || meta.file_attributes() & 0x400 != 0
                || clack_private_fs::require_private_file(&port_path).is_err() {
                return BrowserControlEndpointProbe::Inconclusive;
            }
            match std::fs::read_to_string(&port_path).ok()
                .and_then(|value| value.trim().parse::<u16>().ok()) {
                Some(port) if port != 0 => (port, true),
                _ => return BrowserControlEndpointProbe::Inconclusive,
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (browser_control_fallback_port(session), false),
        Err(_) => return BrowserControlEndpointProbe::Inconclusive,
    };
    if !root_port_present && root_pid.is_none() {
        return BrowserControlEndpointProbe::Absent;
    }
    let Some(listener_pid) = windows_loopback_listener_pid(port) else {
        return BrowserControlEndpointProbe::Inconclusive;
    };
    let result = classify_legacy_browser_listener(listener_pid, root_pid, current_pid, current_verified);
    if !root_port_present && result == BrowserControlEndpointProbe::Inconclusive
        && listener_pid.is_some_and(|pid| Some(pid) != root_pid) {
        BrowserControlEndpointProbe::Absent
    } else {
        result
    }
}

#[cfg(windows)]
fn classify_legacy_browser_listener(
    listener_pid: Option<u32>,
    root_pid: Option<u32>,
    current_pid: Option<u32>,
    current_verified: bool,
) -> BrowserControlEndpointProbe {
    match listener_pid {
        None => BrowserControlEndpointProbe::Absent,
        Some(pid) if current_verified && current_pid == Some(pid) => BrowserControlEndpointProbe::Absent,
        Some(pid) if root_pid == Some(pid) => BrowserControlEndpointProbe::Reachable,
        Some(_) => BrowserControlEndpointProbe::Inconclusive,
    }
}

#[cfg(not(windows))]
fn probe_current_browser_control_endpoint(
    session: &str,
    current: &std::path::Path,
    _verified_legacy_pid: u32,
) -> BrowserControlEndpointProbe {
    probe_browser_control_endpoint(session, current)
}

#[cfg(windows)]
fn probe_current_browser_control_endpoint(
    session: &str,
    current: &std::path::Path,
    verified_legacy_pid: u32,
) -> BrowserControlEndpointProbe {
    let port_path = current.join(format!("{session}.port"));
    let port = match std::fs::symlink_metadata(&port_path) {
        Ok(meta) => {
            use std::os::windows::fs::MetadataExt;
            if !meta.is_file() || meta.file_attributes() & 0x400 != 0
                || clack_private_fs::require_private_file(&port_path).is_err() {
                return BrowserControlEndpointProbe::Inconclusive;
            }
            match std::fs::read_to_string(&port_path).ok()
                .and_then(|value| value.trim().parse::<u16>().ok()) {
                Some(port) if port != 0 => port,
                _ => return BrowserControlEndpointProbe::Inconclusive,
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // The hash port is shared by every control directory for this
            // session. Without either d/ file it cannot identify a d/ daemon.
            match std::fs::symlink_metadata(current.join(format!("{session}.pid"))) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return BrowserControlEndpointProbe::Absent;
                }
                Err(_) => return BrowserControlEndpointProbe::Inconclusive,
                Ok(_) => browser_control_fallback_port(session),
            }
        }
        Err(_) => return BrowserControlEndpointProbe::Inconclusive,
    };
    classify_current_browser_listener(windows_loopback_listener_pid(port), verified_legacy_pid)
}

#[cfg(windows)]
fn classify_current_browser_listener(
    listener: Option<Option<u32>>,
    verified_legacy_pid: u32,
) -> BrowserControlEndpointProbe {
    match listener {
        Some(None) => BrowserControlEndpointProbe::Absent,
        Some(Some(pid)) if pid == verified_legacy_pid => BrowserControlEndpointProbe::Absent,
        Some(Some(_)) | None => BrowserControlEndpointProbe::Inconclusive,
    }
}

fn select_verified_legacy_browser_dir(
    root: &std::path::Path,
    current: &std::path::Path,
    session: &str,
    current_pid_path: &std::path::Path,
    current_present: bool,
    legacy_pid: u32,
) -> Option<PathBuf> {
    let current_conflict = current_present && !read_daemon_pid_file(current_pid_path)
        .is_some_and(|pid| !pid_alive(pid));
    (!current_conflict
        && probe_current_browser_control_endpoint(session, current, legacy_pid)
            == BrowserControlEndpointProbe::Absent)
        .then_some(root.to_path_buf())
}

/// Select the new daemon namespace unless a live daemon is verified in the
/// legacy root. A numeric legacy record for another prefixed session can have
/// the same root filename; an absent root endpoint lets the new daemon use
/// its separate directory without touching that file.
fn browser_daemon_dir_in(root: &std::path::Path, session: &str) -> Option<PathBuf> {
    validate_session_name(session).ok()?;
    let current = root.join(BROWSER_DAEMON_DIR);
    prepare_browser_directory(&current, true)?;
    #[cfg(unix)]
    let current_fits = browser_socket_path_fits(&current, session);
    #[cfg(not(unix))]
    let current_fits = true;
    let current_pid = current.join(format!("{session}.pid"));
    let legacy_pid = root.join(format!("{session}.pid"));
    let present = |path: &std::path::Path| match std::fs::symlink_metadata(path) {
        Ok(_) => Some(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(false),
        Err(_) => None,
    };
    let current_present = present(&current_pid)?;
    let legacy_present = present(&legacy_pid)?;

    if legacy_present {
        if read_browser_pid_record(&legacy_pid).is_some() {
            // The old path is a structured Codemux record for a different
            // session, never the daemon's numeric PID file.
            return (current_fits && probe_legacy_browser_control_endpoint(session, root, &current) == BrowserControlEndpointProbe::Absent)
                .then_some(current);
        }
        if read_daemon_pid_file(&legacy_pid).is_none() {
            return None;
        }
        if verified_legacy_browser_pid_in(root, session) {
            let legacy_pid = read_daemon_pid_file(&legacy_pid)?;
            return select_verified_legacy_browser_dir(
                root, &current, session, &current_pid, current_present, legacy_pid,
            );
        }
        return (current_fits && probe_legacy_browser_control_endpoint(session, root, &current) == BrowserControlEndpointProbe::Absent)
            .then_some(current);
    }
    (current_fits && probe_legacy_browser_control_endpoint(session, root, &current) == BrowserControlEndpointProbe::Absent)
        .then_some(current)
}

fn browser_record_root_from_daemon_dir(dir: &std::path::Path) -> &std::path::Path {
    if dir.file_name() == Some(std::ffi::OsStr::new(BROWSER_DAEMON_DIR)) {
        dir.parent().unwrap_or(dir)
    } else {
        dir
    }
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

const BROWSER_RECORD_DIR: &str = "codemux-records";

fn browser_record_dir_in(dir: &std::path::Path) -> Option<PathBuf> {
    let records = dir.join(BROWSER_RECORD_DIR);
    prepare_browser_directory(&records, true)?;
    Some(records)
}

fn legacy_session_pid_path_in(dir: &std::path::Path, session: &str) -> Option<PathBuf> {
    validate_session_name(session).ok()?;
    Some(dir.join(format!("agent-browser-{session}.pid")))
}

fn session_pid_path_in(dir: &std::path::Path, session: &str) -> Option<PathBuf> {
    validate_session_name(session).ok()?;
    Some(browser_record_dir_in(dir)?.join(format!("agent-browser-{session}.pid")))
}

fn session_pid_record_in(dir: &std::path::Path, session: &str) -> Option<(PathBuf, BrowserPidRecord)> {
    if let Some(current) = session_pid_path_in(dir, session) {
        if let Some(record) = read_browser_pid_record(&current) {
            return Some((current, record));
        }
    }
    let legacy = legacy_session_pid_path_in(dir, session)?;
    read_browser_pid_record(&legacy).map(|record| (legacy, record))
}

fn session_pid_record(session: &str) -> Option<(PathBuf, BrowserPidRecord)> {
    session_pid_record_in(&browser_run_root_for_session(session)?, session)
}

fn browser_pid_record_paths_in(dir: &std::path::Path) -> Vec<PathBuf> {
    // Legacy root records remain readable, but are never removed here: the
    // same root name may be another session's daemon PID by the time cleanup
    // runs. Only the separate current namespace is safe to retire.
    let mut paths = Vec::new();
    for record_dir in [browser_record_dir_in(dir), Some(dir.to_path_buf())].into_iter().flatten() {
        if let Ok(entries) = std::fs::read_dir(record_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if session_from_pid_path(&path).is_some() && read_browser_pid_record(&path).is_some() {
                    paths.push(path);
                }
            }
        }
    }
    paths
}

/// Path to the per-session PID file written by codemux when it spawns
/// the agent-browser daemon. Used by `close()` for targeted PID-based
/// teardown and by `new_with_adoption()` for safe restart adoption.
pub fn session_pid_path(session_name: &str) -> Option<PathBuf> {
    session_pid_path_in(&browser_run_root_for_session(session_name)?, session_name)
}

/// Path to the per-session stderr log file. Captured during daemon
/// spawn so the next investigation has a paper trail.
fn session_log_path(session_name: &str) -> Option<PathBuf> {
    validate_session_name(session_name).ok()?;
    Some(browser_run_root_for_session(session_name)?.join(format!("agent-browser-{session_name}.log")))
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

fn cleanup_stale_browser_lock_files_in(dir: &std::path::Path, session: &str, pid: u32) {
    if validate_session_name(session).is_err() || pid_alive(pid) {
        return;
    }
    let pid_path = dir.join(format!("{session}.pid"));
    for ext in ["port", "engine", "stream", "pid"] {
        // The daemon may have restarted under this session name. Recheck
        // before touching each entry and remove the PID file last.
        if pid_alive(pid) || browser_control_endpoint_ready_in_namespace(session, dir)
            || read_daemon_pid_file(&pid_path) != Some(pid) {
            break;
        }
        let _ = std::fs::remove_file(dir.join(format!("{session}.{ext}")));
    }
}

fn browser_control_endpoint_ready_in_namespace(session: &str, dir: &std::path::Path) -> bool {
    #[cfg(windows)]
    {
        if dir.file_name() == Some(std::ffi::OsStr::new(BROWSER_DAEMON_DIR)) {
            return probe_current_browser_control_endpoint(session, dir, 0)
                != BrowserControlEndpointProbe::Absent;
        }
        return probe_legacy_browser_control_endpoint(session, dir, &dir.join(BROWSER_DAEMON_DIR))
            != BrowserControlEndpointProbe::Absent;
    }
    #[cfg(not(windows))]
    browser_control_endpoint_ready(session, dir)
}

/// Retire files for a daemon whose numeric PID belongs to a different
/// process now. Keep the files when the new process could be a browser daemon
/// starting up, or when the control endpoint is still reachable.
fn cleanup_reused_browser_lock_files_in(dir: &std::path::Path, session: &str, old: &BrowserPidRecord) -> bool {
    if validate_session_name(session).is_err() {
        return false;
    }
    let pid_path = dir.join(format!("{session}.pid"));
    let root = browser_record_root_from_daemon_dir(dir);
    let Some((record_path, _)) = session_pid_record_in(root, session) else { return false };
    let legacy_record = record_path.parent() == Some(root);
    let stale = || {
        (old.token == browser_owner_token() || previous_app_has_exited(old))
            && process_start_time(old.pid).is_some_and(|start| start != old.start_time)
            && process_looks_like_agent_browser(old.pid) == Some(false)
            && !browser_control_endpoint_ready_in_namespace(session, dir)
            && read_daemon_pid_file(&pid_path) == Some(old.pid)
            && read_browser_pid_record(&record_path).as_ref() == Some(old)
    };
    for ext in ["port", "engine", "stream", "pid"] {
        if !stale() {
            return false;
        }
        let path = dir.join(format!("{session}.{ext}"));
        if let Err(error) = std::fs::remove_file(path) {
            if error.kind() != std::io::ErrorKind::NotFound {
                return false;
            }
        }
    }
    if browser_control_endpoint_ready_in_namespace(session, dir)
        || read_browser_pid_record(&record_path).as_ref() != Some(old) {
        return false;
    }
    legacy_record || std::fs::remove_file(record_path).is_ok()
}

#[cfg(windows)]
fn cleanup_owned_browser_lock_files_in(dir: &std::path::Path, session: &str, pid: u32) {
    if validate_session_name(session).is_err() {
        return;
    }
    for _ in 0..10 {
        if !pid_alive(pid) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    cleanup_stale_browser_lock_files_in(dir, session, pid);
}

#[cfg(windows)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct WindowsBrowserFileId(u64, u128);

#[cfg(windows)]
fn open_windows_browser_regular_file(path: &std::path::Path) -> Option<std::fs::File> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;

    let file = std::fs::File::options().read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT).open(path).ok()?;
    let meta = file.metadata().ok()?;
    (meta.is_file() && meta.file_attributes() & 0x400 == 0).then_some(file)
}

#[cfg(windows)]
fn windows_browser_file_id(path: &std::path::Path) -> Option<WindowsBrowserFileId> {
    let file = open_windows_browser_regular_file(path)?;
    let (volume, id) = crate::windows_file_id::file_id(&file).ok()?;
    Some(WindowsBrowserFileId(volume, id))
}

#[cfg(windows)]
fn optional_windows_browser_file_id(path: &std::path::Path) -> Option<Option<WindowsBrowserFileId>> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(None),
        Ok(_) => windows_browser_file_id(path).map(Some),
        Err(_) => None,
    }
}

#[cfg(windows)]
struct WindowsBrowserPortSnapshot {
    id: WindowsBrowserFileId,
    contents: Vec<u8>,
    port: u16,
}

#[cfg(windows)]
fn windows_browser_port_snapshot(path: &std::path::Path) -> Option<WindowsBrowserPortSnapshot> {
    use std::io::Read;

    let mut file = open_windows_browser_regular_file(path)?;
    let (volume, id) = crate::windows_file_id::file_id(&file).ok()?;
    let mut contents = Vec::new();
    (&mut file).take(16).read_to_end(&mut contents).ok()?;
    if contents.len() == 16 {
        return None;
    }
    let port = std::str::from_utf8(&contents).ok()?.trim().parse::<u16>().ok()?;
    (port != 0 && windows_browser_file_id(path) == Some(WindowsBrowserFileId(volume, id)))
        .then_some(WindowsBrowserPortSnapshot {
            id: WindowsBrowserFileId(volume, id), contents, port,
        })
}

#[cfg(windows)]
fn optional_windows_browser_port(path: &std::path::Path) -> Option<Option<WindowsBrowserPortSnapshot>> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(None),
        Ok(_) => windows_browser_port_snapshot(path).map(Some),
        Err(_) => None,
    }
}

#[cfg(windows)]
struct CapturedLegacyWindowsBrowserFiles {
    dir: PathBuf,
    pid: WindowsBrowserFileId,
    port: Option<WindowsBrowserPortSnapshot>,
    engine: Option<WindowsBrowserFileId>,
    stream: Option<WindowsBrowserFileId>,
}

/// Legacy daemon-written files may inherit an ACL broader than Codemux's
/// current private-file policy. Capture their identities while the verified
/// daemon is alive; their ACL and contents do not establish ownership.
#[cfg(windows)]
fn capture_legacy_windows_browser_files(
    dir: &std::path::Path,
    session: &str,
    record: &BrowserPidRecord,
) -> Option<CapturedLegacyWindowsBrowserFiles> {
    if dir.file_name() == Some(std::ffi::OsStr::new(BROWSER_DAEMON_DIR))
        || validate_session_name(session).is_err() {
        return None;
    }
    let pid_path = dir.join(format!("{session}.pid"));
    if read_daemon_pid_file(&pid_path) != Some(record.pid) {
        return None;
    }
    let pid = windows_browser_file_id(&pid_path)?;
    let port = optional_windows_browser_port(&dir.join(format!("{session}.port")))?;
    let engine = optional_windows_browser_file_id(&dir.join(format!("{session}.engine")))?;
    let stream = optional_windows_browser_file_id(&dir.join(format!("{session}.stream")))?;
    (read_daemon_pid_file(&pid_path) == Some(record.pid)
        && windows_browser_file_id(&pid_path) == Some(pid))
        .then_some(CapturedLegacyWindowsBrowserFiles {
            dir: dir.to_path_buf(), pid, port, engine, stream,
        })
}

#[cfg(windows)]
fn optional_windows_browser_file_matches(path: &std::path::Path, expected: Option<WindowsBrowserFileId>) -> bool {
    match expected {
        Some(id) => windows_browser_file_id(path) == Some(id),
        None => matches!(std::fs::symlink_metadata(path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound),
    }
}

#[cfg(windows)]
fn windows_browser_port_matches(path: &std::path::Path, expected: Option<&WindowsBrowserPortSnapshot>) -> bool {
    match expected {
        Some(old) => windows_browser_port_snapshot(path).is_some_and(|current| {
            current.id == old.id && current.contents == old.contents
        }),
        None => matches!(std::fs::symlink_metadata(path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound),
    }
}

#[cfg(windows)]
fn captured_legacy_browser_endpoint_absent(
    session: &str,
    pid: u32,
    files: &CapturedLegacyWindowsBrowserFiles,
) -> bool {
    let port = files.port.as_ref().map_or_else(|| browser_control_fallback_port(session), |port| port.port);
    let current = files.dir.join(BROWSER_DAEMON_DIR);
    let current_pid = read_daemon_pid_file(&current.join(format!("{session}.pid")));
    let current_verified = current_pid.is_some_and(|current_pid| {
        owned_browser_pid(session, current_pid).is_some_and(|record| {
            browser_process_uses_control_dir(&record, session, &current)
                && browser_control_listener_owned_by_pid(session, &current, current_pid)
        })
    });
    let Some(listener_pid) = windows_loopback_listener_pid(port) else { return false };
    let result = classify_legacy_browser_listener(listener_pid, Some(pid), current_pid, current_verified);
    result == BrowserControlEndpointProbe::Absent
        || (files.port.is_none() && result == BrowserControlEndpointProbe::Inconclusive
            && listener_pid.is_some_and(|listener_pid| listener_pid != pid))
}

#[cfg(windows)]
fn cleanup_captured_legacy_windows_browser_files(
    session: &str,
    record: &BrowserPidRecord,
    files: &CapturedLegacyWindowsBrowserFiles,
    old_exited: impl Fn() -> bool,
) -> bool {
    let path = |ext: &str| files.dir.join(format!("{session}.{ext}"));
    let pid_path = path("pid");
    let port_path = path("port");
    let engine_path = path("engine");
    let stream_path = path("stream");
    let mut port = files.port.as_ref();
    let mut engine = files.engine;
    let mut stream = files.stream;
    let still_old = |port: Option<&WindowsBrowserPortSnapshot>, engine, stream| {
        old_exited()
            && read_daemon_pid_file(&pid_path) == Some(record.pid)
            && windows_browser_file_id(&pid_path) == Some(files.pid)
            && windows_browser_port_matches(&port_path, port)
            && optional_windows_browser_file_matches(&engine_path, engine)
            && optional_windows_browser_file_matches(&stream_path, stream)
            && captured_legacy_browser_endpoint_absent(session, record.pid, files)
    };
    if !still_old(port, engine, stream) { return false; }
    if engine.is_some() {
        if std::fs::remove_file(&engine_path).is_err() { return false; }
        engine = None;
    }
    if !still_old(port, engine, stream) { return false; }
    if stream.is_some() {
        if std::fs::remove_file(&stream_path).is_err() { return false; }
        stream = None;
    }
    if !still_old(port, engine, stream) { return false; }
    if port.is_some() {
        if std::fs::remove_file(&port_path).is_err() { return false; }
        port = None;
    }
    still_old(port, engine, stream) && std::fs::remove_file(pid_path).is_ok()
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct BrowserPidRecord {
    pid: u32,
    start_time: u64,
    token: String,
    owner_pid: u32,
    owner_start_time: u64,
    /// `v2` records use the OS process-creation unit. Older seconds-based
    /// records cannot prove identity after a same-second PID recycle.
    native_start: bool,
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
        .env("AGENT_BROWSER_SOCKET_DIR", socket_dir);
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
    let (owner_pid, owner_start_time) = match (fields.next(), fields.next()) {
        (None, None) => (0, 0), // older records cannot prove the app owner exited
        (Some(pid), Some(start_time)) => (pid.parse::<u32>().ok()?, start_time.parse::<u64>().ok()?),
        _ => return None,
    };
    let native_start = match fields.next() {
        None => false,
        Some("v2") => true,
        _ => return None,
    };
    if pid == 0
        || start_time == 0
        || token.len() != 32
        || !token.bytes().all(|byte| byte.is_ascii_hexdigit())
        || (owner_pid == 0) != (owner_start_time == 0)
        || fields.next().is_some()
    {
        return None;
    }
    Some(BrowserPidRecord { pid, start_time, token: token.to_owned(), owner_pid, owner_start_time, native_start })
}

#[cfg(target_os = "linux")]
fn process_start_time(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (_, fields) = stat.rsplit_once(')')?;
    // Fields after comm begin at #3 (state); starttime is #22.
    fields.split_whitespace().nth(19)?.parse::<u64>().ok().filter(|ticks| *ticks > 0)
}

#[cfg(target_os = "macos")]
fn process_start_time(pid: u32) -> Option<u64> {
    let pid = i32::try_from(pid).ok()?;
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    let read = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, info.as_mut_ptr().cast(), size) };
    if read != size {
        return None;
    }
    let info = unsafe { info.assume_init() };
    info.pbi_start_tvsec.checked_mul(1_000_000)?
        .checked_add(info.pbi_start_tvusec)
        .filter(|micros| *micros > 0)
}

#[cfg(windows)]
fn process_start_time(pid: u32) -> Option<u64> {
    use std::os::windows::io::FromRawHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if raw.is_null() {
        return None;
    }
    let _handle = unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(raw) };
    process_creation_ticks(raw)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
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

fn process_looks_like_agent_browser(pid: u32) -> Option<bool> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

    let process_id = Pid::from_u32(pid);
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[process_id]),
        true,
        ProcessRefreshKind::nothing().with_exe(UpdateKind::Always),
    );
    let name = system.process(process_id)?.exe()?.file_name()?.to_string_lossy().to_ascii_lowercase();
    Some(name.starts_with("agent-browser"))
}

fn current_app_process_identity() -> Option<(u32, u64)> {
    let pid = std::process::id();
    Some((pid, process_start_time(pid).filter(|start_time| *start_time > 0)?))
}

fn browser_process_start(pid: u32, session: &str, token: &str) -> Option<u64> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

    let start_before = process_start_time(pid)?;
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
    let start_after = process_start_time(pid)?;
    (has_owner && has_session && start_before == start_after).then_some(start_before)
}

fn browser_process_matches(record: &BrowserPidRecord, session: &str) -> bool {
    record.native_start && browser_process_start(record.pid, session, &record.token) == Some(record.start_time)
}

fn owned_browser_pid(session: &str, pid: u32) -> Option<BrowserPidRecord> {
    if let Some(start_time) = browser_process_start(pid, session, browser_owner_token()) {
        let (owner_pid, owner_start_time) = current_app_process_identity()?;
        return Some(BrowserPidRecord {
            pid,
            start_time,
            token: browser_owner_token().to_owned(),
            owner_pid,
            owner_start_time,
            native_start: true,
        });
    }
    let (_, record) = session_pid_record(session)?;
    (record.pid == pid && browser_process_matches(&record, session)).then_some(record)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BrowserControlEndpointProbe {
    Reachable,
    Absent,
    Inconclusive,
}

/// A full backlog, timeout, or inspection error cannot prove the endpoint is
/// gone. Callers may retire ownership files only after an `Absent` result.
fn probe_browser_control_endpoint(session: &str, dir: &std::path::Path) -> BrowserControlEndpointProbe {
    #[cfg(unix)]
    {
        let path = dir.join(format!("{session}.sock"));
        match connect_unix_browser_control_until(&path, Instant::now() + Duration::from_millis(50)) {
            Ok(_) => BrowserControlEndpointProbe::Reachable,
            Err(error) if matches!(error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused) => BrowserControlEndpointProbe::Absent,
            Err(_) => BrowserControlEndpointProbe::Inconclusive,
        }
    }
    #[cfg(windows)]
    {
        let port = browser_control_port(session, dir);
        match std::net::TcpStream::connect_timeout(
            &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
            Duration::from_millis(50),
        ) {
            Ok(_) => return BrowserControlEndpointProbe::Reachable,
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
                return BrowserControlEndpointProbe::Absent;
            }
            Err(_) => {}
        }
        match windows_loopback_listener_pid(port) {
            Some(None) => BrowserControlEndpointProbe::Absent,
            Some(Some(_)) | None => BrowserControlEndpointProbe::Inconclusive,
        }
    }
}

/// Existing cleanup callers need a conservative yes/no answer.
fn browser_control_endpoint_ready(session: &str, dir: &std::path::Path) -> bool {
    probe_browser_control_endpoint(session, dir) != BrowserControlEndpointProbe::Absent
}

/// A failed Linux close must retain its ownership record while any former
/// daemon namespace still has a PID marker or an unverified control endpoint.
#[cfg(target_os = "linux")]
fn linux_browser_close_namespaces_clear(session: &str) -> bool {
    linux_browser_close_namespaces_clear_in(session, &browser_runtime_dirs())
}

#[cfg(target_os = "linux")]
fn linux_browser_close_namespaces_clear_in(session: &str, roots: &[PathBuf]) -> bool {
    if validate_session_name(session).is_err() {
        return false;
    }
    !roots.is_empty() && roots.iter().all(|root| {
        let current = root.join(BROWSER_DAEMON_DIR);
        [root.as_path(), current.as_path()].iter().all(|dir| {
            let pid_path = dir.join(format!("{session}.pid"));
            let pid_absent = match std::fs::symlink_metadata(&pid_path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
                // A prefixed session can share this legacy root filename
                // with another session's structured Codemux record.
                Ok(_) if *dir == root.as_path() && read_browser_pid_record(&pid_path).is_some() => true,
                _ => false,
            };
            pid_absent && (!browser_socket_path_fits(dir, session)
                || probe_browser_control_endpoint(session, dir) == BrowserControlEndpointProbe::Absent)
        })
    })
}

#[cfg(windows)]
fn browser_control_port(session: &str, dir: &std::path::Path) -> u16 {
    let path = dir.join(format!("{session}.port"));
    std::fs::read_to_string(path)
        .ok()
        .and_then(|value| value.trim().parse::<u16>().ok())
        .unwrap_or_else(|| browser_control_fallback_port(session))
}

#[cfg(windows)]
fn browser_control_fallback_port(session: &str) -> u16 {
    // agent-browser v0.24.0 uses the session hash if its .port file is
    // absent, unreadable, or invalid.
    let hash = session.chars().fold(0i32, |hash, ch| {
        hash.wrapping_shl(5).wrapping_sub(hash).wrapping_add(ch as i32)
    });
    49152 + (hash.unsigned_abs() % 16383) as u16
}

#[cfg(windows)]
fn browser_control_listener_owned_by_pid(session: &str, dir: &std::path::Path, pid: u32) -> bool {
    listener_owned_by_pid(browser_control_port(session, dir), pid)
}

fn require_owned_browser_session(session: &str) -> Result<(), String> {
    require_owned_browser_session_for_cleanup(session)?;
    #[cfg(unix)]
    if let Some(pid) = read_agent_browser_daemon_pid(session) {
        if failed_stream_attempt_is_live(session, pid) {
            return Err(format!("Browser session {session} belongs to a failed stream launch"));
        }
    }
    Ok(())
}

fn require_owned_browser_session_for_cleanup(session: &str) -> Result<(), String> {
    validate_session_name(session)?;
    let dir = run_dir_for_session(session).ok_or("Cannot prepare private browser control directory")?;
    require_owned_browser_session_in(session, &dir)
}

fn require_owned_browser_session_in(session: &str, dir: &std::path::Path) -> Result<(), String> {
    let path = dir.join(format!("{session}.pid"));
    match std::fs::symlink_metadata(&path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if browser_control_endpoint_ready_in_namespace(session, dir) {
                return Err(format!("Browser session {session} has a live control endpoint without an owner PID"));
            }
            return Ok(());
        }
        Err(error) => return Err(format!("Cannot inspect browser session {session}: {error}")),
    }
    let pid = read_daemon_pid_file(&path)
        .ok_or_else(|| format!("Browser session {session} has an invalid daemon PID file"))?;
    if !pid_alive(pid) {
        if browser_control_endpoint_ready_in_namespace(session, dir) {
            return Err(format!("Browser session {session} has a live control endpoint with a stale owner PID"));
        }
        cleanup_stale_browser_lock_files_in(dir, session, pid);
        if read_daemon_pid_file(&path) == Some(pid) {
            return Err(format!("Could not remove stale browser session {session}"));
        }
        return Ok(());
    }
    if owned_browser_pid(session, pid).is_none() {
        let old_record = session_pid_record_in(browser_record_root_from_daemon_dir(dir), session)
            .map(|(_, record)| record);
        if let Some(old_record) = old_record.filter(|record| record.pid == pid) {
            if cleanup_reused_browser_lock_files_in(dir, session, &old_record) {
                return Ok(());
            }
        }
        return Err(format!("Browser session {session} is already used by another process"));
    }
    #[cfg(windows)]
    if !browser_control_listener_owned_by_pid(session, dir, pid) {
        return Err(format!("Browser session {session} has a control port not owned by its daemon"));
    }
    Ok(())
}

#[cfg(windows)]
fn verified_browser_control_dir(session: &str, record: &BrowserPidRecord) -> Option<PathBuf> {
    let dir = run_dir_for_session(session)?;
    let pid_path = dir.join(format!("{session}.pid"));
    (read_daemon_pid_file(&pid_path) == Some(record.pid)
        && browser_process_uses_control_dir(record, session, &dir))
        .then_some(dir)
}

fn stop_owned_browser_daemon(session: &str, record: &BrowserPidRecord, bin: &str) {
    if !browser_process_matches(record, session) {
        return;
    }
    #[cfg(target_os = "linux")]
    let pidfd = verified_browser_pidfd(record, session);
    #[cfg(target_os = "linux")]
    let dir = run_dir_for_session(session);
    #[cfg(target_os = "linux")]
    let mut files = None;
    // Capture the verified directory while its daemon is still alive. After
    // exit, namespace selection can switch from the legacy root to d/.
    #[cfg(windows)]
    let control_dir = verified_browser_control_dir(session, record);
    #[cfg(windows)]
    let legacy_files = control_dir.as_deref()
        .and_then(|dir| capture_legacy_windows_browser_files(dir, session, record));
    // Windows and Linux retain identity-checked process handles. A missing or
    // replaced PID file must not prevent cleanup of that process.
    #[cfg(all(not(windows), not(target_os = "linux")))]
    if read_agent_browser_daemon_pid(session) != Some(record.pid) {
        return;
    }
    // Windows retains a verified process handle. Linux and macOS close
    // through a verified connected peer; a CLI path lookup could reach a
    // replacement daemon after this PID check.
    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    let _ = bin;
    #[cfg(target_os = "linux")]
    {
        let deadline = Instant::now() + CLOSE_TIMEOUT;
        let result = dir.as_ref()
            .map(|dir| dir.join(format!("{session}.sock")))
            .ok_or_else(|| "Browser control path is unavailable".to_string())
            .and_then(|path| close_verified_unix_browser_peer(
                &path, record.pid, deadline, || browser_process_matches(record, session),
                || files = dir.as_ref().and_then(|dir| capture_closed_browser_files(dir, session, record)),
            ));
        if let Err(error) = result {
            eprintln!("[codemux::browser] Could not gracefully close browser session {session}: {error}");
        }
    }
    #[cfg(all(not(windows), not(target_os = "linux"), not(target_os = "macos")))]
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
        if let Some(files) = legacy_files.as_ref() {
            let _ = cleanup_captured_legacy_windows_browser_files(session, record, files, || {
                verified_browser_handle_exited(&handle)
            });
        } else if let Some(dir) = control_dir.as_deref() {
            // An incomplete legacy capture cannot authorize path-based
            // deletion after the process has exited.
            if dir.file_name() == Some(std::ffi::OsStr::new(BROWSER_DAEMON_DIR)) {
                cleanup_owned_browser_lock_files_in(dir, session, record.pid);
            }
        }
    }
    #[cfg(target_os = "linux")]
    if browser_process_matches(record, session) {
        if let Some(pidfd) = pidfd.as_ref() {
            stop_verified_browser_pidfd(pidfd, record, session);
        } else {
            eprintln!("[codemux::browser] Cannot safely signal browser session {session}: pidfd or process identity unavailable");
        }
    }
    #[cfg(target_os = "linux")]
    if let (Some(dir), Some(files), Some(pidfd)) = (dir.as_deref(), files.as_ref(), pidfd.as_ref()) {
        finish_closed_linux_browser_files(dir, session, record, files, pidfd);
    }
    #[cfg(all(not(windows), not(target_os = "linux")))]
    if browser_process_matches(record, session) {
        kill_verified_browser_pid(record, session);
    }
}

/// A full Unix listen backlog can block a normal connect indefinitely.
/// Resolve the control socket under the same deadline as the close request.
#[cfg(unix)]
fn connect_unix_browser_control_until(
    socket_path: &std::path::Path,
    deadline: Instant,
) -> std::io::Result<std::os::unix::net::UnixStream> {
    use std::os::fd::AsRawFd;

    if Instant::now() >= deadline {
        return Err(std::io::ErrorKind::TimedOut.into());
    }
    let socket = socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None)?;
    socket.set_nonblocking(true)?;
    let address = socket2::SockAddr::unix(socket_path)?;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        match socket.connect(&address) {
            Ok(()) => break,
            Err(error) if error.raw_os_error() == Some(libc::EISCONN) => break,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
                || error.kind() == std::io::ErrorKind::Interrupted
                || error.raw_os_error().is_some_and(|code| code == libc::EINPROGRESS || code == libc::EALREADY) => {}
            Err(error) => return Err(error),
        }
        let timeout_ms = remaining.as_millis().clamp(1, i32::MAX as u128) as i32;
        let mut ready = libc::pollfd {
            fd: socket.as_raw_fd(),
            events: libc::POLLOUT,
            revents: 0,
        };
        let result = unsafe { libc::poll(&mut ready, 1, timeout_ms) };
        if result == 0 {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if let Some(error) = socket.take_error()? {
            return Err(error);
        }
        match socket.peer_addr() {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotConnected => {
                // Linux can report POLLOUT|POLLHUP for a full Unix backlog
                // without completing connect. Retry until the deadline.
                std::thread::sleep(remaining.min(Duration::from_millis(1)));
            }
            Err(error) => return Err(error),
        }
    }
    socket.peer_addr()?;
    socket.set_nonblocking(false)?;
    Ok(socket.into())
}

/// Send close on the connection whose kernel-reported peer is the verified
/// daemon. Resolving the socket again through the CLI could reach a process
/// that replaced the listener after the ownership check.
#[cfg(all(unix, any(test, target_os = "linux", target_os = "macos")))]
fn close_verified_unix_browser_peer(
    socket_path: &std::path::Path,
    expected_pid: u32,
    deadline: Instant,
    still_owned: impl FnOnce() -> bool,
    on_verified_peer: impl FnOnce(),
) -> Result<(), String> {
    use std::io::{Read, Write};

    let mut stream = connect_unix_browser_control_until(socket_path, deadline)
        .map_err(|_| "Browser control socket is unavailable".to_string())?;
    if unix_browser_control_peer_pid(&stream).ok() != Some(expected_pid) || !still_owned() {
        return Err("Browser control peer changed during shutdown".into());
    }
    on_verified_peer();
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err("Browser close deadline elapsed".into());
    }
    stream.set_write_timeout(Some(remaining.min(PROC_CONTROL_TIMEOUT)))
        .map_err(|_| "Could not bound browser close request".to_string())?;
    let mut request = serde_json::json!({
        "id": uuid::Uuid::new_v4().simple().to_string(),
        "action": "close",
    }).to_string();
    request.push('\n');
    stream.write_all(request.as_bytes()).map_err(|_| "Browser close request failed".to_string())?;

    let mut response = Vec::new();
    let mut chunk = [0u8; 512];
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("Browser close response timed out".into());
        }
        stream.set_read_timeout(Some(remaining))
            .map_err(|_| "Could not bound browser close response".to_string())?;
        let read = stream.read(&mut chunk).map_err(|_| "Browser close response timed out".to_string())?;
        if read == 0 {
            return Err("Browser close response ended early".into());
        }
        let end = chunk[..read].iter().position(|byte| *byte == b'\n');
        response.extend_from_slice(&chunk[..end.map_or(read, |index| index + 1)]);
        if response.len() > 8 * 1024 {
            return Err("Browser close response was too large".into());
        }
        if end.is_some() {
            break;
        }
    }
    let result: serde_json::Value = serde_json::from_slice(&response)
        .map_err(|_| "Browser close response was invalid".to_string())?;
    if result.get("success").and_then(serde_json::Value::as_bool) != Some(true) {
        return Err("Browser daemon refused close".into());
    }
    Ok(())
}

#[cfg(all(unix, any(test, target_os = "linux", target_os = "macos")))]
fn unix_browser_control_peer_pid(stream: &std::os::unix::net::UnixStream) -> std::io::Result<u32> {
    use std::os::fd::AsRawFd;

    #[cfg(target_os = "macos")]
    {
        let mut pid: libc::pid_t = 0;
        let mut size = std::mem::size_of_val(&pid) as libc::socklen_t;
        let result = unsafe {
            libc::getsockopt(stream.as_raw_fd(), libc::SOL_LOCAL, libc::LOCAL_PEEREPID,
                (&mut pid as *mut libc::pid_t).cast(), &mut size)
        };
        if result == 0 && size as usize == std::mem::size_of_val(&pid) && pid > 0 {
            return Ok(pid as u32);
        }
    }
    #[cfg(target_os = "linux")]
    {
        let mut cred = std::mem::MaybeUninit::<libc::ucred>::zeroed();
        let mut size = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        let result = unsafe {
            libc::getsockopt(stream.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED,
                cred.as_mut_ptr().cast(), &mut size)
        };
        if result == 0 && size as usize == std::mem::size_of::<libc::ucred>() {
            let cred = unsafe { cred.assume_init() };
            if cred.pid > 0 {
                return Ok(cred.pid as u32);
            }
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let _ = stream;
    Err(std::io::Error::other("Browser control peer PID is unavailable"))
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
    if process_creation_ticks(raw)? != record.start_time
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
fn process_creation_ticks(handle: windows_sys::Win32::Foundation::HANDLE) -> Option<u64> {
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
    (ticks > 0).then_some(ticks)
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

#[cfg(windows)]
fn verified_browser_handle_exited(handle: &std::os::windows::io::OwnedHandle) -> bool {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
    use windows_sys::Win32::System::Threading::WaitForSingleObject;

    unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) == WAIT_OBJECT_0 }
}

#[cfg(target_os = "linux")]
fn verified_browser_pidfd(record: &BrowserPidRecord, session: &str) -> Option<std::os::fd::OwnedFd> {
    use std::os::fd::FromRawFd;

    let pid = i32::try_from(record.pid).ok()?;
    // Open the process handle before checking identity. If the process
    // exits after that check, the handle still targets the original
    // process; a later occupant of the numeric PID cannot be signalled.
    let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
    if raw < 0 {
        return None;
    }
    let pidfd = unsafe { std::os::fd::OwnedFd::from_raw_fd(raw as i32) };
    browser_process_matches(record, session).then_some(pidfd)
}

#[cfg(target_os = "linux")]
fn signal_browser_pidfd(pidfd: &std::os::fd::OwnedFd, signal: libc::c_int) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;

    let result = unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            pidfd.as_raw_fd(),
            signal,
            std::ptr::null::<libc::siginfo_t>(),
            0,
        )
    };
    if result < 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }
}

#[cfg(target_os = "linux")]
fn stop_verified_browser_pidfd(pidfd: &std::os::fd::OwnedFd, record: &BrowserPidRecord, session: &str) {
    let _ = signal_browser_pidfd(pidfd, libc::SIGTERM);
    std::thread::sleep(Duration::from_millis(200));
    if browser_process_matches(record, session) {
        let _ = signal_browser_pidfd(pidfd, libc::SIGKILL);
    }
    let started = Instant::now();
    while browser_process_matches(record, session) && started.elapsed() < Duration::from_secs(1) {
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(target_os = "linux")]
fn linux_browser_pidfd_exited(pidfd: &std::os::fd::OwnedFd) -> bool {
    use std::os::fd::AsRawFd;

    let mut descriptor = libc::pollfd { fd: pidfd.as_raw_fd(), events: libc::POLLIN, revents: 0 };
    (unsafe { libc::poll(&mut descriptor, 1, 0) }) == 1 && descriptor.revents & libc::POLLIN != 0
}

#[cfg(target_os = "linux")]
fn finish_closed_linux_browser_files(
    dir: &std::path::Path,
    session: &str,
    record: &BrowserPidRecord,
    files: &ClosedBrowserFiles,
    pidfd: &std::os::fd::OwnedFd,
) {
    let deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < deadline
        && (!linux_browser_pidfd_exited(pidfd) || browser_control_endpoint_ready(session, dir)) {
        std::thread::sleep(Duration::from_millis(25));
    }
    let _ = cleanup_closed_browser_files(dir, session, record, files, || linux_browser_pidfd_exited(pidfd));
}

#[cfg(not(windows))]
fn kill_verified_browser_pid(record: &BrowserPidRecord, session: &str) {
    #[cfg(target_os = "linux")]
    {
        let Some(pidfd) = verified_browser_pidfd(record, session) else {
            eprintln!("[codemux::browser] Cannot safely signal browser session {session}: pidfd or process identity unavailable");
            return;
        };
        stop_verified_browser_pidfd(&pidfd, record, session);
    }
    #[cfg(target_os = "macos")]
    close_verified_browser_pid_until(record, session, Instant::now() + CLOSE_TIMEOUT);
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        eprintln!("[codemux::browser] Cannot safely force-stop browser session {session} on this platform");
        let _ = record;
    }
}

#[cfg(target_os = "macos")]
fn close_verified_browser_pid_until(record: &BrowserPidRecord, session: &str, deadline: Instant) {
    let dir = run_dir_for_session(session);
    let mut files = None;
    let socket = dir.as_ref().map(|dir| dir.join(format!("{session}.sock")));
    let result = socket.ok_or_else(|| "Browser control path is unavailable".to_string())
        .and_then(|path| close_verified_unix_browser_peer(
            &path, record.pid, deadline, || browser_process_matches(record, session),
            || files = dir.as_ref().and_then(|dir| capture_closed_browser_files(dir, session, record)),
        ));
    if let Err(error) = result {
        eprintln!("[codemux::browser] Could not gracefully close browser session {session}: {error}");
        if let (Some(dir), Some(files)) = (dir.as_deref(), files.as_ref()) {
            let _ = cleanup_closed_browser_files(dir, session, record, files, || {
                macos_browser_process_exited(record)
            });
        }
        return;
    }
    if let (Some(dir), Some(files)) = (dir.as_deref(), files.as_ref()) {
        finish_closed_browser_files(dir, session, record, files, deadline);
    }
}

#[cfg(target_os = "macos")]
fn finish_closed_browser_files(
    dir: &std::path::Path,
    session: &str,
    record: &BrowserPidRecord,
    files: &ClosedBrowserFiles,
    deadline: Instant,
) {
    let grace = Instant::now() + Duration::from_secs(1);
    while !macos_browser_process_exited(record) && Instant::now() < grace && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(25));
    }
    let _ = cleanup_closed_browser_files(dir, session, record, files, || {
        macos_browser_process_exited(record)
    });
}

/// A successful close response precedes daemon exit. A recycled PID also
/// identifies a different process, while a zombie is already unable to serve
/// the old socket. A failed process query alone does not prove exit.
#[cfg(target_os = "macos")]
fn macos_browser_process_exited(record: &BrowserPidRecord) -> bool {
    let Ok(pid) = i32::try_from(record.pid) else { return false };
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    let read = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, info.as_mut_ptr().cast(), size) };
    if read == size {
        let info = unsafe { info.assume_init() };
        let start = info.pbi_start_tvsec.checked_mul(1_000_000)
            .and_then(|secs| secs.checked_add(info.pbi_start_tvusec));
        return start.is_some_and(|start| start != record.start_time)
            || (start == Some(record.start_time) && info.pbi_status == libc::SZOMB);
    }
    (unsafe { libc::kill(pid, 0) }) == -1
        && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

#[cfg(unix)]
#[derive(Clone, Copy, PartialEq, Eq)]
struct BrowserFileIdentity {
    device: u64,
    inode: u64,
    mode: u32,
}

#[cfg(unix)]
fn browser_file_identity(path: &std::path::Path) -> Option<BrowserFileIdentity> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(path).ok()?;
    Some(BrowserFileIdentity { device: meta.dev(), inode: meta.ino(), mode: meta.mode() })
}

#[cfg(unix)]
struct ClosedBrowserFiles {
    pid: BrowserFileIdentity,
    record_path: PathBuf,
    record: Option<BrowserFileIdentity>,
    stream: Option<BrowserFileIdentity>,
    socket: Option<BrowserFileIdentity>,
}

/// Record the exact files belonging to the connected daemon before it exits.
/// A later process may publish new files under the same session name.
#[cfg(unix)]
fn capture_closed_browser_files(
    dir: &std::path::Path,
    session: &str,
    record: &BrowserPidRecord,
) -> Option<ClosedBrowserFiles> {
    use std::os::unix::fs::FileTypeExt;

    if validate_session_name(session).is_err() {
        return None;
    }
    let pid_path = dir.join(format!("{session}.pid"));
    let record_path = session_pid_record_in(browser_record_root_from_daemon_dir(dir), session)
        .map(|(path, _)| path)
        .or_else(|| session_pid_path_in(browser_record_root_from_daemon_dir(dir), session))?;
    if read_daemon_pid_file(&pid_path) != Some(record.pid) {
        return None;
    }
    let pid = std::fs::symlink_metadata(&pid_path).ok()?;
    if !pid.file_type().is_file() {
        return None;
    }
    // Failed-open rollback can identify a late daemon from its process
    // marker before Codemux has written its own PID record. An existing
    // record must still match this exact daemon; a newly published record
    // must not be mistaken for the one observed before close.
    let owned = match std::fs::symlink_metadata(&record_path) {
        Ok(meta) if meta.file_type().is_file()
            && read_browser_pid_record(&record_path).as_ref() == Some(record) => {
                Some(browser_file_identity(&record_path)?)
            }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        _ => return None,
    };
    let stream_path = dir.join(format!("{session}.stream"));
    let socket_path = dir.join(format!("{session}.sock"));
    let stream = std::fs::symlink_metadata(&stream_path).ok()
        .filter(|meta| meta.file_type().is_file())
        .and_then(|_| browser_file_identity(&stream_path));
    let socket = std::fs::symlink_metadata(&socket_path).ok()
        .filter(|meta| meta.file_type().is_socket())
        .and_then(|_| browser_file_identity(&socket_path));
    Some(ClosedBrowserFiles {
        pid: browser_file_identity(&pid_path)?,
        record_path,
        record: owned,
        stream,
        socket,
    })
}

#[cfg(unix)]
fn optional_browser_file_matches(path: &std::path::Path, expected: Option<BrowserFileIdentity>) -> bool {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Ok(_) => browser_file_identity(path) == expected,
        Err(_) => false,
    }
}

#[cfg(unix)]
fn captured_browser_record_matches(
    path: &std::path::Path,
    record: &BrowserPidRecord,
    expected: Option<BrowserFileIdentity>,
) -> bool {
    match expected {
        Some(identity) => {
            read_browser_pid_record(path).as_ref() == Some(record)
                && browser_file_identity(path) == Some(identity)
        }
        None => matches!(std::fs::symlink_metadata(path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound),
    }
}

/// Retire the files only while the old daemon is confirmed exited, its
/// control endpoint is gone, and every remaining file is the one observed
/// before close. The Codemux record is removed last.
#[cfg(unix)]
fn cleanup_closed_browser_files(
    dir: &std::path::Path,
    session: &str,
    record: &BrowserPidRecord,
    files: &ClosedBrowserFiles,
    old_exited: impl Fn() -> bool,
) -> bool {
    if validate_session_name(session).is_err() {
        return false;
    }
    let legacy_record = files.record_path.parent() == Some(browser_record_root_from_daemon_dir(dir));
    let pid_path = dir.join(format!("{session}.pid"));
    let record_path = &files.record_path;
    let stream_path = dir.join(format!("{session}.stream"));
    let socket_path = dir.join(format!("{session}.sock"));
    let still_old = || {
        old_exited()
            && !browser_control_endpoint_ready(session, dir)
            && read_daemon_pid_file(&pid_path) == Some(record.pid)
            && browser_file_identity(&pid_path) == Some(files.pid)
            && captured_browser_record_matches(&record_path, record, files.record)
            && optional_browser_file_matches(&stream_path, files.stream)
            && optional_browser_file_matches(&socket_path, files.socket)
    };
    for (path, expected) in [
        (&stream_path, files.stream),
        (&socket_path, files.socket),
        (&pid_path, Some(files.pid)),
    ] {
        if !still_old() {
            return false;
        }
        if let Some(expected) = expected {
            if browser_file_identity(path) == Some(expected)
                && std::fs::remove_file(path).is_err_and(|error| error.kind() != std::io::ErrorKind::NotFound)
            {
                return false;
            }
        }
    }
    if !old_exited() || browser_control_endpoint_ready(session, dir)
        || [pid_path.as_path(), stream_path.as_path(), socket_path.as_path()]
            .iter().any(|path| !matches!(std::fs::symlink_metadata(path),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound))
        || !captured_browser_record_matches(&record_path, record, files.record)
    {
        return false;
    }
    legacy_record || files.record.is_none() || std::fs::remove_file(record_path).is_ok()
}

#[cfg(windows)]
fn kill_verified_browser_pid(record: &BrowserPidRecord, session: &str) {
    if let Some(handle) = open_verified_browser_handle(record, session) {
        kill_verified_browser_handle(&handle, record.pid);
    }
}

/// Write the tracked PID to our own private record directory. Callers launching
/// a daemon must handle a write failure
/// before sending the first command, or they could leave it untracked.
fn write_session_pid(session_name: &str, record: &BrowserPidRecord) -> std::io::Result<()> {
    let path = session_pid_path(session_name)
        .ok_or_else(|| std::io::Error::other("Cannot prepare private browser PID path"))?;
    write_browser_pid_record(&path, record)
}

fn write_browser_pid_record(path: &std::path::Path, record: &BrowserPidRecord) -> std::io::Result<()> {
    use std::io::Write;

    if record.owner_pid == 0 || record.owner_start_time == 0 || !record.native_start {
        return Err(std::io::Error::other("Browser owner process identity is missing"));
    }
    let content = format!(
        "{} {} {} {} {} v2\n",
        record.pid, record.start_time, record.token, record.owner_pid, record.owner_start_time
    );
    #[cfg(windows)]
    let result = (|| -> std::io::Result<()> {
        let mut file = match clack_private_fs::create_new_file(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                clack_private_fs::require_private_file(path)?;
                std::fs::OpenOptions::new().write(true).truncate(true).open(path)?
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
        staged.persist(path).map_err(|error| error.error)?;
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
        // Close or signal this exact process. A session-wide CLI `close`
        // could reach a replacement at the same control socket.
        kill_verified_browser_pid(&record, session);
    }
}

#[cfg(unix)]
fn failed_stream_attempts() -> &'static std::sync::Mutex<HashMap<String, HashSet<String>>> {
    static FAILED: OnceLock<std::sync::Mutex<HashMap<String, HashSet<String>>>> = OnceLock::new();
    FAILED.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

#[cfg(unix)]
fn mark_failed_stream_attempt(session: &str, attempt: &str) {
    let mut failed = failed_stream_attempts().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    failed.entry(session.to_owned()).or_default().insert(attempt.to_owned());
}

#[cfg(unix)]
fn clear_failed_stream_attempt(session: &str, attempt: &str) {
    let mut failed = failed_stream_attempts().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(attempts) = failed.get_mut(session) {
        attempts.remove(attempt);
        if attempts.is_empty() {
            failed.remove(session);
        }
    }
}

#[cfg(unix)]
fn failed_stream_attempt_is_live(session: &str, pid: u32) -> bool {
    let Some(attempt) = browser_process_open_attempt(pid) else { return false };
    failed_stream_attempts()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(session)
        .is_some_and(|attempts| attempts.contains(&attempt))
}

#[cfg(unix)]
fn browser_process_open_attempt(pid: u32) -> Option<String> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

    let process_id = Pid::from_u32(pid);
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[process_id]),
        true,
        ProcessRefreshKind::nothing().with_environ(UpdateKind::Always),
    );
    system.process(process_id)
        .and_then(|process| process.environ().iter().find_map(|entry| {
            entry.to_string_lossy().strip_prefix("CODEMUX_BROWSER_OPEN_ATTEMPT=").map(str::to_owned)
        }))
}

#[cfg(unix)]
fn browser_process_has_open_attempt(pid: u32, attempt: &str) -> bool {
    browser_process_open_attempt(pid).as_deref() == Some(attempt)
}

#[cfg(unix)]
fn browser_daemon_for_open_attempt(session: &str, attempt: &str) -> Option<BrowserPidRecord> {
    browser_daemons_for_open_attempts(session, &HashSet::from([attempt.to_owned()]))
        .into_iter()
        .next()
        .map(|(_, record)| record)
}

#[cfg(unix)]
fn browser_daemons_for_open_attempts(
    session: &str,
    attempts: &HashSet<String>,
) -> Vec<(String, BrowserPidRecord)> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

    if attempts.is_empty() {
        return Vec::new();
    }
    let Some((owner_pid, owner_start_time)) = current_app_process_identity() else { return Vec::new() };
    let owner = format!("CODEMUX_BROWSER_OWNER={}", browser_owner_token());
    let session_env = format!("CODEMUX_BROWSER_SESSION={session}");
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_exe(UpdateKind::Always)
            .with_environ(UpdateKind::Always),
    );
    system.processes().values().filter_map(|process| {
        let env = process.environ();
        if ![&owner, &session_env]
            .iter()
            .all(|expected| env.iter().any(|entry| entry.to_string_lossy() == expected.as_str()))
        {
            return None;
        }
        let attempt = env.iter().find_map(|entry| {
            let value = entry.to_string_lossy();
            value.strip_prefix("CODEMUX_BROWSER_OPEN_ATTEMPT=")
                .filter(|attempt| attempts.contains(*attempt))
                .map(str::to_owned)
        })?;
        let pid = process.pid().as_u32();
        let start_time = process_start_time(pid)?;
        let record = BrowserPidRecord {
            pid,
            start_time,
            token: browser_owner_token().to_owned(),
            owner_pid,
            owner_start_time,
            native_start: true,
        };
        browser_process_matches(&record, session).then_some((attempt, record))
    }).collect()
}

#[cfg(unix)]
fn rollback_failed_unix_stream_open(session: &str, before_open: Option<&BrowserPidRecord>, attempt: &str) {
    // If teardown cannot confirm exit, later commands must still refuse this
    // launch attempt. A fresh attempt has a different marker and remains usable.
    mark_failed_stream_attempt(session, attempt);
    #[cfg(not(target_os = "macos"))]
    rollback_failed_unix_open(session, before_open);
    #[cfg(target_os = "macos")]
    let _ = before_open;

    // The detached daemon can publish its PID after the launcher times out.
    // Finish rollback in the blocking launch worker before its session gate
    // opens. A queued command must not reuse this process while we may kill it.
    let started = Instant::now();
    let mut last_scan = Instant::now();
    let mut first_scan = true;
    while started.elapsed() < Duration::from_secs(5) {
        let from_pid = read_agent_browser_daemon_pid(session)
            .and_then(|pid| owned_browser_pid(session, pid))
            .filter(|record| record.token == browser_owner_token()
                && browser_process_has_open_attempt(record.pid, attempt));
        let from_process = if from_pid.is_none()
            && (first_scan || last_scan.elapsed() >= Duration::from_millis(250))
        {
            first_scan = false;
            last_scan = Instant::now();
            browser_daemon_for_open_attempt(session, attempt)
        } else {
            None
        };
        if let Some(record) = from_pid.or(from_process) {
            // The launch marker identifies this process without overwriting
            // a later attempt's PID record. On macOS, its control socket may
            // appear after the process, so keep trying a peer-bound close.
            #[cfg(target_os = "macos")]
            close_verified_browser_pid_until(&record, session, started + Duration::from_secs(5));
            #[cfg(not(target_os = "macos"))]
            kill_verified_browser_pid(&record, session);
            if !browser_process_matches(&record, session) {
                clear_failed_stream_attempt(session, attempt);
                return;
            }
            #[cfg(not(target_os = "macos"))]
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(unix)]
fn stop_failed_stream_daemons_on_close(session: &str) -> bool {
    let attempts: HashSet<String> = failed_stream_attempts()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(session)
        .cloned()
        .unwrap_or_default();
    for (attempt, record) in browser_daemons_for_open_attempts(session, &attempts) {
        kill_verified_browser_pid(&record, session);
        if !browser_process_matches(&record, session) {
            clear_failed_stream_attempt(session, &attempt);
        }
    }
    !attempts.is_empty()
}

#[cfg(not(windows))]
fn verify_unix_stream_after_open(
    session: &str,
    port: u16,
    before_open: Option<&BrowserPidRecord>,
    attempt: &str,
) -> Result<(bool, Option<BrowserPidRecord>), String> {
    let port_alive = probe_stream_port(port, 25, Duration::from_millis(200));
    let mut record = track_current_browser_daemon(session);
    if record.is_none() {
        std::thread::sleep(Duration::from_millis(100));
        record = track_current_browser_daemon(session);
    }
    let verified = match record {
        _ if !port_alive => Err(format!("Browser daemon did not open stream port {port}")),
        None => Err(format!("Could not verify ownership of browser session {session}")),
        Some(record) if !listener_owned_by_pid(port, record.pid) => {
            Err(format!("Browser stream port {port} is not owned by session {session}"))
        }
        Some(record) => Ok((true, Some(record))),
    };
    if verified.is_err() {
        rollback_failed_unix_stream_open(session, before_open, attempt);
    }
    verified
}

#[cfg(not(windows))]
fn output_capture_and_track_browser_daemon(
    command: std::process::Command,
    timeout: Duration,
    session: &str,
) -> std::io::Result<std::process::Output> {
    output_capture_and_track_browser_daemon_with_status(command, timeout, session)
        .map_err(|failure| failure.error)
}

#[cfg(not(windows))]
#[derive(Debug)]
struct UnixBrowserLaunchFailure {
    error: std::io::Error,
    may_have_launched: bool,
}

#[cfg(not(windows))]
fn output_capture_and_track_browser_daemon_with_status(
    mut command: std::process::Command,
    timeout: Duration,
    session: &str,
) -> Result<std::process::Output, UnixBrowserLaunchFailure> {
    command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut spawned = false;
    let outcome = spawn_wait_collect_with_child(command, timeout, |_| {
        spawned = true;
        Ok(())
    });
    // A first command may start the daemon before failing or timing out.
    // Persist its verified identity before the caller handles either result.
    // A failed Command::spawn could not have started the launcher.
    if spawned && track_current_browser_daemon(session).is_none() {
        track_late_browser_daemon(session);
    }
    outcome.map_err(|error| UnixBrowserLaunchFailure {
        error,
        may_have_launched: spawned,
    })
}

#[cfg(not(windows))]
fn browser_daemon_trackers() -> &'static std::sync::Mutex<HashMap<String, Instant>> {
    static TRACKERS: OnceLock<std::sync::Mutex<HashMap<String, Instant>>> = OnceLock::new();
    TRACKERS.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

#[cfg(not(windows))]
fn track_late_browser_daemon(session: &str) {
    let trackers = browser_daemon_trackers();
    let Ok(mut pending) = trackers.lock() else { return };
    let deadline = Instant::now() + Duration::from_secs(5);
    if let Some(existing) = pending.get_mut(session) {
        // A later action may start while an earlier screenshot's tracker is
        // still running. Give this action its own full publication window.
        *existing = deadline;
        return;
    }
    pending.insert(session.to_owned(), deadline);
    drop(pending);

    let session = session.to_owned();
    let session_for_tracker = session.clone();
    let result = std::thread::Builder::new()
        .name("browser-daemon-tracker".to_owned())
        .spawn(move || {
            loop {
                if track_current_browser_daemon(&session_for_tracker).is_some() {
                    break;
                }
                let Ok(mut pending) = trackers.lock() else { return };
                let Some(deadline) = pending.get(&session_for_tracker).copied() else { return };
                if Instant::now() >= deadline {
                    // Check and remove under one lock so a newer action
                    // cannot extend the deadline between those operations.
                    pending.remove(&session_for_tracker);
                    return;
                }
                drop(pending);
                std::thread::sleep(Duration::from_millis(50));
            }
            if let Ok(mut pending) = trackers.lock() {
                pending.remove(&session_for_tracker);
            }
        });
    if result.is_err() {
        // Without a tracker, the launch outcome is inconclusive. Keep the
        // marker so settlement times out and preserves provisional state.
        eprintln!("[codemux::browser] Could not start late daemon tracker for {session}");
    }
}

#[cfg(not(windows))]
async fn wait_for_late_browser_daemon_tracking(session: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(6);
    while Instant::now() < deadline {
        let pending = browser_daemon_trackers()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains_key(session);
        if !pending {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

fn previous_app_has_exited(record: &BrowserPidRecord) -> bool {
    if record.owner_pid == 0 || record.owner_start_time == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        let Ok(pid) = i32::try_from(record.owner_pid) else { return false };
        if unsafe { libc::kill(pid, 0) } == -1 {
            return std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
        }
        record.native_start && process_start_time(record.owner_pid)
            .is_some_and(|start_time| start_time != record.owner_start_time)
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::FromRawHandle;
        use windows_sys::Win32::Foundation::{GetLastError, ERROR_INVALID_PARAMETER, WAIT_OBJECT_0};
        use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE};

        let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE, 0, record.owner_pid) };
        if raw.is_null() {
            return unsafe { GetLastError() } == ERROR_INVALID_PARAMETER;
        }
        let _handle = unsafe { std::os::windows::io::OwnedHandle::from_raw_handle(raw) };
        (record.native_start && process_creation_ticks(raw).is_some_and(|start_time| start_time != record.owner_start_time))
            || unsafe { WaitForSingleObject(raw, 0) } == WAIT_OBJECT_0
    }
    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}

fn stop_previous_browser_daemon_before_stream(session: &str, bin: &str) -> Result<(), String> {
    require_owned_browser_session_for_cleanup(session)?;
    if let Some(pid) = read_agent_browser_daemon_pid(session) {
        if pid_alive(pid) {
            let record = owned_browser_pid(session, pid)
                .ok_or_else(|| format!("Browser session {session} is already used by another process"))?;
            if record.token != browser_owner_token() && !previous_app_has_exited(&record) {
                return Err(format!("Browser session {session} belongs to another running or unverified app instance"));
            }
            stop_owned_browser_daemon(session, &record, bin);
            if pid_alive(pid) {
                return Err(format!("Could not stop browser session {session}"));
            }
        }
    }
    Ok(())
}

fn live_owned_browser_record(session: &str) -> Option<BrowserPidRecord> {
    let pid = read_agent_browser_daemon_pid(session)?;
    owned_browser_pid(session, pid)
}

#[cfg(not(windows))]
fn provisional_browser_record_departed(record: &BrowserPidRecord) -> bool {
    process_exit_confirmed(record.pid)
        || (record.native_start
            && process_start_time(record.pid).is_some_and(|start| start != record.start_time)
            && process_looks_like_agent_browser(record.pid) == Some(false))
}

#[cfg(not(windows))]
fn retire_exited_provisional_browser_files(session: &str) {
    if let (Some(pid), Some(pid_path), Some(dir)) = (
        read_agent_browser_daemon_pid(session),
        agent_browser_daemon_pid_path(session),
        run_dir_for_session(session),
    ) {
        let recorded = session_pid_record(session).filter(|(_, record)| {
            record.pid == pid && (record.token == browser_owner_token() || previous_app_has_exited(record))
        });
        let departed = || {
            process_exit_confirmed(pid) || recorded.as_ref().is_some_and(|(path, record)| {
                read_browser_pid_record(path).as_ref() == Some(record)
                    && provisional_browser_record_departed(record)
            })
        };
        // Only the stale PID marker pins provisional state. Leave port,
        // engine, and stream files to their owner: another app instance may
        // be publishing a new launch before its PID or listener appears.
        if departed()
            && probe_browser_control_endpoint(session, &dir) == BrowserControlEndpointProbe::Absent
            && read_daemon_pid_file(&pid_path) == Some(pid)
            && departed()
            && probe_browser_control_endpoint(session, &dir) == BrowserControlEndpointProbe::Absent {
            let _ = std::fs::remove_file(pid_path);
        }
    }
    let Some(record_path) = session_pid_path(session) else { return };
    let Some(record) = read_browser_pid_record(&record_path) else { return };
    if !(record.token == browser_owner_token() || previous_app_has_exited(&record))
        || !provisional_browser_record_departed(&record) {
        return;
    }
    let Some(pid_path) = agent_browser_daemon_pid_path(session) else { return };
    let Some(dir) = run_dir_for_session(session) else { return };
    if matches!(std::fs::symlink_metadata(pid_path), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
        && probe_browser_control_endpoint(session, &dir) == BrowserControlEndpointProbe::Absent
        && read_browser_pid_record(&record_path).as_ref() == Some(&record)
        && provisional_browser_record_departed(&record) {
        let _ = std::fs::remove_file(record_path);
    }
}

fn shutdown_record_owned_by_this_app(session: &str, removed_pid: Option<u32>) -> Option<BrowserPidRecord> {
    let pid = removed_pid.or_else(|| read_agent_browser_daemon_pid(session))?;
    let record = owned_browser_pid(session, pid)?;
    (record.token == browser_owner_token()).then_some(record)
}

/// Remove our PID file once the daemon is gone. Best-effort.
fn clear_session_pid(session_name: &str) {
    // An old root record name can be another session's daemon PID file now.
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
    listener_ownership(port, pid) == Some(true)
}

/// `None` means the OS lookup failed; callers reusing a tracked browser must
/// not treat that as proof that another process took its listener.
fn listener_ownership(port: u16, pid: u32) -> Option<bool> {
    if pid == 0 {
        return Some(false);
    }
    #[cfg(target_os = "linux")]
    {
        let table = std::fs::read_to_string("/proc/net/tcp").ok()?;
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
            return Some(false);
        }
        let fds = std::fs::read_dir(format!("/proc/{pid}/fd")).ok()?;
        let mut unreadable = false;
        for fd in fds {
            let Ok(fd) = fd else { unreadable = true; continue };
            let Ok(target) = std::fs::read_link(fd.path()) else { unreadable = true; continue };
            let target = target.to_string_lossy();
            if target.strip_prefix("socket:[")
                .and_then(|inode| inode.strip_suffix(']'))
                .is_some_and(|inode| inodes.contains(inode)) {
                return Some(true);
            }
        }
        if unreadable { None } else { Some(false) }
    }
    #[cfg(target_os = "macos")]
    {
        let mut command = std::process::Command::new("lsof");
        command.args([
            "-nP", "-a", "-p", &pid.to_string(),
            &format!("-iTCP:{port}"), "-sTCP:LISTEN", "-F", "n",
        ]);
        let output = output_capture_with_timeout(command, PROC_CONTROL_TIMEOUT).ok()?;
        if output.status.success() {
            Some(String::from_utf8_lossy(&output.stdout)
                .lines()
                .any(|line| line == format!("n127.0.0.1:{port}")))
        } else if output.status.code() == Some(1) && output.stderr.is_empty() {
            Some(false)
        } else {
            None
        }
    }
    #[cfg(windows)]
    {
        Some(windows_loopback_listener_pid(port)? == Some(pid))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        let _ = port;
        None
    }
}

#[cfg(windows)]
fn windows_loopback_listener_pid(port: u16) -> Option<Option<u32>> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID, MIB_TCP_STATE_LISTEN,
    };
    use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};

    let ipv4 = windows_listener_table(u32::from(AF_INET))?;
    let ipv6 = windows_listener_table(u32::from(AF_INET6))?;
    let mut owner = None;
    for index in 0..windows_listener_row_count::<MIB_TCPROW_OWNER_PID>(&ipv4)? {
        let row = unsafe {
            std::ptr::read_unaligned(ipv4.as_ptr().add(4 + index * std::mem::size_of::<MIB_TCPROW_OWNER_PID>())
                .cast::<MIB_TCPROW_OWNER_PID>())
        };
        if row.dwState != MIB_TCP_STATE_LISTEN as u32
            || u16::from_be(row.dwLocalPort as u16) != port
            || ![0, u32::from_ne_bytes([127, 0, 0, 1])].contains(&row.dwLocalAddr)
        {
            continue;
        }
        if owner.is_some_and(|existing| existing != row.dwOwningPid) {
            return None;
        }
        owner = Some(row.dwOwningPid);
    }
    for index in 0..windows_listener_row_count::<MIB_TCP6ROW_OWNER_PID>(&ipv6)? {
        let row = unsafe {
            std::ptr::read_unaligned(ipv6.as_ptr().add(4 + index * std::mem::size_of::<MIB_TCP6ROW_OWNER_PID>())
                .cast::<MIB_TCP6ROW_OWNER_PID>())
        };
        let wildcard = row.ucLocalAddr.iter().all(|byte| *byte == 0);
        let mapped_v4 = row.ucLocalAddr[..10].iter().all(|byte| *byte == 0)
            && row.ucLocalAddr[10..12] == [0xff, 0xff]
            && (row.ucLocalAddr[12..16] == [127, 0, 0, 1]
                || row.ucLocalAddr[12..16] == [0, 0, 0, 0]);
        if row.dwState != MIB_TCP_STATE_LISTEN as u32
            || u16::from_be(row.dwLocalPort as u16) != port
            || !(wildcard || mapped_v4)
        {
            continue;
        }
        if owner.is_some_and(|existing| existing != row.dwOwningPid) {
            return None;
        }
        owner = Some(row.dwOwningPid);
    }
    Some(owner)
}

#[cfg(windows)]
fn windows_listener_table(family: u32) -> Option<Vec<u8>> {
    use windows_sys::Win32::NetworkManagement::IpHelper::{GetExtendedTcpTable, TCP_TABLE_OWNER_PID_LISTENER};

    let mut size = 0u32;
    unsafe {
        GetExtendedTcpTable(std::ptr::null_mut(), &mut size, 0, family, TCP_TABLE_OWNER_PID_LISTENER, 0);
    }
    if size < std::mem::size_of::<u32>() as u32 {
        return None;
    }
    let mut table = vec![0u8; size as usize];
    let status = unsafe {
        GetExtendedTcpTable(table.as_mut_ptr().cast(), &mut size, 0, family, TCP_TABLE_OWNER_PID_LISTENER, 0)
    };
    if status != 0 || size as usize > table.len() || size < std::mem::size_of::<u32>() as u32 {
        return None;
    }
    table.truncate(size as usize);
    Some(table)
}

#[cfg(windows)]
fn windows_listener_row_count<Row>(table: &[u8]) -> Option<usize> {
    if table.len() < std::mem::size_of::<u32>() {
        return None;
    }
    let entries = unsafe { std::ptr::read_unaligned(table.as_ptr().cast::<u32>()) } as usize;
    let available = (table.len() - std::mem::size_of::<u32>()) / std::mem::size_of::<Row>();
    (entries <= available).then_some(entries)
}

async fn inspect_existing_stream_identity_with<F>(
    lookup: F,
) -> Result<(Option<BrowserPidRecord>, Option<bool>), String>
where
    F: FnOnce() -> (Option<BrowserPidRecord>, Option<bool>) + Send + 'static,
{
    tokio::task::spawn_blocking(lookup)
        .await
        .map_err(|error| format!("Could not inspect browser stream identity: {error}"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BrowserAttachment {
    Attached,
    Unattached,
    Inconclusive,
}

async fn browser_stream_reports_attached_client(port: u16) -> Option<bool> {
    use futures_util::StreamExt;

    let response = async {
        let stream = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.ok()?;
        let url = format!("ws://127.0.0.1:{port}");
        let (mut socket, _) = tokio_tungstenite::client_async(&url, stream).await.ok()?;
        let message = socket.next().await?.ok()?;
        let text = message.to_text().ok()?;
        let status: serde_json::Value = serde_json::from_str(text).ok()?;
        (status.get("type")?.as_str()? == "status")
            .then(|| status.get("connected")?.as_bool())?
    };
    tokio::time::timeout(Duration::from_millis(800), response)
        .await
        .ok()
        .flatten()
}

async fn browser_stream_attached_to_daemon(port: u16, pid: u32, attempts: usize) -> BrowserAttachment {
    let owned = tokio::task::spawn_blocking(move || listener_owned_by_pid(port, pid))
        .await
        .unwrap_or(false);
    if !owned {
        return BrowserAttachment::Inconclusive;
    }
    let mut inconclusive = attempts == 0;
    for _ in 0..attempts {
        match browser_stream_reports_attached_client(port).await {
            Some(true) => {
                let still_owned = tokio::task::spawn_blocking(move || listener_owned_by_pid(port, pid))
                    .await
                    .unwrap_or(false);
                return if still_owned {
                    BrowserAttachment::Attached
                } else {
                    BrowserAttachment::Inconclusive
                };
            }
            Some(false) => {}
            None => inconclusive = true,
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    if inconclusive {
        BrowserAttachment::Inconclusive
    } else {
        BrowserAttachment::Unattached
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

/// Only a definite process exit permits releasing a tracked stream. A failed
/// ownership lookup alone can be transient while the browser remains alive.
#[cfg(unix)]
fn process_exit_confirmed(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return true;
    }
    (unsafe { libc::kill(pid as i32, 0) }) == -1
        && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

#[cfg(windows)]
fn process_exit_confirmed(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_INVALID_PARAMETER, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    if pid == 0 {
        return true;
    }
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return unsafe { GetLastError() } == ERROR_INVALID_PARAMETER;
    }
    let mut exit_code = 0;
    let checked = unsafe { GetExitCodeProcess(handle, &mut exit_code) } != 0;
    unsafe { CloseHandle(handle) };
    checked && exit_code != STILL_ACTIVE as u32
}

/// Per-session stream state. The session is keyed externally by the
/// session id (workspace_id, cli_session_name, or browser_id depending
/// on caller); inside we cache the things we actually need to reason
/// about lifecycle: the port, the daemon PID (when known), and a
/// freshness timestamp for the optional reaper.
struct StreamSession {
    /// Stream port allocated for this session.
    port: u16,
    /// Whether a daemon process is believed to be live. Commands may set
    /// this provisionally before the browser has attached.
    running: bool,
    /// A completed command or stream open indicates a browser may be ready.
    /// Stream URLs also require a connected status from this daemon.
    browser_ready: bool,
    /// The daemon has accepted an open request but is waiting for approval.
    /// A pane retry must not replace it while that request is pending.
    confirmation_pending: bool,
    /// PID of the spawned agent-browser daemon, when known. None when
    /// the session was adopted from a stale PID file or when the
    /// agent-browser CLI didn't write its lock file in time.
    pid: Option<u32>,
    /// Start time of that exact daemon. A recycled numeric PID is not the
    /// same browser, even if a process still exists at that PID.
    pid_start_time: Option<u64>,
    /// Last time we saw signs of life on this session (TCP probe success
    /// or first-frame timestamp passed in by the frontend). Used by the
    /// optional reaper background task.
    #[allow(dead_code)]
    last_seen_at: Option<Instant>,
}

#[cfg(test)]
fn stream_can_reuse_browser(stream: &StreamSession) -> bool {
    stream.running && stream.browser_ready
}

#[derive(Clone)]
pub struct AgentBrowserManager {
    /// Atomic counter for the next port to try allocating.
    next_port: Arc<AtomicU16>,
    /// Per-session state keyed by session identifier (workspace_id or cli_session_name).
    sessions: Arc<Mutex<HashMap<String, StreamSession>>>,
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
    // Waiting for the gate remains cancellable. Once acquired, transfer the
    // guard to a worker so cancellation cannot interrupt running cleanup.
    let lifecycle = browser_lifecycle_gate(&session).lock_owned().await;
    tokio::spawn(async move {
        let _lifecycle = lifecycle;
        work.await
    })
    .await
    .map_err(|error| format!("Browser lifecycle task failed: {error}"))?
}

fn validate_session_name(name: &str) -> Result<(), String> {
    // The name also becomes a PID/log filename. Keep it to one segment even
    // though Unix command arguments are quoted separately below.
    if name.is_empty()
        || name.len() > 64
        || name == "."
        || name == ".."
        || name.contains(['/', '\\', '\0'])
        || name.chars().any(char::is_control)
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
    // The fallback is two fixed argv words; every resolved path is one word.
    if binary == "npx agent-browser" {
        binary.to_string()
    } else {
        shell_quote(binary)
    }
}

#[cfg(not(target_os = "windows"))]
fn require_unix_browser_launcher(binary: &str) -> Result<(), String> {
    let program = if binary == "npx agent-browser" { "npx" } else { binary };
    which::which(program)
        .map(|_| ())
        .map_err(|_| "Browser launcher is unavailable".to_string())
}

#[cfg(not(windows))]
fn browser_action_command(
    binary: &str,
    argv: &[String],
    session: &str,
    stream_port: u16,
) -> Result<std::process::Command, String> {
    let mut command = if binary == "npx agent-browser" {
        let mut command = std::process::Command::new("npx");
        command.arg("agent-browser");
        command
    } else {
        std::process::Command::new(binary)
    };
    set_browser_owner_env(&mut command, session)?;
    command.args(argv)
        .env("AGENT_BROWSER_STREAM_PORT", stream_port.to_string())
        .env("AGENT_BROWSER_ARGS", STEALTH_CHROMIUM_ARGS)
        .env("AGENT_BROWSER_USER_AGENT", stealth_user_agent());
    Ok(command)
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

/// Resolve the path to the agent-browser native binary.
///
/// Search order (first match wins):
/// 1. System PATH — covers AUR `agent-browser` package and manual installs
/// 2. Tauri sidecar — bundled next to the executable in AppImage/deb/rpm
/// 3. node_modules — dev mode (`npm run tauri dev`)
/// 4. npx fallback on Unix. Windows control requires a native executable so
///    a timed-out command cannot leave a wrapper child using a retired relay.
#[cfg_attr(target_os = "windows", allow(unreachable_code))]
fn resolve_binary() -> String {
    // 1. System PATH (AUR/system package, cargo install, manual install).
    //
    // Resolve PATH in-process instead of spawning the `which` command.
    // Windows prefers its bundled sidecar before trying PATH below.
    #[cfg(unix)]
    {
        if let Ok(path) = which::which("agent-browser") {
            let path = path.to_string_lossy();
            // Skip the node_modules/.bin shim — we want the native binary directly.
            if !path.contains("node_modules/.bin") {
                return path.into_owned();
            }
        }
    }

    // Windows takes a different discovery path because the rest of this
    // function is shaped around the Linux/macOS sidecar naming convention
    // (`agent-browser-{triple}` next to the main exe) and the bash `which`
    // probe at the top. Specifically:
    //   1. Tauri's Windows bundler copies the staged sidecar
    //      `src-tauri/binaries/agent-browser-x86_64-pc-windows-msvc.exe` to
    //      `target/{debug,release}/agent-browser.exe` (drops the triple,
    //      keeps `.exe`) — so the runtime lookup must match THAT name.
    //   2. The original target_triple match below has no Windows branch and
    //      hard-returns `"npx agent-browser"`, which `Command::new("npx")`
    //      can't actually spawn because Windows only auto-appends `.exe` and
    //      `npx` ships as `npx.cmd` (per microsoft/CreateProcessW docs).
    //   3. The `which` crate (already a dep) honors PATHEXT and is the
    //      canonical Windows-safe way to probe for an executable on PATH.
    #[cfg(target_os = "windows")]
    {
        // (a) Tauri runtime convention — sidecar copied next to main exe.
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let candidate = dir.join("agent-browser.exe");
                if candidate.exists() {
                    return candidate.to_string_lossy().to_string();
                }
            }
        }
        // (b) Dev mode — npm-installed binary in node_modules.
        let npm_binary = "agent-browser-win32-x64.exe";
        let mut search_dirs: Vec<std::path::PathBuf> = Vec::new();
        if let Ok(cwd) = std::env::current_dir() {
            search_dirs.push(cwd.clone());
            if let Some(parent) = cwd.parent() {
                search_dirs.push(parent.to_path_buf());
            }
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                search_dirs.push(dir.to_path_buf());
            }
        }
        for base in &search_dirs {
            let candidate = base.join("node_modules/agent-browser/bin").join(npm_binary);
            if candidate.exists() {
                return candidate.to_string_lossy().to_string();
            }
        }
        // (c) Native install on PATH — `which` honors PATHEXT so this finds
        //     `agent-browser.exe`/`.cmd` without explicit suffix probing.
        if let Ok(path) = which::which("agent-browser") {
            return path.to_string_lossy().to_string();
        }
        // (d) Preserve the legacy fallback value for diagnostics. Verified
        //     Windows control rejects wrappers if no native binary was found.
        if let Ok(npx_path) = which::which("npx") {
            return format!("{} agent-browser", npx_path.to_string_lossy());
        }
        return "npx.cmd agent-browser".to_string();
    }

    // Determine the platform-specific sidecar name
    let target_triple = if cfg!(target_os = "linux") && cfg!(target_arch = "x86_64") {
        "x86_64-unknown-linux-gnu"
    } else if cfg!(target_os = "linux") && cfg!(target_arch = "aarch64") {
        "aarch64-unknown-linux-gnu"
    } else if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
        "aarch64-apple-darwin"
    } else if cfg!(target_os = "macos") && cfg!(target_arch = "x86_64") {
        "x86_64-apple-darwin"
    } else {
        return "npx agent-browser".to_string();
    };

    let npm_binary = if cfg!(target_os = "linux") && cfg!(target_arch = "x86_64") {
        "agent-browser-linux-x64"
    } else if cfg!(target_os = "linux") && cfg!(target_arch = "aarch64") {
        "agent-browser-linux-arm64"
    } else if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
        "agent-browser-darwin-arm64"
    } else if cfg!(target_os = "macos") && cfg!(target_arch = "x86_64") {
        "agent-browser-darwin-x64"
    } else {
        return "npx agent-browser".to_string();
    };

    let sidecar_name = format!("agent-browser-{target_triple}");

    // 2. Tauri sidecar — next to the executable (AppImage/deb/rpm installs)
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(&sidecar_name);
            if candidate.exists() {
                return candidate.to_string_lossy().to_string();
            }
        }
    }

    // 3. node_modules — dev mode (npm run tauri dev)
    // Tauri runs from src-tauri/ but node_modules is at the project root.
    // Check cwd, parent of cwd, and parent of exe for the npm binary.
    let mut search_dirs = vec![
        std::env::current_dir().unwrap_or_default(),
    ];
    // Parent of cwd (project root when cwd is src-tauri/)
    if let Ok(cwd) = std::env::current_dir() {
        if let Some(parent) = cwd.parent() {
            search_dirs.push(parent.to_path_buf());
        }
    }
    // Directory containing the executable
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            search_dirs.push(dir.to_path_buf());
        }
    }
    for base in &search_dirs {
        let candidate = base.join("node_modules/agent-browser/bin").join(npm_binary);
        if candidate.exists() {
            return candidate.to_string_lossy().to_string();
        }
    }

    // 4. npx fallback — spawns Node.js shim, requires npm + Node.js
    "npx agent-browser".to_string()
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

/// Argv-form sibling of `build_agent_browser_command`. Direct execution lets
/// Unix distinguish an exec failure from a launcher that actually ran.
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
            #[cfg(windows)]
            let mut open_argv: Vec<String> = windows_executable_path_args();
            #[cfg(not(windows))]
            let mut open_argv: Vec<String> = Vec::new();
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
struct NativeBrowserLaunchFailure {
    error: std::io::Error,
    // True only while this call might still have a live daemon or publish a
    // daemon PID after returning; a reaped direct daemon has no such path.
    may_have_launched: bool,
}

#[cfg(target_os = "windows")]
#[derive(Debug)]
struct NativeBrowserCommandOutput {
    output: std::process::Output,
    may_have_launched: bool,
}

#[cfg(target_os = "windows")]
fn browser_daemon_exit_confirmed(
    record: &BrowserPidRecord,
    retained: Option<&std::os::windows::io::OwnedHandle>,
) -> bool {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
    use windows_sys::Win32::System::Threading::WaitForSingleObject;

    if let Some(handle) = retained {
        return unsafe { WaitForSingleObject(handle.as_raw_handle(), 0) } == WAIT_OBJECT_0;
    }
    process_exit_confirmed(record.pid)
        || process_start_time(record.pid).is_some_and(|start| start != record.start_time)
}

#[cfg(target_os = "windows")]
impl NativeBrowserLaunchFailure {
    fn before_launch(error: std::io::Error) -> Self {
        Self { error, may_have_launched: false }
    }

    fn after_launch(error: std::io::Error) -> Self {
        Self { error, may_have_launched: true }
    }

    fn confirmed_stopped(error: std::io::Error) -> Self {
        Self { error, may_have_launched: false }
    }
}

#[cfg(target_os = "windows")]
fn ensure_owned_browser_daemon(bin: &str, session: &str, stream_port: u16, timeout: Duration) -> std::io::Result<(BrowserPidRecord, bool)> {
    ensure_owned_browser_daemon_with_status(bin, session, stream_port, timeout)
        .map_err(|failure| failure.error)
}

#[cfg(target_os = "windows")]
fn ensure_owned_browser_daemon_with_status(
    bin: &str,
    session: &str,
    stream_port: u16,
    timeout: Duration,
) -> Result<(BrowserPidRecord, bool), NativeBrowserLaunchFailure> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    // Serialize the missing-PID check and direct spawn across callers. A
    // second daemon for the same session would race on both lock files. The
    // lock wait consumes the caller's startup budget just like readiness.
    let started = Instant::now();
    let _start_guard = loop {
        if started.elapsed() >= timeout {
            return Err(NativeBrowserLaunchFailure::before_launch(std::io::Error::new(
                std::io::ErrorKind::TimedOut, "Browser daemon startup timed out",
            )));
        }
        match BROWSER_DAEMON_START_LOCK.try_lock() {
            Ok(guard) => break guard,
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return Err(NativeBrowserLaunchFailure::before_launch(std::io::Error::other(
                    "Browser daemon start lock was poisoned",
                )));
            }
            Err(std::sync::TryLockError::WouldBlock) => {
                std::thread::sleep(Duration::from_millis(10).min(timeout.saturating_sub(started.elapsed())));
            }
        }
    };
    require_owned_browser_session(session)
        .map_err(std::io::Error::other)
        .map_err(NativeBrowserLaunchFailure::before_launch)?;
    if let Some(pid) = read_agent_browser_daemon_pid(session) {
        return owned_browser_pid(session, pid)
            .map(|record| (record, false))
            .ok_or_else(|| NativeBrowserLaunchFailure::after_launch(
                std::io::Error::other("Browser daemon identity changed"),
            ));
    }

    // Starting daemon mode directly avoids sending the first `open` command
    // through the CLI's unauthenticated port lookup. The relay below handles
    // that command only after the daemon is ready and identified.
    let program = native_browser_program(bin).map_err(NativeBrowserLaunchFailure::before_launch)?;
    let mut cmd = std::process::Command::new(program);
    set_browser_owner_env(&mut cmd, session)
        .map_err(std::io::Error::other)
        .map_err(NativeBrowserLaunchFailure::before_launch)?;
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
    let mut child = cmd.spawn().map_err(NativeBrowserLaunchFailure::before_launch)?;
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
                // AGENT_BROWSER_DAEMON runs in this direct child. try_wait
                // reaped it, so no daemon can publish a PID after this call.
                return Err(NativeBrowserLaunchFailure::confirmed_stopped(std::io::Error::other(
                    format!("Browser daemon exited before control startup: {status}"),
                )));
            }
            Ok(None) => {}
            Err(error) => {
                let pid = child.id();
                let _ = child.kill();
                let reaped = child.wait().is_ok();
                if let Some(dir) = run_dir_for_session(session) {
                    cleanup_stale_browser_lock_files_in(&dir, session, pid);
                }
                return Err(if reaped {
                    NativeBrowserLaunchFailure::confirmed_stopped(error)
                } else {
                    NativeBrowserLaunchFailure::after_launch(error)
                });
            }
        }
        if started.elapsed() >= timeout {
            let pid = child.id();
            let _ = child.kill();
            let reaped = child.wait().is_ok();
            if let Some(dir) = run_dir_for_session(session) {
                cleanup_stale_browser_lock_files_in(&dir, session, pid);
            }
            let error = std::io::Error::new(
                std::io::ErrorKind::TimedOut, "Browser daemon control startup timed out",
            );
            return Err(if reaped {
                NativeBrowserLaunchFailure::confirmed_stopped(error)
            } else {
                NativeBrowserLaunchFailure::after_launch(error)
            });
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
    run_agent_browser_native_with_status(bin, argv, session, stream_port, discard_stderr, timeout)
        .map(|result| result.output)
        .map_err(|failure| failure.error)
}

#[cfg(target_os = "windows")]
fn run_agent_browser_native_with_status(
    bin: &str,
    argv: &[String],
    session: &str,
    stream_port: u16,
    discard_stderr: bool,
    timeout: Duration,
) -> Result<NativeBrowserCommandOutput, NativeBrowserLaunchFailure> {
    let started = Instant::now();
    let (record, started_here) = ensure_owned_browser_daemon_with_status(bin, session, stream_port, timeout)?;
    // Retain the verified process handle across command teardown so an exit
    // check cannot accidentally observe a later occupant of the numeric PID.
    let retained = open_verified_browser_handle(&record, session);
    track_browser_daemon_before_command(session, bin, &record, started_here)
        .map_err(|error| NativeBrowserLaunchFailure {
            error,
            may_have_launched: !browser_daemon_exit_confirmed(&record, retained.as_ref()),
        })?;
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
    finish_browser_command_with_status(result, session, bin, &record, started_here, retained.as_ref())
}

#[cfg(target_os = "windows")]
fn finish_browser_command_with_status(
    result: std::io::Result<std::process::Output>,
    session: &str,
    bin: &str,
    record: &BrowserPidRecord,
    started_here: bool,
    retained: Option<&std::os::windows::io::OwnedHandle>,
) -> Result<NativeBrowserCommandOutput, NativeBrowserLaunchFailure> {
    let result = finish_browser_command(result, session, bin, record, started_here);
    let may_have_launched = !browser_daemon_exit_confirmed(record, retained);
    result
        .map(|output| NativeBrowserCommandOutput { output, may_have_launched })
        .map_err(|error| NativeBrowserLaunchFailure { error, may_have_launched })
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
    let mut relay = VerifiedBrowserRelay::new(session, record, remaining)?;
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
        .env_remove("AGENT_BROWSER_DAEMON");
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BrowserOpenOutcome {
    Completed,
    ConfirmationPending,
}

fn stream_open_outcome(output: &std::process::Output) -> Result<BrowserOpenOutcome, String> {
    if !output.status.success() {
        return Err(format!("Browser stream open failed with status {}", output.status));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    if browser_confirmation_required(&stdout) {
        return Ok(BrowserOpenOutcome::ConfirmationPending);
    }
    if !browser_action_completed(true, &stdout) {
        return Err(format!("Browser stream open reported failure: {stdout}"));
    }
    Ok(BrowserOpenOutcome::Completed)
}

#[cfg(windows)]
fn require_successful_browser_stream_open(
    result: std::io::Result<std::process::Output>,
    log_file: Option<std::fs::File>,
) -> Result<BrowserOpenOutcome, String> {
    use std::io::Write;

    let output = result.map_err(|error| format!("Browser stream open failed: {error}"))?;
    if let Some(mut log) = log_file {
        let _ = log.write_all(&output.stderr);
    }
    stream_open_outcome(&output)
}

#[cfg(not(windows))]
fn require_successful_browser_stream_open_status(
    result: std::io::Result<std::process::Output>,
) -> Result<BrowserOpenOutcome, String> {
    let output = result.map_err(|error| format!("Browser stream open failed: {error}"))?;
    stream_open_outcome(&output)
}

#[cfg(windows)]
fn finish_browser_stream_open(
    result: std::io::Result<std::process::Output>,
    log_file: Option<std::fs::File>,
    session: &str,
    bin: &str,
    record: &BrowserPidRecord,
    started_here: bool,
) -> Result<BrowserOpenOutcome, String> {
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
    let port = browser_control_port(session, &dir);
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
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, MIB_TCP_STATE_ESTAB, TCP_TABLE_OWNER_PID_CONNECTIONS,
    };
    use windows_sys::Win32::Networking::WinSock::AF_INET;

    let (Ok(std::net::SocketAddr::V4(peer)), Ok(std::net::SocketAddr::V4(local))) =
        (stream.peer_addr(), stream.local_addr())
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
}

#[cfg(windows)]
impl Drop for RelayDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.port_file);
        let _ = std::fs::remove_dir(&self.path);
    }
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
    fn new(session: &str, record: BrowserPidRecord, timeout: Duration) -> std::io::Result<Self> {
        use std::io::{Read, Write};
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

        let base = run_dir_for_session(session).ok_or_else(|| std::io::Error::other("Cannot prepare private browser control directory"))?;
        let (cli_session, listener) = (0..32).find_map(|_| {
            let alias = format!("relay-{}", uuid::Uuid::new_v4().simple());
            let port = browser_control_fallback_port(&alias);
            bind_exclusive_browser_relay_listener(port).ok().map(|listener| (alias, listener))
        }).ok_or_else(|| std::io::Error::other("No free browser relay control port"))?;
        let path = base.join(format!("relay-{}", uuid::Uuid::new_v4().simple()));
        clack_private_fs::create_dir(&path)?;
        let directory = RelayDirectory {
            port_file: path.join(format!("{cli_session}.port")),
            path,
        };
        let directory_guard = clack_private_fs::require_private_directory(&directory.path)?;
        // Keep the port bound until the CLI has exited even if the worker
        // thread fails. The alias hash resolves to this same port if the CLI
        // cannot read its `.port` file.
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
        if process_creation_ticks(process.as_raw_handle()).is_none() {
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
fn build_agent_browser_command(session: &str, action: &str, params: &serde_json::Value) -> Result<String, String> {
    build_agent_browser_command_for_binary(&resolve_binary(), session, action, params)
}

#[cfg(not(target_os = "windows"))]
fn build_agent_browser_command_for_binary(
    resolved: &str,
    session: &str,
    action: &str,
    params: &serde_json::Value,
) -> Result<String, String> {
    require_unix_browser_launcher(resolved)?;
    let bin = shell_binary(resolved);
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

fn browser_action_has_usable_output(status_success: bool, stdout: &str) -> bool {
    status_success || stdout.contains("✓") || stdout.contains("{") || stdout.contains("- ")
}

fn browser_plain_confirmation(stdout: &str) -> bool {
    let mut lines = stdout.trim_end_matches(['\r', '\n']).lines();
    if lines.next() != Some("Confirmation required:") {
        return false;
    }
    let Some(description) = lines.next() else { return false };
    let Some(confirm_id) = lines.next().and_then(|line| line.strip_prefix("  Run: agent-browser confirm ")) else { return false };
    let Some(deny_id) = lines.next().and_then(|line| line.strip_prefix("  Or:  agent-browser deny ")) else { return false };
    description.starts_with("  ")
        && !description.trim().is_empty()
        && confirm_id == deny_id
        && lines.next().is_none()
}

fn browser_confirmation_required(stdout: &str) -> bool {
    if browser_plain_confirmation(stdout) {
        return true;
    }
    serde_json::from_str::<serde_json::Value>(stdout)
        .ok()
        .is_some_and(|value| {
            value.get("success").and_then(|value| value.as_bool()).is_some()
                && (value.pointer("/data/confirmation_required").and_then(|value| value.as_bool()) == Some(true)
                    || value.pointer("/data/result/data/confirmation_required").and_then(|value| value.as_bool()) == Some(true))
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BrowserActionOutcome {
    Completed,
    ConfirmationPending,
    Failed,
}

struct BrowserActionFailure {
    message: String,
    // A daemon may remain alive or publish its PID after this result.
    may_have_launched: bool,
}

impl BrowserActionFailure {
    fn before_launch(message: String) -> Self {
        Self { message, may_have_launched: false }
    }

    fn after_launch(message: String) -> Self {
        Self { message, may_have_launched: true }
    }
}

fn browser_action_outcome(status_success: bool, stdout: &str) -> BrowserActionOutcome {
    if browser_confirmation_required(stdout) {
        return BrowserActionOutcome::ConfirmationPending;
    }
    if !status_success {
        return BrowserActionOutcome::Failed;
    }
    let failed = serde_json::from_str::<serde_json::Value>(stdout)
        .ok()
        .is_some_and(|value| {
            value.get("success").and_then(|value| value.as_bool()) == Some(false)
        });
    if failed { BrowserActionOutcome::Failed } else { BrowserActionOutcome::Completed }
}

fn browser_action_completed(status_success: bool, stdout: &str) -> bool {
    browser_action_outcome(status_success, stdout) == BrowserActionOutcome::Completed
}

fn execute_agent_browser_action_with_status(
    browser_id: &str,
    action: &str,
    params: serde_json::Value,
    stream_port: u16,
) -> Result<(BrowserAutomationResult, BrowserActionOutcome), BrowserActionFailure> {
    let session = session_name(browser_id);
    validate_session_name(session).map_err(BrowserActionFailure::before_launch)?;
    require_owned_browser_session(session).map_err(BrowserActionFailure::before_launch)?;
    #[cfg(target_os = "macos")]
    let closing_files = (action == "close").then(|| {
        let dir = run_dir_for_session(session)?;
        let pid = read_agent_browser_daemon_pid(session)?;
        let record = owned_browser_pid(session, pid)?;
        let files = capture_closed_browser_files(&dir, session, &record)?;
        Some((dir, record, files))
    }).flatten();

    #[cfg(target_os = "windows")]
    let (stdout, stderr, status_success, may_have_launched) = {
        let bin = resolve_binary();
        let groups = build_agent_browser_argv_groups(session, action, &params)
            .map_err(BrowserActionFailure::before_launch)?;
        let mut combined_stdout = String::new();
        let mut combined_stderr = String::new();
        let mut all_ok = true;
        let mut previous_command_may_have_launched = false;
        for argv in &groups {
            let command = run_agent_browser_native_with_status(&bin, argv, session, stream_port, false, action_timeout(action))
                .map_err(|failure| BrowserActionFailure {
                    message: format!("Failed to run agent-browser: {}", failure.error),
                    may_have_launched: previous_command_may_have_launched || failure.may_have_launched,
                })?;
            previous_command_may_have_launched |= command.may_have_launched;
            let out = command.output;
            combined_stdout.push_str(&String::from_utf8_lossy(&out.stdout));
            combined_stderr.push_str(&String::from_utf8_lossy(&out.stderr));
            if !out.status.success() {
                all_ok = false;
                break; // matches `cmd1 && cmd2` short-circuit semantics
            }
        }
        (combined_stdout, combined_stderr, all_ok, previous_command_may_have_launched)
    };

    #[cfg(not(target_os = "windows"))]
    let (stdout, stderr, status_success, may_have_launched) = {
        let bin = resolve_binary();
        let groups = build_agent_browser_argv_groups(session, action, &params)
            .map_err(BrowserActionFailure::before_launch)?;
        let mut combined_stdout = String::new();
        let mut combined_stderr = String::new();
        let mut all_ok = true;
        let mut launched = false;
        for argv in &groups {
            let command = browser_action_command(&bin, argv, session, stream_port)
                .map_err(|message| BrowserActionFailure { message, may_have_launched: launched })?;
            let output = output_capture_and_track_browser_daemon_with_status(
                command, action_timeout(action), session,
            ).map_err(|failure| BrowserActionFailure {
                message: format!("Failed to run agent-browser: {}", failure.error),
                may_have_launched: launched || failure.may_have_launched,
            })?;
            launched = true;
            combined_stdout.push_str(&String::from_utf8_lossy(&output.stdout));
            combined_stderr.push_str(&String::from_utf8_lossy(&output.stderr));
            if !output.status.success() {
                all_ok = false;
                break;
            }
        }
        (combined_stdout, combined_stderr, all_ok, launched)
    };

    if (!may_have_launched && action != "close")
        || !browser_action_has_usable_output(status_success, &stdout)
    {
        return Err(BrowserActionFailure {
            message: format!("agent-browser failed: {} {}", stdout, stderr),
            may_have_launched,
        });
    }
    if action == "close" {
        #[cfg(target_os = "macos")]
        if let Some((dir, record, files)) = closing_files.as_ref() {
            finish_closed_browser_files(dir, session, record, files, Instant::now() + CLOSE_TIMEOUT);
        }
        #[cfg(not(target_os = "macos"))]
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
            let bin = resolve_binary();
            match build_agent_browser_argv_groups(session, "eval", &dom_params) {
                Ok(groups) if !groups.is_empty() => {
                    run_agent_browser_native(&bin, &groups[0], session, stream_port, false, ACTION_TIMEOUT).ok()
                }
                _ => None,
            }
        };
        #[cfg(not(target_os = "windows"))]
        let dom_output_opt = match build_agent_browser_command(session, "eval", &dom_params) {
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
                return Ok((BrowserAutomationResult {
                    request_id: make_request_id(),
                    browser_id: browser_id.to_string(),
                    data: serde_json::json!({ "tree": combined }),
                    message: None,
                }, browser_action_outcome(status_success, &stdout)));
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

    Ok((BrowserAutomationResult {
        request_id: make_request_id(),
        browser_id: browser_id.to_string(),
        data,
        message: None,
    }, browser_action_outcome(status_success, &stdout)))
}

fn execute_agent_browser_action(browser_id: &str, action: &str, params: serde_json::Value, stream_port: u16) -> Result<BrowserAutomationResult, String> {
    execute_agent_browser_action_with_status(browser_id, action, params, stream_port)
        .map(|(result, _)| result)
        .map_err(|error| error.message)
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

    /// Keep live records and discard dead records from the private namespace.
    /// Legacy root records remain in place because their names can be daemon
    /// PID files for another session. Recheck ownership before any later kill.
    fn reconcile_runtime_files() {
        for dir in browser_runtime_dirs() {
            for path in browser_pid_record_paths_in(&dir) {
                let Some(_session) = session_from_pid_path(&path) else { continue };
                let Some(record) = read_browser_pid_record(&path) else { continue };
                if path.parent() != Some(dir.as_path()) && !pid_alive(record.pid) {
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
                    browser_ready: false,
                    confirmation_pending: false,
                    pid: None,
                    pid_start_time: None,
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

    async fn clear_stale_running_stream_allocation(&self, session_key: &str, port: u16) {
        let mut sessions = self.sessions.lock().await;
        if sessions.get(session_key).is_some_and(|s| s.port == port) {
            sessions.remove(session_key);
        }
    }

    /// Reconcile a provisional `running` flag with the verified daemon after
    /// a first or failed command. The lifecycle gate remains held by callers.
    async fn refresh_running_session(&self, browser_id: &str, session: &str, outcome: BrowserActionOutcome) -> Result<bool, String> {
        let previous = self.sessions.lock().await.get(browser_id)
            .map(|entry| (entry.pid, entry.pid_start_time));
        let lookup_session = session.to_owned();
        // Socket connect and process inspection may wait on the OS. Keep
        // both off the async executor and outside the shared session map.
        let (record, confirmed_gone, endpoint_live) = tokio::task::spawn_blocking(move || {
            let record = live_owned_browser_record(&lookup_session);
            let confirmed_gone = previous.is_some_and(|(pid, start)| {
                pid.is_some_and(process_exit_confirmed)
                    || pid.zip(start).is_some_and(|(old_pid, old_start)| {
                        process_start_time(old_pid).is_some_and(|live_start| live_start != old_start)
                    })
            });
            let endpoint_live = record.is_none() && run_dir_for_session(&lookup_session)
                .map_or(true, |dir| browser_control_endpoint_ready(&lookup_session, &dir));
            (record, confirmed_gone, endpoint_live)
        })
            .await
            .map_err(|error| format!("Could not inspect browser session {session}: {error}"))?;
        let pid = record.as_ref().map(|record| record.pid);
        let mut ready = false;
        if let Some(entry) = self.sessions.lock().await.get_mut(browser_id) {
            if record.is_none() {
                if (!confirmed_gone || endpoint_live) && (entry.running || entry.browser_ready || entry.confirmation_pending
                    || outcome == BrowserActionOutcome::ConfirmationPending) {
                    entry.confirmation_pending |= outcome == BrowserActionOutcome::ConfirmationPending;
                    let detail = if entry.confirmation_pending { "pending browser daemon" } else { "browser daemon" };
                    return Err(format!("Could not verify {detail} for session {session}"));
                }
            }
            let replaced = record.as_ref().is_some_and(|record| {
                entry.pid.is_some_and(|old_pid| old_pid != record.pid)
                    || entry.pid_start_time.is_some_and(|old_start| old_start != record.start_time)
            });
            ready = pid.is_some() && (outcome == BrowserActionOutcome::Completed || (entry.browser_ready && !replaced));
            entry.running = pid.is_some();
            entry.browser_ready = ready;
            if outcome == BrowserActionOutcome::ConfirmationPending && pid.is_some() {
                entry.confirmation_pending = true;
            } else if ready || pid.is_none() || replaced {
                entry.confirmation_pending = false;
            }
            entry.pid = pid;
            entry.pid_start_time = record.as_ref().map(|record| record.start_time);
        }
        Ok(ready)
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
        #[cfg(not(windows))]
        self.settle_provisional_session(browser_id, &session, None).await;
        let port = self.allocate_port(browser_id).await?;

        let was_running = {
            let sessions = self.sessions.lock().await;
            sessions.get(browser_id).is_some_and(|s| s.running)
        };
        if was_running {
            let ready = self.refresh_running_session(browser_id, &session, BrowserActionOutcome::Failed).await?;
            let pending = self.sessions.lock().await.get(browser_id)
                .is_some_and(|entry| entry.running && entry.confirmation_pending);
            if pending {
                return Err(format!("Browser confirmation is pending for session {session}"));
            }
            if ready {
                return Ok(());
            }
        }

        // The launch shells out to agent-browser (which forks the daemon) and
        // can block on a real page load — run it on a blocking thread with a
        // hard timeout so it can never stall the async runtime (issue #96).
        let (record, outcome) = tokio::task::spawn_blocking(move || -> Result<(BrowserPidRecord, BrowserActionOutcome), String> {
            require_owned_browser_session(&session)?;
            let bin = resolve_binary();

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
                output_capture_and_track_browser_daemon(cmd, OPEN_TIMEOUT, &session)
                    .map_err(|e| format!("Failed to start agent-browser: {}", e))?
            };

            let outcome = browser_action_outcome(output.status.success(), &String::from_utf8_lossy(&output.stdout));
            if outcome == BrowserActionOutcome::Failed {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(format!("Failed to start browser: {} {}", String::from_utf8_lossy(&output.stdout), stderr));
            }
            track_current_browser_daemon(&session)
                .map(|record| (record, outcome))
                .ok_or_else(|| "Could not verify the started browser daemon".to_string())
        })
        .await
        .map_err(|e| format!("agent-browser spawn task failed: {e}"))??;

        self.sessions.lock().await.entry(browser_id.to_string()).and_modify(|s| {
            s.running = true;
            s.browser_ready = outcome == BrowserActionOutcome::Completed;
            s.confirmation_pending = outcome == BrowserActionOutcome::ConfirmationPending;
            s.pid = Some(record.pid);
            s.pid_start_time = Some(record.start_time);
        });
        if outcome == BrowserActionOutcome::ConfirmationPending {
            Err(format!("Browser confirmation is pending for session {}", session_name(browser_id)))
        } else {
            Ok(())
        }
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
                    std::mem::replace(&mut s.running, true)
                }
            }
            None => false,
        }
    }

    #[cfg(not(windows))]
    async fn prepare_unix_action(
        &self,
        browser_id: &str,
        session: &str,
        action: &str,
        params: &serde_json::Value,
        binary: &str,
    ) -> Result<bool, String> {
        require_unix_browser_launcher(binary)?;
        build_agent_browser_argv_groups(session, action, params)?;
        Ok(self.note_action_start(browser_id, action).await)
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
            s.browser_ready = false;
            s.confirmation_pending = false;
            s.pid = None;
            s.pid_start_time = None;
        }
    }

    async fn reconcile_action_result(
        &self,
        browser_id: &str,
        session: &str,
        action: &str,
        result: &Result<(BrowserAutomationResult, BrowserActionOutcome), BrowserActionFailure>,
    ) {
        // A JSON error may be returned as a payload after a failed exit, and
        // a confirmation response may exit successfully before the action.
        let outcome = result.as_ref().map_or(BrowserActionOutcome::Failed, |(_, outcome)| *outcome);
        if action == "close" {
            self.note_close_finished(browser_id).await;
        } else {
            let _ = self.refresh_running_session(browser_id, session, outcome).await;
        }
    }

    #[cfg(not(windows))]
    async fn settle_provisional_session(
        &self,
        browser_id: &str,
        session: &str,
        action_result: Option<(&str, &Result<(BrowserAutomationResult, BrowserActionOutcome), BrowserActionFailure>)>,
    ) {
        // A CLI can exit before its detached daemon publishes a PID. Hold
        // the per-session lifecycle gate through the late tracker, then
        // retire a provisional session only when both PID files and the
        // endpoint are absent. Spawn, command, and pane retries call this too.
        let provisional = self.sessions.lock().await.get(browser_id).is_some_and(|entry| {
            entry.running && entry.pid.is_none() && !entry.browser_ready && !entry.confirmation_pending
        });
        if !provisional || !wait_for_late_browser_daemon_tracking(session).await {
            return;
        }
        if let Some((action, result)) = action_result {
            self.reconcile_action_result(browser_id, session, action, result).await;
        }
        let lookup_session = session.to_owned();
        let confirmed_absent = tokio::task::spawn_blocking(move || {
            retire_exited_provisional_browser_files(&lookup_session);
            let file_absent = |path: Option<PathBuf>| path.is_some_and(|path| {
                matches!(std::fs::symlink_metadata(path), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
            });
            let Some(dir) = run_dir_for_session(&lookup_session) else { return false };
            let legacy_safe = browser_run_root_for_session(&lookup_session)
                .and_then(|root| legacy_session_pid_path_in(&root, &lookup_session)).is_some_and(|path| {
                match std::fs::symlink_metadata(&path) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
                    Ok(_) => read_browser_pid_record(&path)
                        .map_or_else(|| read_daemon_pid_file(&path).is_some(), |record| {
                            provisional_browser_record_departed(&record)
                        }),
                    Err(_) => false,
                }
            });
            live_owned_browser_record(&lookup_session).is_none()
                && file_absent(agent_browser_daemon_pid_path(&lookup_session))
                && file_absent(session_pid_path(&lookup_session))
                && legacy_safe
                && probe_browser_control_endpoint(&lookup_session, &dir) == BrowserControlEndpointProbe::Absent
        }).await.unwrap_or(false);
        if confirmed_absent {
            let mut sessions = self.sessions.lock().await;
            if sessions.get(browser_id).is_some_and(|entry| {
                entry.pid.is_none() && !entry.browser_ready && !entry.confirmation_pending
            }) {
                sessions.remove(browser_id);
            }
        }
    }

    pub async fn run_command(&self, browser_id: &str, action: &str, params: serde_json::Value) -> Result<BrowserAutomationResult, String> {
        let session = session_name(browser_id).to_string();
        validate_session_name(&session)?;
        let manager = self.clone();
        let browser_id = browser_id.to_owned();
        let action = action.to_owned();
        run_detached_browser_lifecycle(session, async move {
            manager.run_command_locked(
                &browser_id,
                &action,
                params,
                crate::browser_viewport::configured_default_viewport(),
            ).await
        }).await
    }

    async fn run_command_locked(
        &self,
        browser_id: &str,
        action: &str,
        params: serde_json::Value,
        default_viewport: Option<crate::browser_viewport::ViewportSpec>,
    ) -> Result<BrowserAutomationResult, String> {
        let session = session_name(browser_id).to_string();
        #[cfg(not(windows))]
        self.settle_provisional_session(browser_id, &session, None).await;
        let port = self.allocate_port(browser_id).await?;
        // Reject an invalid requested action before a configured viewport
        // could launch a daemon for a command that will never run.
        #[cfg(windows)]
        let was_running = {
            build_agent_browser_argv_groups(&session, action, &params)?;
            self.note_action_start(browser_id, action).await
        };
        #[cfg(not(windows))]
        let was_running = self.prepare_unix_action(
            browser_id, &session, action, &params, &resolve_binary(),
        ).await?;

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
        let mut viewport_may_have_launched = false;
        if !was_running && action != "viewport" && action != "close" {
            if let Some(spec) = default_viewport {
                let bid = browser_id.to_string();
                let vp_params = crate::browser_viewport::socket_action(spec);
                let viewport_result = tokio::task::spawn_blocking(move || {
                    execute_agent_browser_action_with_status(&bid, "viewport", vp_params, port)
                })
                .await;
                viewport_may_have_launched = match viewport_result {
                    Ok(Ok(_)) | Err(_) => true,
                    Ok(Err(error)) => error.may_have_launched,
                };
            }
        }

        // Run the (timeout-bounded) CLI work on a blocking thread so a slow
        // agent-browser round-trip never stalls the async executor.
        let owned_browser_id = browser_id.to_string();
        let owned_action = action.to_string();
        let result = tokio::task::spawn_blocking(move || {
            execute_agent_browser_action_with_status(&owned_browser_id, &owned_action, params, port)
        })
        .await
        .map_err(|e| BrowserActionFailure::after_launch(format!("agent-browser action task failed: {e}")))
        .and_then(|result| result);

        self.reconcile_action_attempt(
            browser_id, &session, action, was_running, viewport_may_have_launched, &result,
        ).await;

        result.map(|(result, _)| result).map_err(|error| error.message)
    }

    async fn reconcile_action_attempt(
        &self,
        browser_id: &str,
        session: &str,
        action: &str,
        was_running: bool,
        viewport_may_have_launched: bool,
        result: &Result<(BrowserAutomationResult, BrowserActionOutcome), BrowserActionFailure>,
    ) {
        // The agent-facing "close" goes through run_command (not close()),
        // so completed attempts must reset its state before the next open.
        let before_launch = result.as_ref().err().is_some_and(|error| !error.may_have_launched);
        if before_launch && !viewport_may_have_launched {
            if !was_running {
                let mut sessions = self.sessions.lock().await;
                if let Some(entry) = sessions.get_mut(browser_id) {
                    if entry.pid.is_none() && !entry.browser_ready && !entry.confirmation_pending {
                        entry.running = false;
                    }
                }
            }
        } else {
            self.reconcile_action_result(browser_id, session, action, result).await;
            #[cfg(not(windows))]
            if !was_running && action != "close" {
                self.settle_provisional_session(browser_id, session, Some((action, result))).await;
            }
        }
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
            let bin = resolve_binary();

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
                output_capture_and_track_browser_daemon(cmd, ACTION_TIMEOUT, &session)
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
    ///   3. Close the verified daemon using the platform's bound process or
    ///      control peer, with a Linux pidfd fallback when necessary.
    ///   4. Clean up our PID file in the private record directory.
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
        #[cfg(not(target_os = "macos"))]
        let had_session = removed.is_some();
        #[cfg(target_os = "macos")]
        let closing_files = if let Some(removed) = removed.as_ref() {
            let session = session.clone();
            let removed_pid = removed.pid;
            tokio::task::spawn_blocking(move || {
                let dir = run_dir_for_session(&session)?;
                let (_, record) = session_pid_record(&session)?;
                if record.token != browser_owner_token()
                    || removed_pid.is_some_and(|pid| pid != record.pid)
                {
                    return None;
                }
                let files = capture_closed_browser_files(&dir, &session, &record)?;
                Some((dir, record, files))
            })
            .await
            .ok()
            .flatten()
        } else {
            None
        };

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

        // Failed stream attempts keep their launch marker even after the
        // taken port allocation is released. Closing the pane retries
        // verified cleanup of any still-live daemon from those attempts.
        #[cfg(unix)]
        let had_failed_attempts = tokio::task::spawn_blocking({
            let session = session.clone();
            move || stop_failed_stream_daemons_on_close(&session)
        })
        .await
        .unwrap_or(false);
        #[cfg(not(unix))]
        let had_failed_attempts = false;

        // Keep the ownership record if verified teardown failed so the
        // process-exit sweep can retry stopping the same daemon.
        #[cfg(not(target_os = "macos"))]
        {
            if had_session || had_failed_attempts {
                let session = session.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    let owned_daemon_still_running = read_agent_browser_daemon_pid(&session)
                        .and_then(|pid| owned_browser_pid(&session, pid))
                        .is_some_and(|record| browser_process_matches(&record, &session));
                    #[cfg(target_os = "linux")]
                    let namespace_clear = linux_browser_close_namespaces_clear(&session);
                    #[cfg(not(target_os = "linux"))]
                    let namespace_clear = true;
                    if !owned_daemon_still_running && namespace_clear {
                        clear_session_pid(&session);
                    }
                })
                .await;
            }
        }
        // A daemon can exit between the first ownership check and the peer
        // close. Retire only the files captured before teardown, and keep
        // the record if a replacement endpoint or file appeared.
        #[cfg(target_os = "macos")]
        {
            let _ = had_failed_attempts;
            if let Some((dir, record, files)) = closing_files {
                let _ = tokio::task::spawn_blocking(move || {
                    cleanup_closed_browser_files(&dir, &session, &record, &files, || {
                        macos_browser_process_exited(&record)
                    })
                })
                .await;
            }
        }

        Ok(())
    }

    /// Best-effort teardown of the daemon behind a closed session. Linux
    /// uses CLI close with a pidfd fallback; macOS sends close to a verified
    /// control peer. This can block, so close runs it off the async executor.
    #[cfg(not(test))]
    fn shutdown_session_processes(session: &str, removed: Option<StreamSession>) {
        let Some(removed) = removed else { return };
        let Some(record) = shutdown_record_owned_by_this_app(session, removed.pid) else { return };
        stop_owned_browser_daemon(session, &record, &resolve_binary());
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
    ///   4. Probe the port and read its WebSocket status — only mark
    ///      `running = true` when its owned daemon reports an attached browser. The
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

    async fn note_stream_open_confirmation_pending(&self, browser_id: &str, port_alive: bool, record: Option<BrowserPidRecord>) {
        self.sessions.lock().await.entry(browser_id.to_string()).and_modify(|entry| {
            entry.running = port_alive;
            entry.browser_ready = false;
            entry.confirmation_pending = true;
            if let Some(record) = &record {
                entry.pid = Some(record.pid);
                entry.pid_start_time = Some(record.start_time);
            }
        });
    }

    async fn reconcile_existing_stream_attachment(
        &self,
        browser_id: &str,
        session: &str,
        attachment: BrowserAttachment,
    ) -> Result<bool, String> {
        match attachment {
            BrowserAttachment::Attached => {
                let mut sessions = self.sessions.lock().await;
                let stream = sessions.get_mut(browser_id)
                    .ok_or_else(|| format!("Browser session {session} closed during attachment check"))?;
                stream.running = true;
                stream.browser_ready = true;
                stream.confirmation_pending = false;
                Ok(true)
            }
            BrowserAttachment::Unattached => {
                if self.sessions.lock().await.get(browser_id).is_some_and(|stream| stream.confirmation_pending) {
                    Err(format!("Browser confirmation is pending for session {session}"))
                } else {
                    Ok(false)
                }
            }
            BrowserAttachment::Inconclusive => {
                Err(format!("Could not verify browser attachment for session {session}"))
            }
        }
    }

    async fn reconcile_existing_stream_listener(
        &self,
        browser_id: &str,
        session: &str,
        port: u16,
        ownership: Option<bool>,
    ) -> Result<(), String> {
        match ownership {
            Some(true) => Ok(()),
            Some(false) => {
                self.clear_stale_running_stream_allocation(browser_id, port).await;
                Err(format!("Browser stream port {port} is not owned by session {session}"))
            }
            None => Err(format!("Could not verify stream listener ownership for session {session}")),
        }
    }

    async fn reconcile_existing_stream_process(
        &self,
        browser_id: &str,
        session: &str,
        port: u16,
        expected_pid: u32,
        record: Option<BrowserPidRecord>,
        ownership: Option<bool>,
    ) -> Result<BrowserPidRecord, String> {
        let expected_start = self.sessions.lock().await.get(browser_id).and_then(|entry| entry.pid_start_time);
        let lookup_session = session.to_owned();
        let record_missing = record.is_none();
        // Process inspection and Unix socket connection can block. Do not
        // hold up the async executor while checking whether a session exited.
        let departed = tokio::task::spawn_blocking(move || -> Result<Option<&'static str>, String> {
            let reused = expected_start.is_some_and(|start| {
                process_start_time(expected_pid).is_some_and(|current| current != start)
            });
            if reused {
                let dir = run_dir_for_session(&lookup_session)
                    .ok_or_else(|| format!("Cannot inspect browser session {lookup_session}"))?;
                if probe_browser_control_endpoint(&lookup_session, &dir) != BrowserControlEndpointProbe::Absent {
                    return Err(format!("Could not verify replaced browser session {lookup_session} control endpoint"));
                }
                if read_daemon_pid_file(&dir.join(format!("{lookup_session}.pid"))) == Some(expected_pid) {
                    let old = session_pid_record_in(browser_record_root_from_daemon_dir(&dir), &lookup_session)
                        .map(|(_, record)| record)
                        .filter(|old| old.pid == expected_pid && Some(old.start_time) == expected_start)
                        .ok_or_else(|| format!("Could not retire replaced browser session {lookup_session}"))?;
                    if !cleanup_reused_browser_lock_files_in(&dir, &lookup_session, &old) {
                        return Err(format!("Could not retire replaced browser session {lookup_session}"));
                    }
                }
                return Ok(Some("daemon was replaced"));
            }
            if record_missing && process_exit_confirmed(expected_pid) {
                let probe = run_dir_for_session(&lookup_session)
                    .map_or(BrowserControlEndpointProbe::Inconclusive, |dir| {
                        probe_browser_control_endpoint(&lookup_session, &dir)
                    });
                if probe != BrowserControlEndpointProbe::Absent {
                    return Err(format!("Could not verify browser session {lookup_session} control endpoint"));
                }
                return Ok(Some("daemon exited"));
            }
            Ok(None)
        })
        .await
        .map_err(|error| format!("Could not inspect browser session {session}: {error}"))??;
        if let Some(reason) = departed {
            self.clear_stale_running_stream_allocation(browser_id, port).await;
            return Err(format!("Browser session {session} {reason}"));
        }
        if record.as_ref().is_some_and(|record| record.pid != expected_pid) {
            return Err(format!("Could not verify browser session {session} daemon identity"));
        }
        self.reconcile_existing_stream_listener(browser_id, session, port, ownership).await?;
        record.ok_or_else(|| format!("Could not verify ownership of browser session {session}"))
    }

    async fn start_stream_locked(&self, browser_id: &str) -> Result<String, String> {
        let session = session_name(browser_id).to_string();
        #[cfg(not(windows))]
        self.settle_provisional_session(browser_id, &session, None).await;

        // The per-session lifecycle gate serializes competing starts and
        // commands for this session. A manager-wide gate would make starts
        // for unrelated sessions wait after their callers were cancelled.
        // Re-check running after acquiring that session gate.
        // A tracked daemon might still have a live browser. Verify its
        // listener and attachment before deciding whether to reuse it.
        if let Some(port) = self.get_port(browser_id).await {
            let (tracked, mut ready_pid) = {
                let sessions = self.sessions.lock().await;
                sessions.get(browser_id).map_or((false, None), |s| {
                    (s.running || s.browser_ready || s.confirmation_pending, s.pid)
                })
            };
            if tracked && ready_pid.is_none() {
                let lookup_session = session.clone();
                let record = tokio::task::spawn_blocking(move || live_owned_browser_record(&lookup_session))
                    .await.ok().flatten()
                    .filter(|record| record.token == browser_owner_token());
                let Some(record) = record else {
                    return Err(format!("Could not verify ownership of browser session {session}"));
                };
                let mut sessions = self.sessions.lock().await;
                let entry = sessions.get_mut(browser_id)
                    .filter(|entry| entry.port == port)
                    .ok_or_else(|| format!("Browser session {session} closed during daemon lookup"))?;
                entry.pid = Some(record.pid);
                entry.pid_start_time = Some(record.start_time);
                ready_pid = Some(record.pid);
                if entry.confirmation_pending {
                    return Err(format!("Browser confirmation is pending for session {session}"));
                }
            }
            if let Some(pid) = ready_pid {
                let lookup_session = session.clone();
                let (owned, ownership) = inspect_existing_stream_identity_with(move || {
                    (owned_browser_pid(&lookup_session, pid), listener_ownership(port, pid))
                }).await?;
                let record = self.reconcile_existing_stream_process(
                    browser_id, &session, port, pid, owned, ownership,
                ).await?;
                if self.reconcile_existing_stream_attachment(
                    browser_id,
                    &session,
                    browser_stream_attached_to_daemon(port, record.pid, 3).await,
                ).await? {
                    return Ok(format!("ws://127.0.0.1:{port}"));
                }
            }
            // A dead socket or a live daemon without an attached browser
            // cannot satisfy the stream. Reopen it below.
            self.sessions
                .lock()
                .await
                .entry(browser_id.to_string())
                .and_modify(|s| {
                    s.running = false;
                    s.browser_ready = false;
                    s.confirmation_pending = false;
                    s.pid = None;
                    s.pid_start_time = None;
                });
        }

        // A previous daemon for this session must be stopped before we assign
        // a new stream port. Only a verified Codemux-owned daemon is eligible.
        let bin = {
            let session_for_close = session.clone();
            tokio::task::spawn_blocking(move || -> Result<String, String> {
                let bin = resolve_binary();
                stop_previous_browser_daemon_before_stream(&session_for_close, &bin)?;
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
        let session_after_launch = session.clone();
        let launch_result = tokio::task::spawn_blocking(move || -> Result<(bool, Option<BrowserPidRecord>, BrowserOpenOutcome), String> {
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
            let open_outcome = {
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
                )?
            };
            #[cfg(not(target_os = "windows"))]
            {
                let before_open = track_current_browser_daemon(&session);
                let attempt = uuid::Uuid::new_v4().simple().to_string();
                let mut cmd = browser_open_command(&bin, &session, port)?;
                cmd.env("CODEMUX_BROWSER_OPEN_ATTEMPT", &attempt);
                if let Some(log) = log_file {
                    cmd.stderr(log);
                }
                let opened = require_successful_browser_stream_open_status(
                    launch_status_with_timeout(cmd, OPEN_TIMEOUT),
                );
                let open_outcome = match opened {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        rollback_failed_unix_stream_open(&session, before_open.as_ref(), &attempt);
                        return Err(error);
                    }
                };
                let (port_alive, record) = verify_unix_stream_after_open(&session, port, before_open.as_ref(), &attempt)?;
                return Ok((port_alive, record, open_outcome));
            }

            #[cfg(windows)]
            {
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

            Ok((port_alive, Some(record), open_outcome))
            }
        })
        .await
        .map_err(|e| format!("agent-browser launch task failed: {e}"))
        .and_then(|result| result);
        let (port_alive, record, open_outcome) = match launch_result {
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
                        entry.browser_ready = false;
                        entry.confirmation_pending = false;
                        entry.pid = Some(record.pid);
                        entry.pid_start_time = Some(record.start_time);
                    });
                    return Err(error);
                }
                // The next attempt must choose a fresh port if another
                // process claimed this allocation before the launch.
                self.clear_failed_stream_allocation(browser_id, port).await;
                return Err(error);
            }
        };

        if open_outcome == BrowserOpenOutcome::ConfirmationPending {
            self.note_stream_open_confirmation_pending(browser_id, port_alive, record).await;
            return Err(format!("Browser confirmation is pending for session {session_after_launch}"));
        }

        let attachment = if let Some(record) = &record {
            browser_stream_attached_to_daemon(port, record.pid, 20).await
        } else {
            BrowserAttachment::Inconclusive
        };
        if attachment != BrowserAttachment::Attached {
            // Keep the verified PID with its allocation so pane close can
            // retry attachment or teardown, even if the CLI exited first.
            self.sessions.lock().await.entry(browser_id.to_string()).and_modify(|entry| {
                entry.running = false;
                entry.browser_ready = false;
                entry.confirmation_pending = false;
                entry.pid = record.as_ref().map(|record| record.pid);
                entry.pid_start_time = record.as_ref().map(|record| record.start_time);
            });
            return Err(format!("Browser session {session_after_launch} has no verified attached browser ({attachment:?})"));
        }

        let now = Some(Instant::now());
        self.sessions
            .lock()
            .await
            .entry(browser_id.to_string())
            .and_modify(|s| {
                s.running = port_alive;
                s.browser_ready = port_alive;
                s.confirmation_pending = false;
                s.pid = record.as_ref().map(|record| record.pid);
                s.pid_start_time = record.as_ref().map(|record| record.start_time);
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

    #[cfg(target_os = "linux")]
    fn saturated_control_socket(path: &std::path::Path) -> (socket2::Socket, Vec<socket2::Socket>) {
        let address = socket2::SockAddr::unix(path).unwrap();
        let listener = socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None).unwrap();
        listener.bind(&address).unwrap();
        listener.listen(1).unwrap();
        let mut clients = Vec::new();
        for _ in 0..32 {
            let client = socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None).unwrap();
            client.set_nonblocking(true).unwrap();
            match client.connect(&address) {
                Ok(()) => clients.push(client),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    clients.push(client);
                    return (listener, clients);
                }
                Err(error) => panic!("Could not fill browser control backlog: {error}"),
            }
        }
        panic!("Browser control backlog did not fill");
    }

    #[cfg(unix)]
    #[test]
    fn control_endpoint_probe_distinguishes_absence_from_inspection_error() {
        let dir = tempfile::tempdir().unwrap();
        let session = "browser-control-probe";
        assert_eq!(probe_browser_control_endpoint(session, dir.path()), BrowserControlEndpointProbe::Absent);
        assert!(!browser_control_endpoint_ready(session, dir.path()));

        let socket_path = dir.path().join(format!("{session}.sock"));
        let listener = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
        assert_eq!(probe_browser_control_endpoint(session, dir.path()), BrowserControlEndpointProbe::Reachable);
        drop(listener);
        std::fs::remove_file(socket_path).unwrap();

        let overlong_dir = dir.path().join("x".repeat(120));
        assert_eq!(probe_browser_control_endpoint(session, &overlong_dir), BrowserControlEndpointProbe::Inconclusive);
        assert!(browser_control_endpoint_ready(session, &overlong_dir));
    }

    #[cfg(windows)]
    #[test]
    fn control_endpoint_probe_distinguishes_reachable_from_absent_port() {
        let dir = tempfile::tempdir().unwrap();
        let session = "browser-control-probe";
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::fs::write(dir.path().join(format!("{session}.port")), port.to_string()).unwrap();
        assert_eq!(probe_browser_control_endpoint(session, dir.path()), BrowserControlEndpointProbe::Reachable);
        drop(listener);
        assert_eq!(probe_browser_control_endpoint(session, dir.path()), BrowserControlEndpointProbe::Absent);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn full_control_socket_backlog_is_bounded_and_inconclusive() {
        let dir = tempfile::tempdir().unwrap();
        let session = "browser-control-backlog";
        let socket_path = dir.path().join(format!("{session}.sock"));
        let (_listener, _clients) = saturated_control_socket(&socket_path);
        let start = Instant::now();
        assert_eq!(probe_browser_control_endpoint(session, dir.path()), BrowserControlEndpointProbe::Inconclusive);
        assert!(start.elapsed() < Duration::from_secs(1));
        assert!(browser_control_endpoint_ready(session, dir.path()));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn unix_control_connect_recovers_when_backlog_drains() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("browser-control-drain.sock");
        let (listener, _clients) = saturated_control_socket(&socket_path);
        let accept_listener = listener.try_clone().unwrap();
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            accept_listener.accept().unwrap().0
        });
        let connected = connect_unix_browser_control_until(&socket_path, Instant::now() + Duration::from_secs(1));
        assert!(connected.is_ok(), "{connected:?}");
        let _accepted = releaser.join().unwrap();
    }

    #[cfg(target_os = "linux")]
    #[tokio::test(flavor = "current_thread")]
    async fn stalled_control_probe_keeps_session_map_and_executor_available() {
        let manager = std::sync::Arc::new(AgentBrowserManager::new());
        let session = format!("browser-probe-lock-{}", uuid::Uuid::new_v4().simple());
        let dir = run_dir_for_session(&session).unwrap();
        let socket_path = dir.join(format!("{session}.sock"));
        let (_listener, _clients) = saturated_control_socket(&socket_path);
        manager.allocate_port(&session).await.unwrap();
        manager.sessions.lock().await.get_mut(&session).unwrap().running = true;

        let start = Instant::now();
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let manager = manager.clone();
            let session = session.clone();
            tasks.push(tokio::spawn(async move {
                manager.refresh_running_session(&session, &session, BrowserActionOutcome::Failed).await
            }));
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(start.elapsed() < Duration::from_millis(250), "Control probes blocked the async executor");
        let guard = tokio::time::timeout(Duration::from_millis(250), manager.sessions.lock()).await.unwrap();
        assert!(guard.get(&session).unwrap().running);
        drop(guard);
        for task in tasks {
            assert!(task.await.unwrap().is_err());
        }
        std::fs::remove_file(socket_path).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[tokio::test(flavor = "current_thread")]
    async fn pane_close_with_saturated_legacy_sockets_keeps_executor_available() {
        let manager = std::sync::Arc::new(AgentBrowserManager::new());
        let mut sockets = Vec::new();
        let mut sessions = Vec::new();
        for _ in 0..6 {
            let session = format!("browser-close-probe-{}", uuid::Uuid::new_v4().simple());
            manager.allocate_port(&session).await.unwrap();
            let root = browser_run_root_for_session(&session).unwrap();
            let socket_path = root.join(format!("{session}.sock"));
            let (listener, clients) = saturated_control_socket(&socket_path);
            sockets.push((socket_path, listener, clients));
            sessions.push(session);
        }
        let mut closing = Vec::new();
        for session in sessions {
            let manager = manager.clone();
            closing.push(tokio::spawn(async move { manager.close(&session).await }));
        }

        let started = Instant::now();
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(started.elapsed() < Duration::from_millis(200), "Pane close probes blocked the async executor");
        for task in closing {
            task.await.unwrap().unwrap();
        }
        for (socket_path, listener, clients) in sockets {
            drop(clients);
            drop(listener);
            std::fs::remove_file(socket_path).unwrap();
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn slow_stream_identity_lookup_keeps_executor_available() {
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let started = Instant::now();
        let lookup = tokio::spawn(async move {
            inspect_existing_stream_identity_with(move || {
                let _ = entered_tx.send(());
                std::thread::sleep(Duration::from_millis(600));
                (None, None)
            }).await
        });
        entered_rx.await.unwrap();
        assert!(started.elapsed() < Duration::from_millis(300), "stream identity lookup blocked the async executor");
        tokio::time::timeout(Duration::from_millis(300), tokio::time::sleep(Duration::from_millis(10)))
            .await.unwrap();
        assert!(matches!(lookup.await.unwrap(), Ok((None, None))));
    }

    fn test_browser_record(pid: u32, start_time: u64, token: String) -> BrowserPidRecord {
        let (owner_pid, owner_start_time) = current_app_process_identity().unwrap();
        BrowserPidRecord { pid, start_time, token, owner_pid, owner_start_time, native_start: true }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn different_native_start_in_same_second(start: u64) -> u64 {
        #[cfg(target_os = "linux")]
        let per_second = u64::try_from(unsafe { libc::sysconf(libc::_SC_CLK_TCK) }).unwrap();
        #[cfg(target_os = "macos")]
        let per_second = 1_000_000;
        assert!(per_second > 1);
        let other = if start % per_second + 1 < per_second { start + 1 } else { start - 1 };
        assert_eq!(other / per_second, start / per_second);
        other
    }

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
        let pid_file = TestBrowserPidFile::unowned(&session);
        let mgr = AgentBrowserManager::new();
        assert!(mgr.run_command(&session, "viewport", serde_json::json!({})).await.is_err());
        // The ownership check failed before launch, so its provisional flag
        // must not block a later attempt after the conflicting PID is gone.
        assert!(!mgr.sessions.lock().await.get(&session).unwrap().running);
        let spawn = mgr.spawn(&session).await;
        assert!(spawn.as_ref().is_err_and(|error| error.contains("already used by another process")), "{spawn:?}");
        drop(pid_file);
        assert!(require_owned_browser_session(&session).is_ok());
        mgr.allocate_port(&session).await.unwrap();
        assert!(!mgr.note_action_start(&session, "viewport").await);
    }

    #[tokio::test]
    async fn invalid_first_action_clears_its_prelaunch_flag() {
        let manager = AgentBrowserManager::new();
        let session = format!("browser-invalid-first-{}", uuid::Uuid::new_v4().simple());
        let error = manager.run_command_locked(
            &session,
            "not_an_action",
            serde_json::json!({}),
            Some(crate::browser_viewport::ViewportSpec::new(1920, 1080, 1.0)),
        ).await.unwrap_err();
        assert!(error.contains("Unknown action"), "{error}");
        let entry = manager.sessions.lock().await;
        assert!(!entry.get(&session).unwrap().running);
    }

    #[tokio::test]
    async fn failed_default_viewport_and_action_do_not_mark_session_running() {
        let manager = AgentBrowserManager::new();
        let session = format!("browser-failed-viewport-{}", uuid::Uuid::new_v4().simple());
        let _pid_file = TestBrowserPidFile::unowned(&session);
        let error = manager.run_command_locked(
            &session,
            "open",
            serde_json::json!({"url": "about:blank"}),
            Some(crate::browser_viewport::ViewportSpec::new(1920, 1080, 1.0)),
        ).await.unwrap_err();
        assert!(error.contains("already used by another process"), "{error}");
        let entry = manager.sessions.lock().await;
        assert!(!entry.get(&session).unwrap().running);
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

    #[tokio::test]
    async fn cancelled_queued_lifecycle_request_never_runs() {
        let session = format!("browser-queued-cancel-{}", uuid::Uuid::new_v4().simple());
        let held = browser_lifecycle_gate(&session).lock_owned().await;
        let (ran, ran_rx) = tokio::sync::oneshot::channel();
        let caller = tokio::spawn(run_detached_browser_lifecycle(session, async move {
            let _ = ran.send(());
            Ok(())
        }));
        tokio::task::yield_now().await;
        assert!(!caller.is_finished());
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        drop(held);
        assert!(ran_rx.await.is_err(), "cancelled queued command ran after the gate opened");
    }

    #[cfg(unix)]
    #[test]
    fn failed_open_never_rolls_back_a_foreign_or_preexisting_daemon() {
        let own = test_browser_record(10, 100, browser_owner_token().to_owned());
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
    #[tokio::test]
    async fn failed_windows_daemon_spawn_allows_next_start_attempt() {
        let manager = AgentBrowserManager::new();
        let session = format!("browser-spawn-retry-{}", uuid::Uuid::new_v4().simple());
        let port = manager.allocate_port(&session).await.unwrap();
        let was_running = manager.note_action_start(&session, "open").await;
        assert!(!was_running);

        let missing = std::env::temp_dir().join(format!("missing-agent-browser-{}.exe", uuid::Uuid::new_v4().simple()));
        assert!(!missing.exists());
        let failure = run_agent_browser_native_with_status(
            missing.to_str().unwrap(),
            &["open".into(), "about:blank".into(), "--session".into(), session.clone()],
            &session,
            port,
            false,
            Duration::from_secs(5),
        ).unwrap_err();
        assert_eq!(failure.error.kind(), std::io::ErrorKind::NotFound);
        assert!(!failure.may_have_launched);
        let result = Err(BrowserActionFailure::before_launch(failure.error.to_string()));
        manager.reconcile_action_attempt(&session, &session, "open", was_running, false, &result).await;
        assert!(!manager.sessions.lock().await.get(&session).unwrap().running);

        // A now-available executable gets a fresh launch attempt. It exits
        // without publishing a browser control endpoint, which must also
        // release the provisional state after the direct child is reaped.
        let second_was_running = manager.note_action_start(&session, "open").await;
        assert!(!second_was_running);
        let available = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32")
            .join("where.exe");
        assert!(available.is_file());
        let second_failure = run_agent_browser_native_with_status(
            available.to_str().unwrap(),
            &["open".into(), "about:blank".into(), "--session".into(), session.clone()],
            &session,
            port,
            false,
            Duration::from_secs(5),
        ).unwrap_err();
        assert!(second_failure.error.to_string().contains("exited before control startup"));
        assert!(!second_failure.may_have_launched);
        let second_result = Err(BrowserActionFailure::before_launch(second_failure.error.to_string()));
        manager.reconcile_action_attempt(&session, &session, "open", second_was_running, false, &second_result).await;
        assert!(!manager.note_action_start(&session, "open").await);
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
        let record = test_browser_record(pid, start_time, token);
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
    #[tokio::test]
    async fn failed_first_browser_command_stops_a_tracked_daemon_and_allows_retry() {
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
        let manager = AgentBrowserManager::new();
        manager.allocate_port(&session).await.unwrap();
        let was_running = manager.note_action_start(&session, "open").await;
        assert!(!was_running);
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
        let record = test_browser_record(pid, start_time, token);
        let dir = run_dir_for_session(&session).unwrap();
        let pid_path = dir.join(format!("{session}.pid"));
        let own_record_path = session_pid_path(&session).unwrap();
        std::fs::write(&pid_path, pid.to_string()).unwrap();
        track_browser_daemon_before_command(&session, "unused.exe", &record, true).unwrap();
        assert_eq!(read_browser_pid_record(&own_record_path).unwrap().pid, pid);
        let retained = open_verified_browser_handle(&record, &session).expect("verified daemon handle");

        let failed = finish_browser_command_with_status(
            Err(std::io::Error::other("relay failed")),
            &session,
            "unused.exe",
            &record,
            true,
            Some(&retained),
        );
        let failure = failed.err().expect("relay error must be reported");
        assert!(!failure.may_have_launched, "reaped daemon must release provisional state");
        let stopped_waiting = Instant::now();
        while child.0.try_wait().unwrap().is_none() {
            assert!(stopped_waiting.elapsed() < Duration::from_secs(3), "failed command left its daemon running");
            std::thread::sleep(Duration::from_millis(25));
        }
        let result = Err(BrowserActionFailure {
            message: failure.error.to_string(),
            may_have_launched: failure.may_have_launched,
        });
        manager.reconcile_action_attempt(&session, &session, "open", was_running, false, &result).await;
        assert!(!manager.note_action_start(&session, "open").await, "next launch must be fresh");
        let _ = std::fs::remove_file(pid_path);
        let _ = std::fs::remove_file(own_record_path);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn reused_browser_daemon_exit_allows_first_action_retry() {
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
        let session = format!("browser-reused-exit-{}", uuid::Uuid::new_v4().simple());
        let manager = AgentBrowserManager::new();
        manager.allocate_port(&session).await.unwrap();
        let was_running = manager.note_action_start(&session, "open").await;
        assert!(!was_running);

        let prior_owner = uuid::Uuid::new_v4().simple().to_string();
        let child = std::process::Command::new(&binary)
            .args(["-t", "127.0.0.1"])
            .env("CODEMUX_BROWSER_OWNER", &prior_owner)
            .env("CODEMUX_BROWSER_SESSION", &session)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let mut child = ChildGuard(child);
        let pid = child.0.id();
        let started_waiting = Instant::now();
        let start_time = loop {
            if let Some(start) = browser_process_start(pid, &session, &prior_owner) {
                break start;
            }
            assert!(started_waiting.elapsed() < Duration::from_secs(3));
            std::thread::sleep(Duration::from_millis(25));
        };
        let record = test_browser_record(pid, start_time, prior_owner);
        let retained = open_verified_browser_handle(&record, &session).expect("verified reused daemon handle");

        child.0.kill().unwrap();
        child.0.wait().unwrap();
        let failure = finish_browser_command_with_status(
            Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "daemon exited during command")),
            &session,
            "unused.exe",
            &record,
            false,
            Some(&retained),
        ).err().expect("failed command must be reported");
        assert!(!failure.may_have_launched, "reused daemon exit must release provisional state");
        let result = Err(BrowserActionFailure {
            message: failure.error.to_string(),
            may_have_launched: failure.may_have_launched,
        });
        manager.reconcile_action_attempt(&session, &session, "open", was_running, false, &result).await;
        assert!(!manager.note_action_start(&session, "open").await, "next launch must be fresh");
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
        assert_eq!(browser_control_port("default", temp.path()), 50838);
        std::fs::write(temp.path().join("default.port"), "invalid").unwrap();
        assert_eq!(browser_control_port("default", temp.path()), 50838);
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
        let record = test_browser_record(pid, start_time, token);

        let dir = run_dir_for_session(&session).unwrap();
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
        assert!(output_capture_with_timeout(fake_command, Duration::from_secs(2)).unwrap().status.success());
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
    fn stopping_legacy_daemon_cleans_broad_acl_port_file() {
        struct ChildGuard(std::process::Child);
        impl Drop for ChildGuard {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        struct RuntimeFiles(Vec<PathBuf>);
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
        let session = format!("legacy-stop-{}", uuid::Uuid::new_v4().simple());
        let root = browser_run_root_for_session(&session).unwrap();
        let token = browser_owner_token().to_owned();
        let child = std::process::Command::new(&binary)
            .args(["-t", "127.0.0.1"])
            .env("CODEMUX_BROWSER_OWNER", &token)
            .env("CODEMUX_BROWSER_SESSION", &session)
            .env("AGENT_BROWSER_SOCKET_DIR", &root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn().unwrap();
        let mut child = ChildGuard(child);
        let pid = child.0.id();
        let started = Instant::now();
        let start_time = loop {
            if let Some(start_time) = browser_process_start(pid, &session, &token) {
                break start_time;
            }
            assert!(started.elapsed() < Duration::from_secs(3), "browser identity unavailable");
            std::thread::sleep(Duration::from_millis(25));
        };
        let record = test_browser_record(pid, start_time, token);
        let pid_path = root.join(format!("{session}.pid"));
        let port_path = root.join(format!("{session}.port"));
        let record_path = legacy_session_pid_path_in(&root, &session).unwrap();
        let _runtime_files = RuntimeFiles(vec![pid_path.clone(), port_path.clone(), record_path.clone()]);
        std::fs::write(&pid_path, pid.to_string()).unwrap();
        write_browser_pid_record(&record_path, &record).unwrap();
        let ephemeral = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        // The daemon uses fs::write, which can inherit a broader legacy ACL.
        std::fs::write(&port_path, ephemeral.local_addr().unwrap().port().to_string()).unwrap();
        let acl = std::process::Command::new("icacls")
            .arg(&port_path).args(["/grant", "*S-1-5-32-545:R"])
            .output().unwrap();
        assert!(acl.status.success(), "could not set legacy ACL: {acl:?}");
        assert!(clack_private_fs::require_private_file(&port_path).is_err());
        drop(ephemeral);

        assert_eq!(verified_browser_control_dir(&session, &record), Some(root.clone()));
        stop_owned_browser_daemon(&session, &record, "unused.exe");
        let stopped = Instant::now();
        while child.0.try_wait().unwrap().is_none() {
            assert!(stopped.elapsed() < Duration::from_secs(3), "verified legacy daemon survived stop");
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(!pid_path.exists(), "cleanup must use the root captured before process exit");
        assert!(!port_path.exists());
        assert_eq!(run_dir_for_session(&session), Some(root.join(BROWSER_DAEMON_DIR)));
    }

    #[cfg(windows)]
    #[test]
    fn captured_legacy_cleanup_preserves_replacement_daemon_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let session = format!("legacy-replaced-{}", uuid::Uuid::new_v4().simple());
        let current = root.join(BROWSER_DAEMON_DIR);
        prepare_browser_directory(&current, true).unwrap();
        let old_pid = u32::MAX;
        let old_pid_path = root.join(format!("{session}.pid"));
        std::fs::write(&old_pid_path, old_pid.to_string()).unwrap();
        let ephemeral = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        std::fs::write(root.join(format!("{session}.port")), ephemeral.local_addr().unwrap().port().to_string()).unwrap();
        drop(ephemeral);
        for ext in ["engine", "stream"] {
            std::fs::write(root.join(format!("{session}.{ext}")), b"old").unwrap();
        }
        let record = test_browser_record(old_pid, 1, browser_owner_token().to_owned());
        let captured = capture_legacy_windows_browser_files(root, &session, &record).unwrap();
        for ext in ["pid", "port", "engine", "stream"] {
            std::fs::write(current.join(format!("{session}.{ext}")), old_pid.to_string()).unwrap();
        }
        assert_eq!(browser_daemon_dir_in(root, &session), Some(current.clone()));

        assert!(cleanup_captured_legacy_windows_browser_files(&session, &record, &captured, || true));
        for ext in ["pid", "port", "engine", "stream"] {
            assert!(!root.join(format!("{session}.{ext}")).exists());
            assert_eq!(std::fs::read_to_string(current.join(format!("{session}.{ext}"))).unwrap(),
                old_pid.to_string(), "replacement d/ file was touched: {ext}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn captured_legacy_cleanup_keeps_replaced_root_port() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let session = format!("legacy-port-replaced-{}", uuid::Uuid::new_v4().simple());
        let old_pid = u32::MAX;
        let pid_path = root.join(format!("{session}.pid"));
        let port_path = root.join(format!("{session}.port"));
        std::fs::write(&pid_path, old_pid.to_string()).unwrap();
        std::fs::write(&port_path, b"53001").unwrap();
        let record = test_browser_record(old_pid, 1, browser_owner_token().to_owned());
        let captured = capture_legacy_windows_browser_files(root, &session, &record).unwrap();
        let old_port = root.join("captured-old.port");
        std::fs::rename(&port_path, &old_port).unwrap();
        std::fs::write(&port_path, b"53001").unwrap();

        assert!(!cleanup_captured_legacy_windows_browser_files(&session, &record, &captured, || true));
        assert!(pid_path.exists());
        assert!(port_path.exists());
        assert!(old_port.exists());
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
        assert!(status.status.success());
    }

    #[cfg(unix)]
    #[test]
    fn stream_open_preserves_a_pending_confirmation_reply() {
        for command in [
            "printf 'Confirmation required:\\n  Navigate to example.com\\n  Run: agent-browser confirm c1\\n  Or:  agent-browser deny c1\\n'",
            "printf 'Confirmation required:\\n  navigation: example.com\\n  Run: agent-browser confirm \\n  Or:  agent-browser deny \\n'",
        ] {
            let mut cmd = std::process::Command::new("sh");
            cmd.args(["-c", command]);
            let output = launch_status_with_timeout(cmd, Duration::from_secs(5)).unwrap();
            assert_eq!(
                require_successful_browser_stream_open_status(Ok(output)),
                Ok(BrowserOpenOutcome::ConfirmationPending),
            );
        }
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
    fn resolve_binary_returns_existing_path_or_npx_fallback() {
        let result = resolve_binary();
        if result.starts_with("npx ") {
            // Fallback is acceptable (e.g., binary not in cwd/node_modules)
            assert_eq!(result, "npx agent-browser");
        } else {
            // Must be a real path that exists and is a file
            let path = std::path::Path::new(&result);
            assert!(path.exists(), "resolve_binary returned non-existent path: {}", result);
            assert!(path.is_file(), "resolve_binary returned non-file: {}", result);
        }
    }

    // Structurally Linux/macOS only: the assertion hardcodes
    // `agent-browser-linux-x64` / `agent-browser-darwin` binary names.
    // Windows needs its own version once `resolve_binary()` grows a
    // Windows target-triple branch.
    #[cfg(unix)]
    #[test]
    fn resolve_binary_finds_native_binary_from_project_root() {
        // Run from the project root where node_modules exists
        let result = resolve_binary();
        // Step 1 of resolve_binary() prefers a system-wide install on
        // PATH (e.g. /usr/bin/agent-browser from a packaged install).
        // On dev machines that also run a packaged Codemux, that branch
        // legitimately wins over the node_modules lookup this test was
        // written for — accept it instead of failing on those hosts.
        if let Ok(system) = which::which("agent-browser") {
            if result == system.to_string_lossy() {
                assert!(system.is_file(), "system agent-browser vanished: {result}");
                return;
            }
        }
        // On this machine, the native binary should be found in node_modules
        if !result.starts_with("npx ") {
            assert!(
                result.contains("agent-browser-linux-x64") || result.contains("agent-browser-darwin"),
                "Expected platform-specific binary name, got: {}",
                result
            );
            // Verify it's executable
            let output = std::process::Command::new(&result)
                .arg("--version")
                .output();
            assert!(output.is_ok(), "Binary at {} is not executable", result);
            let out = output.unwrap();
            let version = String::from_utf8_lossy(&out.stdout);
            assert!(
                version.contains("agent-browser") || out.status.success(),
                "Binary didn't respond to --version: {}",
                version
            );
        }
    }

    #[cfg(not(target_os = "windows"))]
    #[tokio::test]
    async fn missing_unix_launcher_is_rejected_before_command_build_and_can_retry() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let launcher = temp.path().join("agent-browser");
        let session = "browser-launcher-retry";
        let params = serde_json::json!({"url": "about:blank"});
        let manager = AgentBrowserManager::new();
        manager.allocate_port(session).await.unwrap();
        let binary = launcher.to_str().unwrap();
        assert!(manager.prepare_unix_action(session, session, "open", &params, binary).await.is_err());
        assert!(!manager.sessions.lock().await.get(session).unwrap().running);

        std::fs::write(&launcher, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o700)).unwrap();
        let command = build_agent_browser_command_for_binary(binary, session, "open", &params).unwrap();
        assert!(command.contains("open 'about:blank' --session 'browser-launcher-retry'"));
        assert!(!manager.prepare_unix_action(session, session, "open", &params, binary).await.unwrap(),
            "the same session must accept a fresh first action");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unix_launcher_with_missing_interpreter_releases_first_action_for_retry() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let temp = tempfile::tempdir().unwrap();
        let interpreter = temp.path().join("interpreter");
        let launcher = temp.path().join("agent-browser");
        std::fs::write(&launcher, format!("#!{}\nexit 0\n", interpreter.display())).unwrap();
        std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o700)).unwrap();
        let binary = launcher.to_str().unwrap();
        let session = format!("browser-exec-retry-{}", uuid::Uuid::new_v4().simple());
        let params = serde_json::json!({"url": "about:blank"});
        let manager = AgentBrowserManager::new();
        let port = manager.allocate_port(&session).await.unwrap();
        let was_running = manager.prepare_unix_action(&session, &session, "open", &params, binary).await.unwrap();
        assert!(!was_running);

        let argv = build_agent_browser_argv_groups(&session, "open", &params).unwrap().remove(0);
        let command = browser_action_command(binary, &argv, &session, port).unwrap();
        let failure = output_capture_and_track_browser_daemon_with_status(
            command, Duration::from_secs(2), &session,
        ).err().expect("a missing shebang interpreter must prevent exec");
        assert_eq!(failure.error.kind(), std::io::ErrorKind::NotFound);
        assert!(!failure.may_have_launched);
        let result = Err(BrowserActionFailure::before_launch(failure.error.to_string()));
        manager.reconcile_action_attempt(&session, &session, "open", was_running, false, &result).await;
        assert!(!manager.sessions.lock().await.get(&session).unwrap().running);

        symlink("/bin/sh", &interpreter).unwrap();
        assert!(!manager.prepare_unix_action(&session, &session, "open", &params, binary).await.unwrap());
        let command = browser_action_command(binary, &argv, &session, port).unwrap();
        let output = output_capture_and_track_browser_daemon_with_status(
            command, Duration::from_secs(2), &session,
        ).unwrap();
        assert!(output.status.success());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exited_unix_launcher_without_daemon_allows_same_session_retry() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let launcher = temp.path().join("agent-browser");
        std::fs::write(&launcher, "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o700)).unwrap();
        let binary = launcher.to_str().unwrap();
        let session = format!("browser-exited-retry-{}", uuid::Uuid::new_v4().simple());
        let params = serde_json::json!({"url": "about:blank"});
        let manager = AgentBrowserManager::new();
        let port = manager.allocate_port(&session).await.unwrap();
        let was_running = manager.prepare_unix_action(&session, &session, "open", &params, binary).await.unwrap();
        assert!(!was_running);

        let argv = build_agent_browser_argv_groups(&session, "open", &params).unwrap().remove(0);
        let command = browser_action_command(binary, &argv, &session, port).unwrap();
        let output = output_capture_and_track_browser_daemon_with_status(
            command, Duration::from_secs(2), &session,
        ).unwrap();
        assert!(!output.status.success());
        let result = Err(BrowserActionFailure::after_launch("launcher exited without a daemon".into()));
        manager.reconcile_action_attempt(&session, &session, "open", was_running, false, &result).await;
        assert!(manager.sessions.lock().await.get(&session).is_none(), "failed first action must release its provisional port");

        std::fs::write(&launcher, "#!/bin/sh\nexit 0\n").unwrap();
        let retry_port = manager.allocate_port(&session).await.unwrap();
        assert!(!manager.prepare_unix_action(&session, &session, "open", &params, binary).await.unwrap());
        let command = browser_action_command(binary, &argv, &session, retry_port).unwrap();
        let output = output_capture_and_track_browser_daemon_with_status(
            command, Duration::from_secs(2), &session,
        ).unwrap();
        assert!(output.status.success());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn failed_viewport_launch_then_missing_action_releases_first_session() {
        let session = format!("browser-viewport-retry-{}", uuid::Uuid::new_v4().simple());
        let manager = AgentBrowserManager::new();
        manager.allocate_port(&session).await.unwrap();
        let was_running = manager.note_action_start(&session, "open").await;
        assert!(!was_running);

        let mut viewport = std::process::Command::new("sh");
        viewport.args(["-c", "exit 1"]);
        let output = output_capture_and_track_browser_daemon_with_status(
            viewport, Duration::from_secs(2), &session,
        ).unwrap();
        assert!(!output.status.success());

        let result = Err(BrowserActionFailure::before_launch("main action did not spawn".into()));
        manager.reconcile_action_attempt(&session, &session, "open", was_running, true, &result).await;
        assert!(manager.sessions.lock().await.get(&session).is_none());
        manager.allocate_port(&session).await.unwrap();
        assert!(!manager.note_action_start(&session, "open").await);
        assert!(manager.sessions.lock().await.get(&session).unwrap().running);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn malformed_daemon_pid_file_keeps_provisional_session() {
        let session = format!("browser-malformed-pid-{}", uuid::Uuid::new_v4().simple());
        let manager = AgentBrowserManager::new();
        manager.allocate_port(&session).await.unwrap();
        assert!(!manager.note_action_start(&session, "open").await);
        let pid_path = agent_browser_daemon_pid_path(&session).unwrap();
        std::fs::write(&pid_path, "still writing").unwrap();

        let result = Err(BrowserActionFailure::after_launch("launcher failed".into()));
        manager.settle_provisional_session(&session, &session, Some(("open", &result))).await;
        assert!(manager.sessions.lock().await.get(&session).unwrap().running);
        std::fs::remove_file(pid_path).unwrap();

        let record_path = session_pid_path(&session).unwrap();
        std::fs::write(&record_path, "still writing").unwrap();
        manager.settle_provisional_session(&session, &session, None).await;
        assert!(manager.sessions.lock().await.get(&session).unwrap().running);
        std::fs::remove_file(record_path).unwrap();

        manager.settle_provisional_session(&session, &session, None).await;
        assert!(manager.sessions.lock().await.get(&session).is_none());
        manager.allocate_port(&session).await.unwrap();
        assert!(!manager.note_action_start(&session, "open").await);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reaped_daemon_pid_file_allows_provisional_spawn_and_pane_retries() {
        for (retry, write_record) in [("spawn", false), ("pane", true)] {
            let session = format!("browser-dead-pid-{retry}-{}", uuid::Uuid::new_v4().simple());
            let (_temp, mut daemon, record) = browser_identity_fixture(&session, browser_owner_token());
            let pid_path = agent_browser_daemon_pid_path(&session).unwrap();
            std::fs::write(&pid_path, record.pid.to_string()).unwrap();
            if write_record {
                write_session_pid(&session, &record).unwrap();
            }
            let manager = AgentBrowserManager::new();
            manager.allocate_port(&session).await.unwrap();
            assert!(!manager.note_action_start(&session, "open").await);

            daemon.kill().unwrap();
            daemon.wait().unwrap();
            assert!(process_exit_confirmed(record.pid));
            let new_port_path = run_dir_for_session(&session).unwrap().join(format!("{session}.port"));
            std::fs::write(&new_port_path, b"new launch metadata").unwrap();
            manager.settle_provisional_session(&session, &session, None).await;
            assert!(!pid_path.exists(), "{retry} left a dead daemon PID file");
            assert!(!session_pid_path(&session).unwrap().exists(), "{retry} left a dead ownership record");
            assert_eq!(std::fs::read(&new_port_path).unwrap(), b"new launch metadata");
            assert!(manager.sessions.lock().await.get(&session).is_none(), "{retry} kept the provisional session");
            std::fs::remove_file(new_port_path).unwrap();
            manager.allocate_port(&session).await.unwrap();
            assert!(!manager.note_action_start(&session, "open").await);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn recycled_daemon_pid_file_allows_provisional_spawn_and_pane_retries() {
        let pid = std::process::id();
        let current_start = process_start_time(pid).unwrap();
        assert_eq!(process_looks_like_agent_browser(pid), Some(false));
        for retry in ["spawn", "pane"] {
            let session = format!("browser-reused-pid-{retry}-{}", uuid::Uuid::new_v4().simple());
            let record = test_browser_record(pid, current_start + 1, browser_owner_token().to_owned());
            let daemon_pid_path = agent_browser_daemon_pid_path(&session).unwrap();
            std::fs::write(&daemon_pid_path, pid.to_string()).unwrap();
            write_session_pid(&session, &record).unwrap();
            let manager = AgentBrowserManager::new();
            manager.allocate_port(&session).await.unwrap();
            assert!(!manager.note_action_start(&session, "open").await);

            manager.settle_provisional_session(&session, &session, None).await;
            assert!(!daemon_pid_path.exists(), "{retry} left a reused daemon PID marker");
            assert!(!session_pid_path(&session).unwrap().exists(), "{retry} left a stale ownership record");
            assert!(manager.sessions.lock().await.get(&session).is_none(), "{retry} kept the provisional session");
            assert_eq!(process_start_time(pid), Some(current_start), "unrelated process was affected");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn legacy_record_blocks_provisional_retry_only_while_its_daemon_may_live() {
        let session = format!("browser-legacy-retry-{}", uuid::Uuid::new_v4().simple());
        let (_temp, mut daemon, record) = browser_identity_fixture(&session, browser_owner_token());
        let dir = browser_run_root_for_session(&session).unwrap();
        let legacy = legacy_session_pid_path_in(&dir, &session).unwrap();
        write_browser_pid_record(&legacy, &record).unwrap();
        let manager = AgentBrowserManager::new();
        manager.allocate_port(&session).await.unwrap();
        assert!(!manager.note_action_start(&session, "open").await);

        manager.settle_provisional_session(&session, &session, None).await;
        assert!(manager.sessions.lock().await.get(&session).unwrap().running);
        daemon.kill().unwrap();
        daemon.wait().unwrap();
        manager.settle_provisional_session(&session, &session, None).await;
        assert!(manager.sessions.lock().await.get(&session).is_none());
        assert!(legacy.exists(), "legacy records are left for coordinated migration");
        std::fs::remove_file(legacy).unwrap();

        let colliding = format!("browser-numeric-legacy-{}", uuid::Uuid::new_v4().simple());
        let dir = browser_run_root_for_session(&colliding).unwrap();
        let numeric = legacy_session_pid_path_in(&dir, &colliding).unwrap();
        std::fs::write(&numeric, b"12345\n").unwrap();
        manager.allocate_port(&colliding).await.unwrap();
        assert!(!manager.note_action_start(&colliding, "open").await);
        manager.settle_provisional_session(&colliding, &colliding, None).await;
        assert!(manager.sessions.lock().await.get(&colliding).is_none());
        assert_eq!(std::fs::read(&numeric).unwrap(), b"12345\n");
        std::fs::remove_file(numeric).unwrap();

        let reused = format!("browser-reused-legacy-{}", uuid::Uuid::new_v4().simple());
        let dir = browser_run_root_for_session(&reused).unwrap();
        let legacy = legacy_session_pid_path_in(&dir, &reused).unwrap();
        let pid = std::process::id();
        let old = test_browser_record(pid, process_start_time(pid).unwrap() + 1, browser_owner_token().to_owned());
        write_browser_pid_record(&legacy, &old).unwrap();
        manager.allocate_port(&reused).await.unwrap();
        assert!(!manager.note_action_start(&reused, "open").await);
        manager.settle_provisional_session(&reused, &reused, None).await;
        assert!(manager.sessions.lock().await.get(&reused).is_none());
        assert_eq!(read_browser_pid_record(&legacy), Some(old));
        std::fs::remove_file(legacy).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_new_action_extends_late_daemon_tracking_for_the_same_session() {
        let session = format!("browser-tracker-extension-{}", uuid::Uuid::new_v4().simple());
        track_late_browser_daemon(&session);
        let first = browser_daemon_trackers().lock().unwrap()[&session];
        std::thread::sleep(Duration::from_millis(20));
        track_late_browser_daemon(&session);
        let second = browser_daemon_trackers().lock().unwrap()[&session];
        assert!(second > first);
        browser_daemon_trackers().lock().unwrap().remove(&session);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn failed_unix_action_preserves_a_daemon_that_publishes_its_pid_late() {
        let session = format!("browser-action-late-pid-{}", uuid::Uuid::new_v4().simple());
        let (_temp, mut daemon, record) = browser_identity_fixture(&session, browser_owner_token());
        let pid_path = run_dir_for_session(&session).unwrap().join(format!("{session}.pid"));
        let manager = AgentBrowserManager::new();
        manager.allocate_port(&session).await.unwrap();
        let was_running = manager.note_action_start(&session, "open").await;
        assert!(!was_running);
        let daemon_pid = daemon.id();
        let pid_path_for_writer = pid_path.clone();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            std::fs::write(pid_path_for_writer, daemon_pid.to_string()).unwrap();
        });

        let mut command = std::process::Command::new("sh");
        command.args(["-c", "exit 1"]);
        let output = output_capture_and_track_browser_daemon_with_status(
            command, Duration::from_secs(2), &session,
        ).unwrap();
        assert!(!output.status.success());
        let result = Err(BrowserActionFailure::after_launch("launcher exited before PID publication".into()));
        manager.reconcile_action_attempt(&session, &session, "open", was_running, false, &result).await;
        writer.join().unwrap();
        let current = manager.sessions.lock().await.get(&session).map(|entry| (entry.running, entry.pid));
        assert_eq!(current, Some((true, Some(record.pid))));

        kill_verified_browser_pid(&record, &session);
        let _ = daemon.wait();
        let _ = std::fs::remove_file(pid_path);
        clear_session_pid(&session);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn build_command_open_uses_navigation_load_wait_only() {
        let cmd = build_agent_browser_command("test-session", "open", &serde_json::json!({"url": "https://example.com"})).unwrap();
        let bin = resolve_binary();
        assert!(cmd.starts_with(&shell_binary(&bin)), "Command should start with resolved binary: {}", cmd);
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
        let cmd = build_agent_browser_command(&session, "screenshot", &serde_json::json!({})).unwrap();
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
        let cmd = build_agent_browser_command("s", "viewport", &serde_json::json!({"width": 800, "height": 600})).unwrap();
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
        let result = build_agent_browser_command("s", "nonexistent_action", &serde_json::json!({}));
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
    async fn unverified_running_stream_allocation_keeps_its_port_for_retry() {
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
        let mut sessions = mgr.sessions.lock().await;
        let stream = sessions.get_mut(&session).unwrap();
        stream.running = true;
        stream.browser_ready = true;
        drop(sessions);

        let error = mgr.start_stream(&session).await.unwrap_err();
        assert!(error.contains("Could not verify ownership"), "{error}");
        assert_eq!(mgr.get_port(&session).await, Some(port));
        assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_ok());
        assert_eq!(mgr.allocate_port(&session).await.unwrap(), port);
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

    /// `session_pid_path` returns a path under the private record dir when
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
        assert_eq!(path.parent().unwrap(), browser_run_root_for_session("ws-layout-test").unwrap().join(BROWSER_RECORD_DIR));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
        }
        #[cfg(windows)]
        clack_private_fs::require_private_directory(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn browser_record_namespace_does_not_collide_with_another_daemon_pid() {
        let temp = tempfile::tempdir().unwrap();
        let first = format!("x-{}", uuid::Uuid::new_v4().simple());
        let second = format!("agent-browser-{first}");
        validate_session_name(&second).unwrap();
        let first_record = test_browser_record(12345, 100, "a".repeat(32));
        let second_record = test_browser_record(23456, 200, "b".repeat(32));
        let first_path = session_pid_path_in(temp.path(), &first).unwrap();
        let second_path = session_pid_path_in(temp.path(), &second).unwrap();
        let second_daemon_pid = temp.path().join(format!("{second}.pid"));
        assert_eq!(second_daemon_pid, legacy_session_pid_path_in(temp.path(), &first).unwrap());

        write_browser_pid_record(&first_path, &first_record).unwrap();
        write_browser_pid_record(&second_path, &second_record).unwrap();
        std::fs::write(&second_daemon_pid, b"23456\n").unwrap();
        assert_eq!(session_pid_record_in(temp.path(), &first).unwrap().1, first_record);
        assert_eq!(session_pid_record_in(temp.path(), &second).unwrap().1, second_record);
        assert_eq!(read_daemon_pid_file(&second_daemon_pid), Some(23456));

        std::fs::remove_file(&first_path).unwrap();
        assert!(session_pid_record_in(temp.path(), &first).is_none());
        assert_eq!(session_pid_record_in(temp.path(), &second).unwrap().1, second_record);
        assert_eq!(read_daemon_pid_file(&second_daemon_pid), Some(23456));
    }

    #[test]
    fn new_daemon_directory_avoids_structured_legacy_record_collision() {
        let temp = tempfile::tempdir().unwrap();
        let first = format!("x-{}", uuid::Uuid::new_v4().simple());
        let second = format!("agent-browser-{first}");
        let old_record = test_browser_record(12345, 100, "a".repeat(32));
        let old_path = legacy_session_pid_path_in(temp.path(), &first).unwrap();
        write_browser_pid_record(&old_path, &old_record).unwrap();

        let selected = browser_daemon_dir_in(temp.path(), &second).unwrap();
        assert_eq!(selected, temp.path().join(BROWSER_DAEMON_DIR));
        let new_pid = selected.join(format!("{second}.pid"));
        std::fs::write(&new_pid, b"23456\n").unwrap();
        assert_eq!(browser_daemon_dir_in(temp.path(), &second), Some(selected));
        assert_eq!(read_browser_pid_record(&old_path), Some(old_record));
        assert_eq!(read_daemon_pid_file(&new_pid), Some(23456));
    }

    #[test]
    fn numeric_legacy_record_does_not_block_new_daemon_namespace() {
        let temp = tempfile::tempdir().unwrap();
        let first = format!("x-{}", uuid::Uuid::new_v4().simple());
        let second = format!("agent-browser-{first}");
        let old_path = legacy_session_pid_path_in(temp.path(), &first).unwrap();
        std::fs::write(&old_path, b"12345\n").unwrap();

        let selected = browser_daemon_dir_in(temp.path(), &second).unwrap();
        assert_eq!(selected, temp.path().join(BROWSER_DAEMON_DIR));
        assert_eq!(std::fs::read(&old_path).unwrap(), b"12345\n");
        let new_pid = selected.join(format!("{second}.pid"));
        std::fs::write(&new_pid, b"23456\n").unwrap();
        assert_eq!(browser_daemon_dir_in(temp.path(), &second), Some(selected));
        assert_eq!(std::fs::read(&old_path).unwrap(), b"12345\n");
        assert_eq!(read_daemon_pid_file(&new_pid), Some(23456));
    }

    #[cfg(unix)]
    #[test]
    fn live_legacy_endpoint_blocks_a_second_daemon_namespace() {
        use std::os::unix::net::UnixListener;

        let temp = tempfile::tempdir().unwrap();
        let session = "legacy-endpoint-busy";
        std::fs::write(temp.path().join(format!("{session}.pid")), b"12345\n").unwrap();
        let socket = temp.path().join(format!("{session}.sock"));
        let listener = UnixListener::bind(&socket).unwrap();
        assert!(browser_daemon_dir_in(temp.path(), session).is_none());
        drop(listener);
    }

    #[cfg(windows)]
    #[test]
    fn windows_stale_root_port_attributes_verified_daemon_child() {
        use std::io::Write;

        let temp = tempfile::tempdir().unwrap();
        let (session, listener) = (0..32).find_map(|_| {
            let first = format!("x-{}", uuid::Uuid::new_v4().simple());
            let session = format!("agent-browser-{first}");
            let port = browser_control_fallback_port(&session);
            std::net::TcpListener::bind(("127.0.0.1", port)).ok().map(|listener| (session, listener))
        }).expect("a free hash-derived browser port");
        let first = session.strip_prefix("agent-browser-").unwrap();
        let foreign = legacy_session_pid_path_in(temp.path(), first).unwrap();
        std::fs::write(&foreign, u32::MAX.to_string()).unwrap();
        let selected = temp.path().join(BROWSER_DAEMON_DIR);
        prepare_browser_directory(&selected, true).unwrap();
        std::fs::write(selected.join(format!("{session}.pid")), std::process::id().to_string()).unwrap();
        let mut stale_port = clack_private_fs::create_new_file(&temp.path().join(format!("{session}.port"))).unwrap();
        write!(stale_port, "{}", browser_control_fallback_port(&session)).unwrap();
        stale_port.sync_all().unwrap();
        let listener_pid = windows_loopback_listener_pid(browser_control_fallback_port(&session)).unwrap();
        assert_eq!(listener_pid, Some(std::process::id()));
        let clean_root = tempfile::tempdir().unwrap();
        assert_eq!(probe_legacy_browser_control_endpoint(&session, clean_root.path(), &clean_root.path().join(BROWSER_DAEMON_DIR)),
            BrowserControlEndpointProbe::Absent,
            "a hash-port listener does not establish legacy ownership without root metadata");
        assert_eq!(classify_legacy_browser_listener(listener_pid, Some(u32::MAX), Some(std::process::id()), true),
            BrowserControlEndpointProbe::Absent);
        assert_eq!(classify_legacy_browser_listener(listener_pid, Some(u32::MAX), Some(std::process::id()), false),
            BrowserControlEndpointProbe::Inconclusive);
        assert_eq!(probe_current_browser_control_endpoint(&session, &selected, std::process::id()),
            BrowserControlEndpointProbe::Absent,
            "a verified legacy daemon's hash listener is not a d/ endpoint");
        assert_eq!(classify_current_browser_listener(Some(Some(std::process::id())), u32::MAX),
            BrowserControlEndpointProbe::Inconclusive,
            "another process's listener must keep the conflict blocked");
        assert_eq!(probe_legacy_browser_control_endpoint(&session, temp.path(), &selected),
            BrowserControlEndpointProbe::Inconclusive,
            "the fixture process is not an agent-browser daemon and must not be trusted");
        assert_eq!(std::fs::read_to_string(&foreign).unwrap(), u32::MAX.to_string());
        drop(listener);
    }

    #[cfg(windows)]
    #[test]
    fn windows_verified_legacy_hash_listener_selects_root_without_daemon_child() {
        let temp = tempfile::tempdir().unwrap();
        let (session, listener) = (0..32).find_map(|_| {
            let session = format!("legacy-{}", uuid::Uuid::new_v4().simple());
            let port = browser_control_fallback_port(&session);
            std::net::TcpListener::bind(("127.0.0.1", port)).ok().map(|listener| (session, listener))
        }).expect("a free hash-derived browser port");
        let current = temp.path().join(BROWSER_DAEMON_DIR);
        prepare_browser_directory(&current, true).unwrap();
        let pid = std::process::id();
        let legacy_pid_path = temp.path().join(format!("{session}.pid"));
        std::fs::write(&legacy_pid_path, pid.to_string()).unwrap();
        let record = test_browser_record(pid, process_start_time(pid).unwrap(), browser_owner_token().to_owned());
        let record_path = legacy_session_pid_path_in(temp.path(), &session).unwrap();
        write_browser_pid_record(&record_path, &record).unwrap();
        assert_eq!(read_browser_pid_record(&record_path), Some(record));

        // The caller has already verified the root process identity; here
        // the listener owner must not masquerade as a d/ endpoint.
        assert_eq!(select_verified_legacy_browser_dir(
            temp.path(), &current, &session, &current.join(format!("{session}.pid")), false, pid,
        ), Some(temp.path().to_path_buf()));
        assert_eq!(classify_current_browser_listener(Some(Some(pid)), u32::MAX),
            BrowserControlEndpointProbe::Inconclusive);
        drop(listener);
    }

    #[cfg(windows)]
    #[test]
    fn windows_legacy_ephemeral_port_ignores_unattributed_hash_listener() {
        use std::io::Write;

        let temp = tempfile::tempdir().unwrap();
        let (session, listener) = (0..32).find_map(|_| {
            let session = format!("legacy-{}", uuid::Uuid::new_v4().simple());
            let port = browser_control_fallback_port(&session);
            std::net::TcpListener::bind(("127.0.0.1", port)).ok().map(|listener| (session, listener))
        }).expect("a free hash-derived browser port");
        let current = temp.path().join(BROWSER_DAEMON_DIR);
        prepare_browser_directory(&current, true).unwrap();
        let ephemeral = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let mut legacy_port = clack_private_fs::create_new_file(&temp.path().join(format!("{session}.port"))).unwrap();
        write!(legacy_port, "{}", ephemeral.local_addr().unwrap().port()).unwrap();
        legacy_port.sync_all().unwrap();
        drop(ephemeral);

        let legacy_pid = u32::MAX;
        assert_eq!(windows_loopback_listener_pid(browser_control_fallback_port(&session)),
            Some(Some(std::process::id())));
        assert_eq!(probe_current_browser_control_endpoint(&session, &current, legacy_pid),
            BrowserControlEndpointProbe::Absent);
        assert!(require_owned_browser_session_in(&session, &current).is_ok(),
            "a fresh d/ session must reach agent-browser's ephemeral-port fallback");
        assert_eq!(select_verified_legacy_browser_dir(
            temp.path(), &current, &session, &current.join(format!("{session}.pid")), false, legacy_pid,
        ), Some(temp.path().to_path_buf()));

        let current_pid = current.join(format!("{session}.pid"));
        std::fs::write(&current_pid, legacy_pid.to_string()).unwrap();
        assert_eq!(probe_current_browser_control_endpoint(&session, &current, legacy_pid),
            BrowserControlEndpointProbe::Inconclusive,
            "d/ PID metadata keeps an unrelated hash listener ambiguous");
        std::fs::remove_file(current_pid).unwrap();
        let mut current_port = clack_private_fs::create_new_file(&current.join(format!("{session}.port"))).unwrap();
        write!(current_port, "{}", browser_control_fallback_port(&session)).unwrap();
        current_port.sync_all().unwrap();
        assert_eq!(probe_current_browser_control_endpoint(&session, &current, legacy_pid),
            BrowserControlEndpointProbe::Inconclusive,
            "d/ port metadata also keeps the listener ambiguous");
        assert!(require_owned_browser_session_in(&session, &current).is_err(),
            "startup must still reject a port file without an owner PID");
        drop(current_port);
        std::fs::write(current.join(format!("{session}.port")), b"invalid").unwrap();
        assert!(require_owned_browser_session_in(&session, &current).is_err(),
            "invalid d/ metadata must stay fail-closed");
        drop(listener);
    }

    #[test]
    fn new_browser_commands_use_the_private_daemon_directory() {
        let session = format!("new-{}", uuid::Uuid::new_v4().simple());
        let root = browser_run_root_for_session(&session).unwrap();
        let mut command = std::process::Command::new("unused-browser-fixture");
        set_browser_owner_env(&mut command, &session).unwrap();
        let selected = command.get_envs()
            .find(|(key, _)| *key == "AGENT_BROWSER_SOCKET_DIR")
            .and_then(|(_, value)| value)
            .map(PathBuf::from)
            .unwrap();
        assert_eq!(selected, root.join(BROWSER_DAEMON_DIR));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&selected).unwrap().permissions().mode() & 0o777, 0o700);
        }
        #[cfg(windows)]
        clack_private_fs::require_private_directory(&selected).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn verified_legacy_daemon_keeps_its_original_control_directory() {
        let session = format!("legacy-{}", uuid::Uuid::new_v4().simple());
        let root = browser_run_root_for_session(&session).unwrap();
        let (_temp, mut child, record) = browser_identity_fixture_with_attempt_and_dir(
            &session, browser_owner_token(), None, Some(&root),
        );
        let old_pid = root.join(format!("{session}.pid"));
        let old_record = legacy_session_pid_path_in(&root, &session).unwrap();
        write_browser_pid_record(&old_record, &record).unwrap();
        std::fs::write(&old_pid, child.id().to_string()).unwrap();

        assert_eq!(run_dir_for_session(&session), Some(root.clone()));
        assert!(require_owned_browser_session(&session).is_ok());
        let mut command = std::process::Command::new("unused-browser-fixture");
        set_browser_owner_env(&mut command, &session).unwrap();
        let selected = command.get_envs()
            .find(|(key, _)| *key == "AGENT_BROWSER_SOCKET_DIR")
            .and_then(|(_, value)| value)
            .map(PathBuf::from);
        assert_eq!(selected, Some(root));

        child.kill().unwrap();
        child.wait().unwrap();
        std::fs::remove_file(old_pid).unwrap();
        std::fs::remove_file(old_record).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn stale_legacy_pid_equal_to_new_daemon_pid_does_not_redirect_control() {
        let first = format!("x-{}", uuid::Uuid::new_v4().simple());
        let session = format!("agent-browser-{first}");
        let root = browser_run_root_for_session(&session).unwrap();
        let current = browser_daemon_dir_in(&root, &session).unwrap();
        let (_temp, mut child, record) = browser_identity_fixture_with_attempt_and_dir(
            &session, browser_owner_token(), None, Some(&current),
        );
        let current_pid = current.join(format!("{session}.pid"));
        let foreign_numeric = root.join(format!("{session}.pid"));
        std::fs::write(&current_pid, child.id().to_string()).unwrap();
        std::fs::write(&foreign_numeric, child.id().to_string()).unwrap();
        write_session_pid(&session, &record).unwrap();

        assert!(!verified_legacy_browser_pid_in(&root, &session));
        assert_eq!(run_dir_for_session(&session), Some(current));
        assert_eq!(read_agent_browser_daemon_pid(&session), Some(child.id()));
        assert_eq!(std::fs::read_to_string(&foreign_numeric).unwrap(), child.id().to_string());

        child.kill().unwrap();
        child.wait().unwrap();
        std::fs::remove_file(current_pid).unwrap();
        std::fs::remove_file(foreign_numeric).unwrap();
        clear_session_pid(&session);
    }

    #[test]
    fn legacy_record_fallback_never_touches_daemon_pid_files() {
        let temp = tempfile::tempdir().unwrap();
        let session = "legacy-browser-record";
        let legacy = legacy_session_pid_path_in(temp.path(), session).unwrap();
        let record = test_browser_record(12345, 100, "a".repeat(32));
        write_browser_pid_record(&legacy, &record).unwrap();
        let current = session_pid_path_in(temp.path(), session).unwrap();
        assert!(!current.exists());
        assert_eq!(session_pid_record_in(temp.path(), session), Some((legacy.clone(), record.clone())));
        assert_eq!(browser_pid_record_paths_in(temp.path()), vec![legacy.clone()]);
        assert!(legacy.exists(), "legacy names must not be unlinked across a daemon PID race");
        let colliding_daemon = format!("agent-browser-{session}");
        assert!(require_owned_browser_session_in(&colliding_daemon, temp.path())
            .unwrap_err().contains("invalid daemon PID file"));

        let numeric_session = "other-daemon-pid";
        let numeric = legacy_session_pid_path_in(temp.path(), numeric_session).unwrap();
        std::fs::write(&numeric, b"54321\n").unwrap();
        assert!(session_pid_record_in(temp.path(), numeric_session).is_none());
        assert_eq!(std::fs::read(&numeric).unwrap(), b"54321\n");

        std::fs::remove_dir(current.parent().unwrap()).unwrap();
        std::fs::write(current.parent().unwrap(), b"unusable directory").unwrap();
        assert!(session_pid_path_in(temp.path(), session).is_none());
        assert_eq!(session_pid_record_in(temp.path(), session), Some((legacy, record)));
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
        assert!(browser_socket_path_fits(&dir.join(BROWSER_DAEMON_DIR), session));
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
    fn short_runtime_dir_preserves_socket_path_boundary_with_daemon_child() {
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
        assert_eq!(dir.join(BROWSER_DAEMON_DIR).join(format!("{session}.sock")).as_os_str().as_bytes().len(), 103);
        assert!(browser_socket_path_fits(&dir.join(BROWSER_DAEMON_DIR), &session));
    }

    #[cfg(unix)]
    #[test]
    fn live_legacy_home_endpoint_prevents_switch_to_short_runtime() {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::net::UnixListener;

        let temp = tempfile::tempdir_in("/tmp").unwrap();
        let session = "s".repeat(20);
        let home = (1..100).map(|length| temp.path().join("h".repeat(length)))
            .find(|home| browser_run_dir_for(home).join(format!("{session}.sock"))
                .as_os_str().as_bytes().len() == 103).unwrap();
        std::fs::create_dir(&home).unwrap();
        let root = browser_run_dir_for(&home);
        prepare_run_dir(&root).unwrap();
        assert!(!browser_socket_path_fits(&root.join(BROWSER_DAEMON_DIR), &session));
        let listener = UnixListener::bind(root.join(format!("{session}.sock"))).unwrap();
        let runtime_root = temp.path().join("rt");
        std::fs::create_dir(&runtime_root).unwrap();
        std::fs::set_permissions(&runtime_root, std::fs::Permissions::from_mode(0o700)).unwrap();

        assert_eq!(browser_run_root_for_roots(Some(&home), Some(&runtime_root), &session), Some(root.clone()));
        assert!(browser_daemon_dir_in(&root, &session).is_none());
        drop(listener);
    }

    #[cfg(unix)]
    #[test]
    fn live_legacy_short_runtime_endpoint_prevents_home_fallback() {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::net::UnixListener;

        let temp = tempfile::tempdir_in("/tmp").unwrap();
        let session = "s".repeat(20);
        let runtime_root = (1..100).map(|length| temp.path().join("r".repeat(length)))
            .find(|runtime_root| runtime_root.join("cmx-browser").join(format!("{session}.sock"))
                .as_os_str().as_bytes().len() == 103).unwrap();
        std::fs::create_dir(&runtime_root).unwrap();
        std::fs::set_permissions(&runtime_root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let root = private_runtime_run_dir(&runtime_root).unwrap();
        prepare_run_dir(&root).unwrap();
        assert!(!browser_socket_path_fits(&root.join(BROWSER_DAEMON_DIR), &session));
        let listener = UnixListener::bind(root.join(format!("{session}.sock"))).unwrap();
        let home = temp.path().join("h".repeat(120));
        std::fs::create_dir(&home).unwrap();

        assert_eq!(browser_run_root_for_roots(Some(&home), Some(&runtime_root), &session), Some(root.clone()));
        assert!(browser_daemon_dir_in(&root, &session).is_none());
        drop(listener);
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
        // We write to the private codemux-records directory and read
        // via session_pid_path (NOT the agent-browser-CLI's own PID
        // file). Use a unique name so parallel test runs don't collide.
        let name = format!("ws-roundtrip-{}", std::process::id());
        let first = test_browser_record(12345, 100, "a".repeat(32));
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
            let second = test_browser_record(54321, 200, "b".repeat(32));
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
        let record = test_browser_record(pid, start_time, token.clone());
        assert!(browser_process_matches(&record, session));
        assert!(!browser_process_matches(&record, "another-session"));
        assert!(!browser_process_matches(&BrowserPidRecord { token: "b".repeat(32), ..record.clone() }, session));
        assert!(!browser_process_matches(&BrowserPidRecord { start_time: start_time + 1, ..record }, session));
        assert!(browser_process_start(std::process::id(), session, &token).is_none());
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[cfg(unix)]
    fn browser_identity_fixture(
        session: &str,
        token: &str,
    ) -> (tempfile::TempDir, std::process::Child, BrowserPidRecord) {
        browser_identity_fixture_with_attempt(session, token, None)
    }

    #[cfg(unix)]
    fn browser_identity_fixture_with_attempt(
        session: &str,
        token: &str,
        attempt: Option<&str>,
    ) -> (tempfile::TempDir, std::process::Child, BrowserPidRecord) {
        browser_identity_fixture_with_attempt_and_dir(session, token, attempt, None)
    }

    #[cfg(unix)]
    fn browser_identity_fixture_with_attempt_and_dir(
        session: &str,
        token: &str,
        attempt: Option<&str>,
        control_dir: Option<&std::path::Path>,
    ) -> (tempfile::TempDir, std::process::Child, BrowserPidRecord) {
        let temp = tempfile::tempdir().unwrap();
        let binary = temp.path().join("agent-browser");
        std::fs::copy(which::which("bash").unwrap(), &binary).unwrap();
        let mut command = std::process::Command::new(&binary);
        command.args(["-c", "while :; do sleep 1; done"])
            .env("CODEMUX_BROWSER_OWNER", token)
            .env("CODEMUX_BROWSER_SESSION", session);
        if let Some(control_dir) = control_dir {
            command.env("AGENT_BROWSER_SOCKET_DIR", control_dir);
        }
        if let Some(attempt) = attempt {
            command.env("CODEMUX_BROWSER_OPEN_ATTEMPT", attempt);
        }
        let spawn_started = Instant::now();
        let mut child = loop {
            match command.spawn() {
                Ok(child) => break child,
                Err(error) if error.raw_os_error() == Some(libc::ETXTBSY)
                    && spawn_started.elapsed() < Duration::from_secs(3) =>
                {
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(error) => panic!("failed to start browser fixture: {error}"),
            }
        };
        let start = Instant::now();
        let start_time = loop {
            if let Some(start_time) = browser_process_start(child.id(), session, token) {
                break start_time;
            }
            if start.elapsed() >= Duration::from_secs(3) {
                let status = child.try_wait().unwrap();
                let _ = child.kill();
                let _ = child.wait();
                panic!("browser fixture did not expose its identity: {status:?}");
            }
            std::thread::sleep(Duration::from_millis(25));
        };
        let record = test_browser_record(child.id(), start_time, token.to_owned());
        (temp, child, record)
    }

    #[cfg(unix)]
    #[test]
    fn browser_close_sends_only_to_a_verified_connected_peer() {
        use std::io::{BufRead, Write};
        use std::os::unix::net::UnixListener;

        let temp = tempfile::tempdir().unwrap();
        let socket = temp.path().join("control.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        assert_eq!(
            connect_unix_browser_control_until(&socket, Instant::now()).unwrap_err().kind(),
            std::io::ErrorKind::TimedOut,
        );
        let server = std::thread::spawn(move || {
            let mut received = Vec::new();
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut line = String::new();
                std::io::BufReader::new(&stream).read_line(&mut line).unwrap();
                if !line.is_empty() {
                    received.push(serde_json::from_str::<serde_json::Value>(&line).unwrap());
                    stream.write_all(b"{\"success\":true}\n").unwrap();
                }
            }
            received
        });

        let deadline = || Instant::now() + Duration::from_secs(2);
        assert!(close_verified_unix_browser_peer(&socket, std::process::id() + 1, deadline(), || true, || {}).is_err());
        assert!(close_verified_unix_browser_peer(&socket, std::process::id(), deadline(), || false, || {}).is_err());
        close_verified_unix_browser_peer(&socket, std::process::id(), deadline(), || true, || {}).unwrap();
        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["action"], "close");
    }

    #[cfg(unix)]
    #[test]
    fn browser_close_response_uses_one_deadline_across_chunks() {
        use std::io::{BufRead, Write};
        use std::os::unix::net::UnixListener;

        let temp = tempfile::tempdir().unwrap();
        let socket = temp.path().join("control.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            std::io::BufReader::new(&stream).read_line(&mut line).unwrap();
            for _ in 0..20 {
                if stream.write_all(b" ").is_err() { break; }
                std::thread::sleep(Duration::from_millis(60));
            }
        });

        let started = Instant::now();
        let result = close_verified_unix_browser_peer(
            &socket, std::process::id(), started + Duration::from_millis(250), || true, || {},
        );
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
        server.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn browser_close_captures_pid_published_after_peer_connection() {
        use std::io::{BufRead, Write};
        use std::os::unix::net::UnixListener;

        let dir = tempfile::tempdir().unwrap();
        let session = "browser-close-late-pid";
        let record = test_browser_record(std::process::id(), 1, browser_owner_token().to_owned());
        let socket_path = dir.path().join(format!("{session}.sock"));
        assert!(capture_closed_browser_files(dir.path(), session, &record).is_none());
        let listener = UnixListener::bind(&socket_path).unwrap();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let server_dir = dir.path().to_path_buf();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            std::fs::write(server_dir.join(format!("{session}.pid")), std::process::id().to_string()).unwrap();
            std::fs::write(server_dir.join(format!("{session}.stream")), "43210").unwrap();
            ready_tx.send(()).unwrap();
            let mut line = String::new();
            std::io::BufReader::new(&stream).read_line(&mut line).unwrap();
            assert_eq!(serde_json::from_str::<serde_json::Value>(&line).unwrap()["action"], "close");
            stream.write_all(b"{\"success\":true}\n").unwrap();
        });

        let mut files = None;
        close_verified_unix_browser_peer(
            &socket_path, std::process::id(), Instant::now() + Duration::from_secs(10),
            || true,
            || {
                ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                files = capture_closed_browser_files(dir.path(), session, &record);
            },
        ).unwrap();
        server.join().unwrap();
        let files = files.expect("late PID and socket were captured after peer verification");
        assert!(files.record.is_none());
        assert!(cleanup_closed_browser_files(dir.path(), session, &record, &files, || true));
        assert!(!socket_path.exists());
    }

    #[cfg(unix)]
    fn closed_browser_file_fixture(
        dir: &std::path::Path,
        session: &str,
        record: &BrowserPidRecord,
        with_record: bool,
    ) -> ClosedBrowserFiles {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        use std::os::unix::net::UnixListener;

        std::fs::write(dir.join(format!("{session}.pid")), record.pid.to_string()).unwrap();
        std::fs::write(dir.join(format!("{session}.stream")), "43210").unwrap();
        let socket = UnixListener::bind(dir.join(format!("{session}.sock"))).unwrap();
        drop(socket);
        if with_record {
            let mut owned = std::fs::OpenOptions::new().write(true).create_new(true)
                .mode(0o600)
                .open(session_pid_path_in(dir, session).unwrap()).unwrap();
            writeln!(owned, "{} {} {} {} {} v2", record.pid, record.start_time,
                record.token, record.owner_pid, record.owner_start_time).unwrap();
        }
        capture_closed_browser_files(dir, session, record).unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn confirmed_browser_close_retires_only_the_captured_files() {
        let dir = tempfile::tempdir().unwrap();
        let session = "browser-close-metadata";
        let record = test_browser_record(12345, 1, browser_owner_token().to_owned());
        let files = closed_browser_file_fixture(dir.path(), session, &record, true);
        assert!(!cleanup_closed_browser_files(dir.path(), session, &record, &files, || false));
        assert!(dir.path().join(format!("{session}.pid")).exists());
        assert!(cleanup_closed_browser_files(dir.path(), session, &record, &files, || true));
        for suffix in ["pid", "stream", "sock"] {
            assert!(!dir.path().join(format!("{session}.{suffix}")).exists());
        }
        assert!(!session_pid_path_in(dir.path(), session).unwrap().exists());
    }

    #[cfg(unix)]
    #[test]
    fn confirmed_browser_close_preserves_legacy_record_and_retires_daemon_files() {
        let dir = tempfile::tempdir().unwrap();
        let session = "browser-close-legacy";
        let record = test_browser_record(12345, 1, browser_owner_token().to_owned());
        closed_browser_file_fixture(dir.path(), session, &record, false);
        let legacy = legacy_session_pid_path_in(dir.path(), session).unwrap();
        write_browser_pid_record(&legacy, &record).unwrap();
        let files = capture_closed_browser_files(dir.path(), session, &record).unwrap();
        assert_eq!(files.record_path, legacy);
        assert!(cleanup_closed_browser_files(dir.path(), session, &record, &files, || true));
        for suffix in ["pid", "stream", "sock"] {
            assert!(!dir.path().join(format!("{session}.{suffix}")).exists());
        }
        assert_eq!(read_browser_pid_record(&legacy), Some(record));
    }

    #[cfg(unix)]
    #[test]
    fn browser_close_metadata_cleanup_preserves_replacement_endpoint_and_record() {
        use std::os::unix::net::UnixListener;

        let dir = tempfile::tempdir().unwrap();
        let session = "browser-close-replaced";
        let record = test_browser_record(12345, 1, browser_owner_token().to_owned());
        let files = closed_browser_file_fixture(dir.path(), session, &record, true);
        let socket_path = dir.path().join(format!("{session}.sock"));
        std::fs::remove_file(&socket_path).unwrap();
        let replacement = UnixListener::bind(&socket_path).unwrap();
        assert!(!cleanup_closed_browser_files(dir.path(), session, &record, &files, || true));
        assert!(dir.path().join(format!("{session}.pid")).exists());
        assert!(session_pid_path_in(dir.path(), session).unwrap().exists());
        drop(replacement);

        std::fs::remove_file(&socket_path).unwrap();
        let mut changed = record.clone();
        changed.token = uuid::Uuid::new_v4().simple().to_string();
        std::fs::write(
            session_pid_path_in(dir.path(), session).unwrap(),
            format!("{} {} {} {} {} v2\n", changed.pid, changed.start_time,
                changed.token, changed.owner_pid, changed.owner_start_time),
        ).unwrap();
        assert!(!cleanup_closed_browser_files(dir.path(), session, &record, &files, || true));
        assert!(read_browser_pid_record(&session_pid_path_in(dir.path(), session).unwrap()).as_ref() == Some(&changed));
    }

    #[cfg(unix)]
    #[test]
    fn browser_close_without_record_retires_only_the_untracked_daemon_files() {
        let dir = tempfile::tempdir().unwrap();
        let session = "browser-close-untracked";
        let record = test_browser_record(12345, 1, browser_owner_token().to_owned());
        let files = closed_browser_file_fixture(dir.path(), session, &record, false);
        assert!(files.record.is_none());
        assert!(!cleanup_closed_browser_files(dir.path(), session, &record, &files, || false));
        assert!(cleanup_closed_browser_files(dir.path(), session, &record, &files, || true));
        for suffix in ["pid", "stream", "sock"] {
            assert!(!dir.path().join(format!("{session}.{suffix}")).exists());
        }
        assert!(!session_pid_path_in(dir.path(), session).unwrap().exists());
    }

    #[cfg(unix)]
    #[test]
    fn browser_close_without_record_preserves_a_new_ownership_record() {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;

        let dir = tempfile::tempdir().unwrap();
        let session = "browser-close-record-race";
        let record = test_browser_record(12345, 1, browser_owner_token().to_owned());
        let files = closed_browser_file_fixture(dir.path(), session, &record, false);
        let path = session_pid_path_in(dir.path(), session).unwrap();
        let mut replacement = std::fs::OpenOptions::new().write(true).create_new(true)
            .mode(0o600).open(&path).unwrap();
        writeln!(replacement, "{} {} {} {} {} v2", record.pid, record.start_time,
            uuid::Uuid::new_v4().simple(), record.owner_pid, record.owner_start_time).unwrap();
        assert!(!cleanup_closed_browser_files(dir.path(), session, &record, &files, || true));
        assert!(dir.path().join(format!("{session}.pid")).exists());
        assert!(path.exists());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_failed_open_daemon_fixture() {
        use std::io::{BufRead, Write};
        use std::os::unix::net::UnixListener;

        let Ok(session) = std::env::var("CODEMUX_TEST_FAILED_OPEN_SESSION") else { return };
        let release = std::env::var("CODEMUX_TEST_FAILED_OPEN_RELEASE").unwrap();
        let dir = run_dir_for_session(&session).unwrap();
        let started = Instant::now();
        while !std::path::Path::new(&release).exists() {
            assert!(started.elapsed() < Duration::from_secs(3), "fixture was never released");
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(750));
        std::fs::write(dir.join(format!("{session}.pid")), std::process::id().to_string()).unwrap();
        std::fs::write(dir.join(format!("{session}.stream")), "43210").unwrap();
        let listener = UnixListener::bind(dir.join(format!("{session}.sock"))).unwrap();
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut line = String::new();
        std::io::BufReader::new(&stream).read_line(&mut line).unwrap();
        let request: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(request["action"], "close");
        stream.write_all(b"{\"success\":true}\n").unwrap();
        std::process::exit(0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn failed_mac_open_closes_its_late_published_daemon() {
        let session = format!("browser-mac-late-{}", uuid::Uuid::new_v4().simple());
        let attempt = uuid::Uuid::new_v4().simple().to_string();
        let executable = std::env::current_exe().unwrap();
        let temp = tempfile::tempdir_in(executable.parent().unwrap()).unwrap();
        let daemon_binary = temp.path().join("agent-browser");
        let release = temp.path().join("release");
        std::fs::hard_link(executable, &daemon_binary).unwrap();
        let mut daemon = std::process::Command::new(&daemon_binary)
            .args(["--exact", "agent_browser::tests::macos_failed_open_daemon_fixture", "--nocapture"])
            .env("CODEMUX_TEST_FAILED_OPEN_SESSION", &session)
            .env("CODEMUX_TEST_FAILED_OPEN_RELEASE", &release)
            .env("CODEMUX_BROWSER_OWNER", browser_owner_token())
            .env("CODEMUX_BROWSER_SESSION", &session)
            .env("CODEMUX_BROWSER_OPEN_ATTEMPT", &attempt)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn().unwrap();
        let started = Instant::now();
        while browser_process_start(daemon.id(), &session, browser_owner_token()).is_none() {
            assert!(started.elapsed() < Duration::from_secs(3), "fixture daemon did not start");
            std::thread::sleep(Duration::from_millis(25));
        }
        let dir = run_dir_for_session(&session).unwrap();
        let pid_path = dir.join(format!("{session}.pid"));
        let stream_path = dir.join(format!("{session}.stream"));
        let socket_path = dir.join(format!("{session}.sock"));
        assert!(!pid_path.exists(), "fixture published its PID before rollback");

        std::fs::write(&release, b"go").unwrap();
        rollback_failed_unix_stream_open(&session, None, &attempt);
        let started = Instant::now();
        let stopped = loop {
            if daemon.try_wait().unwrap().is_some() { break true; }
            if started.elapsed() >= Duration::from_secs(3) { break false; }
            std::thread::sleep(Duration::from_millis(25));
        };
        if !stopped {
            let _ = daemon.kill();
            let _ = daemon.wait();
        }
        assert!(stopped, "failed macOS open left its late daemon running");
        assert!(!pid_path.exists(), "close left the old daemon PID file");
        assert!(!stream_path.exists(), "close left the old stream file");
        assert!(!socket_path.exists(), "close left the old control socket");
        assert!(!session_pid_path(&session).unwrap().exists(), "close left the old Codemux record");
        assert!(!failed_stream_attempts().lock().unwrap().get(&session)
            .is_some_and(|attempts| attempts.contains(&attempt)));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_shutdown_rejects_a_replaced_control_endpoint() {
        use std::io::Read;
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::net::UnixListener;

        struct ChildGuard(std::process::Child);
        impl Drop for ChildGuard {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        struct RuntimeFiles([PathBuf; 2]);
        impl Drop for RuntimeFiles {
            fn drop(&mut self) {
                for path in &self.0 {
                    let _ = std::fs::remove_file(path);
                }
            }
        }

        let session = format!("browser-replaced-close-{}", uuid::Uuid::new_v4().simple());
        let (temp, daemon, record) = browser_identity_fixture(&session, browser_owner_token());
        let mut daemon = ChildGuard(daemon);
        let dir = run_dir_for_session(&session).unwrap();
        let pid_path = dir.join(format!("{session}.pid"));
        let socket_path = dir.join(format!("{session}.sock"));
        let _runtime_files = RuntimeFiles([pid_path.clone(), socket_path.clone()]);
        std::fs::write(&pid_path, record.pid.to_string()).unwrap();
        let replacement = UnixListener::bind(&socket_path).unwrap();

        // The old CLI would resolve the replaced socket after validating the
        // daemon. A marker also catches attempts to invoke that CLI at all.
        let marker = temp.path().join("close-called");
        let cli = temp.path().join("agent-browser-close-marker");
        std::fs::write(&cli, format!("#!/bin/sh\nprintf called > {}\n", shell_quote(marker.to_str().unwrap()))).unwrap();
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700)).unwrap();
        stop_owned_browser_daemon(&session, &record, cli.to_str().unwrap());

        assert!(!marker.exists(), "shutdown must not resolve the endpoint through the CLI");
        replacement.set_nonblocking(true).unwrap();
        let started = Instant::now();
        let mut connections = 0;
        loop {
            match replacement.accept() {
                Ok((mut connection, _)) => {
                    connection.set_nonblocking(false).unwrap();
                    connection.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
                    let mut request = Vec::new();
                    connection.read_to_end(&mut request).unwrap();
                    assert!(request.is_empty(), "replacement peer must not receive close");
                    connections += 1;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock
                    && connections == 0 && started.elapsed() < Duration::from_secs(2) =>
                {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) => panic!("verified close did not inspect the replacement endpoint: {error}"),
            }
        }
        assert!(connections > 0, "shutdown must inspect the connected peer");
        assert!(daemon.0.try_wait().unwrap().is_some(), "verified daemon must stop through its pidfd");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_close_retires_files_only_after_the_captured_process_exits() {
        let session = format!("browser-close-pidfd-{}", uuid::Uuid::new_v4().simple());
        let (_temp, mut daemon, record) = browser_identity_fixture(&session, browser_owner_token());
        let pidfd = verified_browser_pidfd(&record, &session).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let files = closed_browser_file_fixture(dir.path(), &session, &record, true);
        let record_path = session_pid_path_in(dir.path(), &session).unwrap();

        assert!(!linux_browser_pidfd_exited(&pidfd));
        finish_closed_linux_browser_files(dir.path(), &session, &record, &files, &pidfd);
        assert!(dir.path().join(format!("{session}.pid")).exists());
        assert!(record_path.exists());

        daemon.kill().unwrap();
        daemon.wait().unwrap();
        assert!(linux_browser_pidfd_exited(&pidfd));
        finish_closed_linux_browser_files(dir.path(), &session, &record, &files, &pidfd);
        assert!(!dir.path().join(format!("{session}.pid")).exists());
        assert!(!record_path.exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_close_keeps_the_owner_record_while_a_stale_pid_remains() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let current = root.join(BROWSER_DAEMON_DIR);
        std::fs::create_dir(&current).unwrap();
        let roots = [root.to_path_buf()];
        let session = "agent-browser-collision";
        let pid_path = current.join(format!("{session}.pid"));
        assert!(linux_browser_close_namespaces_clear_in(session, &roots));
        std::fs::write(&pid_path, "12345").unwrap();
        assert!(!linux_browser_close_namespaces_clear_in(session, &roots));
        std::fs::remove_file(&pid_path).unwrap();
        assert!(linux_browser_close_namespaces_clear_in(session, &roots));

        let foreign_record = root.join(format!("{session}.pid"));
        let record = test_browser_record(12345, 1, browser_owner_token().to_owned());
        write_browser_pid_record(&foreign_record, &record).unwrap();
        assert!(linux_browser_close_namespaces_clear_in(session, &roots));
        std::fs::write(&foreign_record, "12345").unwrap();
        assert!(!linux_browser_close_namespaces_clear_in(session, &roots));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_close_skips_impossible_long_socket_paths() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("a".repeat(95));
        std::fs::create_dir(&root).unwrap();
        let session = "browser-long-control-path";
        assert!(!browser_socket_path_fits(&root, session));
        assert!(linux_browser_close_namespaces_clear_in(session, &[root]));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn pidfd_signal_stays_bound_to_the_verified_browser_process() {
        let session = format!("browser-pidfd-{}", uuid::Uuid::new_v4().simple());
        let (_temp, mut daemon, record) = browser_identity_fixture(&session, browser_owner_token());
        let pidfd = verified_browser_pidfd(&record, &session).expect("verified daemon has a pidfd");
        let stale = BrowserPidRecord { start_time: record.start_time + 1, ..record.clone() };
        assert!(verified_browser_pidfd(&stale, &session).is_none());
        assert!(daemon.try_wait().unwrap().is_none());

        daemon.kill().unwrap();
        daemon.wait().unwrap();
        let error = signal_browser_pidfd(&pidfd, libc::SIGKILL).unwrap_err();
        assert_eq!(error.raw_os_error(), Some(libc::ESRCH));
    }

    #[cfg(unix)]
    #[test]
    fn foreign_daemon_survives_stream_start_and_failed_open() {
        let session = format!("browser-foreign-open-{}", uuid::Uuid::new_v4().simple());
        let token = uuid::Uuid::new_v4().simple().to_string();
        let (temp, mut child, record) = browser_identity_fixture(&session, &token);
        let daemon_pid_path = run_dir_for_session(&session)
            .unwrap()
            .join(format!("{session}.pid"));
        std::fs::write(&daemon_pid_path, child.id().to_string()).unwrap();
        write_session_pid(&session, &record).unwrap();

        let start = stop_previous_browser_daemon_before_stream(&session, temp.path().join("agent-browser").to_str().unwrap());
        assert!(start.as_ref().is_err_and(|error| error.contains("another running or unverified app instance")), "{start:?}");
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
    fn pane_shutdown_does_not_stop_a_live_foreign_owner() {
        let session = format!("browser-foreign-close-{}", uuid::Uuid::new_v4().simple());
        let token = uuid::Uuid::new_v4().simple().to_string();
        let (_temp, mut daemon, mut record) = browser_identity_fixture(&session, &token);
        let daemon_pid_path = run_dir_for_session(&session)
            .unwrap()
            .join(format!("{session}.pid"));
        std::fs::write(&daemon_pid_path, daemon.id().to_string()).unwrap();

        let mut owner = std::process::Command::new(which::which("sleep").unwrap())
            .arg("30")
            .spawn()
            .unwrap();
        record.owner_pid = owner.id();
        let started = Instant::now();
        record.owner_start_time = loop {
            if let Some(start_time) = process_start_time(owner.id()) {
                break start_time;
            }
            assert!(started.elapsed() < Duration::from_secs(3));
            std::thread::sleep(Duration::from_millis(25));
        };
        write_session_pid(&session, &record).unwrap();
        assert!(!previous_app_has_exited(&record));
        // A locally recorded PID does not authorize this pane to close a
        // daemon that still belongs to another app instance.
        assert!(shutdown_record_owned_by_this_app(&session, Some(daemon.id())).is_none());
        assert!(daemon.try_wait().unwrap().is_none(), "pane shutdown targeted the foreign daemon");

        let _ = daemon.kill();
        let _ = daemon.wait();
        let _ = owner.kill();
        let _ = owner.wait();
        let _ = std::fs::remove_file(daemon_pid_path);
        clear_session_pid(&session);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn daemon_from_an_exited_app_can_be_reclaimed() {
        let session = format!("browser-dead-owner-{}", uuid::Uuid::new_v4().simple());
        let token = uuid::Uuid::new_v4().simple().to_string();
        let (temp, mut daemon, mut record) = browser_identity_fixture(&session, &token);
        let daemon_pid_path = run_dir_for_session(&session)
            .unwrap()
            .join(format!("{session}.pid"));
        std::fs::write(&daemon_pid_path, daemon.id().to_string()).unwrap();

        let mut prior_app = std::process::Command::new(which::which("sleep").unwrap())
            .arg("30")
            .spawn()
            .unwrap();
        record.owner_pid = prior_app.id();
        let started = Instant::now();
        record.owner_start_time = loop {
            if let Some(start_time) = process_start_time(prior_app.id()) {
                break start_time;
            }
            assert!(started.elapsed() < Duration::from_secs(3));
            std::thread::sleep(Duration::from_millis(25));
        };
        write_session_pid(&session, &record).unwrap();
        assert!(!previous_app_has_exited(&record));
        prior_app.kill().unwrap();
        prior_app.wait().unwrap();
        assert!(previous_app_has_exited(&record));

        let reaper = std::thread::spawn(move || {
            let start = Instant::now();
            loop {
                if daemon.try_wait().unwrap().is_some() {
                    return true;
                }
                if start.elapsed() >= Duration::from_secs(5) {
                    let _ = daemon.kill();
                    let _ = daemon.wait();
                    return false;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        });
        let reclaimed = stop_previous_browser_daemon_before_stream(
            &session,
            temp.path().join("agent-browser").to_str().unwrap(),
        );
        let stopped = reaper.join().unwrap();
        let _ = std::fs::remove_file(daemon_pid_path);
        clear_session_pid(&session);
        assert!(reclaimed.is_ok(), "{reclaimed:?}");
        assert!(stopped, "daemon survived reclaim from an exited app");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn failed_first_command_tracks_daemon_for_app_exit_cleanup() {
        let session = format!("browser-failed-command-{}", uuid::Uuid::new_v4().simple());
        let (temp, mut child, record) = browser_identity_fixture(&session, browser_owner_token());
        let daemon_pid_path = run_dir_for_session(&session)
            .unwrap()
            .join(format!("{session}.pid"));
        std::fs::write(&daemon_pid_path, child.id().to_string()).unwrap();

        let mut failed = std::process::Command::new("sh");
        failed.args(["-c", "exit 7"]);
        let output = output_capture_and_track_browser_daemon(failed, Duration::from_secs(2), &session).unwrap();
        assert!(!output.status.success());
        let tracked_path = session_pid_path(&session).unwrap();
        assert_eq!(read_browser_pid_record(&tracked_path).unwrap().pid, record.pid);

        std::fs::remove_file(&tracked_path).unwrap();
        let mut timed_out = std::process::Command::new("sh");
        timed_out.args(["-c", "sleep 30"]);
        assert!(output_capture_and_track_browser_daemon(timed_out, Duration::from_millis(100), &session).is_err());
        assert_eq!(read_browser_pid_record(&tracked_path).unwrap().pid, record.pid);

        let cleanup_record = session_pid_path_in(temp.path(), &session).unwrap();
        std::fs::copy(&tracked_path, &cleanup_record).unwrap();
        kill_owned_browser_daemons_in(temp.path());
        let start = Instant::now();
        let exited = loop {
            if child.try_wait().unwrap().is_some() {
                break true;
            }
            if start.elapsed() >= Duration::from_secs(3) {
                break false;
            }
            std::thread::sleep(Duration::from_millis(25));
        };
        if !exited {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_file(daemon_pid_path);
        clear_session_pid(&session);
        assert!(exited, "app exit cleanup missed the failed command's daemon");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn failed_first_json_command_daemon_cannot_satisfy_stream_fast_path() {
        let session = format!("browser-unready-{}", uuid::Uuid::new_v4().simple());
        let (_temp, mut daemon, _record) = browser_identity_fixture(&session, browser_owner_token());
        let daemon_pid_path = run_dir_for_session(&session).unwrap().join(format!("{session}.pid"));
        std::fs::write(&daemon_pid_path, daemon.id().to_string()).unwrap();

        let manager = AgentBrowserManager::new();
        manager.allocate_port(&session).await.unwrap();
        assert!(!manager.note_action_start(&session, "get_styles").await);
        // JSON error output is still returned to the caller, but its failed
        // process status cannot establish that a browser attached.
        assert!(browser_action_has_usable_output(false, r#"{"error":"browser launch failed"}"#));
        let result = Ok((BrowserAutomationResult {
            request_id: "failed-json".into(),
            browser_id: session.clone(),
            data: serde_json::json!({"error": "browser launch failed"}),
            message: None,
        }, BrowserActionOutcome::Failed));
        manager.reconcile_action_result(&session, &session, "get_styles", &result).await;
        assert!(result.is_ok());
        let sessions = manager.sessions.lock().await;
        let tracked = sessions.get(&session).unwrap();
        assert!(tracked.running);
        assert_eq!(tracked.pid, Some(daemon.id()));
        assert!(!stream_can_reuse_browser(tracked));
        drop(sessions);

        let _ = daemon.kill();
        let _ = daemon.wait();
        let _ = std::fs::remove_file(daemon_pid_path);
        clear_session_pid(&session);
    }

    #[test]
    fn confirmation_response_does_not_complete_a_browser_action() {
        assert!(!browser_action_completed(true, "Confirmation required:\n  Navigate to example.com\n  Run: agent-browser confirm c1\n  Or:  agent-browser deny c1\n"));
        let policy_reply = "Confirmation required:\n  navigation: example.com\n  Run: agent-browser confirm \n  Or:  agent-browser deny \n";
        assert_eq!(browser_action_outcome(true, policy_reply), BrowserActionOutcome::ConfirmationPending);
        assert!(!browser_action_completed(
            true,
            r#"{"id":"c1","success":true,"data":{"confirmation_required":true}}"#,
        ));
        assert!(!browser_action_completed(
            true,
            r#"{"id":"c1","success":true,"data":{"result":{"data":{"confirmation_required":true}}}}"#,
        ));
        assert!(!browser_action_completed(true, r#"{"success":false,"data":{"error":"denied"}}"#));
        assert!(!browser_action_completed(true, r#"{"success":true,"data":{"confirmation_required":true}}"#));
        assert!(browser_action_completed(true, r#"{"success":true,"data":{"url":"about:blank"}}"#));
        assert!(browser_action_completed(true, "✓ Confirmation required: a page title\n  https://example.com\n"));
        assert!(browser_action_completed(true, "✓ Page title\n  https://example.com/Confirmation required:\n"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn automation_confirmation_keeps_its_daemon_for_pane_retry() {
        let session = format!("browser-confirm-action-{}", uuid::Uuid::new_v4().simple());
        let (_temp, mut daemon, _record) = browser_identity_fixture(&session, browser_owner_token());
        let daemon_pid_path = run_dir_for_session(&session).unwrap().join(format!("{session}.pid"));
        std::fs::write(&daemon_pid_path, daemon.id().to_string()).unwrap();
        let manager = AgentBrowserManager::new();
        manager.allocate_port(&session).await.unwrap();
        let policy_reply = "Confirmation required:\n  navigation: example.com\n  Run: agent-browser confirm \n  Or:  agent-browser deny \n";
        let result = Ok((BrowserAutomationResult {
            request_id: "pending-open".into(),
            browser_id: session.clone(),
            data: serde_json::json!({"result": policy_reply}),
            message: None,
        }, browser_action_outcome(true, policy_reply)));

        manager.reconcile_action_result(&session, &session, "open", &result).await;
        let sessions = manager.sessions.lock().await;
        let tracked = sessions.get(&session).unwrap();
        assert!(tracked.running);
        assert!(!tracked.browser_ready);
        assert!(tracked.confirmation_pending);
        assert_eq!(tracked.pid, Some(daemon.id()));
        drop(sessions);

        let retry = manager.reconcile_existing_stream_attachment(
            &session, &session, BrowserAttachment::Unattached,
        ).await;
        assert!(retry.unwrap_err().contains("confirmation is pending"));
        assert!(manager.sessions.lock().await.get(&session).unwrap().confirmation_pending);

        // A temporary failure to read this daemon's PID must not let spawn
        // send another open and overwrite the pending request.
        std::fs::remove_file(&daemon_pid_path).unwrap();
        let uncertain = manager.spawn(&session).await.unwrap_err();
        assert!(uncertain.contains("Could not verify pending browser daemon"));
        assert!(manager.sessions.lock().await.get(&session).unwrap().confirmation_pending);
        assert!(daemon.try_wait().unwrap().is_none());
        std::fs::write(&daemon_pid_path, daemon.id().to_string()).unwrap();

        let spawn = manager.spawn(&session).await;
        assert!(spawn.unwrap_err().contains("confirmation is pending"));
        assert!(daemon.try_wait().unwrap().is_none());
        let tracked = manager.sessions.lock().await;
        assert!(tracked.get(&session).unwrap().confirmation_pending);
        assert_eq!(tracked.get(&session).unwrap().pid, Some(daemon.id()));
        drop(tracked);

        daemon.kill().unwrap();
        daemon.wait().unwrap();
        let _ = std::fs::remove_file(daemon_pid_path);
        clear_session_pid(&session);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn stream_attachment_requires_a_connected_status_from_its_listener() {
        use futures_util::SinkExt;

        for (connected, expected) in [
            (false, BrowserAttachment::Unattached),
            (true, BrowserAttachment::Attached),
        ] {
            let listener = Arc::new(tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap());
            let port = listener.local_addr().unwrap().port();
            let server_listener = listener.clone();
            let server = tokio::spawn(async move {
                let (stream, _) = server_listener.accept().await.unwrap();
                let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
                socket.send(tokio_tungstenite::tungstenite::Message::Text(
                    format!(r#"{{"type":"status","connected":{connected}}}"#).into(),
                )).await.unwrap();
            });
            assert_eq!(browser_stream_attached_to_daemon(port, std::process::id(), 1).await, expected);
            server.await.unwrap();
        }

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            tokio::time::sleep(Duration::from_millis(900)).await;
            let _ = socket.send(tokio_tungstenite::tungstenite::Message::Text(
                r#"{"type":"status","connected":true}"#.into(),
            )).await;
        });
        assert_eq!(
            browser_stream_attached_to_daemon(port, std::process::id(), 1).await,
            BrowserAttachment::Inconclusive,
        );
        server.abort();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            socket.send(tokio_tungstenite::tungstenite::Message::Text(
                r#"{"type":"frame","connected":false}"#.into(),
            )).await.unwrap();
        });
        assert_eq!(
            browser_stream_attached_to_daemon(port, std::process::id(), 1).await,
            BrowserAttachment::Inconclusive,
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn inconclusive_attachment_preserves_a_ready_stream_for_retry() {
        let manager = AgentBrowserManager::new();
        let session = format!("browser-status-delay-{}", uuid::Uuid::new_v4().simple());
        let port = manager.allocate_port(&session).await.unwrap();
        {
            let mut sessions = manager.sessions.lock().await;
            let stream = sessions.get_mut(&session).unwrap();
            stream.running = true;
            stream.browser_ready = true;
            stream.pid = Some(std::process::id());
        }

        let error = manager.reconcile_existing_stream_attachment(
            &session,
            &session,
            BrowserAttachment::Inconclusive,
        ).await.unwrap_err();
        assert!(error.contains("Could not verify browser attachment"));
        let sessions = manager.sessions.lock().await;
        let stream = sessions.get(&session).unwrap();
        assert_eq!(stream.port, port);
        assert!(stream.running);
        assert!(stream.browser_ready);
        assert_eq!(stream.pid, Some(std::process::id()));
    }

    #[tokio::test]
    async fn inconclusive_listener_lookup_preserves_a_ready_stream_for_retry() {
        let manager = AgentBrowserManager::new();
        let session = format!("browser-listener-lookup-{}", uuid::Uuid::new_v4().simple());
        let port = manager.allocate_port(&session).await.unwrap();
        {
            let mut sessions = manager.sessions.lock().await;
            let stream = sessions.get_mut(&session).unwrap();
            stream.running = true;
            stream.browser_ready = true;
            stream.pid = Some(std::process::id());
        }

        let error = manager.reconcile_existing_stream_listener(
            &session, &session, port, None,
        ).await.unwrap_err();
        assert!(error.contains("Could not verify stream listener ownership"));
        assert!(manager.reconcile_existing_stream_listener(
            &session, &session, port, Some(true),
        ).await.is_ok());
        let sessions = manager.sessions.lock().await;
        let stream = sessions.get(&session).unwrap();
        assert_eq!(stream.port, port);
        assert!(stream.running);
        assert!(stream.browser_ready);
        assert_eq!(stream.pid, Some(std::process::id()));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn inconclusive_process_lookup_preserves_a_ready_stream_for_retry() {
        let session = format!("browser-process-lookup-{}", uuid::Uuid::new_v4().simple());
        let (_temp, mut daemon, record) = browser_identity_fixture(&session, browser_owner_token());
        let manager = AgentBrowserManager::new();
        let port = manager.allocate_port(&session).await.unwrap();
        {
            let mut sessions = manager.sessions.lock().await;
            let stream = sessions.get_mut(&session).unwrap();
            stream.running = true;
            stream.browser_ready = true;
            stream.pid = Some(daemon.id());
        }

        for ownership in [Some(true), None] {
            let error = manager.reconcile_existing_stream_process(
                &session, &session, port, daemon.id(), None, ownership,
            ).await.err().unwrap();
            assert!(error.contains("Could not verify"));
            let sessions = manager.sessions.lock().await;
            let stream = sessions.get(&session).unwrap();
            assert_eq!(stream.port, port);
            assert!(stream.running);
            assert!(stream.browser_ready);
            assert_eq!(stream.pid, Some(daemon.id()));
        }

        let verified = manager.reconcile_existing_stream_process(
            &session, &session, port, daemon.id(), Some(record), Some(true),
        ).await.unwrap();
        assert_eq!(verified.pid, daemon.id());
        assert!(manager.reconcile_existing_stream_attachment(
            &session, &session, BrowserAttachment::Attached,
        ).await.unwrap());

        daemon.kill().unwrap();
        daemon.wait().unwrap();
    }

    #[tokio::test]
    async fn first_pending_action_without_a_pid_keeps_its_stream_allocation() {
        let manager = AgentBrowserManager::new();
        let session = format!("browser-first-pending-{}", uuid::Uuid::new_v4().simple());
        let port = manager.allocate_port(&session).await.unwrap();
        manager.note_action_start(&session, "open").await;
        let result = Ok((BrowserAutomationResult {
            request_id: "first-pending".into(),
            browser_id: session.clone(),
            data: serde_json::json!({"confirmation_required": true}),
            message: None,
        }, BrowserActionOutcome::ConfirmationPending));
        manager.reconcile_action_result(&session, &session, "open", &result).await;

        let tracked = manager.sessions.lock().await;
        let entry = tracked.get(&session).unwrap();
        assert_eq!(entry.port, port);
        assert!(entry.running && entry.confirmation_pending);
        assert!(!entry.browser_ready);
        assert_eq!(entry.pid, None);
        drop(tracked);

        let retry = manager.start_stream_locked(&session).await.unwrap_err();
        assert!(retry.contains("Could not verify ownership"), "{retry}");
        assert_eq!(manager.get_port(&session).await, Some(port));

        #[cfg(unix)]
        {
            let (_temp, mut daemon, _) = browser_identity_fixture(&session, browser_owner_token());
            let daemon_pid_path = run_dir_for_session(&session).unwrap().join(format!("{session}.pid"));
            std::fs::write(&daemon_pid_path, daemon.id().to_string()).unwrap();
            let pane_retry = manager.start_stream_locked(&session).await.unwrap_err();
            assert!(pane_retry.contains("confirmation is pending"), "{pane_retry}");
            assert_eq!(manager.sessions.lock().await.get(&session).unwrap().pid, Some(daemon.id()));
            let retry = manager.spawn(&session).await.unwrap_err();
            assert!(retry.contains("confirmation is pending"), "{retry}");
            assert!(daemon.try_wait().unwrap().is_none());
            assert!(manager.sessions.lock().await.get(&session).unwrap().confirmation_pending);
            let _ = daemon.kill();
            let _ = daemon.wait();
            let _ = std::fs::remove_file(daemon_pid_path);
            clear_session_pid(&session);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn transient_daemon_pid_lookup_preserves_a_ready_browser() {
        let session = format!("browser-ready-lookup-{}", uuid::Uuid::new_v4().simple());
        let (_temp, mut daemon, record) = browser_identity_fixture(&session, browser_owner_token());
        let daemon_pid_path = run_dir_for_session(&session).unwrap().join(format!("{session}.pid"));
        std::fs::write(&daemon_pid_path, daemon.id().to_string()).unwrap();
        let manager = AgentBrowserManager::new();
        let port = manager.allocate_port(&session).await.unwrap();
        {
            let mut sessions = manager.sessions.lock().await;
            let stream = sessions.get_mut(&session).unwrap();
            stream.running = true;
            stream.browser_ready = true;
            stream.pid = Some(record.pid);
            stream.pid_start_time = Some(record.start_time);
        }

        std::fs::remove_file(&daemon_pid_path).unwrap();
        let uncertain = manager.refresh_running_session(&session, &session, BrowserActionOutcome::Failed).await.unwrap_err();
        assert!(uncertain.contains("Could not verify browser daemon"), "{uncertain}");
        let tracked = manager.sessions.lock().await;
        let stream = tracked.get(&session).unwrap();
        assert_eq!(stream.port, port);
        assert!(stream.running && stream.browser_ready);
        assert_eq!(stream.pid_start_time, Some(record.start_time));
        drop(tracked);

        std::fs::write(&daemon_pid_path, daemon.id().to_string()).unwrap();
        manager.spawn(&session).await.unwrap();
        assert!(daemon.try_wait().unwrap().is_none());
        assert!(manager.sessions.lock().await.get(&session).unwrap().browser_ready);
        let _ = daemon.kill();
        let _ = daemon.wait();
        let _ = std::fs::remove_file(daemon_pid_path);
        clear_session_pid(&session);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn exited_daemon_releases_its_occupied_stream_port() {
        let manager = AgentBrowserManager::new();
        let session = format!("browser-dead-port-{}", uuid::Uuid::new_v4().simple());
        let port = manager.allocate_port(&session).await.unwrap();
        let listener = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
        let mut daemon = std::process::Command::new("sh")
            .args(["-c", "true"])
            .spawn().unwrap();
        let dead_pid = daemon.id();
        daemon.wait().unwrap();
        assert!(process_exit_confirmed(dead_pid));
        {
            let mut sessions = manager.sessions.lock().await;
            let stream = sessions.get_mut(&session).unwrap();
            stream.running = true;
            stream.browser_ready = true;
            stream.pid = Some(dead_pid);
        }

        let error = manager.reconcile_existing_stream_process(
            &session, &session, port, dead_pid, None, None,
        ).await.err().unwrap();
        assert!(error.contains("daemon exited"));
        assert!(manager.get_port(&session).await.is_none());
        assert_ne!(manager.allocate_port(&session).await.unwrap(), port);
        drop(listener);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[tokio::test]
    async fn recycled_pid_releases_stale_stream_without_touching_the_new_process() {
        let manager = AgentBrowserManager::new();
        let session = format!("browser-reused-{}", uuid::Uuid::new_v4().simple());
        let port = manager.allocate_port(&session).await.unwrap();
        let listener = std::net::TcpListener::bind(("127.0.0.1", port)).unwrap();
        let pid = std::process::id();
        let start_time = process_start_time(pid).unwrap();
        let old = BrowserPidRecord {
            pid,
            start_time: different_native_start_in_same_second(start_time),
            token: browser_owner_token().to_owned(),
            owner_pid: pid,
            owner_start_time: start_time,
            native_start: true,
        };
        let dir = run_dir_for_session(&session).unwrap();
        let daemon_pid_path = dir.join(format!("{session}.pid"));
        std::fs::write(&daemon_pid_path, pid.to_string()).unwrap();
        write_session_pid(&session, &old).unwrap();
        {
            let mut sessions = manager.sessions.lock().await;
            let stream = sessions.get_mut(&session).unwrap();
            stream.running = true;
            stream.browser_ready = true;
            stream.pid = Some(pid);
            stream.pid_start_time = Some(old.start_time);
        }

        let error = manager.reconcile_existing_stream_process(
            &session, &session, port, pid, None, None,
        ).await.err().unwrap();
        assert!(error.contains("daemon was replaced"), "{error}");
        assert!(manager.get_port(&session).await.is_none());
        assert!(!daemon_pid_path.exists());
        assert!(!session_pid_path(&session).unwrap().exists());
        assert!(listener.local_addr().is_ok());
        assert_eq!(process_start_time(pid), Some(start_time));
        assert_ne!(manager.allocate_port(&session).await.unwrap(), port);
        drop(listener);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn recycled_pid_files_are_kept_for_a_live_endpoint_or_changed_owner() {
        let session = format!("reused-{}", uuid::Uuid::new_v4().simple());
        let dir = run_dir_for_session(&session).unwrap();
        let pid = std::process::id();
        let start_time = process_start_time(pid).unwrap();
        let old = BrowserPidRecord {
            pid,
            start_time: different_native_start_in_same_second(start_time),
            token: browser_owner_token().to_owned(),
            owner_pid: pid,
            owner_start_time: start_time,
            native_start: true,
        };
        let daemon_pid_path = dir.join(format!("{session}.pid"));
        std::fs::write(&daemon_pid_path, pid.to_string()).unwrap();
        write_session_pid(&session, &old).unwrap();
        let socket_path = dir.join(format!("{session}.sock"));
        let endpoint = std::os::unix::net::UnixListener::bind(&socket_path).unwrap();
        assert!(!cleanup_reused_browser_lock_files_in(&dir, &session, &old));
        assert!(daemon_pid_path.exists());
        drop(endpoint);
        std::fs::remove_file(socket_path).unwrap();

        let mut changed = old.clone();
        changed.token = uuid::Uuid::new_v4().simple().to_string();
        write_session_pid(&session, &changed).unwrap();
        assert!(!cleanup_reused_browser_lock_files_in(&dir, &session, &old));
        assert!(!cleanup_reused_browser_lock_files_in(&dir, &session, &changed));
        assert!(read_browser_pid_record(&session_pid_path(&session).unwrap()).as_ref() == Some(&changed));
        let _ = std::fs::remove_file(daemon_pid_path);
        clear_session_pid(&session);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn recycled_pid_lock_does_not_block_a_new_browser_session() {
        let session = format!("reused-start-{}", uuid::Uuid::new_v4().simple());
        let dir = run_dir_for_session(&session).unwrap();
        let pid = std::process::id();
        let start_time = process_start_time(pid).unwrap();
        let old = BrowserPidRecord {
            pid,
            start_time: different_native_start_in_same_second(start_time),
            token: browser_owner_token().to_owned(),
            owner_pid: pid,
            owner_start_time: start_time,
            native_start: true,
        };
        let daemon_pid_path = dir.join(format!("{session}.pid"));
        std::fs::write(&daemon_pid_path, pid.to_string()).unwrap();
        write_session_pid(&session, &old).unwrap();

        assert!(require_owned_browser_session_in(&session, &dir).is_ok());
        assert!(!daemon_pid_path.exists());
        assert!(!session_pid_path(&session).unwrap().exists());
        assert_eq!(process_start_time(pid), Some(start_time));
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn recycled_pid_cleanup_keeps_legacy_record_name() {
        let dir = tempfile::tempdir().unwrap();
        let session = "reused-legacy-record";
        let pid = std::process::id();
        let start_time = process_start_time(pid).unwrap();
        let old = BrowserPidRecord {
            pid,
            start_time: different_native_start_in_same_second(start_time),
            token: browser_owner_token().to_owned(),
            owner_pid: pid,
            owner_start_time: start_time,
            native_start: true,
        };
        let daemon_pid = dir.path().join(format!("{session}.pid"));
        let legacy = legacy_session_pid_path_in(dir.path(), session).unwrap();
        std::fs::write(&daemon_pid, pid.to_string()).unwrap();
        write_browser_pid_record(&legacy, &old).unwrap();

        assert!(cleanup_reused_browser_lock_files_in(dir.path(), session, &old));
        assert!(!daemon_pid.exists());
        assert_eq!(read_browser_pid_record(&legacy), Some(old));
        assert_eq!(process_start_time(pid), Some(start_time));
    }

    #[tokio::test]
    async fn pending_stream_confirmation_survives_a_retry() {
        let manager = AgentBrowserManager::new();
        let session = format!("browser-confirm-{}", uuid::Uuid::new_v4().simple());
        let port = manager.allocate_port(&session).await.unwrap();
        let pid = std::process::id();
        let record = BrowserPidRecord {
            pid,
            start_time: process_start_time(pid).unwrap(),
            token: browser_owner_token().to_owned(),
            owner_pid: pid,
            owner_start_time: process_start_time(pid).unwrap(),
            native_start: true,
        };
        manager.note_stream_open_confirmation_pending(&session, true, Some(record.clone())).await;

        let error = manager.reconcile_existing_stream_attachment(
            &session,
            &session,
            BrowserAttachment::Unattached,
        ).await.unwrap_err();
        assert!(error.contains("confirmation is pending"));
        let sessions = manager.sessions.lock().await;
        let stream = sessions.get(&session).unwrap();
        assert_eq!(stream.port, port);
        assert!(stream.running);
        assert!(stream.confirmation_pending);
        assert_eq!(stream.pid, Some(std::process::id()));
        assert_eq!(stream.pid_start_time, Some(record.start_time));
        drop(sessions);

        assert!(manager.reconcile_existing_stream_attachment(
            &session,
            &session,
            BrowserAttachment::Attached,
        ).await.unwrap());
        let sessions = manager.sessions.lock().await;
        let stream = sessions.get(&session).unwrap();
        assert!(stream.browser_ready);
        assert!(!stream.confirmation_pending);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn successful_command_refreshes_a_replacement_daemon_pid() {
        let session = format!("browser-replacement-{}", uuid::Uuid::new_v4().simple());
        let (_old_temp, mut old, _old_record) = browser_identity_fixture(&session, browser_owner_token());
        let daemon_pid_path = run_dir_for_session(&session).unwrap().join(format!("{session}.pid"));
        std::fs::write(&daemon_pid_path, old.id().to_string()).unwrap();
        let manager = AgentBrowserManager::new();
        manager.allocate_port(&session).await.unwrap();
        {
            let mut sessions = manager.sessions.lock().await;
            let stream = sessions.get_mut(&session).unwrap();
            stream.running = true;
            stream.browser_ready = true;
            stream.pid = Some(old.id());
        }
        old.kill().unwrap();
        old.wait().unwrap();
        let (_new_temp, mut replacement, _new_record) = browser_identity_fixture(&session, browser_owner_token());
        std::fs::write(&daemon_pid_path, replacement.id().to_string()).unwrap();

        let result = Ok((BrowserAutomationResult {
            request_id: "replacement-open".into(),
            browser_id: session.clone(),
            data: serde_json::json!({"result": "opened"}),
            message: None,
        }, BrowserActionOutcome::Completed));
        manager.reconcile_action_result(&session, &session, "open", &result).await;
        let sessions = manager.sessions.lock().await;
        let tracked = sessions.get(&session).unwrap();
        assert!(stream_can_reuse_browser(tracked));
        assert_eq!(tracked.pid, Some(replacement.id()));
        drop(sessions);

        let _ = replacement.kill();
        let _ = replacement.wait();
        let _ = std::fs::remove_file(daemon_pid_path);
        clear_session_pid(&session);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn post_open_wrong_listener_rolls_back_its_daemon() {
        let session = format!("browser-wrong-port-{}", uuid::Uuid::new_v4().simple());
        let attempt = uuid::Uuid::new_v4().simple().to_string();
        let (_temp, mut daemon, _record) = browser_identity_fixture_with_attempt(
            &session, browser_owner_token(), Some(&attempt),
        );
        let daemon_pid_path = run_dir_for_session(&session).unwrap().join(format!("{session}.pid"));
        std::fs::write(&daemon_pid_path, daemon.id().to_string()).unwrap();
        // Another process wins the selected port after the bind check. The
        // sidecar can still report a successful open on a fallback port.
        let other_listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = other_listener.local_addr().unwrap().port();
        let result = verify_unix_stream_after_open(&session, port, None, &attempt);
        assert!(result.as_ref().is_err_and(|error| error.contains("not owned")));
        let started = Instant::now();
        while daemon.try_wait().unwrap().is_none() && started.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(25));
        }
        let stopped = daemon.try_wait().unwrap().is_some();
        if !stopped {
            let _ = daemon.kill();
            let _ = daemon.wait();
        }
        assert!(stopped, "failed stream verification left its daemon alive");
        assert!(other_listener.local_addr().is_ok(), "rollback stopped the unrelated listener");
        let _ = std::fs::remove_file(daemon_pid_path);
        clear_session_pid(&session);
    }

    #[cfg(unix)]
    #[test]
    fn failed_stream_attempt_is_ineligible_for_browser_commands() {
        let session = format!("browser-failed-quarantine-{}", uuid::Uuid::new_v4().simple());
        let attempt = uuid::Uuid::new_v4().simple().to_string();
        let (_temp, mut daemon, _record) = browser_identity_fixture_with_attempt(
            &session, browser_owner_token(), Some(&attempt),
        );
        let daemon_pid_path = run_dir_for_session(&session).unwrap().join(format!("{session}.pid"));
        std::fs::write(&daemon_pid_path, daemon.id().to_string()).unwrap();
        mark_failed_stream_attempt(&session, &attempt);
        assert!(require_owned_browser_session_for_cleanup(&session).is_ok());
        assert!(require_owned_browser_session(&session)
            .is_err_and(|error| error.contains("failed stream launch")));

        let _ = daemon.kill();
        let _ = daemon.wait();
        let _ = std::fs::remove_file(daemon_pid_path);
        clear_session_pid(&session);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn pane_close_retries_unconfirmed_failed_stream_daemon() {
        let session = format!("browser-close-failed-{}", uuid::Uuid::new_v4().simple());
        let attempt = uuid::Uuid::new_v4().simple().to_string();
        let (_temp, mut daemon, _record) = browser_identity_fixture_with_attempt(
            &session, browser_owner_token(), Some(&attempt),
        );
        let daemon_pid_path = run_dir_for_session(&session).unwrap().join(format!("{session}.pid"));
        std::fs::write(&daemon_pid_path, daemon.id().to_string()).unwrap();
        let manager = AgentBrowserManager::new();
        let port = manager.allocate_port(&session).await.unwrap();
        manager.clear_failed_stream_allocation(&session, port).await;
        assert!(manager.get_port(&session).await.is_none());
        // Rollback could not confirm exit, so only the launch marker remains
        // after its occupied port allocation was released.
        mark_failed_stream_attempt(&session, &attempt);

        manager.close(&session).await.unwrap();
        let started = Instant::now();
        while daemon.try_wait().unwrap().is_none() && started.elapsed() < Duration::from_secs(3) {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let stopped = daemon.try_wait().unwrap().is_some();
        if !stopped {
            let _ = daemon.kill();
            let _ = daemon.wait();
        }
        assert!(stopped, "pane close did not retry the failed daemon cleanup");
        let _ = std::fs::remove_file(daemon_pid_path);
        clear_session_pid(&session);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn timed_out_command_tracks_daemon_after_late_pid_publication() {
        let session = format!("browser-late-pid-{}", uuid::Uuid::new_v4().simple());
        let (_temp, mut child, record) = browser_identity_fixture(&session, browser_owner_token());
        let daemon_pid_path = run_dir_for_session(&session)
            .unwrap()
            .join(format!("{session}.pid"));
        let tracked_path = session_pid_path(&session).unwrap();
        let pid = child.id();
        let path_for_writer = daemon_pid_path.clone();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            std::fs::write(path_for_writer, pid.to_string()).unwrap();
        });

        let mut timed_out = std::process::Command::new("sh");
        timed_out.args(["-c", "sleep 30"]);
        assert!(output_capture_and_track_browser_daemon(timed_out, Duration::from_millis(100), &session).is_err());
        assert!(!tracked_path.exists(), "daemon was already tracked before its PID was published");
        writer.join().unwrap();

        let started = Instant::now();
        while read_browser_pid_record(&tracked_path).is_none() && started.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(25));
        }
        assert_eq!(read_browser_pid_record(&tracked_path).map(|record| record.pid), Some(record.pid));

        kill_verified_browser_pid(&record, &session);
        let started = Instant::now();
        while child.try_wait().unwrap().is_none() && started.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(25));
        }
        if child.try_wait().unwrap().is_none() {
            let _ = child.kill();
            let _ = child.wait();
            panic!("late-published daemon survived cleanup");
        }
        let _ = std::fs::remove_file(daemon_pid_path);
        clear_session_pid(&session);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn failed_stream_open_stops_its_daemon_after_late_pid_publication() {
        let session = format!("browser-late-open-{}", uuid::Uuid::new_v4().simple());
        let attempt = uuid::Uuid::new_v4().simple().to_string();
        let (_temp, mut daemon, _record) = browser_identity_fixture_with_attempt(
            &session,
            browser_owner_token(),
            Some(&attempt),
        );
        let daemon_pid_path = run_dir_for_session(&session)
            .unwrap()
            .join(format!("{session}.pid"));
        let pid = daemon.id();
        let path_for_writer = daemon_pid_path.clone();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            std::fs::write(path_for_writer, pid.to_string()).unwrap();
        });

        // This is the failed start_stream branch: rollback initially sees no
        // PID, then the detached daemon publishes it after the CLI is gone.
        assert!(!daemon_pid_path.exists());
        rollback_failed_unix_stream_open(&session, None, &attempt);
        writer.join().unwrap();
        let started = Instant::now();
        let stopped = loop {
            if daemon.try_wait().unwrap().is_some() {
                break true;
            }
            if started.elapsed() >= Duration::from_secs(3) {
                break false;
            }
            std::thread::sleep(Duration::from_millis(25));
        };
        if !stopped {
            let _ = daemon.kill();
            let _ = daemon.wait();
        }
        assert!(stopped, "failed stream open left a late-published daemon running");
        assert!(read_browser_pid_record(&session_pid_path(&session).unwrap()).is_none());
        let _ = std::fs::remove_file(daemon_pid_path);
        clear_session_pid(&session);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn failed_stream_rollback_blocks_queued_work_until_late_daemon_is_gone() {
        let session = format!("browser-queued-late-open-{}", uuid::Uuid::new_v4().simple());
        let attempt = uuid::Uuid::new_v4().simple().to_string();
        let daemon_attempt = attempt.clone();
        let rollback_session = session.clone();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let rollback = tokio::spawn(run_detached_browser_lifecycle(session.clone(), async move {
            let _ = started_tx.send(());
            tokio::task::spawn_blocking(move || {
                rollback_failed_unix_stream_open(&rollback_session, None, &attempt);
            }).await.map_err(|error| error.to_string())?;
            Ok(())
        }));
        started_rx.await.unwrap();

        let (queued_tx, mut queued_rx) = tokio::sync::oneshot::channel();
        let queued_session = session.clone();
        let queued = tokio::spawn(run_detached_browser_lifecycle(session.clone(), async move {
            let still_owned = read_agent_browser_daemon_pid(&queued_session)
                .and_then(|pid| owned_browser_pid(&queued_session, pid))
                .is_some();
            let _ = queued_tx.send(still_owned);
            Ok(())
        }));
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut queued_rx).await.is_err(),
            "queued command entered while rollback could still stop its daemon"
        );

        // The daemon appears after the failed launcher has started rollback.
        // The waiting command must not run until this exact attempt is dead.
        let (_temp, mut daemon, _record) = browser_identity_fixture_with_attempt(
            &session, browser_owner_token(), Some(&daemon_attempt),
        );
        let daemon_pid_path = run_dir_for_session(&session).unwrap().join(format!("{session}.pid"));
        std::fs::write(&daemon_pid_path, daemon.id().to_string()).unwrap();
        tokio::time::timeout(Duration::from_secs(7), rollback).await.unwrap().unwrap().unwrap();
        queued.await.unwrap().unwrap();
        assert!(!queued_rx.await.unwrap(), "queued command entered while the failed daemon was still owned");
        let started = Instant::now();
        while daemon.try_wait().unwrap().is_none() && started.elapsed() < Duration::from_secs(3) {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let stopped = daemon.try_wait().unwrap().is_some();
        if !stopped {
            let _ = daemon.kill();
            let _ = daemon.wait();
        }
        assert!(stopped, "queued work could reuse the failed daemon");
        let _ = std::fs::remove_file(daemon_pid_path);
        clear_session_pid(&session);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn failed_stream_open_stops_daemon_without_a_pid_file() {
        let session = format!("browser-no-pid-open-{}", uuid::Uuid::new_v4().simple());
        let attempt = uuid::Uuid::new_v4().simple().to_string();
        let (_temp, mut daemon, _record) = browser_identity_fixture_with_attempt(
            &session,
            browser_owner_token(),
            Some(&attempt),
        );
        assert!(read_agent_browser_daemon_pid(&session).is_none());

        rollback_failed_unix_stream_open(&session, None, &attempt);
        let started = Instant::now();
        let stopped = loop {
            if daemon.try_wait().unwrap().is_some() {
                break true;
            }
            if started.elapsed() >= Duration::from_secs(3) {
                break false;
            }
            std::thread::sleep(Duration::from_millis(25));
        };
        if !stopped {
            let _ = daemon.kill();
            let _ = daemon.wait();
        }
        assert!(stopped, "failed stream open left a daemon without a PID file");
        clear_session_pid(&session);
    }

    #[cfg(unix)]
    #[test]
    fn failed_stream_rollback_does_not_stop_a_new_attempt() {
        let session = format!("browser-new-open-{}", uuid::Uuid::new_v4().simple());
        let failed_attempt = uuid::Uuid::new_v4().simple().to_string();
        let new_attempt = uuid::Uuid::new_v4().simple().to_string();
        let (_temp, mut daemon, _record) = browser_identity_fixture_with_attempt(
            &session,
            browser_owner_token(),
            Some(&new_attempt),
        );
        let daemon_pid_path = run_dir_for_session(&session)
            .unwrap()
            .join(format!("{session}.pid"));
        rollback_failed_unix_stream_open(&session, None, &failed_attempt);
        std::fs::write(&daemon_pid_path, daemon.id().to_string()).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert!(daemon.try_wait().unwrap().is_none(), "failed attempt stopped a newer daemon");

        let _ = daemon.kill();
        let _ = daemon.wait();
        let _ = std::fs::remove_file(daemon_pid_path);
        clear_session_pid(&session);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn app_exit_finds_owned_daemon_without_a_pid_file() {
        let session = format!("browser-untracked-{}", uuid::Uuid::new_v4().simple());
        let (_temp, mut child, _record) = browser_identity_fixture(&session, browser_owner_token());
        assert!(read_agent_browser_daemon_pid(&session).is_none());

        kill_untracked_browser_daemons(Some(&session));
        let started = Instant::now();
        let stopped = loop {
            if child.try_wait().unwrap().is_some() {
                break true;
            }
            if started.elapsed() >= Duration::from_secs(3) {
                break false;
            }
            std::thread::sleep(Duration::from_millis(25));
        };
        if !stopped {
            let _ = child.kill();
            let _ = child.wait();
        }
        assert!(stopped, "app exit missed a daemon before PID publication");
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
        let path = session_pid_path_in(temp.path(), session).unwrap();
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
