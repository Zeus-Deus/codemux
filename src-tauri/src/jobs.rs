//! Scheduling helpers for the periodic background loops in `lib.rs`.
//!
//! The loops themselves stay in `lib.rs` because they need the `AppHandle`.
//! What lives here is the decision-making worth testing on its own: which
//! workspaces a given tick is allowed to touch, which of those actually need
//! git subprocesses run for them, and how long a loop waits before its first
//! tick so the timers don't phase-align.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use rand::Rng;

/// Non-active workspaces are refreshed on one tick in `GIT_SWEEP_STRIDE`.
/// At the 5 s tick that means every ~30 s for a background row, which is
/// well inside the window where a sidebar branch label reads as live, while
/// the per-tick subprocess count drops to roughly `N / 6 + 1`.
pub const GIT_SWEEP_STRIDE: u64 = 6;

/// How many planned visits an inactive checkout may be skipped on its
/// `.git` fingerprint alone before a full gather is forced regardless.
///
/// The fingerprint sees every change that goes through `.git` (commit,
/// checkout, `git add`, fetch) but NOT an edit to a tracked file, which
/// only moves the `changed_files` / diff-stat badge. A visit happens once
/// per stride cycle (`GIT_SWEEP_STRIDE` × 5 s = 30 s), so 2 visits means a
/// background row's dirty-file count can lag a plain editor save by at
/// most ~60 s. The active workspace is never gated, so the row the user is
/// looking at stays exact every tick.
pub const GIT_SWEEP_FULL_GATHER_MIN_VISITS: u32 = 2;

/// Upper bound for the forced-gather interval once a checkout has proven
/// quiet. Every forced gather that finds nothing changed doubles the
/// interval (2 → 4 → 8 visits, i.e. 60 s → 120 s → 240 s); any observed
/// change — in the fingerprint or in the gathered values — snaps it back
/// to `GIT_SWEEP_FULL_GATHER_MIN_VISITS`. An idle 200-checkout fleet thus
/// converges to a handful of forks per tick instead of ~200, while a repo
/// an agent is editing in the background stays on the 60 s cadence.
pub const GIT_SWEEP_FULL_GATHER_MAX_VISITS: u32 = 8;

/// Cap on the `refs/remotes` directory walk inside `repo_fingerprint`.
/// Keeps the stat budget of one fingerprint bounded on repos with
/// thousands of remote branches; anything past the cap is covered by the
/// periodic forced gather.
const FINGERPRINT_REF_DIR_BUDGET: usize = 256;

/// A generation-aware singleflight gate for periodic background jobs.
///
/// A request that arrives while work is already running is coalesced into that
/// run: it does not invalidate the only result we have without also scheduling
/// a successor. The next periodic tick after completion can then start a fresh
/// generation. `complete` executes the publish closure while holding the gate,
/// so a new request cannot slip between the ownership check and the state
/// write.
///
/// This is deliberately synchronous: callers hold it for a few instructions
/// around scheduling or committing, never while doing filesystem or subprocess
/// work.
#[derive(Default)]
pub struct SingleflightJob {
    state: Mutex<SingleflightState>,
}

#[derive(Default)]
struct SingleflightState {
    generation: u64,
    running: bool,
    overlap_count: u64,
    stale_result_count: u64,
}

/// Bounded scalar telemetry for a singleflight gate. These counters contain no
/// workspace/job identifiers and saturate instead of wrapping.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SingleflightStats {
    pub overlap_count: u64,
    pub stale_result_count: u64,
}

impl SingleflightJob {
    /// Register a requested run. Returns its generation only when this caller
    /// owns the single in-flight slot. Overlapping requests are coalesced and
    /// leave the current generation publishable; otherwise a job whose runtime
    /// exceeds its polling period could have every result invalidated forever.
    pub fn request(&self) -> Option<u64> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.running {
            state.overlap_count = state.overlap_count.saturating_add(1);
            return None;
        }
        state.generation = state.generation.wrapping_add(1).max(1);
        state.running = true;
        Some(state.generation)
    }

    /// Release the in-flight slot and publish only if `generation` still owns
    /// it. Returns whether `publish` ran. A duplicate/late completion cannot
    /// accidentally release a newer generation.
    pub fn complete(&self, generation: u64, publish: impl FnOnce()) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.running || state.generation != generation {
            state.stale_result_count = state.stale_result_count.saturating_add(1);
            return false;
        }
        state.running = false;
        publish();
        true
    }

    pub fn stats(&self) -> SingleflightStats {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        SingleflightStats {
            overlap_count: state.overlap_count,
            stale_result_count: state.stale_result_count,
        }
    }

    #[cfg(test)]
    fn is_running(&self) -> bool {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).running
    }
}

pub fn should_poll_ports(window_active: bool, remote_viewers: bool) -> bool {
    window_active || remote_viewers
}

/// One workspace's slot in a git-sweep tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitSweepStep {
    pub workspace_id: String,
    pub cwd: String,
    /// False when an earlier step in the same tick already gathered this
    /// exact checkout — the caller reuses that result instead of forking
    /// the same git subprocesses a second time. Two workspaces pointed at
    /// one directory produce identical branch / ahead-behind / diff-stat
    /// output by construction.
    pub gather: bool,
}

