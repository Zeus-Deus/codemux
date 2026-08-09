use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU16, Ordering};
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
fn stealth_user_agent() -> String {
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
    mut command: std::process::Command,
    timeout: Duration,
) -> std::io::Result<std::process::Output> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Own process group so a timeout can SIGKILL the whole `sh -c …`
        // subtree, not just the shell wrapper.
        command.process_group(0);
    }
    let mut child = command.spawn()?;
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

/// Kill all agent-browser daemon processes.
/// Called on app shutdown to prevent stale daemons across restarts.
pub fn kill_stream_daemons() {
    #[cfg(windows)]
    {
        // CREATE_NO_WINDOW prevents a console flash on app shutdown when we
        // shell out to taskkill. Same pattern as kill_process_on_port below.
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        // The Tauri-bundled sidecar is renamed to `agent-browser.exe`, but
        // the dev-mode binary at `node_modules/agent-browser/bin/...` keeps
        // its full name `agent-browser-win32-x64.exe`. Kill both forms so
        // restarts don't leak daemons in either deployment mode.
        for image in ["agent-browser.exe", "agent-browser-win32-x64.exe"] {
            let mut cmd = std::process::Command::new("taskkill");
            cmd.args(["/F", "/IM", image])
                .creation_flags(CREATE_NO_WINDOW);
            let _ = output_capture_with_timeout(cmd, PROC_CONTROL_TIMEOUT);
        }

        // After killing the processes, sweep the per-session lock files
        // agent-browser leaves under `%USERPROFILE%\.agent-browser\`. The
        // CLI trusts those `.pid`/`.port`/`.engine`/`.stream` files as
        // proof that a daemon is alive — when they outlive the process
        // (e.g. previous codemux exited without cleanup), every new
        // `agent-browser open` for that session prints
        // "daemon already running" and times out trying to connect to a
        // ghost. We just killed every agent-browser, so any lock file
        // here is by definition stale.
        cleanup_agent_browser_lock_files();
    }
    #[cfg(not(windows))]
    {
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", "pkill -f 'agent-browser.*daemon' 2>/dev/null; pkill -f 'agent-browser.*--session' 2>/dev/null"]);
        let _ = output_capture_with_timeout(cmd, PROC_CONTROL_TIMEOUT);
    }
}

/// Remove all lock/session files in `%USERPROFILE%\.agent-browser\`.
/// Windows-only because Linux daemons clean up properly on SIGTERM via
/// the `pkill -f` path above; this gap only exists on Windows where
/// `kill_session_tree` is a no-op (no Job Object support yet).
#[cfg(windows)]
fn cleanup_agent_browser_lock_files() {
    let Ok(home) = std::env::var("USERPROFILE") else {
        return;
    };
    let dir = std::path::PathBuf::from(home).join(".agent-browser");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
            continue;
        };
        if matches!(ext, "pid" | "port" | "engine" | "stream") {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Pure parser: given raw `netstat -ano` stdout, return the set of PIDs
/// owning a TCP LISTENING socket on the exact `port`.
///
/// Lives at the top of the module (not inside the `#[cfg(windows)]` block
/// of `kill_process_on_port`) so tests can drive it with hardcoded
/// fixtures on any platform. The filter is done in Rust rather than via
/// `findstr :{port}` so we can:
///   - Handle both IPv4 (`0.0.0.0:9223`) and IPv6 (`[::]:9223`) rows.
///   - Match the port EXACTLY — `findstr :9223` would also match `:92230`,
///     `:92231`, etc. and kill the wrong process. We compare `port_str`
///     as a parsed u16 to avoid the substring trap.
///   - Skip non-LISTENING states (ESTABLISHED, TIME_WAIT) even when they
///     happen to reference the same port.
#[allow(dead_code)] // only called from the Windows branch below
fn pids_listening_on_port(netstat_stdout: &str, port: u16) -> std::collections::HashSet<u32> {
    let mut pids: std::collections::HashSet<u32> = std::collections::HashSet::new();
    for line in netstat_stdout.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 5 {
            continue;
        }
        if fields[0] != "TCP" || fields[3] != "LISTENING" {
            continue;
        }
        let Some(port_str) = fields[1].rsplit(':').next() else {
            continue;
        };
        let Ok(listen_port) = port_str.parse::<u16>() else {
            continue;
        };
        if listen_port != port {
            continue;
        }
        if let Ok(pid) = fields[4].parse::<u32>() {
            pids.insert(pid);
        }
    }
    pids
}

/// Kill any process bound to the given TCP port. Used to reclaim stale
/// agent-browser daemon ports from previous app runs.
fn kill_process_on_port(port: u16) {
    #[cfg(windows)]
    {
        // CREATE_NO_WINDOW suppresses a console flash each time
        // kill_process_on_port runs (startup, port allocation, session close).
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        let netstat_cmd = {
            let mut c = std::process::Command::new("netstat");
            c.args(["-ano"]).creation_flags(CREATE_NO_WINDOW);
            c
        };
        let Ok(netstat) = output_capture_with_timeout(netstat_cmd, PROC_CONTROL_TIMEOUT) else {
            eprintln!(
                "[codemux::browser] kill_process_on_port({}): failed to spawn netstat",
                port
            );
            return;
        };
        if !netstat.status.success() {
            return;
        }

        let stdout = String::from_utf8_lossy(&netstat.stdout);
        let pids = pids_listening_on_port(&stdout, port);

        for pid in pids {
            let mut cmd = std::process::Command::new("taskkill");
            cmd.args(["/PID", &pid.to_string(), "/F"])
                .creation_flags(CREATE_NO_WINDOW);
            let _ = output_capture_with_timeout(cmd, PROC_CONTROL_TIMEOUT);
        }
    }
    #[cfg(not(windows))]
    {
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", &format!("fuser -k {}/tcp 2>/dev/null", port)]);
        let _ = output_capture_with_timeout(cmd, PROC_CONTROL_TIMEOUT);
    }
}

