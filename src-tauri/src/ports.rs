use serde::{Deserialize, Serialize};
use std::collections::HashMap;
// `std::fs` is cross-platform. The Linux-only `/proc/...` scanning helpers are
// each individually `#[cfg(target_os = "linux")]`, but `load_static_ports()`
// also reads `.codemux/ports.json` on every platform, so the import must not
// be gated to linux.
use std::fs;
use std::path::Path;

/// Ports that are always excluded from detection (system services, databases, Codemux internals).
const IGNORED_PORTS: &[u16] = &[22, 80, 443, 5432, 3306, 6379, 27017];

/// Process names that are considered Windows system services and should
/// be filtered out of port detection.
///
/// This list is the Windows equivalent of Linux's natural `/proc/*/fd/`
/// permission filter. On Linux, `linux_detect_listening_ports` walks
/// `/proc/*/fd/` symlinks to resolve socket inodes to PIDs — and a
/// non-root process can only read its own user's fd dirs, so sockets
/// owned by root/systemd/etc. are silently dropped at the inode→PID
/// resolution step. The Windows path uses `netstat -ano` which lists
/// EVERY listening socket regardless of owner, so without an explicit
/// filter the UI would show 16+ system ports (135 RPC, 139 NetBIOS,
/// 445 SMB, etc.) that Linux Codemux never shows.
///
/// Filtering by process name (not by port number) matches the user's
/// stated preference and is robust against process-relocation: if a
/// system service moves to a different port in a future Windows
/// version, the filter still catches it as long as the process name
/// is stable. Most of the ports a typical Windows host listens on are
/// owned by `System` (PID 4) or `svchost.exe` — both covered here.
///
/// **Comparison is case-insensitive** because Windows process names
/// canonically end in `.exe` but can be reported by tasklist with
/// varying casing depending on locale and Windows version.
///
/// **What is NOT in this list (and why):**
/// - User-runnable dev tools (`node.exe`, `python.exe`, etc.) — never
///   filter user processes
/// - Browser processes (`chrome.exe`, `firefox.exe`, `msedge.exe`) —
///   user might be running a browser-attached dev workflow
/// - Database servers (`postgres.exe`, `mysqld.exe`, `redis-server.exe`)
///   — already covered by the port-level `IGNORED_PORTS` filter
/// - IDE language servers (`rust-analyzer.exe`, `pylsp.exe`) — user
///   might want to see their listen ports
///
/// `#[allow(dead_code)]` because the only runtime caller is inside
/// `#[cfg(windows)] mod windows_impl`. On non-Windows non-test builds
/// the constant compiles but is unreferenced — keeping it cross-platform
/// compilable lets the parser_tests module exercise it on Linux CI
/// without a `#[cfg(windows)]` test gate that would silently skip
/// regressions until they reach a Windows runner.
#[allow(dead_code)]
const WINDOWS_SYSTEM_PROCESS_NAMES: &[&str] = &[
    // Core kernel + session management
    "System",                  // NT Kernel — owns 139 NetBIOS, 445 SMB, etc. (PID 4)
    "Idle",                    // System Idle Process (PID 0)
    "System Idle Process",     // Tasklist sometimes reports PID 0 with this name
    "smss.exe",                // Session Manager Subsystem
    "csrss.exe",               // Client/Server Runtime Subsystem
    "wininit.exe",             // Windows Initialization
    "winlogon.exe",            // Windows Logon
    "services.exe",            // Service Control Manager
    "lsass.exe",               // Local Security Authority
    "lsm.exe",                 // Local Session Manager
    // Service host — the catch-all for hundreds of Windows services,
    // including 135 RPC, 5040 Delivery Optimization, and the dynamic
    // RPC ephemeral ports (often 1042, 1043, etc.)
    "svchost.exe",
    // Desktop / windowing
    "dwm.exe",                 // Desktop Window Manager
    "fontdrvhost.exe",         // Font Driver Host
    // Print + spooler
    "spoolsv.exe",             // Print Spooler
    // Search + indexing + defender — common ephemeral-port owners
    "SearchIndexer.exe",       // Windows Search
    "SearchProtocolHost.exe",  // Windows Search content extraction
    "SearchFilterHost.exe",    // Windows Search filtering
    "MsMpEng.exe",             // Microsoft Antimalware Engine (Defender)
    "NisSrv.exe",              // Network Inspection Service (Defender)
    "SecurityHealthService.exe", // Windows Security
    // Update + delivery
    "TrustedInstaller.exe",    // Windows Module Installer
    // Misc system services that bind sockets
    "WmiPrvSE.exe",            // WMI Provider Host
    "dllhost.exe",             // COM Surrogate
    "taskhostw.exe",           // Task Host
    "RuntimeBroker.exe",       // Runtime Broker
    "ApplicationFrameHost.exe", // UWP host
];

/// Returns true if the given process name matches a known Windows
/// system process. Case-insensitive ASCII comparison.
///
/// Cross-platform pure function: no Windows-specific syscalls or types,
/// so the test suite can validate it on Linux CI before it reaches a
/// Windows runner. Only called from the Windows-specific
/// `detect_listening_ports` path; Linux's own port detection uses
/// `/proc/*/fd/` permissions to filter system processes implicitly,
/// so this helper isn't invoked there.
///
/// See `WINDOWS_SYSTEM_PROCESS_NAMES` for the rationale on which names
/// are included and which are intentionally absent.
///
/// `#[allow(dead_code)]` for the same reason as the constant above:
/// the runtime caller is `#[cfg(windows)]`-gated but Linux test builds
/// reference the function via `parser_tests`, so we keep it
/// unconditionally compiled and just suppress the dead-code warning
/// on non-Windows non-test builds.
#[allow(dead_code)]
fn is_windows_system_process(process_name: &str) -> bool {
    WINDOWS_SYSTEM_PROCESS_NAMES
        .iter()
        .any(|sys| sys.eq_ignore_ascii_case(process_name))
}