/// Decide which workspaces a git-sweep tick refreshes.
///
/// The active workspace is refreshed on every tick — it is the row the user
/// is looking at and the one an agent is most likely changing under them.
/// Every other workspace is assigned to a stride bucket by list position, so
/// the background fleet is spread evenly across `stride` ticks instead of
/// landing on one expensive tick together.
///
/// `workspaces` must be in a stable order (sort by workspace id) or a
/// workspace can starve by shifting buckets every tick.
pub fn plan_git_sweep(
    tick: u64,
    workspaces: &[(String, String)],
    active_id: Option<&str>,
    stride: u64,
) -> Vec<GitSweepStep> {
    let stride = stride.max(1);
    let mut seen: HashSet<&str> = HashSet::new();
    let mut steps = Vec::new();
    for (index, (workspace_id, cwd)) in workspaces.iter().enumerate() {
        let is_active = active_id == Some(workspace_id.as_str());
        if !is_active && (index as u64) % stride != tick % stride {
            continue;
        }
        steps.push(GitSweepStep {
            workspace_id: workspace_id.clone(),
            cwd: cwd.clone(),
            gather: seen.insert(cwd.as_str()),
        });
    }
    steps
}

// ---------------------------------------------------------------------------
// Change gate for the git sweep
// ---------------------------------------------------------------------------
//
// `gather_workspace_git_info` forks ~6 git subprocesses per checkout. On a
// 200-checkout profile that was ~200 forks every 5 s tick (800-1000 ms of
// blocking-pool time) while the user did nothing. Everything below exists
// so an unchanged checkout costs a handful of `stat` calls instead.

/// `(mtime, size)` of one path, or `None` when it does not exist. A path
/// appearing or vanishing is itself a change worth noticing (`git init`,
/// `packed-refs` being written for the first time, ...).
type Stamp = Option<(SystemTime, u64)>;

fn stamp(path: &Path) -> Stamp {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?;
    Some((mtime, meta.len()))
}

/// The directories that make up one checkout, located without forking git.
///
/// Mirrors what `git rev-parse --git-dir` / `--git-common-dir` would say:
/// for a main checkout all three are trivially related; for a linked
/// worktree `.git` is a FILE holding `gitdir: <main>/.git/worktrees/<name>`
/// and that directory's `commondir` file points back at the shared
/// `<main>/.git`, which is where `refs/` and `packed-refs` live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitDirs {
    /// Top of the working tree (the directory holding `.git`).
    pub worktree: PathBuf,
    /// Per-checkout metadata: `HEAD`, `index`, `logs/HEAD`.
    pub git_dir: PathBuf,
    /// Shared metadata: `refs/`, `packed-refs`, objects.
    pub common_dir: PathBuf,
}

fn join_relative(base: &Path, raw: &str) -> PathBuf {
    let path = PathBuf::from(raw.trim());
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

/// Walk up from `cwd` to the nearest `.git` and resolve its directories.
/// Returns `None` for a directory that is not inside any repository.
pub fn locate_git_dirs(cwd: &Path) -> Option<GitDirs> {
    let mut current = cwd.to_path_buf();
    loop {
        let dot_git = current.join(".git");
        if let Ok(meta) = std::fs::metadata(&dot_git) {
            if meta.is_dir() {
                return Some(GitDirs {
                    worktree: current,
                    git_dir: dot_git.clone(),
                    common_dir: dot_git,
                });
            }
            if meta.is_file() {
                // Worktree or submodule pointer file. An unreadable or
                // malformed one is still "a repo root as far as we can
                // tell": the gather will report what git says, and the
                // fingerprint falls back to the pointer file itself.
                let git_dir = std::fs::read_to_string(&dot_git)
                    .ok()
                    .and_then(|content| {
                        content
                            .lines()
                            .find_map(|line| line.strip_prefix("gitdir:"))
                            .map(|rest| join_relative(&current, rest))
                    })
                    .unwrap_or_else(|| dot_git.clone());
                let common_dir = std::fs::read_to_string(git_dir.join("commondir"))
                    .ok()
                    .map(|content| join_relative(&git_dir, &content))
                    .unwrap_or_else(|| git_dir.clone());
                return Some(GitDirs {
                    worktree: current,
                    git_dir,
                    common_dir,
                });
            }
        }
        if !current.pop() {
            return None;
        }
    }
}

/// Cheap filesystem identity of a checkout's git state.
///
/// Built purely from `stat` and one small file read; no subprocess. Two
/// fingerprints compare equal exactly when none of the observed metadata
/// moved. What it covers, and what each entry catches:
///
/// - `<git_dir>/HEAD` stamp + content — branch switch, detached HEAD.
/// - `<git_dir>/index` — `git add`/`reset`/checkout and the stat-cache
///   refresh git itself does after a tracked file changed.
/// - `<git_dir>/logs/HEAD` — every commit, reset, checkout (the reflog
///   only grows, so size alone is a reliable signal on coarse clocks).
/// - `<common_dir>/refs/heads/<branch>` + its reflog — the current
///   branch's tip moving, including from ANOTHER worktree of the repo.
/// - `<common_dir>/refs`, `refs/heads`, `refs/remotes/**` dirs and
///   `packed-refs` — fetches, `git gc`, branch creation/deletion, i.e. the
///   upstream side of the ahead/behind count.
/// - The worktree root directory and its `.git` entry — a new/removed
///   top-level file, `git init` in a folder that was not a repo.
///
/// What it does NOT cover, on purpose: edits to tracked files below the
/// root. Those only touch the file itself, and stat-walking a whole tree
/// every 5 s is the cost this gate exists to avoid. `SweepGate` covers that
/// gap with a periodic forced gather.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RepoFingerprint {
    stamps: Vec<(PathBuf, Stamp)>,
    head: Option<String>,
    /// Indices into `stamps` that the gather itself may rewrite
    /// (`<git_dir>/index` and the `<git_dir>` directory whose mtime moves
    /// when `index.lock` is renamed over it). Full equality still counts
    /// them — a `git add` from outside must be seen — but `gate_gather`
    /// ignores them when deciding whether the gather absorbed a change.
    gather_volatile: Vec<usize>,
}