/// Directory where codemux stores per-session runtime files (PID + log
/// files for each agent-browser daemon). On Unix this is `~/.codemux/run`
/// and on Windows it follows the same data-dir convention as the rest of
/// the app via `dirs::home_dir()`.
///
/// The directory is created lazily on first use. Returns `None` if no
/// home directory is discoverable — callers should treat that as "skip
/// log/PID-file integration" rather than fail the whole flow.
pub fn run_dir() -> Option<PathBuf> {
    let dir = dirs::home_dir()?.join(".codemux").join("run");
    if std::fs::create_dir_all(&dir).is_err() {
        return None;
    }
    Some(dir)
}

/// Path to the per-session PID file written by codemux when it spawns
/// the agent-browser daemon. Used by `close()` for targeted PID-based
/// teardown and by `new_with_adoption()` for safe restart adoption.
pub fn session_pid_path(session_name: &str) -> Option<PathBuf> {
    Some(run_dir()?.join(format!("agent-browser-{session_name}.pid")))
}

/// Path to the per-session stderr log file. Captured during daemon
/// spawn so the next investigation has a paper trail.
fn session_log_path(session_name: &str) -> Option<PathBuf> {
    Some(run_dir()?.join(format!("agent-browser-{session_name}.log")))
}

/// Read the agent-browser daemon's own PID file at
/// `~/.agent-browser/{session_name}.pid`. agent-browser CLI writes this
/// file when its daemon binds — it's the most reliable source of the
/// real daemon PID since the CLI command we run is just a thin wrapper
/// that forks the daemon and exits.
fn read_agent_browser_daemon_pid(session_name: &str) -> Option<u32> {
    let home = dirs::home_dir()?;
    let path = home
        .join(".agent-browser")
        .join(format!("{session_name}.pid"));
    let content = std::fs::read_to_string(path).ok()?;
    content.trim().parse::<u32>().ok()
}

/// Write the tracked PID to our own per-session PID file under
/// `~/.codemux/run/`. Best-effort — failures are logged and swallowed
/// because an agent-browser session can still function without the PID
/// file (we just lose the safe-adoption path on next restart).
fn write_session_pid(session_name: &str, pid: u32) {
    let Some(path) = session_pid_path(session_name) else {
        return;
    };
    if let Err(error) = std::fs::write(&path, pid.to_string()) {
        eprintln!(
            "[codemux::browser] failed to write PID file {}: {error}",
            path.display()
        );
    }
}

/// Remove our PID file once the daemon is gone. Best-effort.
fn clear_session_pid(session_name: &str) {
    if let Some(path) = session_pid_path(session_name) {
        let _ = std::fs::remove_file(path);
    }
}