/// Codemux reserves high ports for agent-browser debugging and streaming.
fn is_codemux_internal_port(port: u16) -> bool {
    port >= 9222
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortInfo {
    pub port: u16,
    pub pid: u32,
    pub process_name: String,
    pub workspace_id: Option<String>,
    pub label: Option<String>,
    /// Where this port was discovered. `None` for the OS-level scan
    /// (`/proc` on Linux, `netstat` on Windows); `Some("docker")` for a
    /// published container port surfaced via the Docker CLI. The frontend
    /// uses this to render container ports under a dedicated "Docker" group.
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StaticPortsConfig {
    ports: Vec<StaticPortEntry>,
}

#[derive(Debug, Deserialize)]
struct StaticPortEntry {
    port: u16,
    label: String,
}

/// Detect all listening TCP ports owned by the current user.
///
/// - Linux: parses `/proc/net/tcp` + `/proc/net/tcp6` for LISTEN-state sockets
///   and resolves owning PIDs via `/proc/*/fd/` symlinks.
/// - Windows: shells out to `netstat -ano` and parses LISTENING rows + PID.
///   Process names are resolved with a single `tasklist /NH /FO csv` call per
///   scan so we don't spawn one process per detected port.
/// - Other platforms: returns an empty list.
pub fn detect_listening_ports() -> Vec<PortInfo> {
    #[cfg(target_os = "linux")]
    {
        linux_detect_listening_ports()
    }
    #[cfg(windows)]
    {
        windows_impl::detect_listening_ports()
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        Vec::new()
    }
}

#[cfg(target_os = "linux")]
fn linux_detect_listening_ports() -> Vec<PortInfo> {
    let listening = parse_proc_net_tcp();
    if listening.is_empty() {
        return Vec::new();
    }

    let inode_to_port: HashMap<u64, u16> = listening.into_iter().collect();
    resolve_pids_for_inodes(&inode_to_port)
}

/// Parse /proc/net/tcp and /proc/net/tcp6 for listening sockets.
/// Returns Vec<(inode, port)>.
#[cfg(target_os = "linux")]
fn parse_proc_net_tcp() -> Vec<(u64, u16)> {
    let mut results = Vec::new();
    for path in &["/proc/net/tcp", "/proc/net/tcp6"] {
        if let Ok(contents) = fs::read_to_string(path) {
            for line in contents.lines().skip(1) {
                if let Some(entry) = parse_tcp_line(line) {
                    if !IGNORED_PORTS.contains(&entry.1) && !is_codemux_internal_port(entry.1) {
                        results.push(entry);
                    }
                }
            }
        }
    }
    results
}

/// Parse a single line from /proc/net/tcp.
/// Format: sl local_address rem_address st tx_queue:rx_queue tr:tm->when retrnsmt uid timeout inode ...
/// Returns Some((inode, port)) if state is LISTEN (0A).
#[cfg(target_os = "linux")]
fn parse_tcp_line(line: &str) -> Option<(u64, u16)> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() < 10 {
        return None;
    }

    // Field 3 is state — 0A = TCP_LISTEN
    let state = fields[3];
    if state != "0A" {
        return None;
    }

    // Field 1 is local_address as hex_ip:hex_port
    let local_addr = fields[1];
    let port_hex = local_addr.split(':').nth(1)?;
    let port = u16::from_str_radix(port_hex, 16).ok()?;

    // Field 9 is inode
    let inode = fields[9].parse::<u64>().ok()?;
    if inode == 0 {
        return None;
    }

    Some((inode, port))
}

/// Scan /proc/*/fd/ to find which PIDs own the given socket inodes.
#[cfg(target_os = "linux")]
fn resolve_pids_for_inodes(inode_to_port: &HashMap<u64, u16>) -> Vec<PortInfo> {
    let mut results = Vec::new();
    let mut seen_ports = std::collections::HashSet::new();

    let Ok(proc_dir) = fs::read_dir("/proc") else {
        return results;
    };

    for entry in proc_dir.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Only numeric directories (PIDs)
        let pid: u32 = match name_str.parse() {
            Ok(p) => p,
            Err(_) => continue,
        };

        let fd_dir = format!("/proc/{}/fd", pid);
        let Ok(fds) = fs::read_dir(&fd_dir) else {
            continue;
        };

        for fd_entry in fds.flatten() {
            let Ok(link) = fs::read_link(fd_entry.path()) else {
                continue;
            };
            let link_str = link.to_string_lossy();

            // Socket symlinks look like "socket:[12345]"
            if let Some(inode_str) = link_str.strip_prefix("socket:[").and_then(|s| s.strip_suffix(']')) {
                if let Ok(inode) = inode_str.parse::<u64>() {
                    if let Some(&port) = inode_to_port.get(&inode) {
                        if seen_ports.insert(port) {
                            let process_name = read_process_name(pid);
                            results.push(PortInfo {
                                port,
                                pid,
                                process_name,
                                workspace_id: None,
                                label: None,
                                source: None,
                            });
                        }
                    }
                }
            }
        }
    }

    results.sort_by_key(|p| p.port);
    results
}

/// Read process name from /proc/<pid>/comm.
#[cfg(target_os = "linux")]
fn read_process_name(pid: u32) -> String {
    fs::read_to_string(format!("/proc/{}/comm", pid))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".into())
}

/// Read the parent PID for the given pid.
///
/// - Linux: parses `/proc/{pid}/stat`.
/// - Windows: best-effort via `wmic process get ProcessId,ParentProcessId`.
///   Returns `None` if the lookup fails — ancestor attribution is a
///   nice-to-have, not a correctness requirement.
fn read_ppid(pid: u32) -> Option<u32> {
    #[cfg(target_os = "linux")]
    {
        linux_read_ppid(pid)
    }
    #[cfg(windows)]
    {
        windows_impl::read_ppid(pid)
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    {
        let _ = pid;
        None
    }
}

#[cfg(target_os = "linux")]
fn linux_read_ppid(pid: u32) -> Option<u32> {
    let stat = fs::read_to_string(format!("/proc/{}/stat", pid)).ok()?;
    // Format: pid (comm) state ppid ...
    // comm can contain spaces and parens, so find the last ')' first
    let after_comm = stat.rfind(')')? + 2;
    let remainder = stat.get(after_comm..)?;
    let fields: Vec<&str> = remainder.split_whitespace().collect();
    // fields[0] = state, fields[1] = ppid
    fields.get(1)?.parse().ok()
}

/// Pure parser for `netstat -ano` stdout. Defined at module level (NOT
/// inside `windows_impl`) so its tests can run on every platform — the
/// function body is pure string parsing with no Windows-specific types or
/// syscalls. `detect_listening_ports` on Windows shells out to netstat
/// and hands the stdout to this parser; on other platforms this function
/// is unused at runtime but still compiled and tested on Linux CI, which
/// is why we suppress the dead_code warning.
#[allow(dead_code)]
///
/// netstat layout (localized headers are skipped because we match on the
/// literal "LISTENING" state):
///
/// ```text
///   Proto  Local Address     Foreign Address   State        PID
///   TCP    0.0.0.0:135       0.0.0.0:0         LISTENING    1024
///   TCP    [::]:135          [::]:0            LISTENING    1024
/// ```
///
/// IPv4 and IPv6 addresses both end with `:<port>`, so the last `:` is the
/// split point. Filters applied:
///   - non-`TCP` rows → skipped (UDP, headers, blank lines)
///   - non-`LISTENING` state → skipped (ESTABLISHED, TIME_WAIT, etc.)
///   - port in `IGNORED_PORTS` or Codemux-internal range → skipped
///   - duplicates (same port on IPv4 + IPv6) → first wins
///   - unparseable port or PID → skipped (not an error)
fn parse_netstat_output(
    stdout: &str,
    process_names: &HashMap<u32, String>,
) -> Vec<PortInfo> {
    let mut results: Vec<PortInfo> = Vec::new();
    let mut seen_ports: std::collections::HashSet<u16> = std::collections::HashSet::new();

    for line in stdout.lines() {
        // Whitespace-split row format. Skip headers / blanks by requiring
        // exactly the TCP+LISTENING shape.
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 5 {
            continue;
        }
        if fields[0] != "TCP" {
            continue;
        }
        if fields[3] != "LISTENING" {
            continue;
        }

        let local_addr = fields[1];
        let Some(port_str) = local_addr.rsplit(':').next() else {
            continue;
        };
        let Ok(port) = port_str.parse::<u16>() else {
            continue;
        };
        if IGNORED_PORTS.contains(&port) || is_codemux_internal_port(port) {
            continue;
        }
        if !seen_ports.insert(port) {
            continue;
        }

        let Ok(pid) = fields[4].parse::<u32>() else {
            continue;
        };

        let process_name = process_names
            .get(&pid)
            .cloned()
            .unwrap_or_else(|| "unknown".into());

        results.push(PortInfo {
            port,
            pid,
            process_name,
            workspace_id: None,
            label: None,
            source: None,
        });
    }

    results.sort_by_key(|p| p.port);
    results
}

/// Pure parser for `tasklist /NH /FO csv` stdout. Cross-platform for the
/// same reason as `parse_netstat_output` — no Windows-specific types,
/// safe to test on Linux CI.
#[allow(dead_code)]
///
/// CSV rows look like:
/// ```text
/// "svchost.exe","1024","Services","0","12,345 K"
/// ```
///
/// We only care about the first two fields (name, pid). The parser is
/// deliberately minimal — no real CSV library — because tasklist's output
/// format is fixed and well-documented. Malformed lines (wrong quote
/// count, non-numeric pid) are silently skipped so a garbage process
/// entry can't crash port detection.
fn parse_tasklist_csv(stdout: &str) -> HashMap<u32, String> {
    let mut map = HashMap::new();
    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Minimal CSV split: strip the two quoted cells we care about.
        // Format guarantees the first two fields are `"name","pid"`.
        let mut parts = trimmed.splitn(3, ',');
        let Some(name_field) = parts.next() else {
            continue;
        };
        let Some(pid_field) = parts.next() else {
            continue;
        };
        let name = name_field.trim().trim_matches('"').to_string();
        let pid_str = pid_field.trim().trim_matches('"');
        if let Ok(pid) = pid_str.parse::<u32>() {
            if !name.is_empty() {
                map.insert(pid, name);
            }
        }
    }
    map
}

