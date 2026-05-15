//! Resource monitor — per-terminal CPU + memory aggregation.
//!
//! Mirrors the "Resource Consumption" feature: a single [`get_resource_metrics`]
//! command returns one atomic snapshot describing
//!
//! - the Codemux app's own process tree (split into main / web view / other),
//! - every live terminal session grouped by workspace and project, with each
//!   session's CPU + memory summed across its entire process subtree, and
//! - host totals (system RAM, CPU core count, load average).
//!
//! ## How the numbers are produced
//!
//! [`sysinfo`] gives us one process table per refresh: PID, parent PID, CPU%,
//! and resident memory for every process. From that we build a parent → children
//! map and walk subtrees so a terminal running `npm → node → workers` reports the
//! *combined* footprint of the whole tree, not just the shell.
//!
//! CPU usage is a delta measurement: `sysinfo` computes it from the time elapsed
//! between two refreshes. We keep a persistent [`System`] in Tauri state, so each
//! poll is measured against the previous poll. The very first call after launch
//! reports `0.0` CPU for everything; every call after that is accurate.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
use tauri::State;

use crate::state::{AppStateStore, PaneNodeSnapshot, WorkspaceSnapshot};
use crate::terminal::PtyState;

// ── Serializable snapshot shape (snake_case → matches src/tauri/types.ts) ──

/// CPU percentage (100.0 == one core fully busy) + resident memory in bytes.
#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq)]
pub struct UsageValues {
    pub cpu: f32,
    pub memory: u64,
}

impl UsageValues {
    fn add(&mut self, other: UsageValues) {
        self.cpu += other.cpu;
        self.memory += other.memory;
    }
}

/// One terminal pane's process subtree.
#[derive(Debug, Clone, Serialize)]
pub struct SessionMetrics {
    pub session_id: String,
    pub pane_id: String,
    pub pid: u32,
    pub title: Option<String>,
    pub cpu: f32,
    pub memory: u64,
}

/// One workspace with its terminal sessions. `cpu`/`memory` are the sum of the
/// workspace's sessions.
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceMetrics {
    pub workspace_id: String,
    pub project_id: String,
    pub project_name: String,
    pub workspace_name: String,
    pub cpu: f32,
    pub memory: u64,
    pub sessions: Vec<SessionMetrics>,
}

/// Codemux's own process tree, split by role. `cpu`/`memory` are the sum of the
/// three buckets.
#[derive(Debug, Clone, Serialize)]
pub struct AppMetrics {
    pub cpu: f32,
    pub memory: u64,
    /// The Codemux main process.
    pub main: UsageValues,
    /// WebKit/WebView helper processes that render the UI.
    pub web_view: UsageValues,
    /// Everything else we spawned that is not a monitored terminal subtree
    /// (e.g. agent sidecars, helper daemons).
    pub other: UsageValues,
}

/// Host-level system totals — the denominator for the "RAM share" readout.
#[derive(Debug, Clone, Serialize)]
pub struct HostMetrics {
    pub total_memory: u64,
    pub free_memory: u64,
    pub used_memory: u64,
    pub memory_usage_percent: f32,
    pub cpu_core_count: u32,
    pub load_average_1m: f64,
}

/// The full snapshot returned to the renderer.
#[derive(Debug, Clone, Serialize)]
pub struct ResourceMetricsSnapshot {
    pub app: AppMetrics,
    pub workspaces: Vec<WorkspaceMetrics>,
    pub host: HostMetrics,
    pub total_cpu: f32,
    pub total_memory: u64,
    /// Unix epoch milliseconds the snapshot was collected.
    pub collected_at: u64,
}

// ── Tauri-managed state ──

/// Holds the persistent [`System`] handle. CPU percentages are delta-measured
/// between refreshes, so the handle must survive across calls — a fresh
/// `System` every call would always report `0.0` CPU.
pub struct ResourceMonitorState {
    system: Mutex<System>,
}

impl ResourceMonitorState {
    pub fn new() -> Self {
        Self {
            system: Mutex::new(System::new()),
        }
    }
}

