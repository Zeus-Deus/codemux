use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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
/// Parses `/proc/net/tcp` and `/proc/net/tcp6` for LISTEN-state sockets,
/// then resolves owning PIDs via `/proc/*/fd/` symlinks.
pub fn detect_listening_ports() -> Vec<PortInfo> {
    let listening = parse_proc_net_tcp();
    if listening.is_empty() {
        return Vec::new();
    }

    let inode_to_port: HashMap<u64, u16> = listening.into_iter().collect();
    resolve_pids_for_inodes(&inode_to_port)
}

/// Parse /proc/net/tcp and /proc/net/tcp6 for listening sockets.
/// Returns Vec<(inode, port)>.
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
fn read_process_name(pid: u32) -> String {
    fs::read_to_string(format!("/proc/{}/comm", pid))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "unknown".into())
}

/// Read the parent PID from /proc/<pid>/stat.
fn read_ppid(pid: u32) -> Option<u32> {
    let stat = fs::read_to_string(format!("/proc/{}/stat", pid)).ok()?;
    // Format: pid (comm) state ppid ...
    // comm can contain spaces and parens, so find the last ')' first
    let after_comm = stat.rfind(')')? + 2;
    let remainder = stat.get(after_comm..)?;
    let fields: Vec<&str> = remainder.split_whitespace().collect();
    // fields[0] = state, fields[1] = ppid
    fields.get(1)?.parse().ok()
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