#[cfg(windows)]
mod windows_impl {
    use super::{
        is_codemux_internal_port, is_windows_system_process, parse_netstat_output,
        parse_tasklist_csv, PortInfo, IGNORED_PORTS,
    };
    use std::collections::HashMap;
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    /// CREATE_NO_WINDOW — suppresses the console flash when a GUI Tauri app
    /// shells out to `netstat.exe` or `tasklist.exe`. `ports::scan_ports`
    /// runs every 3 seconds, so a visible console would be extremely
    /// disruptive. Value from Win32 ProcessCreationFlags.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    fn silent_command(program: &str) -> Command {
        let mut cmd = Command::new(program);
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd
    }

    /// Detect all listening TCP ports on Windows.
    ///
    /// Thin I/O wrapper: shells out to `netstat` and `tasklist` with
    /// `CREATE_NO_WINDOW` (suppresses the console flash that would
    /// otherwise appear every 3 seconds under `scan_ports`), then
    /// delegates all parsing to the top-level `parse_netstat_output`
    /// function — which is cross-platform and unit-tested on Linux CI.
    ///
    /// After parsing, applies a Windows-specific system-process filter:
    /// drops ports owned by `System`, `svchost.exe`, `lsass.exe`, and
    /// other core Windows services. This matches Linux behavior, where
    /// `/proc/*/fd/` permissions implicitly filter system-owned sockets
    /// at the inode→PID resolution step. Without this filter the UI
    /// shows 16+ system ports (135 RPC, 139 NetBIOS, 445 SMB, etc.)
    /// that have no developer relevance.
    ///
    /// Filter is post-parse rather than inside `parse_netstat_output`
    /// so the parser stays a pure cross-platform string function and
    /// the Windows-specific behavior lives in Windows-specific code.
    pub fn detect_listening_ports() -> Vec<PortInfo> {
        let netstat = match silent_command("netstat").args(["-ano"]).output() {
            Ok(o) if o.status.success() => o,
            _ => return Vec::new(),
        };
        let stdout = String::from_utf8_lossy(&netstat.stdout);
        let process_names = load_process_names();
        let mut ports = parse_netstat_output(&stdout, &process_names);
        // Drop ports owned by Windows system processes. See
        // `is_windows_system_process` for the list and rationale.
        ports.retain(|p| !is_windows_system_process(&p.process_name));
        ports
    }

    /// One-shot `tasklist /NH /FO csv` call — builds a PID → image-name
    /// map so `detect_listening_ports` doesn't spawn one `tasklist` per
    /// port. Pure parsing is done in the top-level `parse_tasklist_csv`
    /// function; this wrapper only handles the `Command::output()` call.
    fn load_process_names() -> HashMap<u32, String> {
        let output = match silent_command("tasklist")
            .args(["/NH", "/FO", "csv"])
            .output()
        {
            Ok(o) if o.status.success() => o,
            _ => return HashMap::new(),
        };
        let stdout = String::from_utf8_lossy(&output.stdout);
        parse_tasklist_csv(&stdout)
    }

    /// Best-effort parent-PID lookup via `wmic`. Returns `None` if wmic is
    /// missing (newer Windows 11 images) or the process has exited — callers
    /// use the PPID walk only for workspace attribution, which gracefully
    /// degrades to "unassigned" when unresolved.
    pub fn read_ppid(pid: u32) -> Option<u32> {
        let output = silent_command("wmic")
            .args([
                "process",
                "where",
                &format!("ProcessId={pid}"),
                "get",
                "ParentProcessId",
                "/value",
            ])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        // wmic /value output: `ParentProcessId=5678` (possibly surrounded by
        // blank lines). Find the first matching line and parse the tail.
        for line in stdout.lines() {
            if let Some(rest) = line.trim().strip_prefix("ParentProcessId=") {
                return rest.trim().parse::<u32>().ok();
            }
        }
        None
    }

}

/// Check if a PID is a descendant of any of the given ancestor PIDs.
/// Returns the matching ancestor PID if found.
fn find_ancestor_pid(pid: u32, ancestors: &HashMap<u32, String>) -> Option<u32> {
    let mut current = pid;
    let mut visited = std::collections::HashSet::new();

    loop {
        if ancestors.contains_key(&current) {
            return Some(current);
        }
        if current <= 1 || !visited.insert(current) {
            return None;
        }
        current = read_ppid(current)?;
    }
}

/// Resolve workspace associations for detected ports.
///
/// `session_pids` maps session_id -> child PID.
/// `session_workspaces` maps session_id -> workspace_id.
pub fn resolve_workspace_associations(
    ports: &mut Vec<PortInfo>,
    session_pids: &HashMap<String, u32>,
    session_workspaces: &HashMap<String, String>,
) {
    // Build pid -> workspace_id lookup
    let pid_to_workspace: HashMap<u32, String> = session_pids
        .iter()
        .filter_map(|(session_id, &pid)| {
            session_workspaces
                .get(session_id)
                .map(|ws_id| (pid, ws_id.clone()))
        })
        .collect();

    if pid_to_workspace.is_empty() {
        return;
    }

    for port in ports.iter_mut() {
        if let Some(ancestor_pid) = find_ancestor_pid(port.pid, &pid_to_workspace) {
            port.workspace_id = pid_to_workspace.get(&ancestor_pid).cloned();
        }
    }
}

/// Load static port configuration from .codemux/ports.json in the workspace directory.
/// When this file exists, its entries replace dynamic detection for that workspace.
pub fn load_static_ports(workspace_cwd: &str, workspace_id: &str) -> Option<Vec<PortInfo>> {
    let config_path = Path::new(workspace_cwd).join(".codemux").join("ports.json");
    let contents = fs::read_to_string(&config_path).ok()?;
    let config: StaticPortsConfig = serde_json::from_str(&contents).ok()?;

    Some(
        config
            .ports
            .into_iter()
            .map(|entry| PortInfo {
                port: entry.port,
                pid: 0,
                process_name: String::new(),
                workspace_id: Some(workspace_id.to_string()),
                label: Some(entry.label),
                source: None,
            })
            .collect(),
    )
}

/// Go-template format passed to `docker ps`. Tab-separated so the pure
/// parser can split fields unambiguously — container names, paths, and
/// service names never contain tabs. Docker expands the literal `\t` escape
/// into a real tab in its output (verified against the CLI).
///
/// Fields, in order:
///   1. `.Names` — container name (e.g. `myproj-web-1`)
///   2. compose `working_dir` label — absolute path of the compose project
///      dir, which equals the codemux worktree path for agent-spawned stacks
///   3. compose `service` label — short service name (e.g. `web`)
///   4. `.Ports` — published / exposed port list
const DOCKER_PS_FORMAT: &str = "{{.Names}}\\t{{.Label \"com.docker.compose.project.working_dir\"}}\\t{{.Label \"com.docker.compose.service\"}}\\t{{.Ports}}";

/// Cached availability of the `docker` CLI. 0 = unknown, 1 = present,
/// 2 = binary not found. Only the "not found" verdict is cached: a missing
/// binary won't materialize mid-session, so there's no point spawning a
/// doomed process every 3 seconds. A present-but-erroring daemon (socket
/// down, missing permission) is deliberately NOT cached so detection
/// recovers on its own once the daemon is back.
static DOCKER_CLI_STATE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Run `docker ps` with [`DOCKER_PS_FORMAT`]. Returns raw stdout, or `None`
/// if docker is unavailable or the command failed. Suppresses the console
/// window on Windows — this runs every 3s, so a flashing console would be
/// unacceptable — mirroring the `netstat`/`tasklist` shell-outs.
fn run_docker_ps() -> Option<String> {
    use std::sync::atomic::Ordering;

    // Skip the spawn entirely once we've learned docker isn't installed.
    if DOCKER_CLI_STATE.load(Ordering::Relaxed) == 2 {
        return None;
    }

    let mut cmd = std::process::Command::new("docker");
    cmd.args(["ps", "--no-trunc", "--format", DOCKER_PS_FORMAT]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW — no console flash on the 3s heartbeat.
        cmd.creation_flags(0x0800_0000);
    }

    match cmd.output() {
        Ok(output) if output.status.success() => {
            DOCKER_CLI_STATE.store(1, Ordering::Relaxed);
            Some(String::from_utf8_lossy(&output.stdout).into_owned())
        }
        // Daemon down or no socket permission — transient, don't cache.
        Ok(_) => None,
        Err(e) => {
            if e.kind() == std::io::ErrorKind::NotFound {
                DOCKER_CLI_STATE.store(2, Ordering::Relaxed);
            }
            None
        }
    }
}