/// Send a kill signal to a tracked PID, cross-platform. Targeted alternative
/// to `kill_process_on_port`: instead of "whoever happens to be on port N",
/// kills exactly the process we own. Avoids the collateral-damage failure
/// mode where one workspace's startup wipes another workspace's healthy
/// daemon because the port allocator briefly aliased them.
///
/// Unix: SIGTERM first for graceful shutdown, then SIGKILL fallback after
/// a short grace period if the process is still alive.
///
/// Windows: `taskkill /PID {pid} /T /F` — `/T` kills the process tree
/// (any children agent-browser spawned: chromium, helper procs), `/F`
/// forces termination since we already gave the daemon a graceful close
/// via the `agent-browser close` command before reaching this fallback.
fn kill_pid(pid: u32) {
    if pid == 0 {
        return;
    }
    #[cfg(unix)]
    {
        // SIGTERM first — gives the daemon a chance to flush state.
        let mut term = std::process::Command::new("kill");
        term.args(["-TERM", &pid.to_string()]);
        let _ = output_capture_with_timeout(term, PROC_CONTROL_TIMEOUT);
        // Brief grace period before escalating.
        std::thread::sleep(Duration::from_millis(200));
        // SIGKILL fallback — `kill -0` returns success only while the PID
        // is alive, so use it to decide whether escalation is needed.
        // If the PID is already gone this is a no-op.
        let mut probe = std::process::Command::new("kill");
        probe.args(["-0", &pid.to_string()]);
        if output_capture_with_timeout(probe, PROC_CONTROL_TIMEOUT)
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            let mut hard = std::process::Command::new("kill");
            hard.args(["-9", &pid.to_string()]);
            let _ = output_capture_with_timeout(hard, PROC_CONTROL_TIMEOUT);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut cmd = std::process::Command::new("taskkill");
        cmd.args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(CREATE_NO_WINDOW);
        let _ = output_capture_with_timeout(cmd, PROC_CONTROL_TIMEOUT);
    }
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

/// Outcome of `AgentBrowserManager::reconcile_runtime_files`. Tells the
/// constructor whether the structured PID-file path found anything to
/// work with — the answer decides whether the legacy blanket cleanup
/// should run as a one-time fallback.
enum ReconcileOutcome {
    /// At least one `agent-browser-*.pid` file existed; we either
    /// adopted a healthy daemon or reaped a stale one. The blanket
    /// cleanup must NOT run after this — it would kill daemons that
    /// belong to another codemux instance.
    ReconciledFromPidFiles,
    /// No PID files at all. Either a fresh install or the first launch after
    /// the PID-file migration. Run the legacy cleanup once so any
    /// orphaned daemons from before the migration get reaped.
    NoStateFound,
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
    /// CLI session name used for `agent-browser close --session ...`.
    /// Stored alongside the port so `close()` can issue a graceful
    /// daemon shutdown without the caller having to remember it.
    /// Only read by the process-teardown path, which is compiled out of
    /// unit-test builds (see `shutdown_session_processes`).
    #[cfg_attr(test, allow(dead_code))]
    cli_session_name: String,
    /// Last time we saw signs of life on this session (TCP probe success
    /// or first-frame timestamp passed in by the frontend). Used by the
    /// optional reaper background task.
    #[allow(dead_code)]
    last_seen_at: Option<Instant>,
}

pub struct AgentBrowserManager {
    /// Atomic counter for the next port to try allocating.
    next_port: AtomicU16,
    /// Per-session state keyed by session identifier (workspace_id or cli_session_name).
    sessions: Mutex<HashMap<String, StreamSession>>,
    /// Serializes start_stream calls to prevent concurrent daemon launches
    /// from racing (e.g., React StrictMode double-mount, pane remount + agent action).
    start_lock: Mutex<()>,
}

fn session_name(browser_id: &str) -> &str {
    if browser_id.is_empty() { "default" } else { browser_id }
}

#[cfg(not(target_os = "windows"))]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
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
/// 4. npx fallback — always works if Node.js + npm are present
#[cfg_attr(target_os = "windows", allow(unreachable_code))]
fn resolve_binary() -> String {
    // 1. System PATH (AUR/system package, cargo install, manual install).
    //
    // Unix-only: `which` is a Unix tool, and Git Bash on Windows has
    // different extension-handling semantics — on `windows-latest` CI it
    // was returning paths without the `.exe` suffix that looked valid but
    // pointed at non-existent files, breaking downstream callers. The
    // Windows discovery is not native yet; for now, Windows falls through to
    // the Tauri sidecar lookup and then the `npx agent-browser` fallback.
    #[cfg(unix)]
    {
        let mut which_cmd = std::process::Command::new("which");
        which_cmd.arg("agent-browser");
        if let Ok(output) = output_capture_with_timeout(which_cmd, PROC_CONTROL_TIMEOUT) {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                // Skip the node_modules/.bin shim — we want the native binary directly
                if !path.is_empty() && !path.contains("node_modules/.bin") {
                    return path;
                }
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
        // (d) Last-resort npx fallback. Use `npx.cmd` (not `npx`) because
        //     CreateProcessW won't auto-append `.cmd`. `which::which("npx")`
        //     resolves to the right shim with PATHEXT, so prefer it.
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

/// Split `resolve_binary()`'s return value into `(program, prefix_args)` so it
/// can drive `Command::new(...).args(...)` directly on Windows. The function
/// either returns a single binary path (most installs / dev) or the literal
/// `"npx agent-browser"` fallback (no bundled sidecar AND no node_modules).
/// In the npx case we want the spawn to be `npx -> agent-browser <action> ...`,
/// not `Command::new("npx agent-browser")` which Windows treats as a single
/// (nonexistent) executable name.
#[cfg(target_os = "windows")]
fn split_resolved_binary(bin: &str) -> (String, Vec<String>) {
    let trimmed = bin.trim();
    if let Some(idx) = trimmed.find(' ') {
        let (program, rest) = trimmed.split_at(idx);
        let prefix = rest
            .split_whitespace()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        (program.to_string(), prefix)
    } else {
        (trimmed.to_string(), Vec::new())
    }
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

/// Argv-form sibling of `build_agent_browser_command`. Returns a list of
/// argument vectors, one per agent-browser invocation: most actions are
/// single-shot, but the historical `open_url` shell form was
/// `<bin> open <url> --session <sid> && <bin> wait --load load --session <sid>`,
/// which becomes two sequential argv groups. Used only on Windows so we can
/// spawn `agent-browser.exe` directly without going through `sh -c` (and
/// therefore without depending on Git Bash being installed).
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
            // The follow-up `wait --load load` doesn't need
            // --executable-path; it talks to the already-running daemon.
            vec![
                open_argv,
                vec![
                    "wait".into(),
                    "--load".into(),
                    "load".into(),
                    "--session".into(),
                    s,
                ],
            ]
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

/// Spawn agent-browser directly (no shell) on Windows with the standard
/// stream-port + stealth env vars wired up. Used by the cfg-gated Windows
/// branches at every shell-out site so we don't depend on Git Bash being
/// installed for runtime browser control.
#[cfg(target_os = "windows")]
fn run_agent_browser_native(
    bin: &str,
    argv: &[String],
    stream_port: u16,
    discard_stderr: bool,
    timeout: Duration,
) -> std::io::Result<std::process::Output> {
    let (program, prefix_args) = split_resolved_binary(bin);
    let mut cmd = std::process::Command::new(&program);
    for a in &prefix_args {
        cmd.arg(a);
    }
    for a in argv {
        cmd.arg(a);
    }
    cmd.env("AGENT_BROWSER_STREAM_PORT", stream_port.to_string())
        .env("AGENT_BROWSER_ARGS", STEALTH_CHROMIUM_ARGS)
        .env("AGENT_BROWSER_USER_AGENT", stealth_user_agent());
    cmd.stdout(std::process::Stdio::piped());
    if discard_stderr {
        cmd.stderr(std::process::Stdio::null());
    } else {
        cmd.stderr(std::process::Stdio::piped());
    }
    // Hard timeout + kill so a wedged agent-browser.exe can't block the
    // calling task forever (issue #96).
    spawn_wait_collect(cmd, timeout)
}

#[cfg(not(target_os = "windows"))]
fn build_agent_browser_command(session: &str, action: &str, params: &serde_json::Value) -> Result<String, String> {
    let bin = resolve_binary();
    let command = match action {
        "open_url" | "open" => {
            let url = params.get("url").and_then(|v| v.as_str()).unwrap_or("about:blank");
            format!(
                "{bin} open {url} --session {s} && {bin} wait --load load --session {s}",
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

/// Truncate a string to at most `max_bytes`, backing off to a UTF-8 char
/// boundary. agent-browser output is arbitrary page/DOM text (often multibyte),
/// so a raw byte-index slice for a log preview would panic when the cut lands
/// inside a character.
fn truncate_on_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn execute_agent_browser_action(browser_id: &str, action: &str, params: serde_json::Value, stream_port: u16) -> Result<BrowserAutomationResult, String> {
    let session = session_name(browser_id);

    #[cfg(target_os = "windows")]
    let (stdout, stderr, status_success) = {
        let bin = resolve_binary();
        let groups = build_agent_browser_argv_groups(session, action, &params)?;
        let mut combined_stdout = String::new();
        let mut combined_stderr = String::new();
        let mut all_ok = true;
        for argv in &groups {
            let out = run_agent_browser_native(&bin, argv, stream_port, false, action_timeout(action))
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
        let shell_cmd = build_agent_browser_command(session, action, &params)?;
        let mut cmd = std::process::Command::new("sh");
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

    // Debug logging for snapshot commands
    if action == "snapshot" || action == "accessibility_snapshot" {
        eprintln!("[codemux::browser] Snapshot stdout ({} bytes): {}", stdout.len(), truncate_on_boundary(&stdout, 200));
        if !stderr.is_empty() {
            eprintln!("[codemux::browser] Snapshot stderr: {}", truncate_on_boundary(&stderr, 200));
        }
    }

    if !status_success && !stdout.contains("✓") && !stdout.contains("{") && !stdout.contains("- ") {
        return Err(format!("agent-browser failed: {} {}", stdout, stderr));
    }

    // For snapshot: detect useless ARIA result and fall back to DOM-based query
    if action == "snapshot" || action == "accessibility_snapshot" {
        eprintln!("[codemux::browser] Snapshot raw stdout for fallback check: {:?}", truncate_on_boundary(&stdout, 300));
    }
    if (action == "snapshot" || action == "accessibility_snapshot") && needs_dom_fallback(&stdout) {
        eprintln!("[codemux::browser] ARIA snapshot empty, falling back to DOM query");
        let dom_params = serde_json::json!({ "script": DOM_SNAPSHOT_SCRIPT });

        #[cfg(target_os = "windows")]
        let dom_output_opt = {
            let bin = resolve_binary();
            match build_agent_browser_argv_groups(session, "eval", &dom_params) {
                Ok(groups) if !groups.is_empty() => {
                    run_agent_browser_native(&bin, &groups[0], stream_port, false, ACTION_TIMEOUT).ok()
                }
                _ => None,
            }
        };
        #[cfg(not(target_os = "windows"))]
        let dom_output_opt = match build_agent_browser_command(session, "eval", &dom_params) {
            Ok(dom_cmd) => {
                let mut cmd = std::process::Command::new("sh");
                cmd.args(["-c", &dom_cmd]);
                output_capture_with_timeout(cmd, ACTION_TIMEOUT).ok()
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
            next_port: AtomicU16::new(DEFAULT_STREAM_PORT),
            sessions: Mutex::new(HashMap::new()),
            start_lock: Mutex::new(()),
        }
    }

    /// Create a new manager and reconcile any agent-browser daemons left
    /// over from a previous app run. The old `new_with_cleanup` did a
    /// blanket `pkill -f agent-browser` plus a 9223–9233 port sweep, which
    /// also nuked daemons started by other codemux
    /// sessions (e.g. the user has two app windows open) and any unrelated
    /// process that happened to bind one of those ports.
    ///
    /// The new path:
    ///   1. Walks `~/.codemux/run/` for `agent-browser-*.pid` files we
    ///      wrote ourselves on previous starts.
    ///   2. For each entry, checks `pid_alive` and `probe_stream_port`.
    ///      A live PID with a responsive port is left alone — the daemon
    ///      is healthy and a future `start_stream` for the same session
    ///      will adopt it via the existing "already running" early return.
    ///      Anything else (dead PID, alive PID with a dead port) is
    ///      cleaned up: kill the PID if alive, delete the stale PID file.
    ///   3. The legacy blanket cleanup is *kept as a fallback only* when
    ///      `~/.codemux/run/` is empty (first run after upgrade), so older
    ///      installations still get their orphans reaped once before the new
    ///      path takes over on the next launch.
    pub fn new_with_cleanup() -> Self {
        Self::new_with_adoption()
    }

    /// Public name for the new behavior. `new_with_cleanup` is kept as an
    /// alias so existing callers (Tauri state setup) compile unchanged.
    pub fn new_with_adoption() -> Self {
        let mgr = Self::new();
        match Self::reconcile_runtime_files() {
            ReconcileOutcome::ReconciledFromPidFiles => {
                // Found at least one of our own PID files; precise path
                // ran. Skip the blanket cleanup — anything still around
                // is either healthy and adopted, or already reaped above.
            }
            ReconcileOutcome::NoStateFound => {
                // First boot after upgrade (or fresh install). Apply the
                // legacy cleanup once so users carrying daemons from a
                // pre-fix codemux don't get stuck on their first launch.
                kill_stream_daemons();
                for port in DEFAULT_STREAM_PORT..=DEFAULT_STREAM_PORT + 10 {
                    kill_process_on_port(port);
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        }
        mgr
    }

    /// Walk `~/.codemux/run/` and reap any PID files whose daemon is no
    /// longer healthy. Returns whether any state was found, so the caller
    /// can decide whether the legacy blanket cleanup should run.
    fn reconcile_runtime_files() -> ReconcileOutcome {
        let Some(dir) = run_dir() else {
            return ReconcileOutcome::NoStateFound;
        };
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return ReconcileOutcome::NoStateFound;
        };
        let mut saw_any = false;
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(session_name) = name
                .strip_prefix("agent-browser-")
                .and_then(|n| n.strip_suffix(".pid"))
            else {
                continue;
            };
            saw_any = true;
            let Some(pid) = std::fs::read_to_string(&path)
                .ok()
                .and_then(|s| s.trim().parse::<u32>().ok())
            else {
                let _ = std::fs::remove_file(&path);
                continue;
            };
            if pid_alive(pid) {
                // Process is alive but we don't track its port from disk
                // (codemux state hasn't been restored yet). The next
                // start_stream for this session will adopt it via the
                // early-return path; nothing to do here. Leave the PID
                // file in place so `close()` can find it later.
                eprintln!(
                    "[codemux::browser] Adopting daemon session={session_name} pid={pid}"
                );
            } else {
                // Dead PID — file is stale. Drop it so future allocations
                // don't pretend it's healthy.
                eprintln!(
                    "[codemux::browser] Reaping dead PID file session={session_name} pid={pid}"
                );
                let _ = std::fs::remove_file(&path);
            }
        }
        if saw_any {
            ReconcileOutcome::ReconciledFromPidFiles
        } else {
            ReconcileOutcome::NoStateFound
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
                    cli_session_name: session_key.to_string(),
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
        let port = self.allocate_port(browser_id).await?;

        {
            let sessions = self.sessions.lock().await;
            if sessions.get(browser_id).map_or(false, |s| s.running) {
                return Ok(());
            }
        }

        // The launch shells out to agent-browser (which forks the daemon) and
        // can block on a real page load — run it on a blocking thread with a
        // hard timeout so it can never stall the async runtime (issue #96).
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            let bin = resolve_binary();

            #[cfg(target_os = "windows")]
            let output = {
                // `--executable-path` is a top-level flag — must come before
                // the `open` subcommand or clap rejects it.
                let mut argv: Vec<String> = windows_executable_path_args();
                argv.extend([
                    "open".into(),
                    "about:blank".into(),
                    "--headless".into(),
                    "--session".into(),
                    session.clone(),
                ]);
                run_agent_browser_native(&bin, &argv, port, false, OPEN_TIMEOUT)
                    .map_err(|e| format!("Failed to start agent-browser: {}", e))?
            };
            #[cfg(not(target_os = "windows"))]
            let output = {
                let mut cmd = std::process::Command::new("sh");
                cmd.args([
                    "-c",
                    &format!("{} open about:blank --headless --session {}", bin, session),
                ])
                .env("AGENT_BROWSER_STREAM_PORT", port.to_string())
                .env("AGENT_BROWSER_ARGS", STEALTH_CHROMIUM_ARGS)
                .env("AGENT_BROWSER_USER_AGENT", stealth_user_agent());
                output_capture_with_timeout(cmd, OPEN_TIMEOUT)
                    .map_err(|e| format!("Failed to start agent-browser: {}", e))?
            };

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(format!("Failed to start browser: {}", stderr));
            }
            Ok(())
        })
        .await
        .map_err(|e| format!("agent-browser spawn task failed: {e}"))??;

        self.sessions.lock().await.entry(browser_id.to_string()).and_modify(|s| s.running = true);
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
                    std::mem::replace(&mut s.running, true)
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
        }
    }

    pub async fn run_command(&self, browser_id: &str, action: &str, params: serde_json::Value) -> Result<BrowserAutomationResult, String> {
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
        .map_err(|e| format!("agent-browser action task failed: {e}"))?;

        // The agent-facing "close" goes through this path (not close()), so
        // reset the state here — otherwise the entry keeps running = true and
        // a later open on the fresh daemon would skip the default viewport.
        if action == "close" {
            self.note_close_finished(browser_id).await;
        }

        result
    }

    pub async fn get_screenshot(&self, browser_id: &str) -> Result<String, String> {
        let session = session_name(browser_id).to_string();
        let port = self.allocate_port(browser_id).await?;

        // The CLI call + file read are blocking; run them off the executor
        // under a hard timeout so a wedged screenshot can't hang the runtime.
        tokio::task::spawn_blocking(move || -> Result<String, String> {
            let bin = resolve_binary();

            #[cfg(target_os = "windows")]
            let output = run_agent_browser_native(
                &bin,
                &[
                    "screenshot".into(),
                    "--session".into(),
                    session.clone(),
                ],
                port,
                false,
                ACTION_TIMEOUT,
            )
            .map_err(|e| format!("Failed to get screenshot: {}", e))?;
            #[cfg(not(target_os = "windows"))]
            let output = {
                let mut cmd = std::process::Command::new("sh");
                cmd.args(["-c", &format!("{} screenshot --session {}", bin, session)])
                    .env("AGENT_BROWSER_STREAM_PORT", port.to_string())
                    .env("AGENT_BROWSER_ARGS", STEALTH_CHROMIUM_ARGS)
                    .env("AGENT_BROWSER_USER_AGENT", stealth_user_agent());
                output_capture_with_timeout(cmd, ACTION_TIMEOUT)
                    .map_err(|e| format!("Failed to get screenshot: {}", e))?
            };

            let stdout = String::from_utf8_lossy(&output.stdout).to_string();

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
    ///   2. Issue a graceful `agent-browser close --session ...` command
    ///      so the daemon flushes Chromium state (cookies, localStorage)
    ///      to its `~/.agent-browser/` profile dir. This is best-effort;
    ///      a hung daemon is still gracefully reaped by the next steps.
    ///   3. Hard-kill the tracked PID with `kill_pid()`. Only the daemon
    ///      we own gets the signal — no `fuser -k` collateral damage to
    ///      another workspace's healthy daemon on a different port.
    ///   4. Fall back to `kill_process_on_port` if we never captured a
    ///      PID (e.g. session was adopted from a stale PID file).
    ///   5. Clean up our PID file under `~/.codemux/run/`.
    ///
    /// Returns Ok even if individual steps fail — close is idempotent and
    /// callers should not be blocked from retrying or moving on.
    pub async fn close(&self, browser_id: &str) -> Result<(), String> {
        let session = session_name(browser_id).to_string();

        // Step 1: pop the session under the lock so nothing else races us.
        let removed = self.sessions.lock().await.remove(browser_id);

        // Steps 2–4 spawn external processes (the agent-browser CLI for a
        // graceful shutdown, then PID/port-based kills). They are compiled
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

        // Step 5: drop our PID file. Ignore failures — the file may not
        // exist if PID-file integration was skipped.
        clear_session_pid(&session);

        Ok(())
    }

    /// Best-effort teardown of the daemon behind a closed session:
    /// graceful CLI `close`, then a kill by tracked PID with a port-based
    /// fallback. Spawns external processes — which is exactly why
    /// `close()` only calls this in non-test builds.
    #[cfg(not(test))]
    fn shutdown_session_processes(session: &str, removed: Option<StreamSession>) {
        let bin = resolve_binary();

        // Step 2: graceful daemon shutdown via the CLI (hard-timeout bounded).
        #[cfg(target_os = "windows")]
        {
            let _ = run_agent_browser_native(
                &bin,
                &["close".into(), "--session".into(), session.to_string()],
                0,
                true,
                CLOSE_TIMEOUT,
            );
        }
        #[cfg(not(target_os = "windows"))]
        {
            let mut cmd = std::process::Command::new("sh");
            cmd.args([
                "-c",
                &format!("{} close --session {} 2>/dev/null", bin, session),
            ]);
            let _ = output_capture_with_timeout(cmd, CLOSE_TIMEOUT);
        }

        // Step 3 + 4: kill by tracked PID; fall back to port-based kill
        // only when we never captured one. This is the change that stops
        // the `fuser -k` collateral-damage failure mode.
        if let Some(s) = removed {
            let mut pid = s.pid;
            if pid.is_none() {
                // Late-bind the PID from agent-browser's own lock file in
                // case we missed it during spawn (e.g. concurrent start).
                pid = read_agent_browser_daemon_pid(&s.cli_session_name);
            }
            match pid {
                Some(p) if p != 0 => kill_pid(p),
                _ => kill_process_on_port(s.port),
            }
        } else {
            // Session wasn't tracked but caller asked us to close it —
            // fall through to a best-effort PID-file kill.
            if let Some(p) = read_agent_browser_daemon_pid(session) {
                kill_pid(p);
            }
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
    ///   5. Capture the daemon PID from `~/.agent-browser/{name}.pid` and
    ///      mirror it into our own `~/.codemux/run/{name}.pid` so the next
    ///      process-tree teardown can target it directly.
    ///   6. Return the WebSocket URL.
    pub async fn start_stream(&self, browser_id: &str) -> Result<String, String> {
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
                return Ok(format!("ws://localhost:{port}"));
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
                });
        }

        // Close any stale daemon for this session name (from a previous app run)
        // BEFORE allocating a port. Otherwise allocate_port's bind-test sees the
        // stale daemon's port as occupied, skips it, and allocates a different port.
        // The agent-browser CLI would then reuse the stale daemon (by session name)
        // while BrowserPane connects to the newly allocated (empty) port.
        //
        // resolve_binary() and the stale-close both shell out, so run them on a
        // blocking thread under a hard timeout (issue #96). The resolved binary
        // path is returned so the launch step below can reuse it.
        let bin = {
            let session_for_close = session.clone();
            tokio::task::spawn_blocking(move || {
                let bin = resolve_binary();
                #[cfg(target_os = "windows")]
                {
                    // discard_stderr=true mirrors the `2>/dev/null` redirect on
                    // the Unix shell form below — stale-close errors are noise.
                    let _ = run_agent_browser_native(
                        &bin,
                        &["close".into(), "--session".into(), session_for_close.clone()],
                        0,
                        true,
                        CLOSE_TIMEOUT,
                    );
                }
                #[cfg(not(target_os = "windows"))]
                {
                    let mut cmd = std::process::Command::new("sh");
                    cmd.args([
                        "-c",
                        &format!("{} close --session {} 2>/dev/null", bin, session_for_close),
                    ]);
                    let _ = output_capture_with_timeout(cmd, CLOSE_TIMEOUT);
                }
                bin
            })
            .await
            .map_err(|e| format!("agent-browser stale-close task failed: {e}"))?
        };

        let port = self.allocate_port(browser_id).await?;

        // Reclaim the port, launch the daemon, health-probe it, and capture
        // its PID. Every step here shells out, sleeps, or blocks on a TCP
        // probe, so the whole sequence runs on a blocking thread under hard
        // timeouts so it can never stall the async runtime (issue #96).
        let (port_alive, pid) = tokio::task::spawn_blocking(move || -> (bool, Option<u32>) {
            // Reclaim the port if anything else is sitting on it. With the
            // bind-test in allocate_port this is mostly belt-and-suspenders,
            // but on Linux/macOS a daemon could have grabbed it between the
            // bind-test drop and now.
            kill_process_on_port(port);
            std::thread::sleep(Duration::from_millis(500));

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
            let log_file = log_path.as_ref().and_then(|p| {
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(p)
                    .ok()
            });

            #[cfg(target_os = "windows")]
            {
                // The Windows native runner already has a discard_stderr knob;
                // when log capture is desired we run the spawn manually so we
                // can redirect to the file directly.
                if let Some(log) = log_file {
                    use std::os::windows::process::CommandExt;
                    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
                    let (program, prefix_args) = split_resolved_binary(&bin);
                    let mut cmd = std::process::Command::new(&program);
                    for a in &prefix_args {
                        cmd.arg(a);
                    }
                    let mut argv: Vec<String> = windows_executable_path_args();
                    argv.extend([
                        "open".into(),
                        "about:blank".into(),
                        "--headless".into(),
                        "--session".into(),
                        session.clone(),
                    ]);
                    for a in &argv {
                        cmd.arg(a);
                    }
                    cmd.env("AGENT_BROWSER_STREAM_PORT", port.to_string())
                        .env("AGENT_BROWSER_ARGS", STEALTH_CHROMIUM_ARGS)
                        .env("AGENT_BROWSER_USER_AGENT", stealth_user_agent())
                        .stderr(log)
                        .creation_flags(CREATE_NO_WINDOW);
                    let _ = launch_status_with_timeout(cmd, OPEN_TIMEOUT);
                } else {
                    let mut argv: Vec<String> = windows_executable_path_args();
                    argv.extend([
                        "open".into(),
                        "about:blank".into(),
                        "--headless".into(),
                        "--session".into(),
                        session.clone(),
                    ]);
                    let _ = run_agent_browser_native(&bin, &argv, port, false, OPEN_TIMEOUT);
                }
            }
            #[cfg(not(target_os = "windows"))]
            {
                let launch_cmd = format!(
                    "{} open about:blank --headless --session {}",
                    bin, session
                );
                let mut cmd = std::process::Command::new("sh");
                cmd.args(["-c", &launch_cmd])
                    .env("AGENT_BROWSER_STREAM_PORT", port.to_string())
                    .env("AGENT_BROWSER_ARGS", STEALTH_CHROMIUM_ARGS)
                    .env("AGENT_BROWSER_USER_AGENT", stealth_user_agent());
                if let Some(log) = log_file {
                    cmd.stderr(log);
                }
                let _ = launch_status_with_timeout(cmd, OPEN_TIMEOUT);
            }

            // Probe the port up to ~5s; only mark
            // running when we have actual evidence the daemon is alive.
            let port_alive = probe_stream_port(port, 25, Duration::from_millis(200));

            // Capture the daemon's PID. agent-browser writes its PID file
            // synchronously while initialising, so by the time the probe
            // succeeds the file should exist. We try once before and once
            // after to handle either ordering.
            let mut pid = read_agent_browser_daemon_pid(&session);
            if pid.is_none() {
                std::thread::sleep(Duration::from_millis(100));
                pid = read_agent_browser_daemon_pid(&session);
            }
            if let Some(p) = pid {
                write_session_pid(&session, p);
            }

            if !port_alive {
                eprintln!(
                    "[codemux::browser] WARN session={session} port={port}: daemon launch did not pass health probe; \
                     returning URL but caller should expect a degraded stream"
                );
            }

            (port_alive, pid)
        })
        .await
        .map_err(|e| format!("agent-browser launch task failed: {e}"))?;

        let now = Some(Instant::now());
        self.sessions
            .lock()
            .await
            .entry(browser_id.to_string())
            .and_modify(|s| {
                s.running = port_alive;
                s.pid = pid;
                s.last_seen_at = now;
            });

        Ok(format!("ws://localhost:{port}"))
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

    #[test]
    fn truncate_on_boundary_never_splits_a_multibyte_char() {
        // 67×'技' = 201 bytes; byte 200 is inside the 67th char (198..201),
        // so a raw `&s[..200]` would panic. The helper backs off to byte 198.
        let s = "技".repeat(67);
        let out = truncate_on_boundary(&s, 200);
        assert_eq!(out, "技".repeat(66));
        assert!(out.len() <= 200);
        // Shorter-than-limit and ASCII inputs pass through / cut cleanly.
        assert_eq!(truncate_on_boundary("hello", 200), "hello");
        assert_eq!(truncate_on_boundary("hello", 3), "hel");
        // A cut exactly on a boundary keeps the whole char.
        assert_eq!(truncate_on_boundary("a技b", 4), "a技");
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
    #[test]
    fn build_command_open_chains_wait_load() {
        let cmd = build_agent_browser_command("test-session", "open", &serde_json::json!({"url": "https://example.com"})).unwrap();
        let bin = resolve_binary();
        assert!(cmd.starts_with(&bin), "Command should start with resolved binary: {}", cmd);
        assert!(cmd.contains("--session test-session"));
        assert!(cmd.contains("https://example.com"));
        assert!(cmd.contains("wait --load load"), "Should wait for load event: {}", cmd);
        assert!(!cmd.contains("stream disable"), "Should NOT restart stream: {}", cmd);
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
    fn build_argv_open_chains_wait_load() {
        let groups = build_agent_browser_argv_groups(
            "test-session",
            "open",
            &serde_json::json!({"url": "https://example.com"}),
        )
        .unwrap();
        assert_eq!(groups.len(), 2, "open should produce open + wait groups");

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
        assert_eq!(groups[1][0], "wait");
        assert!(groups[1].contains(&"--load".to_string()));
        assert!(groups[1].contains(&"load".to_string()));
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
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn split_resolved_binary_handles_plain_path() {
        let (program, prefix) =
            split_resolved_binary(r"C:\Users\u\node_modules\agent-browser\bin\agent-browser-win32-x64.exe");
        assert!(program.ends_with("agent-browser-win32-x64.exe"));
        assert!(prefix.is_empty());
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

    // ── pids_listening_on_port (Windows netstat parser) ──────────────
    //
    // These tests are NOT cfg-gated — the parser is a pure string
    // function that can run on any platform. They verify the exact-match
    // semantics that made us avoid `findstr :{port}` in the first place.

    /// Fixture matching the shape agent-browser daemons produce when
    /// they bind to their stream ports (9223..=9299). Includes:
    /// - our target port 9223 with pid 11111
    /// - a substring-lookalike :92230 (would match `findstr :9223`)
    /// - a non-LISTENING row on :9223 (should be ignored)
    /// - unrelated rows
    const KILL_NETSTAT_FIXTURE: &str = "\
Active Connections

  Proto  Local Address          Foreign Address        State           PID
  TCP    127.0.0.1:9223         0.0.0.0:0              LISTENING       11111
  TCP    0.0.0.0:92230          0.0.0.0:0              LISTENING       22222
  TCP    127.0.0.1:9223         192.168.1.1:55555      ESTABLISHED     33333
  TCP    [::]:9223              [::]:0                 LISTENING       44444
  TCP    0.0.0.0:9000           0.0.0.0:0              LISTENING       55555
  UDP    0.0.0.0:5353           *:*                                    66666
";

    #[test]
    fn test_pids_listening_on_port_happy_path() {
        let pids = pids_listening_on_port(KILL_NETSTAT_FIXTURE, 9223);
        // 9223 has two LISTENING rows: IPv4 (pid 11111) + IPv6 (pid 44444).
        // :92230 (22222), ESTABLISHED (33333), and :9000 (55555) must NOT match.
        assert_eq!(pids.len(), 2, "got: {pids:?}");
        assert!(pids.contains(&11111), "should include IPv4 LISTENING pid");
        assert!(pids.contains(&44444), "should include IPv6 LISTENING pid");
        assert!(!pids.contains(&22222), "must NOT match :92230 substring");
        assert!(
            !pids.contains(&33333),
            "must NOT match ESTABLISHED state on port 9223"
        );
        assert!(!pids.contains(&55555), "must NOT match :9000");
    }

    #[test]
    fn test_pids_listening_on_port_exact_match_no_substring_trap() {
        // Regression guard: `findstr :9223` would match both 9223 and
        // 92230 / 92231. Our parser compares the parsed u16, so
        // substring collisions are impossible.
        let fixture = "\
  TCP    0.0.0.0:92230   0.0.0.0:0  LISTENING  1
  TCP    0.0.0.0:92231   0.0.0.0:0  LISTENING  2
  TCP    0.0.0.0:9223    0.0.0.0:0  LISTENING  3
";
        let pids = pids_listening_on_port(fixture, 9223);
        assert_eq!(pids.len(), 1);
        assert!(pids.contains(&3));
    }

    #[test]
    fn test_pids_listening_on_port_no_match_returns_empty() {
        // Port 9999 appears nowhere in the fixture.
        let pids = pids_listening_on_port(KILL_NETSTAT_FIXTURE, 9999);
        assert!(pids.is_empty());

        // Empty input.
        let pids = pids_listening_on_port("", 9223);
        assert!(pids.is_empty());
    }

    #[test]
    fn test_pids_listening_on_port_ipv4_only() {
        // IPv4-only case — real netstat on some Windows versions only
        // emits an IPv4 row when the socket is bound with AF_INET.
        let fixture = "  TCP    0.0.0.0:9223   0.0.0.0:0  LISTENING  7777\n";
        let pids = pids_listening_on_port(fixture, 9223);
        assert_eq!(pids.len(), 1);
        assert!(pids.contains(&7777));
    }

    #[test]
    fn test_pids_listening_on_port_ipv6_only() {
        let fixture = "  TCP    [::]:9223   [::]:0  LISTENING  8888\n";
        let pids = pids_listening_on_port(fixture, 9223);
        assert_eq!(pids.len(), 1);
        assert!(pids.contains(&8888));
    }

    #[test]
    fn test_pids_listening_on_port_never_panics() {
        // Garbage inputs — must not panic.
        let _ = pids_listening_on_port("\x00\x01\x02", 9223);
        let _ = pids_listening_on_port("TCP", 9223);
        let _ = pids_listening_on_port(
            "  TCP    0.0.0.0:99999  0.0.0.0:0  LISTENING  1234\n",
            9223,
        );
        let _ = pids_listening_on_port(
            "  TCP    0.0.0.0:9223  0.0.0.0:0  LISTENING  999999999999999\n",
            9223,
        );
    }

    // ── Browser stream lifecycle regression tests ──
    //
    // These tests target only behavior that lives entirely in this
    // module: pure helpers and the manager's public surface. The
    // daemon-spawning paths (start_stream/spawn) are exercised in the
    // existing manual-smoke flow and intentionally not unit-tested
    // here — they would require staging an agent-browser binary inside
    // CI's sandbox, which is out of scope.

    /// `kill_pid(0)` must be a no-op rather than panic / segfault. Lets
    /// callers pass `Option<u32>::unwrap_or(0)` without a conditional
    /// and is the contract `close()` relies on for adopted sessions
    /// where the PID was never captured.
    #[test]
    fn kill_pid_zero_is_safe_noop() {
        // No assertion needed beyond "this returns" — we just need
        // confidence the function won't take down the test process by
        // signalling pid 0 (which is the process group on Unix and
        // would terminate cargo test itself).
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
        assert!(
            s.contains(".codemux"),
            "pid file should live under .codemux/run/, got: {s}"
        );
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
        write_session_pid(&name, 12345);
        let path = session_pid_path(&name).expect("path resolves");
        let on_disk = std::fs::read_to_string(&path).unwrap_or_default();
        assert_eq!(on_disk.trim(), "12345");
        clear_session_pid(&name);
        assert!(
            !path.exists(),
            "clear_session_pid must remove the file we wrote"
        );
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