impl RepoFingerprint {
    fn push(&mut self, path: PathBuf) {
        let stamp = stamp(&path);
        self.stamps.push((path, stamp));
    }

    fn push_gather_volatile(&mut self, path: PathBuf) {
        self.gather_volatile.push(self.stamps.len());
        self.push(path);
    }

    /// Number of filesystem entries observed. Exposed for tests and
    /// diagnostics only.
    pub fn len(&self) -> usize {
        self.stamps.len()
    }

    /// Paths whose stamp differs between the two fingerprints. Test-only
    /// diagnostic so a failing assertion can say WHAT moved.
    #[cfg(test)]
    fn diff_paths(&self, other: &Self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = self
            .stamps
            .iter()
            .filter(|(path, stamp)| {
                other
                    .stamps
                    .iter()
                    .find(|(p, _)| p == path)
                    .map(|(_, s)| s != stamp)
                    .unwrap_or(true)
            })
            .map(|(path, _)| path.clone())
            .collect();
        if self.head != other.head {
            out.push(PathBuf::from("<HEAD content>"));
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.stamps.is_empty()
    }

    /// True when anything the gather does not itself write differs.
    /// `git status` refreshes the index stat-cache and may rewrite
    /// `<git_dir>/index` (moving the `<git_dir>` mtime with it); those
    /// self-inflicted writes must not read as a change.
    fn differs_beyond_gather_writes(&self, other: &Self) -> bool {
        if self.head != other.head
            || self.stamps.len() != other.stamps.len()
            || self.gather_volatile != other.gather_volatile
        {
            return true;
        }
        self.stamps
            .iter()
            .zip(&other.stamps)
            .enumerate()
            .any(|(index, ((path_a, stamp_a), (path_b, stamp_b)))| {
                path_a != path_b
                    || (stamp_a != stamp_b && !self.gather_volatile.contains(&index))
            })
    }
}

/// Push every directory below `root` (and `root` itself), bounded by
/// `budget`, in a deterministic order so two walks of an unchanged tree
/// yield identical fingerprints.
fn push_dir_tree(fp: &mut RepoFingerprint, root: PathBuf, budget: &mut usize) {
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        if *budget == 0 {
            return;
        }
        *budget -= 1;
        fp.push(dir.clone());
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut children: Vec<PathBuf> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
            .map(|e| e.path())
            .collect();
        // Reverse-sort so the stack pops in ascending order.
        children.sort_by(|a, b| b.cmp(a));
        stack.extend(children);
    }
}

pub fn repo_fingerprint(cwd: &Path) -> RepoFingerprint {
    let mut fp = RepoFingerprint::default();
    let Some(dirs) = locate_git_dirs(cwd) else {
        // Not a repo: watch for one appearing (`git init`, or the folder
        // being replaced by a clone). The root dir mtime moves when
        // `.git` is created or any top-level entry changes.
        fp.push(cwd.to_path_buf());
        fp.push(cwd.join(".git"));
        return fp;
    };

    fp.push(dirs.worktree.clone());
    // For a main checkout `.git` IS the git dir, whose mtime moves when
    // git renames `index.lock` into place; for a linked worktree it is a
    // small pointer file nothing rewrites. Either way the gather may
    // touch the git dir itself, so both are marked volatile.
    fp.push_gather_volatile(dirs.worktree.join(".git"));
    if dirs.git_dir != dirs.worktree.join(".git") {
        fp.push_gather_volatile(dirs.git_dir.clone());
    }
    fp.push(dirs.git_dir.join("HEAD"));
    fp.push_gather_volatile(dirs.git_dir.join("index"));
    fp.push(dirs.git_dir.join("logs/HEAD"));
    let head = std::fs::read_to_string(dirs.git_dir.join("HEAD"))
        .ok()
        .map(|content| content.trim().to_string());
    let symbolic_ref = head
        .as_deref()
        .and_then(|head| head.strip_prefix("ref:"))
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map(str::to_string);
    fp.head = head;
    if let Some(reference) = symbolic_ref {
        // The branch tip lives in the COMMON dir: another worktree
        // committing to the same branch, or `git branch -f`, updates it
        // without touching this checkout's own `.git`.
        fp.push(dirs.common_dir.join(&reference));
        fp.push(dirs.common_dir.join("logs").join(&reference));
    }
    fp.push(dirs.common_dir.join("packed-refs"));
    fp.push(dirs.common_dir.join("refs"));
    fp.push(dirs.common_dir.join("refs").join("heads"));
    let mut budget = FINGERPRINT_REF_DIR_BUDGET;
    push_dir_tree(
        &mut fp,
        dirs.common_dir.join("refs").join("remotes"),
        &mut budget,
    );
    fp
}

/// Why a gated checkout was gathered this tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatherReason {
    /// The active workspace is never gated.
    Active,
    /// No fingerprint on record yet.
    FirstSeen,
    /// `.git` metadata moved since the last gather.
    Changed,
    /// Forced by the periodic cadence, to catch tracked-file edits.
    Periodic,
}

/// Everything `gate_gather` needs to decide, captured on the async side
/// so the decision and the gather run in ONE `spawn_blocking` hop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatePlan {
    previous: Option<RepoFingerprint>,
    periodic_due: bool,
    active: bool,
}