/// Detect published Docker container ports that belong to an open codemux
/// worktree.
///
/// Why this exists: on Linux, published container ports are owned by the
/// root `docker-proxy` process, so the `/proc/*/fd` PID-resolution step in
/// `linux_detect_listening_ports` hits a permission wall and silently drops
/// them. The listening socket is right there in `/proc/net/tcp` but is never
/// attributed to a PID, so the OS scan can't surface it. Querying the Docker
/// CLI recovers these ports with a meaningful label (the container name).
///
/// `workspace_paths` maps absolute path -> workspace_id for every open
/// workspace (both cwd and worktree path). A container is included only if
/// its compose `working_dir` matches one of those paths — this is what
/// scopes detection to codemux worktree containers and excludes unrelated
/// stacks the user runs by hand.
fn detect_docker_ports(workspace_paths: &HashMap<String, String>) -> Vec<PortInfo> {
    // No open workspaces → nothing a container could match → skip the spawn.
    if workspace_paths.is_empty() {
        return Vec::new();
    }
    let Some(stdout) = run_docker_ps() else {
        return Vec::new();
    };
    parse_docker_ps_output(&stdout, workspace_paths)
}

/// Pure parser for `docker ps` output in [`DOCKER_PS_FORMAT`]. Defined at
/// module level (not behind any cfg) so its tests run on every platform —
/// the body is pure string parsing with no syscalls.
///
/// Each line is `name \t working_dir \t service \t ports`. A row is kept only
/// if `working_dir` matches an open workspace path; for each *published* TCP
/// mapping (`IP:HOSTPORT->CPORT/tcp`) the host port becomes a `PortInfo`
/// tagged `source = "docker"` and labeled with the container name. Filtered
/// out: bare exposures with no host binding (`8000/tcp`), UDP mappings,
/// Codemux-internal ports, and the IPv4/IPv6 double-publish of one host port.
fn parse_docker_ps_output(
    stdout: &str,
    workspace_paths: &HashMap<String, String>,
) -> Vec<PortInfo> {
    let mut results: Vec<PortInfo> = Vec::new();
    let mut seen_ports: std::collections::HashSet<u16> = std::collections::HashSet::new();

    for line in stdout.lines() {
        let mut fields = line.splitn(4, '\t');
        let (Some(name), Some(working_dir), Some(service), Some(ports_str)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            continue;
        };

        let name = name.trim();
        // Match the container's compose project dir to an open workspace.
        // Trim a trailing slash so `/path` and `/path/` compare equal.
        let working_dir = working_dir.trim().trim_end_matches('/');
        if working_dir.is_empty() {
            continue;
        }
        let Some(workspace_id) = workspace_paths.get(working_dir) else {
            continue;
        };
        let service = service.trim();

        // `ports_str` looks like:
        //   "0.0.0.0:8099->8000/tcp, [::]:8099->8000/tcp, 8000/tcp"
        for segment in ports_str.split(',') {
            let segment = segment.trim();
            // Published mappings contain "->"; bare "8000/tcp" exposures don't.
            let Some((host_side, _container_side)) = segment.split_once("->") else {
                continue;
            };
            // Only TCP — the OS scan is TCP-only, keep parity.
            if !segment.ends_with("/tcp") {
                continue;
            }
            // `host_side` is "IP:PORT" (IPv4) or "[::]:PORT" (IPv6); the host
            // port is whatever follows the final ':'.
            let Some(port_str) = host_side.rsplit(':').next() else {
                continue;
            };
            let Ok(port) = port_str.parse::<u16>() else {
                continue;
            };
            if is_codemux_internal_port(port) {
                continue;
            }
            // The same host port is published once for IPv4 and once for
            // IPv6 — dedupe so it appears a single time.
            if !seen_ports.insert(port) {
                continue;
            }

            results.push(PortInfo {
                port,
                pid: 0,
                process_name: service.to_string(),
                workspace_id: Some(workspace_id.clone()),
                label: Some(name.to_string()),
                source: Some("docker".to_string()),
            });
        }
    }

    results.sort_by_key(|p| p.port);
    results
}

/// Full port scan: detect ports, resolve workspaces, apply static configs,
/// and fold in Docker-published worktree ports.
///
/// `workspace_paths` (absolute path -> workspace_id, covering cwd and
/// worktree path) is used only for matching Docker containers to worktrees.
pub fn scan_ports(
    session_pids: &HashMap<String, u32>,
    session_workspaces: &HashMap<String, String>,
    workspace_cwds: &HashMap<String, String>,
    workspace_paths: &HashMap<String, String>,
) -> Vec<PortInfo> {
    // Check for static port configs first
    let mut static_workspace_ids = std::collections::HashSet::new();
    let mut all_ports = Vec::new();

    for (ws_id, cwd) in workspace_cwds {
        if let Some(static_ports) = load_static_ports(cwd, ws_id) {
            static_workspace_ids.insert(ws_id.clone());
            all_ports.extend(static_ports);
        }
    }

    // Dynamic detection
    let mut detected = detect_listening_ports();
    resolve_workspace_associations(&mut detected, session_pids, session_workspaces);

    // Add dynamically detected ports, but skip those belonging to workspaces with static config
    for port in detected {
        let dominated_by_static = port
            .workspace_id
            .as_ref()
            .map(|ws_id| static_workspace_ids.contains(ws_id))
            .unwrap_or(false);
        if !dominated_by_static {
            all_ports.push(port);
        }
    }

    // Docker-published container ports. On Linux these are owned by the root
    // `docker-proxy` and so are invisible to the /proc scan above; the Docker
    // CLI recovers them, scoped to open codemux worktrees.
    let docker_ports = detect_docker_ports(workspace_paths);
    if !docker_ports.is_empty() {
        let mut existing: std::collections::HashSet<u16> =
            all_ports.iter().map(|p| p.port).collect();
        for dp in docker_ports {
            // Honor the static-config override: a workspace with a
            // .codemux/ports.json owns its port list outright.
            let dominated_by_static = dp
                .workspace_id
                .as_ref()
                .map(|ws_id| static_workspace_ids.contains(ws_id))
                .unwrap_or(false);
            if dominated_by_static {
                continue;
            }
            // Docker wins on a port collision (rootless docker can surface the
            // same port in the OS scan as `docker-proxy`/`rootlesskit` — the
            // container name is the better label).
            if existing.contains(&dp.port) {
                all_ports.retain(|p| p.port != dp.port);
            }
            existing.insert(dp.port);
            all_ports.push(dp);
        }
    }

    all_ports.sort_by_key(|p| p.port);
    all_ports
}

// ── Tests ─────────────────────────────────────────────────────────────────
//
// These tests drive the `parse_netstat_output` and `parse_tasklist_csv`
// functions directly with hardcoded fixtures. They run on every platform
// (NOT cfg-gated to Windows) so Linux CI catches regressions in the
// parsers before they reach a Windows runner. The parsers are pure text
// functions with no Windows-specific types or syscalls, which is what
// makes this cross-platform testing possible.

#[cfg(test)]
mod parser_tests {
    use super::{
        is_windows_system_process, parse_netstat_output, parse_tasklist_csv, PortInfo,
        WINDOWS_SYSTEM_PROCESS_NAMES,
    };
    use std::collections::HashMap;

    /// Realistic multi-line netstat fixture covering the cases we care about:
    /// - TCP LISTENING (IPv4) on a normal user port (5173)
    /// - TCP LISTENING on an IGNORED_PORTS entry (80) — must be skipped
    /// - TCP LISTENING on a regular user port (3950)
    /// - TCP LISTENING (IPv4, second entry) on another normal port (8080)
    /// - TCP LISTENING (IPv6) on 5173 — dedup candidate with the IPv4 entry
    /// - TCP ESTABLISHED — must be skipped (not LISTENING)
    /// - UDP row — must be skipped (not TCP)
    /// - Headers and blank lines interspersed
    const NETSTAT_FIXTURE: &str = "\
Active Connections

