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

/// Codemux internal port ranges.
fn is_codemux_internal_port(port: u16) -> bool {
    (3900..=4199).contains(&port) || port >= 9222
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortInfo {
    pub port: u16,
    pub pid: u32,
    pub process_name: String,
    pub workspace_id: Option<String>,
    pub label: Option<String>,
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

#[cfg(windows)]
mod windows_impl {
    use super::{
        is_codemux_internal_port, PortInfo, IGNORED_PORTS,
    };
    use std::collections::{HashMap, HashSet};
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

    /// Parse `netstat -ano` (+ `tasklist /NH /FO csv` for process names) and
    /// return the listening TCP ports owned by any user-visible process.
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
    /// split point. Filter out Codemux-internal + common system/db ports so
    /// the UI matches the Linux behavior.
    pub fn detect_listening_ports() -> Vec<PortInfo> {
        let netstat = match silent_command("netstat").args(["-ano"]).output() {
            Ok(o) if o.status.success() => o,
            _ => return Vec::new(),
        };
        let stdout = String::from_utf8_lossy(&netstat.stdout);

        let process_names = load_process_names();

        let mut results: Vec<PortInfo> = Vec::new();
        let mut seen_ports: HashSet<u16> = HashSet::new();

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
            });
        }

        results.sort_by_key(|p| p.port);
        results
    }

    /// One-shot `tasklist /NH /FO csv` call — builds a PID → image-name map
    /// so `detect_listening_ports` doesn't spawn one `tasklist` per port.
    ///
    /// CSV rows look like:
    /// ```text
    /// "svchost.exe","1024","Services","0","12,345 K"
    /// ```
    fn load_process_names() -> HashMap<u32, String> {
        let mut map = HashMap::new();
        let output = match silent_command("tasklist")
            .args(["/NH", "/FO", "csv"])
            .output()
        {
            Ok(o) if o.status.success() => o,
            _ => return map,
        };
        let stdout = String::from_utf8_lossy(&output.stdout);

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
            })
            .collect(),
    )
}

/// Full port scan: detect ports, resolve workspaces, apply static configs.
pub fn scan_ports(
    session_pids: &HashMap<String, u32>,
    session_workspaces: &HashMap<String, String>,
    workspace_cwds: &HashMap<String, String>,
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

    all_ports.sort_by_key(|p| p.port);
    all_ports
}