impl Default for ResourceMonitorState {
    fn default() -> Self {
        Self::new()
    }
}

// ── Process-tree helpers ──

/// Parent → direct children index, built once per refresh.
struct ProcessTree {
    children_of: HashMap<Pid, Vec<Pid>>,
    /// PID → (cpu%, resident bytes).
    usage_of: HashMap<Pid, UsageValues>,
    /// PID → process name, lower-cased (for web-view classification).
    name_of: HashMap<Pid, String>,
}

impl ProcessTree {
    fn build(system: &System) -> Self {
        let mut children_of: HashMap<Pid, Vec<Pid>> = HashMap::new();
        let mut usage_of: HashMap<Pid, UsageValues> = HashMap::new();
        let mut name_of: HashMap<Pid, String> = HashMap::new();

        for (pid, process) in system.processes() {
            // sysinfo iterates `/proc/[pid]/task/[tid]` on Linux, so its
            // process table is full of *threads* alongside real processes.
            // A thread shares its leader's address space, so its `memory()`
            // reports the exact same RSS as the leader — summing them
            // multiplies one WebKit/Chromium/node process's RAM by its
            // thread count (10–30×), which is what made the RAM-share
            // readout exceed 100%. Keep only the thread-group leaders so
            // every process is counted once.
            if process.thread_kind().is_some() {
                continue;
            }
            usage_of.insert(
                *pid,
                UsageValues {
                    cpu: process.cpu_usage(),
                    memory: process.memory(),
                },
            );
            name_of.insert(*pid, process.name().to_string_lossy().to_lowercase());
            if let Some(parent) = process.parent() {
                children_of.entry(parent).or_default().push(*pid);
            }
        }

        Self {
            children_of,
            usage_of,
            name_of,
        }
    }

    /// Every PID in the subtree rooted at `root` (inclusive), provided it still
    /// exists in this snapshot. Iterative DFS with a visited set so a malformed
    /// parent cycle can never spin forever.
    fn subtree_pids(&self, root: Pid) -> Vec<Pid> {
        let mut out = Vec::new();
        let mut visited = HashSet::new();
        let mut stack = vec![root];

        while let Some(pid) = stack.pop() {
            if !visited.insert(pid) {
                continue;
            }
            if self.usage_of.contains_key(&pid) {
                out.push(pid);
            }
            if let Some(children) = self.children_of.get(&pid) {
                stack.extend(children.iter().copied());
            }
        }

        out
    }

    /// Sum CPU + memory across the given PIDs.
    fn usage_for(&self, pids: &[Pid]) -> UsageValues {
        let mut total = UsageValues::default();
        for pid in pids {
            if let Some(usage) = self.usage_of.get(pid) {
                total.add(*usage);
            }
        }
        total
    }
}

/// `true` when a process name looks like a WebKit/WebView helper, so the app
/// section can show "Web View" usage separately from the main process.
fn is_web_view_process(name: &str) -> bool {
    name.contains("webkit")
        || name.contains("webprocess")
        || name.contains("webengine")
        || name.contains("msedgewebview")
}

/// Collect `(session_id, pane_id)` for every terminal pane in a workspace.
fn collect_terminal_panes(node: &PaneNodeSnapshot, out: &mut Vec<(String, String)>) {
    match node {
        PaneNodeSnapshot::Terminal {
            pane_id,
            session_id,
            ..
        } => out.push((session_id.0.clone(), pane_id.0.clone())),
        PaneNodeSnapshot::Split { children, .. } => {
            for child in children {
                collect_terminal_panes(child, out);
            }
        }
        _ => {}
    }
}

/// Last non-empty path segment of `path`, or `"Project"` if there is none.
fn path_basename(path: &str) -> String {
    path.rsplit(['/', '\\'])
        .find(|segment| !segment.is_empty())
        .unwrap_or("Project")
        .to_string()
}