  Proto  Local Address          Foreign Address        State           PID
  TCP    0.0.0.0:5173           0.0.0.0:0              LISTENING       12345
  TCP    0.0.0.0:80             0.0.0.0:0              LISTENING       4
  TCP    0.0.0.0:3950           0.0.0.0:0              LISTENING       9999
  TCP    127.0.0.1:8080         0.0.0.0:0              LISTENING       54321
  TCP    [::]:5173              [::]:0                 LISTENING       12345
  TCP    192.168.1.10:443       93.184.216.34:443      ESTABLISHED     67890
  UDP    0.0.0.0:5353           *:*                                    2468
";

    fn empty_process_names() -> HashMap<u32, String> {
        HashMap::new()
    }

    #[test]
    fn test_parse_netstat_output_happy_path() {
        let process_names = empty_process_names();
        let ports = parse_netstat_output(NETSTAT_FIXTURE, &process_names);

        // Expected: 3950, 5173 (IPv4 first — IPv6 dedup'd), 8080.
        // Excluded: 80 (IGNORED_PORTS), 443 (ESTABLISHED, not LISTENING),
        //           5353 (UDP).
        let got_ports: Vec<u16> = ports.iter().map(|p| p.port).collect();
        assert_eq!(
            got_ports,
            vec![3950, 5173, 8080],
            "unexpected ports: {got_ports:?}"
        );

        // PIDs should match the fixture.
        let pid_for = |port: u16| ports.iter().find(|p| p.port == port).map(|p| p.pid);
        assert_eq!(pid_for(5173), Some(12345));
        assert_eq!(pid_for(8080), Some(54321));

        // Process names default to "unknown" when no tasklist mapping.
        for port in &ports {
            assert_eq!(port.process_name, "unknown");
        }
    }

    #[test]
    fn test_parse_netstat_output_uses_process_names() {
        let mut process_names = HashMap::new();
        process_names.insert(12345, "node.exe".to_string());
        // Intentionally leave 54321 unmapped to verify the fallback.

        let ports = parse_netstat_output(NETSTAT_FIXTURE, &process_names);
        let port_5173 = ports.iter().find(|p| p.port == 5173).expect("5173");
        let port_8080 = ports.iter().find(|p| p.port == 8080).expect("8080");

        assert_eq!(port_5173.process_name, "node.exe");
        assert_eq!(
            port_8080.process_name, "unknown",
            "unmapped pids must fall back to 'unknown'"
        );
    }