impl GatePlan {
    /// The reason to gather given the fingerprint just taken, or `None`
    /// to skip.
    pub fn reason_for(&self, current: &RepoFingerprint) -> Option<GatherReason> {
        if self.active {
            return Some(GatherReason::Active);
        }
        let Some(previous) = &self.previous else {
            return Some(GatherReason::FirstSeen);
        };
        if previous != current {
            return Some(GatherReason::Changed);
        }
        if self.periodic_due {
            return Some(GatherReason::Periodic);
        }
        None
    }
}

/// Result of one gated visit. `gathered` is `None` when the visit was
/// skipped on the fingerprint.
#[derive(Debug)]
pub struct GateOutcome<T> {
    /// The fingerprint to remember for the next visit. See `gate_gather`
    /// for how it is chosen when the gather itself moved `.git`.
    pub fingerprint: RepoFingerprint,
    pub gathered: Option<(GatherReason, T)>,
}

/// Blocking-pool half of the gate: stamp the checkout, and only when the
/// plan says so run `gather`. `T` is opaque — this module does not need
/// to know what a git snapshot looks like.
///
/// The fingerprint is taken again AFTER a gather because `git status`
/// refreshes the index stat-cache and may rewrite `<git_dir>/index` as a
/// side effect (moving the `<git_dir>` mtime with it). Remembering the
/// post-gather stamp keeps that self-inflicted write from reading as a
/// change on the next visit. Anything else that moved during the gather
/// (a commit landing between two of the six git calls) is NOT absorbed:
/// the pre-gather stamp is kept, so the next visit sees a diff and
/// gathers again. An outside `git add` racing the gather is the one case
/// that can be absorbed; the periodic forced gather covers it.
pub fn gate_gather<T>(
    plan: &GatePlan,
    cwd: &Path,
    gather: impl FnOnce(&Path) -> T,
) -> GateOutcome<T> {
    let before = repo_fingerprint(cwd);
    let Some(reason) = plan.reason_for(&before) else {
        return GateOutcome {
            fingerprint: before,
            gathered: None,
        };
    };
    let info = gather(cwd);
    let after = repo_fingerprint(cwd);
    let fingerprint = if before.differs_beyond_gather_writes(&after) {
        before
    } else {
        after
    };
    GateOutcome {
        fingerprint,
        gathered: Some((reason, info)),
    }
}

#[derive(Debug, Clone)]
struct GateEntry {
    fingerprint: RepoFingerprint,
    /// Skipped visits remaining before a forced gather.
    visits_until_full: u32,
    /// Current forced-gather interval, in visits; backs off while quiet.
    full_every: u32,
}

/// Per-checkout memory of the git sweep. Owned by the sweep task in
/// `lib.rs`; keyed by cwd string exactly as `GitSweepStep::cwd`.
#[derive(Debug, Default)]
pub struct SweepGate {
    entries: HashMap<String, GateEntry>,
}

impl SweepGate {
    /// Snapshot what `gate_gather` needs for `cwd`. Pure read — the
    /// countdown only moves in `record`, so a plan that is never recorded
    /// (gather task panicked) leaves the gate untouched and the next visit
    /// simply retries.
    pub fn plan(&self, cwd: &str, active: bool) -> GatePlan {
        let entry = self.entries.get(cwd);
        GatePlan {
            previous: entry.map(|e| e.fingerprint.clone()),
            periodic_due: entry.map(|e| e.visits_until_full == 0).unwrap_or(true),
            active,
        }
    }

    /// Fold one visit back in. `info_changed` is whether the gathered
    /// values differed from what the sidebar already showed; it is the
    /// signal that a tracked-file edit is happening without `.git`
    /// noticing, and it keeps such a repo on the short cadence.
    pub fn record<T>(&mut self, cwd: &str, outcome: &GateOutcome<T>, info_changed: bool) {
        let Some((reason, _)) = &outcome.gathered else {
            if let Some(entry) = self.entries.get_mut(cwd) {
                entry.visits_until_full = entry.visits_until_full.saturating_sub(1);
            }
            return;
        };
        let min = GIT_SWEEP_FULL_GATHER_MIN_VISITS;
        let max = GIT_SWEEP_FULL_GATHER_MAX_VISITS.max(min);
        match self.entries.get_mut(cwd) {
            Some(entry) => {
                entry.fingerprint = outcome.fingerprint.clone();
                entry.full_every = match reason {
                    GatherReason::Periodic if !info_changed => (entry.full_every * 2).min(max),
                    _ => min,
                };
                entry.visits_until_full = entry.full_every;
            }
            None => {
                // Stagger first-seen checkouts across the cadence so a
                // fleet opened at once does not force-gather in lockstep.
                let stagger = (self.entries.len() as u32) % min.max(1);
                self.entries.insert(
                    cwd.to_string(),
                    GateEntry {
                        fingerprint: outcome.fingerprint.clone(),
                        visits_until_full: min.saturating_sub(stagger),
                        full_every: min,
                    },
                );
            }
        }
    }

    /// Drop memory for checkouts no workspace points at any more.
    pub fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        self.entries.retain(|cwd, _| keep(cwd));
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Collapse `(cwd, project_root)` pairs into one fetch per repository.
///
/// Linked worktrees share the object store and the remote refs of their
/// project root, so one `git fetch --prune` updates every workspace opened
/// on that repo. Workspaces whose project root wasn't resolved fall back to
/// their own cwd, which still drops exact duplicates.
pub fn dedupe_fetch_targets(entries: &[(String, Option<String>)]) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut targets = Vec::new();
    for (cwd, project_root) in entries {
        if cwd.is_empty() {
            continue;
        }
        let key = project_root.as_deref().unwrap_or(cwd.as_str());
        let key = key.trim_end_matches('/');
        if seen.insert(key.to_string()) {
            targets.push(cwd.clone());
        }
    }
    targets
}