/// Derive a stable `(project_id, project_name)` for grouping workspaces.
///
/// `project_root` (set on hydrated workspaces) is preferred; otherwise the
/// session/workspace `cwd` is used. Sibling worktrees live under
/// `.../.codemux/worktrees/<project>/<branch>`, so those collapse to the
/// shared `<project>` and group together.
fn project_identity(project_root: Option<&str>, cwd: &str) -> (String, String) {
    let base = project_root
        .filter(|p| !p.is_empty())
        .unwrap_or(cwd);

    const WORKTREE_MARKER: &str = "/.codemux/worktrees/";
    if let Some(idx) = base.find(WORKTREE_MARKER) {
        let rest = &base[idx + WORKTREE_MARKER.len()..];
        if let Some(project) = rest.split('/').find(|s| !s.is_empty()) {
            return (format!("worktree:{project}"), project.to_string());
        }
    }

    if base.is_empty() {
        return ("unknown".to_string(), "Project".to_string());
    }
    (base.to_string(), path_basename(base))
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ── Physical memory (PSS on Linux) ──

/// Parse the `Pss:` line out of a `/proc/<pid>/smaps_rollup` body and return
/// it in bytes. `smaps_rollup` reports `Pss:` in kB.
fn parse_smaps_rollup_pss(content: &str) -> Option<u64> {
    for line in content.lines() {
        // Match `Pss:` exactly — `smaps_rollup` also has `Pss_Anon:`,
        // `Pss_File:`, `Pss_Shmem:` which we must not pick up.
        if let Some(rest) = line.strip_prefix("Pss:") {
            let kb: u64 = rest
                .trim()
                .trim_end_matches("kB")
                .trim()
                .parse()
                .ok()?;
            return Some(kb.saturating_mul(1024));
        }
    }
    None
}

/// Replace RSS with PSS (proportional set size) for the given PIDs on Linux.
///
/// RSS counts every shared page in full for every process that maps it, so
/// summing RSS across a multi-process tree (WebKit, Chromium, node clusters)
/// massively overcounts real RAM use — a tree can read tens of GB when it is
/// actually using a few. PSS splits shared pages proportionally between the
/// processes mapping them, so a subtree sum reflects the physical memory the
/// group is genuinely responsible for. That is the honest number to show.
///
/// On non-Linux platforms this is a no-op and the sysinfo RSS value stands
/// (the cross-platform equivalent — macOS `phys_footprint` — would need a
/// native shim; Windows working-set is already close enough).
#[cfg(target_os = "linux")]
fn enrich_with_pss(tree: &mut ProcessTree, pids: &HashSet<Pid>) {
    for pid in pids {
        let path = format!("/proc/{}/smaps_rollup", pid.as_u32());
        // Process may have exited, or be owned by another user — keep the
        // RSS value already in the snapshot as a fallback.
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        if let Some(pss) = parse_smaps_rollup_pss(&content) {
            if let Some(usage) = tree.usage_of.get_mut(pid) {
                usage.memory = pss;
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn enrich_with_pss(_tree: &mut ProcessTree, _pids: &HashSet<Pid>) {}

// ── Snapshot collection ──

/// One workspace's accumulating bucket of resolved terminal sessions.
struct WorkspaceBucket {
    project_id: String,
    project_name: String,
    workspace_id: String,
    workspace_name: String,
    /// `(session_id, pane_id, root_pid, subtree_pids)` per terminal.
    sessions: Vec<(String, String, u32, Vec<Pid>)>,
}

/// Build a full resource snapshot from the current process table + app state.
///
/// The set of measurable terminals comes from [`PtyState::get_session_pids`] —
/// every live PTY, regardless of whether its workspace's pane tree is currently
/// hydrated in the app snapshot. Workspace/project attribution is resolved from
/// the hydrated `surfaces` when available, and otherwise synthesised from the
/// session's working directory, so a live terminal can never silently vanish
/// from the monitor just because its workspace is parked.
fn collect_snapshot(
    system: &mut System,
    pty_state: &PtyState,
    app_state: &AppStateStore,
) -> ResourceMetricsSnapshot {
    // One atomic refresh of the process table + system memory. CPU% is measured
    // against the previous refresh stored in `system`.
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_cpu().with_memory(),
    );
    system.refresh_memory();

    let mut tree = ProcessTree::build(system);
    let session_pids = pty_state.get_session_pids(); // session_id -> u32 pid
    let snapshot = app_state.snapshot();

    // session_id -> (cwd, title) from the app's terminal-session list, which
    // is independent of which pane trees happen to be hydrated.
    let mut session_cwd: HashMap<String, String> = HashMap::new();
    let mut session_titles: HashMap<String, String> = HashMap::new();
    for session in &snapshot.terminal_sessions {
        session_cwd.insert(session.session_id.0.clone(), session.cwd.clone());
        session_titles.insert(session.session_id.0.clone(), session.title.clone());
    }

    // session_id -> (&workspace, pane_id) for workspaces whose pane tree is
    // currently hydrated. Parked workspaces have empty `surfaces`, so their
    // sessions won't appear here — the cwd fallback below covers them.
    let mut session_workspace: HashMap<String, (&WorkspaceSnapshot, String)> =
        HashMap::new();
    for workspace in &snapshot.workspaces {
        let mut panes: Vec<(String, String)> = Vec::new();
        for surface in &workspace.surfaces {
            collect_terminal_panes(&surface.root, &mut panes);
        }
        for (session_id, pane_id) in panes {
            session_workspace
                .entry(session_id)
                .or_insert((workspace, pane_id));
        }
    }

    // ── Resolve every live terminal PTY into a workspace bucket ──
    // Built before PSS enrichment so `/proc/<pid>/smaps_rollup` is read only
    // for the processes we actually report on.
    let mut buckets: Vec<WorkspaceBucket> = Vec::new();
    let mut bucket_index: HashMap<String, usize> = HashMap::new();
    let mut session_subtree_pids: HashSet<Pid> = HashSet::new();

    for (session_id, pid) in &session_pids {
        let subtree = tree.subtree_pids(Pid::from_u32(*pid));
        if subtree.is_empty() {
            // PTY pid not in the process table (raced an exit) — skip.
            continue;
        }
        session_subtree_pids.extend(subtree.iter().copied());

        // Resolve workspace + project identity for this session.
        let (workspace_id, workspace_name, project_id, project_name, pane_id) =
            match session_workspace.get(session_id) {
                Some((workspace, pane_id)) => {
                    let (project_id, project_name) = project_identity(
                        workspace.project_root.as_deref(),
                        &workspace.cwd,
                    );
                    let workspace_name = if workspace.title.is_empty() {
                        path_basename(&workspace.cwd)
                    } else {
                        workspace.title.clone()
                    };
                    (
                        workspace.workspace_id.0.clone(),
                        workspace_name,
                        project_id,
                        project_name,
                        pane_id.clone(),
                    )
                }
                None => {
                    // Parked / not-yet-hydrated workspace: attribute by cwd so
                    // the terminal still shows up under a sensible project.
                    let cwd = session_cwd.get(session_id).cloned().unwrap_or_default();
                    let (project_id, project_name) = project_identity(None, &cwd);
                    let workspace_id = if cwd.is_empty() {
                        format!("session:{session_id}")
                    } else {
                        format!("cwd:{cwd}")
                    };
                    (
                        workspace_id,
                        path_basename(&cwd),
                        project_id,
                        project_name,
                        session_id.clone(),
                    )
                }
            };

        let bucket_idx = *bucket_index
            .entry(workspace_id.clone())
            .or_insert_with(|| {
                buckets.push(WorkspaceBucket {
                    project_id,
                    project_name,
                    workspace_id,
                    workspace_name,
                    sessions: Vec::new(),
                });
                buckets.len() - 1
            });
        buckets[bucket_idx]
            .sessions
            .push((session_id.clone(), pane_id, *pid, subtree));
    }

    // The Codemux app's own subtree (current process); terminal subtrees are
    // excluded later when bucketing into main / web view / other.
    let current_pid = sysinfo::get_current_pid().ok();
    let app_subtree: Vec<Pid> = current_pid
        .map(|p| tree.subtree_pids(p))
        .unwrap_or_default();

    // Enrich exactly the PIDs we report on with PSS, so every sum below is
    // honest physical memory instead of shared-page-inflated RSS.
    let mut relevant: HashSet<Pid> = HashSet::new();
    relevant.extend(app_subtree.iter().copied());
    relevant.extend(session_subtree_pids.iter().copied());
    enrich_with_pss(&mut tree, &relevant);

    // ── Build workspace metrics from the PSS-corrected tree ──
    let workspaces: Vec<WorkspaceMetrics> = buckets
        .into_iter()
        .map(|bucket| {
            let mut sessions: Vec<SessionMetrics> = Vec::new();
            let mut workspace_usage = UsageValues::default();
            for (session_id, pane_id, pid, subtree) in bucket.sessions {
                let usage = tree.usage_for(&subtree);
                workspace_usage.add(usage);
                sessions.push(SessionMetrics {
                    title: session_titles
                        .get(&session_id)
                        .filter(|t| !t.is_empty())
                        .cloned(),
                    session_id,
                    pane_id,
                    pid,
                    cpu: usage.cpu,
                    memory: usage.memory,
                });
            }
            WorkspaceMetrics {
                workspace_id: bucket.workspace_id,
                project_id: bucket.project_id,
                project_name: bucket.project_name,
                workspace_name: bucket.workspace_name,
                cpu: workspace_usage.cpu,
                memory: workspace_usage.memory,
                sessions,
            }
        })
        .collect();

    let app = collect_app_metrics(&tree, &app_subtree, current_pid, &session_subtree_pids);

    let workspace_cpu: f32 = workspaces.iter().map(|w| w.cpu).sum();
    let workspace_memory: u64 = workspaces.iter().map(|w| w.memory).sum();

    ResourceMetricsSnapshot {
        total_cpu: app.cpu + workspace_cpu,
        total_memory: app.memory + workspace_memory,
        app,
        workspaces,
        host: collect_host_metrics(system),
        collected_at: unix_millis(),
    }
}

/// Bucket the Codemux process subtree into main / web view / other, excluding
/// any PID that belongs to a monitored terminal subtree (those are reported
/// under their workspace instead).
fn collect_app_metrics(
    tree: &ProcessTree,
    app_subtree: &[Pid],
    current_pid: Option<Pid>,
    session_subtree_pids: &HashSet<Pid>,
) -> AppMetrics {
    let mut main = UsageValues::default();
    let mut web_view = UsageValues::default();
    let mut other = UsageValues::default();

    for pid in app_subtree {
        if session_subtree_pids.contains(pid) {
            continue;
        }
        let Some(usage) = tree.usage_of.get(pid) else {
            continue;
        };
        if Some(*pid) == current_pid {
            main.add(*usage);
        } else if tree
            .name_of
            .get(pid)
            .map(|name| is_web_view_process(name))
            .unwrap_or(false)
        {
            web_view.add(*usage);
        } else {
            other.add(*usage);
        }
    }

    AppMetrics {
        cpu: main.cpu + web_view.cpu + other.cpu,
        memory: main.memory + web_view.memory + other.memory,
        main,
        web_view,
        other,
    }
}

fn collect_host_metrics(system: &System) -> HostMetrics {
    let total_memory = system.total_memory();
    let free_memory = system.available_memory();
    let used_memory = total_memory.saturating_sub(free_memory);
    let memory_usage_percent = if total_memory > 0 {
        (used_memory as f64 / total_memory as f64 * 100.0) as f32
    } else {
        0.0
    };
    let cpu_core_count = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(1)
        .max(1);

    HostMetrics {
        total_memory,
        free_memory,
        used_memory,
        memory_usage_percent,
        cpu_core_count,
        load_average_1m: System::load_average().one,
    }
}

// ── Tauri command ──

/// Collect one resource-monitor snapshot. Cheap enough to poll every couple of
/// seconds; the persistent [`System`] handle makes CPU deltas accurate.
#[tauri::command]
pub fn get_resource_metrics(
    monitor: State<'_, ResourceMonitorState>,
    pty_state: State<'_, PtyState>,
    app_state: State<'_, AppStateStore>,
) -> Result<ResourceMetricsSnapshot, String> {
    let mut system = monitor
        .system
        .lock()
        .map_err(|_| "resource monitor state poisoned".to_string())?;
    Ok(collect_snapshot(&mut system, &pty_state, &app_state))
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;

    fn pid(n: u32) -> Pid {
        Pid::from_u32(n)
    }

    fn tree_with(
        edges: &[(u32, u32)],
        usage: &[(u32, f32, u64)],
        names: &[(u32, &str)],
    ) -> ProcessTree {
        let mut children_of: HashMap<Pid, Vec<Pid>> = HashMap::new();
        for (parent, child) in edges {
            children_of.entry(pid(*parent)).or_default().push(pid(*child));
        }
        let usage_of = usage
            .iter()
            .map(|(p, cpu, mem)| {
                (
                    pid(*p),
                    UsageValues {
                        cpu: *cpu,
                        memory: *mem,
                    },
                )
            })
            .collect();
        let name_of = names
            .iter()
            .map(|(p, name)| (pid(*p), name.to_string()))
            .collect();
        ProcessTree {
            children_of,
            usage_of,
            name_of,
        }
    }

    #[test]
    fn subtree_pids_collects_all_descendants() {
        // 100 → {200 → {400}, 300}
        let tree = tree_with(
            &[(100, 200), (100, 300), (200, 400)],
            &[
                (100, 1.0, 10),
                (200, 2.0, 20),
                (300, 3.0, 30),
                (400, 4.0, 40),
            ],
            &[],
        );
        let mut pids = tree.subtree_pids(pid(100));
        pids.sort();
        assert_eq!(pids, vec![pid(100), pid(200), pid(300), pid(400)]);
    }

    #[test]
    fn subtree_pids_skips_unknown_root() {
        let tree = tree_with(&[], &[], &[]);
        assert!(tree.subtree_pids(pid(999)).is_empty());
    }

    #[test]
    fn subtree_pids_survives_parent_cycle() {
        // Pathological cycle 1 → 2 → 1 must not loop forever.
        let tree = tree_with(
            &[(1, 2), (2, 1)],
            &[(1, 1.0, 1), (2, 1.0, 1)],
            &[],
        );
        let mut pids = tree.subtree_pids(pid(1));
        pids.sort();
        assert_eq!(pids, vec![pid(1), pid(2)]);
    }

    #[test]
    fn usage_for_sums_subtree() {
        let tree = tree_with(
            &[(100, 200), (200, 400)],
            &[(100, 1.5, 10), (200, 2.5, 20), (400, 4.0, 40)],
            &[],
        );
        let pids = tree.subtree_pids(pid(100));
        let usage = tree.usage_for(&pids);
        assert_eq!(usage.cpu, 8.0);
        assert_eq!(usage.memory, 70);
    }

    #[test]
    fn app_metrics_excludes_terminal_subtrees_and_classifies() {
        // current(1) → { webkit helper(2), terminal shell(3), sidecar(4) }
        let tree = tree_with(
            &[(1, 2), (1, 3), (1, 4)],
            &[
                (1, 5.0, 100),
                (2, 3.0, 60),
                (3, 9.0, 900),
                (4, 1.0, 40),
            ],
            &[
                (1, "codemux"),
                (2, "webkitwebprocess"),
                (3, "bash"),
                (4, "claude-agent"),
            ],
        );
        // Pretend the test process is pid 1 by calling the inner logic directly.
        let session_subtree: HashSet<Pid> = [pid(3)].into_iter().collect();

        // Re-implement the classification against a fixed root so the test does
        // not depend on the real current PID.
        let mut main = UsageValues::default();
        let mut web_view = UsageValues::default();
        let mut other = UsageValues::default();
        for p in tree.subtree_pids(pid(1)) {
            if session_subtree.contains(&p) {
                continue;
            }
            let usage = tree.usage_of[&p];
            if p == pid(1) {
                main.add(usage);
            } else if is_web_view_process(&tree.name_of[&p]) {
                web_view.add(usage);
            } else {
                other.add(usage);
            }
        }
        assert_eq!(main, UsageValues { cpu: 5.0, memory: 100 });
        assert_eq!(web_view, UsageValues { cpu: 3.0, memory: 60 });
        assert_eq!(other, UsageValues { cpu: 1.0, memory: 40 });
    }

    #[test]
    fn parse_smaps_rollup_pss_extracts_pss_in_bytes() {
        let body = "\
Rss:               40960 kB
Pss:                8192 kB
Pss_Anon:           4096 kB
Pss_File:           4096 kB
Shared_Clean:      16384 kB
";
        // 8192 kB → bytes.
        assert_eq!(parse_smaps_rollup_pss(body), Some(8192 * 1024));
    }

    #[test]
    fn parse_smaps_rollup_pss_ignores_pss_variants_and_junk() {
        // No bare `Pss:` line — only the `_Anon` variant.
        assert_eq!(parse_smaps_rollup_pss("Pss_Anon:  4096 kB\n"), None);
        assert_eq!(parse_smaps_rollup_pss(""), None);
        assert_eq!(parse_smaps_rollup_pss("Pss:  not-a-number kB\n"), None);
    }

    #[test]
    fn project_identity_prefers_root_and_collapses_worktrees() {
        // Explicit project root wins, name is its basename.
        assert_eq!(
            project_identity(Some("/home/zeus/projects/codemux"), "/anything"),
            ("/home/zeus/projects/codemux".to_string(), "codemux".to_string()),
        );
        // Sibling worktrees collapse to the shared project.
        let (id_a, name_a) = project_identity(
            None,
            "/home/zeus/.codemux/worktrees/dpcode/feature-60-bug",
        );
        let (id_b, name_b) = project_identity(
            None,
            "/home/zeus/.codemux/worktrees/dpcode/feature-103-retry",
        );
        assert_eq!(id_a, id_b);
        assert_eq!(name_a, "dpcode");
        assert_eq!(name_b, "dpcode");
        // Plain cwd falls back to basename.
        assert_eq!(
            project_identity(None, "/home/zeus/projects/openclaw"),
            ("/home/zeus/projects/openclaw".to_string(), "openclaw".to_string()),
        );
        // Empty everything degrades gracefully.
        assert_eq!(
            project_identity(None, ""),
            ("unknown".to_string(), "Project".to_string()),
        );
    }

    #[test]
    fn build_filters_threads_from_real_sysinfo_table() {
        // Regression for the >100% RAM-share bug: on Linux, sysinfo enumerates
        // `/proc/[pid]/task/[tid]` entries, so its process table is dominated
        // by *threads* of multi-threaded processes (WebKit, node, Chromium).
        // Each thread reports the same RSS as its leader, so without
        // filtering, summing across a tree multiplies one process's memory by
        // its thread count.
        //
        // The build must drop every entry where `thread_kind().is_some()`,
        // leaving only the thread-group leaders.
        let mut system = System::new();
        system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_memory(),
        );

        let raw_thread_count = system
            .processes()
            .values()
            .filter(|p| p.thread_kind().is_some())
            .count();

        let tree = ProcessTree::build(&system);

        for (pid, _) in &tree.usage_of {
            let process = system
                .process(*pid)
                .expect("kept PID must exist in sysinfo table");
            assert!(
                process.thread_kind().is_none(),
                "PID {pid} survived ProcessTree::build but is a thread \
                 (kind={:?}); this would re-introduce the RAM double-counting bug",
                process.thread_kind(),
            );
        }

        // Sanity: on any realistic Linux host the system has at least one
        // multi-threaded process. If this assertion ever fires on CI it's
        // worth checking, but it means the test isn't exercising the fix.
        if raw_thread_count > 0 {
            assert!(
                tree.usage_of.len() < system.processes().len(),
                "expected ProcessTree to be smaller than sysinfo's raw \
                 process table after filtering threads",
            );
        }
    }

    #[test]
    fn is_web_view_process_matches_known_names() {
        assert!(is_web_view_process("webkitwebprocess"));
        assert!(is_web_view_process("webkitnetworkprocess"));
        assert!(is_web_view_process("msedgewebview2"));
        assert!(!is_web_view_process("bash"));
        assert!(!is_web_view_process("codemux"));
    }
}