    #[test]
    fn test_parse_netstat_output_ipv6_entry_works_when_ipv4_absent() {
        // Only an IPv6 LISTENING row — verifies the `[::]:port` form
        // is parsed identically to `0.0.0.0:port` (both end with
        // `:<port>` so `rsplit(':')` picks up the tail).
        let fixture = "\
  Proto  Local Address    Foreign Address    State        PID
  TCP    [::]:7890        [::]:0             LISTENING    777
";
        let ports = parse_netstat_output(fixture, &empty_process_names());
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].port, 7890);
        assert_eq!(ports[0].pid, 777);
    }

    #[test]
    fn test_parse_netstat_empty_output() {
        let ports = parse_netstat_output("", &empty_process_names());
        assert!(
            ports.is_empty(),
            "empty input must produce empty vec, got {ports:?}"
        );

        // Also test whitespace-only — the header-only netstat output
        // when there are no TCP connections at all.
        let ports = parse_netstat_output("   \n\n\t\n", &empty_process_names());
        assert!(ports.is_empty());
    }

    #[test]
    fn test_parse_netstat_malformed_lines_are_skipped() {
        // Each of these lines is malformed in a different way.
        // None should panic; all should be silently skipped.
        // Only the last well-formed line (9000/1234) should survive.
        let fixture = "\
not-a-header
TCP
TCP without-port LISTENING
TCP 0.0.0.0: 0.0.0.0:0 LISTENING notapid
TCP 0.0.0.0:abc 0.0.0.0:0 LISTENING 1234
TCP 0.0.0.0:9000 0.0.0.0:0 LISTENING 1234
";
        let ports = parse_netstat_output(fixture, &empty_process_names());
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].port, 9000);
        assert_eq!(ports[0].pid, 1234);
    }

    #[test]
    fn test_parse_netstat_dedups_ipv4_ipv6_pair() {
        // Real netstat output typically shows the same service on both
        // 0.0.0.0:port and [::]:port. The parser keeps only the first
        // occurrence so the UI doesn't see two entries for one service.
        let fixture = "\
  TCP    0.0.0.0:4500   0.0.0.0:0  LISTENING  111
  TCP    [::]:4500      [::]:0     LISTENING  222
";
        let ports = parse_netstat_output(fixture, &empty_process_names());
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].port, 4500);
        assert_eq!(
            ports[0].pid, 111,
            "first row wins on dedup — should be the IPv4 entry"
        );
    }

    #[test]
    fn test_parse_netstat_results_sorted_by_port() {
        // Port selection intentionally dodges both filters:
        //   - IGNORED_PORTS = [22, 80, 443, 5432, 3306, 6379, 27017]
        //   - Codemux internal = >= 9222
        // 5000, 7000, 9000 are all outside both.
        let fixture = "\
  TCP    0.0.0.0:9000   0.0.0.0:0  LISTENING  1
  TCP    0.0.0.0:5000   0.0.0.0:0  LISTENING  2
  TCP    0.0.0.0:7000   0.0.0.0:0  LISTENING  3
";
        let ports = parse_netstat_output(fixture, &empty_process_names());
        let got: Vec<u16> = ports.iter().map(|p| p.port).collect();
        assert_eq!(got, vec![5000, 7000, 9000]);
    }

    #[test]
    fn test_parse_netstat_filters_codemux_internal_ports() {
        // Guards the agent-browser debug/stream range. Ports below 9222 are
        // ordinary application ports and must remain visible.
        let fixture = "\
  TCP    0.0.0.0:3950   0.0.0.0:0  LISTENING  11
  TCP    0.0.0.0:4100   0.0.0.0:0  LISTENING  22
  TCP    0.0.0.0:9222   0.0.0.0:0  LISTENING  33
  TCP    0.0.0.0:9500   0.0.0.0:0  LISTENING  44
  TCP    0.0.0.0:8080   0.0.0.0:0  LISTENING  55
";
        let ports = parse_netstat_output(fixture, &empty_process_names());
        let got: Vec<u16> = ports.iter().map(|p| p.port).collect();
        assert_eq!(got, vec![3950, 4100, 8080]);
    }

    #[test]
    fn test_parse_netstat_filters_ignored_ports() {
        // Guards against regressions in IGNORED_PORTS. These are system
        // services the UI intentionally hides (SSH, HTTP(S), common DBs).
        let fixture = "\
  TCP    0.0.0.0:22     0.0.0.0:0  LISTENING  11
  TCP    0.0.0.0:80     0.0.0.0:0  LISTENING  22
  TCP    0.0.0.0:443    0.0.0.0:0  LISTENING  33
  TCP    0.0.0.0:5432   0.0.0.0:0  LISTENING  44
  TCP    0.0.0.0:8080   0.0.0.0:0  LISTENING  55
";
        let ports = parse_netstat_output(fixture, &empty_process_names());
        let got: Vec<u16> = ports.iter().map(|p| p.port).collect();
        // Only 8080 survives — everything else is in IGNORED_PORTS.
        assert_eq!(got, vec![8080]);
    }

    /// Regression guard: `parse_netstat_output` must not panic on any input.
    /// We can't test every possible garbage input, but a few pathological
    /// cases cover the common crash vectors (null bytes, giant numbers,
    /// truncated rows, etc.).
    #[test]
    fn test_parse_netstat_never_panics() {
        let _ = parse_netstat_output("\x00\x01\x02", &empty_process_names());
        let _ = parse_netstat_output("TCP", &empty_process_names());
        let _ = parse_netstat_output("TCP\tTCP\tTCP\tTCP\tTCP", &empty_process_names());
        // Giant port number (>u16::MAX) — must be skipped, not panic.
        let _ = parse_netstat_output(
            "  TCP    0.0.0.0:99999  0.0.0.0:0  LISTENING  1234\n",
            &empty_process_names(),
        );
        // Giant PID (>u32::MAX) — must be skipped, not panic.
        let _ = parse_netstat_output(
            "  TCP    0.0.0.0:9000  0.0.0.0:0  LISTENING  999999999999999\n",
            &empty_process_names(),
        );
    }

    #[test]
    fn test_parse_tasklist_csv_happy_path() {
        let fixture = r#""svchost.exe","1024","Services","0","12,345 K"
"node.exe","5678","Console","1","45,678 K"
"chrome.exe","9999","Console","1","123,456 K"
"#;
        let map = parse_tasklist_csv(fixture);
        assert_eq!(map.get(&1024), Some(&"svchost.exe".to_string()));
        assert_eq!(map.get(&5678), Some(&"node.exe".to_string()));
        assert_eq!(map.get(&9999), Some(&"chrome.exe".to_string()));
        assert_eq!(map.len(), 3);
    }

    #[test]
    fn test_parse_tasklist_csv_empty_input() {
        let map = parse_tasklist_csv("");
        assert!(map.is_empty());

        let map = parse_tasklist_csv("\n\n\t\n");
        assert!(map.is_empty());
    }

    #[test]
    fn test_parse_tasklist_csv_skips_malformed_rows() {
        // Each line is broken in a different way. Only "valid.exe"
        // with pid 4321 should parse.
        let fixture = r#""only-one-field"
"name","not-a-number","rest"
"","1234","rest"
"valid.exe","4321","rest"
"#;
        let map = parse_tasklist_csv(fixture);
        assert_eq!(map.len(), 1);
        assert_eq!(map.get(&4321), Some(&"valid.exe".to_string()));
    }

    // ── Windows system-process filter tests ───────────────────────
    //
    // These guard the fix for the user-reported "Windows port detection
    // shows 16 system ports (135, 139, 445, 1042, 1043, ...)" issue.
    // The filter is the Windows equivalent of Linux's natural
    // /proc/*/fd/ permission filter — Linux drops system-owned sockets
    // implicitly, Windows needs an explicit process-name match.
    //
    // All tests are cross-platform: they call `is_windows_system_process`
    // directly and `parse_netstat_output` + `retain` to simulate what
    // `windows_impl::detect_listening_ports` does. No Windows syscalls.

    #[test]
    fn test_is_windows_system_process_matches_core_kernel_processes() {
        // The two highest-PID-frequency offenders from the user report:
        // PID 4 ("System") owns 139 NetBIOS, 445 SMB, plus a handful of
        // ephemeral ports. PID 0 ("Idle" / "System Idle Process") shows
        // up too on some Windows builds.
        assert!(is_windows_system_process("System"));
        assert!(is_windows_system_process("Idle"));
        assert!(is_windows_system_process("System Idle Process"));
    }

    #[test]
    fn test_is_windows_system_process_matches_svchost() {
        // svchost.exe is THE catch-all for hundreds of Windows services
        // — it owns 135 RPC Endpoint Mapper, 5040 Delivery Optimization,
        // and the dynamic RPC ephemeral ports (1042, 1043, etc. from the
        // user's report). This single match is the most important entry
        // in the filter list.
        assert!(is_windows_system_process("svchost.exe"));
    }

    #[test]
    fn test_is_windows_system_process_matches_security_subsystem() {
        // lsass + the Defender executables — lsass binds RPC, MsMpEng
        // and friends bind ephemeral inspection ports.
        assert!(is_windows_system_process("lsass.exe"));
        assert!(is_windows_system_process("MsMpEng.exe"));
        assert!(is_windows_system_process("NisSrv.exe"));
        assert!(is_windows_system_process("SecurityHealthService.exe"));
    }

    #[test]
    fn test_is_windows_system_process_matches_search_indexer() {
        // Windows Search binds ephemeral ports for content extraction
        // workers. Three separate executables — all need to be filtered.
        assert!(is_windows_system_process("SearchIndexer.exe"));
        assert!(is_windows_system_process("SearchProtocolHost.exe"));
        assert!(is_windows_system_process("SearchFilterHost.exe"));
    }

    #[test]
    fn test_is_windows_system_process_is_case_insensitive() {
        // Tasklist locale + Windows version variations may report the
        // same process with different casing. The filter must catch
        // all variants without per-locale casing tables.
        assert!(is_windows_system_process("SVCHOST.EXE"));
        assert!(is_windows_system_process("svchost.EXE"));
        assert!(is_windows_system_process("SvcHost.Exe"));
        assert!(is_windows_system_process("SYSTEM"));
        assert!(is_windows_system_process("system"));
    }

    #[test]
    fn test_is_windows_system_process_does_not_match_user_processes() {
        // CRITICAL false-positive guard: the filter MUST NOT hide any
        // process a developer might run. If this test fails, the user
        // would see their dev server vanish from the port UI.
        assert!(!is_windows_system_process("node.exe"));
        assert!(!is_windows_system_process("python.exe"));
        assert!(!is_windows_system_process("python3.exe"));
        assert!(!is_windows_system_process("ruby.exe"));
        assert!(!is_windows_system_process("rustc.exe"));
        assert!(!is_windows_system_process("cargo.exe"));
        assert!(!is_windows_system_process("go.exe"));
        assert!(!is_windows_system_process("java.exe"));
        assert!(!is_windows_system_process("dotnet.exe"));
        assert!(!is_windows_system_process("php.exe"));
        assert!(!is_windows_system_process("deno.exe"));
        assert!(!is_windows_system_process("bun.exe"));
    }

    #[test]
    fn test_is_windows_system_process_does_not_match_browsers() {
        // Browsers might be running a CDP debug port the user wants to
        // see — never filter them out. (The Codemux internal port range
        // 9222+ catches the agent-browser CDP port separately.)
        assert!(!is_windows_system_process("chrome.exe"));
        assert!(!is_windows_system_process("firefox.exe"));
        assert!(!is_windows_system_process("msedge.exe"));
        assert!(!is_windows_system_process("brave.exe"));
    }

    #[test]
    fn test_is_windows_system_process_does_not_match_databases() {
        // Database servers should remain visible — the user might be
        // running a non-default port. The IGNORED_PORTS list handles
        // standard ports (5432, 3306, 6379, 27017) at the port level.
        assert!(!is_windows_system_process("postgres.exe"));
        assert!(!is_windows_system_process("mysqld.exe"));
        assert!(!is_windows_system_process("redis-server.exe"));
        assert!(!is_windows_system_process("mongod.exe"));
    }

    #[test]
    fn test_is_windows_system_process_does_not_match_ide_language_servers() {
        // Language servers bind ports for IDE integration. User dev
        // tools — never filter.
        assert!(!is_windows_system_process("rust-analyzer.exe"));
        assert!(!is_windows_system_process("pylsp.exe"));
        assert!(!is_windows_system_process("typescript-language-server.exe"));
        assert!(!is_windows_system_process("clangd.exe"));
    }

    #[test]
    fn test_is_windows_system_process_handles_empty_and_unknown() {
        // Empty string and "unknown" (the fallback when tasklist
        // doesn't have the PID) must NOT be filtered. An unknown
        // process is still potentially user-relevant.
        assert!(!is_windows_system_process(""));
        assert!(!is_windows_system_process("unknown"));
    }

    #[test]
    fn test_windows_system_process_names_constant_is_non_empty() {
        // Compile-time-ish guard: if a future refactor accidentally
        // empties the list, every system port would suddenly leak
        // back into the UI. Loud failure here is much better than
        // a regression.
        assert!(
            !WINDOWS_SYSTEM_PROCESS_NAMES.is_empty(),
            "WINDOWS_SYSTEM_PROCESS_NAMES must contain at least one entry; \
             an empty list disables the filter and reintroduces the \
             16-system-ports-on-Windows bug"
        );
        // The two most important entries — defends against an entry
        // being removed without a paired test update.
        assert!(WINDOWS_SYSTEM_PROCESS_NAMES.contains(&"System"));
        assert!(WINDOWS_SYSTEM_PROCESS_NAMES.contains(&"svchost.exe"));
    }

    /// Apply the same filter that `windows_impl::detect_listening_ports`
    /// applies post-parse. Cross-platform helper so the integration
    /// test below can run on Linux CI.
    fn apply_windows_system_filter(ports: Vec<PortInfo>) -> Vec<PortInfo> {
        ports
            .into_iter()
            .filter(|p| !is_windows_system_process(&p.process_name))
            .collect()
    }

    #[test]
    fn test_windows_system_filter_removes_user_reported_system_ports() {
        // Reproduces the user-reported scenario: a netstat-style port
        // list (parsed) containing a mix of system ports (135, 139,
        // 445, 1042, 1043, 5040) and one legitimate dev server (5173).
        // After applying the filter only the dev server should remain.
        //
        // The PIDs and process names mirror what tasklist produces on
        // a real Windows box — System (PID 4) for SMB/NetBIOS, svchost
        // for RPC and Delivery Optimization. The test simulates the
        // post-parse filter step from `windows_impl::detect_listening_ports`.
        let input = vec![
            PortInfo {
                port: 135,
                pid: 1024,
                process_name: "svchost.exe".into(),
                workspace_id: None,
                label: None,
                source: None,
            },
            PortInfo {
                port: 139,
                pid: 4,
                process_name: "System".into(),
                workspace_id: None,
                label: None,
                source: None,
            },
            PortInfo {
                port: 445,
                pid: 4,
                process_name: "System".into(),
                workspace_id: None,
                label: None,
                source: None,
            },
            PortInfo {
                port: 1042,
                pid: 1024,
                process_name: "svchost.exe".into(),
                workspace_id: None,
                label: None,
                source: None,
            },
            PortInfo {
                port: 1043,
                pid: 1024,
                process_name: "svchost.exe".into(),
                workspace_id: None,
                label: None,
                source: None,
            },
            PortInfo {
                port: 5040,
                pid: 1024,
                process_name: "svchost.exe".into(),
                workspace_id: None,
                label: None,
                source: None,
            },
            PortInfo {
                port: 5173,
                pid: 12345,
                process_name: "node.exe".into(),
                workspace_id: None,
                label: None,
                source: None,
            },
        ];

        let filtered = apply_windows_system_filter(input);

        assert_eq!(
            filtered.len(),
            1,
            "expected only the node.exe dev server to survive, got: {filtered:?}"
        );
        assert_eq!(filtered[0].port, 5173);
        assert_eq!(filtered[0].process_name, "node.exe");
    }

    #[test]
    fn test_windows_system_filter_preserves_user_processes() {
        // Mirror image of the previous test: every entry should
        // SURVIVE the filter, because none of them are system processes.
        // This guards against an over-aggressive future addition to
        // WINDOWS_SYSTEM_PROCESS_NAMES.
        let input = vec![
            PortInfo {
                port: 3000,
                pid: 100,
                process_name: "node.exe".into(),
                workspace_id: None,
                label: None,
                source: None,
            },
            PortInfo {
                port: 8000,
                pid: 200,
                process_name: "python.exe".into(),
                workspace_id: None,
                label: None,
                source: None,
            },
            PortInfo {
                port: 9229,
                pid: 300,
                process_name: "chrome.exe".into(),
                workspace_id: None,
                label: None,
                source: None,
            },
            PortInfo {
                port: 8080,
                pid: 400,
                process_name: "java.exe".into(),
                workspace_id: None,
                label: None,
                source: None,
            },
        ];

        let filtered = apply_windows_system_filter(input.clone());
        assert_eq!(
            filtered, input,
            "filter must preserve all user processes verbatim"
        );
    }

    #[test]
    fn test_windows_system_filter_preserves_unknown_process_names() {
        // When tasklist doesn't have a PID (rare but possible — the
        // process exited between netstat and tasklist), the parser
        // sets process_name = "unknown". The filter must NOT drop
        // these — an unknown process might be user-relevant.
        let input = vec![PortInfo {
            port: 7777,
            pid: 999,
            process_name: "unknown".into(),
            workspace_id: None,
            label: None,
            source: None,
        }];

        let filtered = apply_windows_system_filter(input.clone());
        assert_eq!(filtered, input, "unknown processes must survive the filter");
    }

    /// End-to-end-ish test of the full Windows pipeline:
    /// netstat fixture → parse_netstat_output → system filter.
    /// Mirrors what `windows_impl::detect_listening_ports` does, except
    /// with a string fixture instead of a real netstat call.
    #[test]
    fn test_full_windows_pipeline_filters_system_and_keeps_user_ports() {
        // Realistic mixed fixture: a few system services that should
        // be filtered, one dev server that should survive.
        let fixture = "\
  Proto  Local Address          Foreign Address        State           PID
  TCP    0.0.0.0:135            0.0.0.0:0              LISTENING       1024
  TCP    0.0.0.0:139            0.0.0.0:0              LISTENING       4
  TCP    0.0.0.0:445            0.0.0.0:0              LISTENING       4
  TCP    0.0.0.0:1042           0.0.0.0:0              LISTENING       1024
  TCP    0.0.0.0:5040           0.0.0.0:0              LISTENING       1024
  TCP    0.0.0.0:5173           0.0.0.0:0              LISTENING       12345
";
        let mut process_names = HashMap::new();
        process_names.insert(4, "System".to_string());
        process_names.insert(1024, "svchost.exe".to_string());
        process_names.insert(12345, "node.exe".to_string());

        let parsed = parse_netstat_output(fixture, &process_names);
        // Parsing alone would return 5173 (135 is NOT in IGNORED_PORTS,
        // and the system-process filter hasn't run yet — this confirms
        // the parser is NOT doing the filtering, the post-parse step is).
        let parsed_ports: Vec<u16> = parsed.iter().map(|p| p.port).collect();
        // 135, 1042, 5040, 5173 should be in the parsed output.
        // 139 and 445 are also not in IGNORED_PORTS so they're parsed too.
        assert!(parsed_ports.contains(&5173), "5173 must be parsed");
        assert!(parsed_ports.contains(&135), "135 must be parsed (filter is post-parse)");
        assert!(parsed_ports.contains(&139), "139 must be parsed (filter is post-parse)");
        assert!(parsed_ports.contains(&445), "445 must be parsed (filter is post-parse)");

        let filtered = apply_windows_system_filter(parsed);
        let filtered_ports: Vec<u16> = filtered.iter().map(|p| p.port).collect();
        assert_eq!(
            filtered_ports,
            vec![5173],
            "after the system-process filter only the node.exe dev server should remain"
        );
    }
}