/// A random start-up delay in `0..=max`.
///
/// Every loop here sleeps a whole period before its first tick, so without
/// this they all fire on the same instant at every common multiple of their
/// periods (the 3 s port scan, the 5 s git sweep and the 60 s PR poll meet
/// every minute). Offsetting the starts spreads that collision out for the
/// life of the process.
pub fn startup_jitter(max: Duration) -> Duration {
    let max_ms = max.as_millis() as u64;
    if max_ms == 0 {
        return Duration::ZERO;
    }
    Duration::from_millis(rand::thread_rng().gen_range(0..=max_ms))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(id, cwd)| ((*id).to_string(), (*cwd).to_string()))
            .collect()
    }

    fn ids(steps: &[GitSweepStep]) -> Vec<String> {
        steps.iter().map(|s| s.workspace_id.clone()).collect()
    }

    #[test]
    fn active_workspace_refreshes_on_every_tick() {
        let workspaces = ws(&[("a", "/a"), ("b", "/b"), ("c", "/c")]);
        for tick in 0..12 {
            let steps = plan_git_sweep(tick, &workspaces, Some("c"), GIT_SWEEP_STRIDE);
            assert!(
                ids(&steps).contains(&"c".to_string()),
                "active workspace missing from tick {tick}"
            );
        }
    }

    #[test]
    fn background_workspaces_spread_across_the_stride() {
        let workspaces = ws(&[
            ("a", "/a"),
            ("b", "/b"),
            ("c", "/c"),
            ("d", "/d"),
            ("e", "/e"),
            ("f", "/f"),
        ]);
        // One workspace per tick, and every workspace covered exactly once
        // over a full stride.
        let mut covered: Vec<String> = Vec::new();
        for tick in 0..GIT_SWEEP_STRIDE {
            let steps = plan_git_sweep(tick, &workspaces, None, GIT_SWEEP_STRIDE);
            assert_eq!(
                steps.len(),
                1,
                "tick {tick} refreshed {} workspaces",
                steps.len()
            );
            covered.extend(ids(&steps));
        }
        covered.sort();
        assert_eq!(covered, vec!["a", "b", "c", "d", "e", "f"]);
    }

    #[test]
    fn active_workspace_is_not_planned_twice_on_its_own_stride_tick() {
        let workspaces = ws(&[("a", "/a"), ("b", "/b")]);
        let steps = plan_git_sweep(0, &workspaces, Some("a"), 2);
        assert_eq!(ids(&steps), vec!["a"]);
    }

    #[test]
    fn identical_cwds_gather_once_per_tick() {
        let workspaces = ws(&[("a", "/repo"), ("b", "/repo"), ("c", "/other")]);
        let steps = plan_git_sweep(0, &workspaces, None, 1);
        assert_eq!(ids(&steps), vec!["a", "b", "c"]);
        assert_eq!(
            steps.iter().map(|s| s.gather).collect::<Vec<_>>(),
            vec![true, false, true],
            "the second workspace on /repo must reuse the first gather"
        );
    }

    #[test]
    fn stride_zero_is_treated_as_every_tick() {
        let workspaces = ws(&[("a", "/a"), ("b", "/b")]);
        assert_eq!(
            ids(&plan_git_sweep(3, &workspaces, None, 0)),
            vec!["a", "b"]
        );
    }

    #[test]
    fn fetch_targets_dedupe_by_project_root() {
        let entries = vec![
            ("/repo".to_string(), Some("/repo".to_string())),
            (
                "/repo/.worktrees/feature".to_string(),
                Some("/repo".to_string()),
            ),
            ("/other".to_string(), Some("/other/".to_string())),
            ("/other".to_string(), None),
            ("/loose".to_string(), None),
        ];
        assert_eq!(
            dedupe_fetch_targets(&entries),
            vec![
                "/repo".to_string(),
                "/other".to_string(),
                "/loose".to_string()
            ],
        );
    }

    #[test]
    fn fetch_targets_skip_empty_cwds() {
        let entries = vec![(String::new(), None), ("/repo".to_string(), None)];
        assert_eq!(dedupe_fetch_targets(&entries), vec!["/repo".to_string()]);
    }

    #[test]
    fn startup_jitter_stays_within_bounds() {
        assert_eq!(startup_jitter(Duration::ZERO), Duration::ZERO);
        for _ in 0..64 {
            let delay = startup_jitter(Duration::from_secs(2));
            assert!(
                delay <= Duration::from_secs(2),
                "jitter overshot: {delay:?}"
            );
        }
    }

    #[test]
    fn singleflight_coalesces_overlap_without_starving_the_current_result() {
        let job = SingleflightJob::default();
        let first = job.request().expect("first run owns the slot");
        // Model one job spanning several polling periods. None of these ticks
        // starts a second worker or invalidates the only in-flight result.
        for _ in 0..4 {
            assert!(job.request().is_none(), "overlap must not start");
        }
        let mut published = false;
        assert!(job.complete(first, || published = true));
        assert!(published, "a long-running generation must still publish");
        assert!(!job.is_running());
        assert!(
            job.request().is_some(),
            "the first tick after completion must start the successor"
        );
        assert_eq!(
            job.stats(),
            SingleflightStats {
                overlap_count: 4,
                stale_result_count: 0,
            }
        );
    }

    #[test]
    fn singleflight_current_result_publishes_and_releases_for_retry() {
        let job = SingleflightJob::default();
        let first = job.request().expect("first run");
        let mut published = false;
        assert!(job.complete(first, || published = true));
        assert!(published);
        assert!(job.request().is_some(), "completion must release the slot");
    }

    #[test]
    fn stale_completion_is_counted_and_cannot_release_the_successor() {
        let job = SingleflightJob::default();
        let first = job.request().expect("first run");
        assert!(job.complete(first, || {}));
        let second = job.request().expect("successor run");

        assert!(!job.complete(first, || panic!("stale result published")));
        assert!(job.is_running(), "stale completion released the successor");
        assert_eq!(job.stats().stale_result_count, 1);
        assert!(job.complete(second, || {}));
    }

    #[test]
    fn hidden_port_poll_skips_without_remote_but_runs_for_remote_viewer() {
        assert!(!should_poll_ports(false, false));
        assert!(should_poll_ports(false, true));
        assert!(should_poll_ports(true, false));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn blocking_workspace_refresh_work_does_not_starve_the_runtime() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            let _ = started_tx.send(());
            release_rx.recv().unwrap();
        });
        started_rx.await.unwrap();

        // This runtime has only one async worker. It can still schedule new
        // work while the refresh is held in the blocking pool. The previous
        // wall-clock p99 assertion tested the same boundary but failed under
        // harmless CI scheduler jitter.
        let progressed = Arc::new(AtomicBool::new(false));
        let progressed_in_task = progressed.clone();
        let async_task = tokio::spawn(async move {
            progressed_in_task.store(true, Ordering::SeqCst);
        });
        tokio::task::yield_now().await;
        assert!(progressed.load(Ordering::SeqCst));

        release_tx.send(()).unwrap();
        async_task.await.unwrap();
        blocker.await.unwrap();
    }
}

#[cfg(test)]
mod fingerprint_tests {
    //! The change gate must (a) notice every `.git`-level change the
    //! sidebar renders — branch, commit, staging, upstream — and (b) stay
    //! stable when nothing happened, or the gate saves nothing.

    use super::*;
    use std::process::Command;
    use tempfile::TempDir;

    fn run(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@test.invalid")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@test.invalid")
            .output()
            .expect("git spawn");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn init_repo(dir: &Path) {
        run(dir, &["init", "--initial-branch=main"]);
        std::fs::write(dir.join("README.md"), "hi\n").unwrap();
        run(dir, &["add", "."]);
        run(dir, &["commit", "-m", "init"]);
    }

    /// Filesystem mtimes are coarse on some platforms; two writes within
    /// the same tick would otherwise be indistinguishable by mtime alone.
    /// Sizes usually still differ, but sleep to make the tests robust.
    fn settle() {
        std::thread::sleep(Duration::from_millis(20));
    }

    fn same_path(a: &Path, b: &Path) -> bool {
        match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
            (Ok(x), Ok(y)) => x == y,
            _ => a == b,
        }
    }

    #[test]
    fn fingerprint_is_stable_when_nothing_changed() {
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        let a = repo_fingerprint(tmp.path());
        settle();
        let b = repo_fingerprint(tmp.path());
        assert_eq!(a, b);
        assert!(a.len() >= 8, "expected a handful of stamps, got {}", a.len());
    }

    #[test]
    fn fingerprint_changes_on_commit() {
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        let before = repo_fingerprint(tmp.path());
        settle();
        run(tmp.path(), &["commit", "--allow-empty", "-m", "two"]);
        assert_ne!(before, repo_fingerprint(tmp.path()));
    }

    #[test]
    fn fingerprint_changes_on_branch_switch() {
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        let before = repo_fingerprint(tmp.path());
        settle();
        run(tmp.path(), &["checkout", "-b", "feature"]);
        let after = repo_fingerprint(tmp.path());
        assert_ne!(before, after);
        assert_eq!(after.head.as_deref(), Some("ref: refs/heads/feature"));
    }

    #[test]
    fn fingerprint_changes_on_git_add() {
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        std::fs::write(tmp.path().join("new.txt"), "x").unwrap();
        // Untracked file below the root: the ROOT dir mtime moves, which
        // is a change too, so take the baseline after creating it.
        let before = repo_fingerprint(tmp.path());
        settle();
        run(tmp.path(), &["add", "new.txt"]);
        assert_ne!(before, repo_fingerprint(tmp.path()));
    }

    #[test]
    fn fingerprint_does_not_see_a_tracked_file_edit_in_a_subdirectory() {
        // Documents the known gap the periodic forced gather covers.
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        std::fs::create_dir(tmp.path().join("src")).unwrap();
        std::fs::write(tmp.path().join("src/lib.rs"), "a").unwrap();
        run(tmp.path(), &["add", "."]);
        run(tmp.path(), &["commit", "-m", "src"]);
        let before = repo_fingerprint(tmp.path());
        settle();
        std::fs::write(tmp.path().join("src/lib.rs"), "b").unwrap();
        assert_eq!(before, repo_fingerprint(tmp.path()));
    }

    #[test]
    fn fingerprint_changes_when_a_folder_becomes_a_repo() {
        let tmp = TempDir::new().unwrap();
        let before = repo_fingerprint(tmp.path());
        assert!(locate_git_dirs(tmp.path()).is_none());
        settle();
        run(tmp.path(), &["init"]);
        assert_ne!(before, repo_fingerprint(tmp.path()));
    }

    #[test]
    fn linked_worktree_resolves_gitdir_and_common_dir() {
        let tmp = TempDir::new().unwrap();
        let main = tmp.path().join("main");
        std::fs::create_dir(&main).unwrap();
        init_repo(&main);
        let wt = tmp.path().join("wt");
        run(&main, &["worktree", "add", "-b", "feature", wt.to_str().unwrap()]);

        let dirs = locate_git_dirs(&wt).expect("worktree is a repo");
        assert!(same_path(&dirs.worktree, &wt));
        assert!(same_path(
            &dirs.git_dir,
            &main.join(".git").join("worktrees").join("wt")
        ));
        assert!(same_path(&dirs.common_dir, &main.join(".git")));
        assert!(dirs.git_dir.join("HEAD").is_file());
        assert!(dirs.common_dir.join("refs").is_dir());

        // A subdirectory of the worktree resolves to the same place.
        std::fs::create_dir(wt.join("deep")).unwrap();
        assert_eq!(locate_git_dirs(&wt.join("deep")), Some(dirs));
    }

    #[test]
    fn worktree_fingerprint_sees_commits_made_in_that_worktree() {
        let tmp = TempDir::new().unwrap();
        let main = tmp.path().join("main");
        std::fs::create_dir(&main).unwrap();
        init_repo(&main);
        let wt = tmp.path().join("wt");
        run(&main, &["worktree", "add", "-b", "feature", wt.to_str().unwrap()]);

        let before = repo_fingerprint(&wt);
        settle();
        run(&wt, &["commit", "--allow-empty", "-m", "in worktree"]);
        assert_ne!(before, repo_fingerprint(&wt));
    }

    #[test]
    fn fingerprint_sees_the_current_branch_moving_from_another_worktree() {
        // Two checkouts of one repo: a ref update from the main checkout
        // moves `refs/heads/feature`, which lives in the common dir,
        // without touching the worktree's own gitdir. (`branch -f`
        // refuses on a checked-out branch; plumbing does not.)
        let tmp = TempDir::new().unwrap();
        let main = tmp.path().join("main");
        std::fs::create_dir(&main).unwrap();
        init_repo(&main);
        let wt = tmp.path().join("wt");
        run(&main, &["worktree", "add", "-b", "feature", wt.to_str().unwrap()]);
        run(&main, &["commit", "--allow-empty", "-m", "ahead"]);

        let before = repo_fingerprint(&wt);
        settle();
        run(&main, &["update-ref", "refs/heads/feature", "main"]);
        assert_ne!(before, repo_fingerprint(&wt));
    }

    #[test]
    fn fingerprint_sees_a_fetch_updating_remote_refs() {
        let tmp = TempDir::new().unwrap();
        let origin = tmp.path().join("origin");
        std::fs::create_dir(&origin).unwrap();
        init_repo(&origin);
        let clone = tmp.path().join("clone");
        run(
            tmp.path(),
            &["clone", "-q", origin.to_str().unwrap(), clone.to_str().unwrap()],
        );

        let before = repo_fingerprint(&clone);
        settle();
        run(&origin, &["commit", "--allow-empty", "-m", "upstream moved"]);
        run(&clone, &["fetch", "-q"]);
        assert_ne!(
            before,
            repo_fingerprint(&clone),
            "a fetch changes ahead/behind and must not be skipped"
        );
    }

    #[test]
    fn real_gather_does_not_perturb_its_own_fingerprint() {
        // `git status` refreshes the index stat-cache and may rewrite
        // `.git/index`. If that read as a change, every inactive checkout
        // would be gathered twice per visit and the gate would save
        // nothing. Exercise the REAL gather, not a stub.
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        // A dirty tracked file forces the stat-cache refresh path.
        std::fs::write(tmp.path().join("README.md"), "changed\n").unwrap();
        let cwd = tmp.path().to_str().unwrap().to_string();
        let mut gate = SweepGate::default();

        let plan = gate.plan(&cwd, false);
        let outcome = gate_gather(
            &plan,
            tmp.path(),
            crate::commands::workspace::gather_workspace_git_info,
        );
        let (reason, info) = outcome.gathered.as_ref().expect("first visit gathers");
        assert_eq!(*reason, GatherReason::FirstSeen);
        assert_eq!(info.changed_files, 1);
        gate.record(&cwd, &outcome, true);

        settle();
        let plan = gate.plan(&cwd, false);
        let before = repo_fingerprint(tmp.path());
        let outcome = gate_gather(
            &plan,
            tmp.path(),
            crate::commands::workspace::gather_workspace_git_info,
        );
        assert!(
            outcome.gathered.is_none(),
            "second visit must skip: {:?}; moved: {:?}",
            outcome.gathered.as_ref().map(|(r, _)| r),
            plan.previous.as_ref().map(|p| p.diff_paths(&before))
        );
    }

    #[test]
    fn gate_gather_skips_an_unchanged_inactive_checkout_until_periodic_is_due() {
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        let cwd = tmp.path().to_str().unwrap().to_string();
        let mut gate = SweepGate::default();
        let mut forks = 0u32;

        let visit = |gate: &mut SweepGate, forks: &mut u32, changed: bool| {
            let plan = gate.plan(&cwd, false);
            let outcome = gate_gather(&plan, tmp.path(), |_| {
                *forks += 1;
            });
            let reason = outcome.gathered.as_ref().map(|(r, _)| *r);
            gate.record(&cwd, &outcome, changed);
            reason
        };

        // First visit always gathers.
        assert_eq!(visit(&mut gate, &mut forks, true), Some(GatherReason::FirstSeen));
        assert_eq!(forks, 1);
        // The gather's own index refresh must not read as a change.
        settle();
        for _ in 0..GIT_SWEEP_FULL_GATHER_MIN_VISITS {
            assert_eq!(visit(&mut gate, &mut forks, false), None);
        }
        assert_eq!(forks, 1, "unchanged repo must cost zero gathers");
        // Countdown exhausted: forced gather.
        assert_eq!(visit(&mut gate, &mut forks, false), Some(GatherReason::Periodic));
        assert_eq!(forks, 2);
        // Quiet periodic gather doubled the interval.
        for _ in 0..(GIT_SWEEP_FULL_GATHER_MIN_VISITS * 2) {
            assert_eq!(visit(&mut gate, &mut forks, false), None);
        }
        assert_eq!(visit(&mut gate, &mut forks, false), Some(GatherReason::Periodic));
        assert_eq!(forks, 3);
    }

    #[test]
    fn gate_gather_runs_immediately_after_a_commit() {
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        let cwd = tmp.path().to_str().unwrap().to_string();
        let mut gate = SweepGate::default();

        let plan = gate.plan(&cwd, false);
        let outcome = gate_gather(&plan, tmp.path(), |_| ());
        gate.record(&cwd, &outcome, true);
        settle();
        let plan = gate.plan(&cwd, false);
        let outcome = gate_gather(&plan, tmp.path(), |_| ());
        assert!(outcome.gathered.is_none());
        gate.record(&cwd, &outcome, false);

        run(tmp.path(), &["commit", "--allow-empty", "-m", "two"]);
        let plan = gate.plan(&cwd, false);
        let outcome = gate_gather(&plan, tmp.path(), |_| ());
        assert_eq!(
            outcome.gathered.as_ref().map(|(r, _)| *r),
            Some(GatherReason::Changed)
        );
        gate.record(&cwd, &outcome, true);

        // ...and settles back to skipping.
        settle();
        let plan = gate.plan(&cwd, false);
        let outcome = gate_gather(&plan, tmp.path(), |_| ());
        assert!(outcome.gathered.is_none());
    }

    #[test]
    fn active_checkout_is_never_gated() {
        let tmp = TempDir::new().unwrap();
        init_repo(tmp.path());
        let cwd = tmp.path().to_str().unwrap().to_string();
        let mut gate = SweepGate::default();
        for _ in 0..3 {
            let plan = gate.plan(&cwd, true);
            let outcome = gate_gather(&plan, tmp.path(), |_| ());
            assert_eq!(
                outcome.gathered.as_ref().map(|(r, _)| *r),
                Some(GatherReason::Active)
            );
            gate.record(&cwd, &outcome, false);
        }
    }

    #[test]
    fn a_changed_result_on_a_periodic_gather_snaps_the_cadence_back() {
        // Simulate a repo whose tracked files keep changing without any
        // `.git` write: the fingerprint is constant, but the gathered
        // values move. The interval must not back off for it.
        let fp = RepoFingerprint::default();
        let mut gate = SweepGate::default();
        let outcome = GateOutcome {
            fingerprint: fp.clone(),
            gathered: Some((GatherReason::FirstSeen, ())),
        };
        gate.record("/r", &outcome, true);

        let visits_until_forced = |gate: &mut SweepGate| {
            let mut n = 0;
            loop {
                let plan = gate.plan("/r", false);
                if let Some(reason) = plan.reason_for(&fp) {
                    return (n, reason);
                }
                gate.record(
                    "/r",
                    &GateOutcome::<()> {
                        fingerprint: fp.clone(),
                        gathered: None,
                    },
                    false,
                );
                n += 1;
            }
        };

        let (skipped, reason) = visits_until_forced(&mut gate);
        assert_eq!(reason, GatherReason::Periodic);
        assert!(skipped <= GIT_SWEEP_FULL_GATHER_MIN_VISITS);
        // Quiet: interval doubles.
        gate.record(
            "/r",
            &GateOutcome {
                fingerprint: fp.clone(),
                gathered: Some((GatherReason::Periodic, ())),
            },
            false,
        );
        assert_eq!(visits_until_forced(&mut gate).0, GIT_SWEEP_FULL_GATHER_MIN_VISITS * 2);
        // Busy: values moved on the forced gather, back to the minimum.
        gate.record(
            "/r",
            &GateOutcome {
                fingerprint: fp.clone(),
                gathered: Some((GatherReason::Periodic, ())),
            },
            true,
        );
        assert_eq!(visits_until_forced(&mut gate).0, GIT_SWEEP_FULL_GATHER_MIN_VISITS);
    }

    #[test]
    fn periodic_interval_is_capped() {
        let fp = RepoFingerprint::default();
        let mut gate = SweepGate::default();
        gate.record(
            "/r",
            &GateOutcome {
                fingerprint: fp.clone(),
                gathered: Some((GatherReason::FirstSeen, ())),
            },
            false,
        );
        for _ in 0..10 {
            gate.record(
                "/r",
                &GateOutcome {
                    fingerprint: fp.clone(),
                    gathered: Some((GatherReason::Periodic, ())),
                },
                false,
            );
        }
        let mut skipped = 0;
        while gate.plan("/r", false).reason_for(&fp).is_none() {
            gate.record(
                "/r",
                &GateOutcome::<()> {
                    fingerprint: fp.clone(),
                    gathered: None,
                },
                false,
            );
            skipped += 1;
        }
        assert_eq!(skipped, GIT_SWEEP_FULL_GATHER_MAX_VISITS);
    }

    #[test]
    fn retain_drops_forgotten_checkouts() {
        let mut gate = SweepGate::default();
        for cwd in ["/a", "/b"] {
            gate.record(
                cwd,
                &GateOutcome {
                    fingerprint: RepoFingerprint::default(),
                    gathered: Some((GatherReason::FirstSeen, ())),
                },
                false,
            );
        }
        assert_eq!(gate.len(), 2);
        gate.retain(|cwd| cwd == "/a");
        assert_eq!(gate.len(), 1);
        assert!(gate.plan("/b", false).previous.is_none());
    }
}