// ── Docker parser tests ─────────────────────────────────────────────────────
//
// Exercise `parse_docker_ps_output` with hand-built `docker ps` fixtures.
// The parser is pure string-processing (no `docker` binary, no syscalls), so
// these run on every platform on CI — the same cross-platform strategy used
// for the netstat/tasklist parsers above.

#[cfg(test)]
mod docker_parser_tests {
    use super::{parse_docker_ps_output, PortInfo};
    use std::collections::HashMap;

    const WT: &str = "/home/u/.codemux/worktrees/proj/feature-x";
    const WS_ID: &str = "ws-feature-x";

    /// One open workspace whose worktree path is `WT`.
    fn worktree_map() -> HashMap<String, String> {
        let mut m = HashMap::new();
        m.insert(WT.to_string(), WS_ID.to_string());
        m
    }

    /// Build a single `docker ps` line: name \t working_dir \t service \t ports.
    fn line(name: &str, working_dir: &str, service: &str, ports: &str) -> String {
        format!("{name}\t{working_dir}\t{service}\t{ports}")
    }

    #[test]
    fn published_port_for_matching_worktree_is_detected() {
        // A real container publishes the same host port on IPv4 and IPv6.
        let stdout = line(
            "proj-web-1",
            WT,
            "web",
            "0.0.0.0:8099->8000/tcp, [::]:8099->8000/tcp",
        );
        let ports = parse_docker_ps_output(&stdout, &worktree_map());

        assert_eq!(ports.len(), 1, "IPv4+IPv6 double-publish must dedupe to one");
        let p = &ports[0];
        assert_eq!(p.port, 8099);
        assert_eq!(p.label.as_deref(), Some("proj-web-1"), "label = container name");
        assert_eq!(p.process_name, "web", "process_name = compose service");
        assert_eq!(p.workspace_id.as_deref(), Some(WS_ID));
        assert_eq!(p.source.as_deref(), Some("docker"));
        assert_eq!(p.pid, 0, "Docker ports carry no OS pid");
    }

    #[test]
    fn unpublished_exposure_is_ignored() {
        // `EXPOSE`d but not published: no "->" host mapping.
        let stdout = line("proj-db-1", WT, "db", "5432/tcp, 8000/tcp");
        assert!(
            parse_docker_ps_output(&stdout, &worktree_map()).is_empty(),
            "ports with no host binding must not appear"
        );
    }

    #[test]
    fn container_outside_any_worktree_is_excluded() {
        // working_dir doesn't match an open workspace — unrelated stack.
        let stdout = line(
            "random-redis-1",
            "/opt/other/stack",
            "redis",
            "0.0.0.0:6400->6379/tcp",
        );
        assert!(
            parse_docker_ps_output(&stdout, &worktree_map()).is_empty(),
            "containers outside codemux worktrees must be filtered out"
        );
    }

    #[test]
    fn codemux_internal_ports_are_skipped() {
        // 9300 is Codemux-internal; ordinary app ports 4000 and 8090 survive.
        let stdout = line(
            "proj-svc-1",
            WT,
            "svc",
            "127.0.0.1:4000->80/tcp, 127.0.0.1:9300->80/tcp, 127.0.0.1:8090->80/tcp",
        );
        let ports = parse_docker_ps_output(&stdout, &worktree_map());
        let nums: Vec<u16> = ports.iter().map(|p| p.port).collect();
        assert_eq!(nums, vec![4000, 8090]);
    }

    #[test]
    fn udp_mappings_are_ignored() {
        let stdout = line("proj-dns-1", WT, "dns", "0.0.0.0:5353->53/udp");
        assert!(
            parse_docker_ps_output(&stdout, &worktree_map()).is_empty(),
            "UDP mappings must be ignored (parity with the TCP-only OS scan)"
        );
    }

    #[test]
    fn multiple_services_in_one_project_each_appear() {
        let stdout = format!(
            "{}\n{}\n{}",
            line("proj-web-1", WT, "web", "127.0.0.1:7000->7000/tcp"),
            line("proj-api-1", WT, "api", "127.0.0.1:8080->8080/tcp"),
            line("proj-cache-1", WT, "cache", "6379/tcp"), // unpublished → skipped
        );
        let ports = parse_docker_ps_output(&stdout, &worktree_map());
        let nums: Vec<u16> = ports.iter().map(|p| p.port).collect();
        assert_eq!(nums, vec![7000, 8080], "results are sorted by port");
        let services: Vec<&str> = ports.iter().map(|p| p.process_name.as_str()).collect();
        assert_eq!(services, vec!["web", "api"]);
    }

    #[test]
    fn trailing_slash_in_working_dir_still_matches() {
        // Compose can report the dir with a trailing slash; the workspace
        // map stores it without one. They must still match.
        let with_slash = format!("{WT}/");
        let stdout = line("proj-web-1", &with_slash, "web", "0.0.0.0:9090->80/tcp");
        let ports = parse_docker_ps_output(&stdout, &worktree_map());
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].port, 9090);
    }

    #[test]
    fn blank_and_malformed_lines_are_skipped() {
        // Empty lines and rows with too few tab fields must not panic.
        let stdout = format!(
            "\n{}\nshort-line-no-tabs\n{}",
            line("proj-web-1", WT, "web", "0.0.0.0:8099->8000/tcp"),
            "name-only\tdir-only",
        );
        let ports = parse_docker_ps_output(&stdout, &worktree_map());
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].port, 8099);
    }

    #[test]
    fn empty_workspace_map_yields_nothing() {
        let stdout = line("proj-web-1", WT, "web", "0.0.0.0:8099->8000/tcp");
        assert!(parse_docker_ps_output(&stdout, &HashMap::new()).is_empty());
    }

    /// Ensures the field constants stay aligned with what we construct: a
    /// `PortInfo` from this parser always has `source = Some("docker")`.
    #[test]
    fn detected_ports_are_tagged_docker() {
        let stdout = line("proj-web-1", WT, "web", "0.0.0.0:8099->8000/tcp");
        let ports: Vec<PortInfo> = parse_docker_ps_output(&stdout, &worktree_map());
        assert!(ports.iter().all(|p| p.source.as_deref() == Some("docker")));
    }
}

// ── Live Docker integration test (requires a running Docker daemon) ──────────
//
// `#[ignore]`d so `cargo test` and CI stay hermetic (no docker dependency).
// Unlike the pure-parser tests above, this drives the *real* `detect_docker_ports`
// path — an actual `docker ps` spawn with `DOCKER_PS_FORMAT`, then parse + the
// worktree-scope filter. Run it manually against a labeled container whose
// compose `working_dir` you pass via env:
//
//   docker run -d --name demo \
//     --label com.docker.compose.project.working_dir="$PWD" \
//     --label com.docker.compose.service=web -p 8099:8000 \
//     python:3.12-slim python -m http.server 8000
//   CODEMUX_DOCKER_TEST_WD="$PWD" CODEMUX_DOCKER_TEST_PORT=8099 \
//     cargo test --manifest-path src-tauri/Cargo.toml --lib \
//     docker_live_tests -- --ignored --nocapture
#[cfg(test)]
mod docker_live_tests {
    use super::detect_docker_ports;
    use std::collections::HashMap;

    #[test]
    #[ignore = "requires a running Docker daemon and a labeled test container"]
    fn detects_live_published_port_for_worktree() {
        let wd = std::env::var("CODEMUX_DOCKER_TEST_WD")
            .expect("set CODEMUX_DOCKER_TEST_WD to the container's compose working_dir");
        let expected_port: u16 = std::env::var("CODEMUX_DOCKER_TEST_PORT")
            .expect("set CODEMUX_DOCKER_TEST_PORT")
            .parse()
            .expect("CODEMUX_DOCKER_TEST_PORT must be a u16");

        let mut paths = HashMap::new();
        paths.insert(
            wd.trim_end_matches('/').to_string(),
            "ws-live-test".to_string(),
        );

        let ports = detect_docker_ports(&paths);
        eprintln!("detect_docker_ports({wd}) -> {ports:#?}");

        let hit = ports
            .iter()
            .find(|p| p.port == expected_port)
            .unwrap_or_else(|| panic!("port {expected_port} not detected; got {ports:?}"));
        assert_eq!(hit.source.as_deref(), Some("docker"));
        assert_eq!(hit.workspace_id.as_deref(), Some("ws-live-test"));
        assert!(
            hit.label.as_deref().is_some_and(|l| !l.is_empty()),
            "label should be the container name"
        );
    }

    /// A container whose `working_dir` matches NO open workspace must be
    /// excluded — proves the worktree-scope filter against the live daemon.
    #[test]
    #[ignore = "requires a running Docker daemon and a labeled test container"]
    fn excludes_live_container_outside_worktrees() {
        let expected_port: u16 = std::env::var("CODEMUX_DOCKER_TEST_PORT")
            .expect("set CODEMUX_DOCKER_TEST_PORT")
            .parse()
            .expect("CODEMUX_DOCKER_TEST_PORT must be a u16");

        // Map points at an unrelated path, so the live container shouldn't match.
        let mut paths = HashMap::new();
        paths.insert("/nonexistent/worktree".to_string(), "ws-other".to_string());

        let ports = detect_docker_ports(&paths);
        assert!(
            !ports.iter().any(|p| p.port == expected_port),
            "container outside the workspace path set must be filtered out; got {ports:?}"
        );
    }
}
