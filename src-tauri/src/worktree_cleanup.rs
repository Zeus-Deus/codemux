//! Stale worktree cleanup.
//!
//! Worktrees Codemux created outlive the workspaces that used them: an
//! archived workspace keeps its checkout on disk, and a closed one leaves it
//! behind entirely. This module finds the ones that are no longer needed and
//! removes them, either after the user reviews the list or automatically
//! when the user opted in (`AUTO_SETTING_KEY`).
//!
//! Only worktrees with no live workspace are ever considered, so no chat
//! or terminal Codemux owns can be running in one. Within those, a worktree
//! is *stale* when its branch is merged into the default branch, its pull
//! request merged, or it was archived/closed at least [`IDLE_AFTER_SECS`]
//! ago. A stale worktree is still *kept* when it holds anything that exists
//! nowhere else: uncommitted or untracked files, a detached HEAD, commits
//! that no remote has, a pending Hermes cleanup, or (Linux) a process whose
//! working directory is inside it.
//!
//! Removal is `git worktree remove` without `--force` (git re-checks the
//! tree is clean) and never touches the branch, so its commits survive and
//! an archived workspace can still be restored from it.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use tauri::{Emitter, Manager};

use crate::state::{AppStateSnapshot, AppStateStore, WorkspaceSnapshot};

/// Local setting: "true" removes stale worktrees without asking.
pub const AUTO_SETTING_KEY: &str = "worktree_cleanup.auto";
/// UI-state key holding the worktrees this device closed (JSON array).
const CLOSED_KEY: &str = "worktree_cleanup.closed";
/// Same age the archive list labels "stale".
pub const IDLE_AFTER_SECS: u64 = 30 * 86_400;
/// Event carrying a sweep's result to the frontend.
pub const EVENT: &str = "worktree-cleanup";

const FIRST_SWEEP_DELAY: Duration = Duration::from_secs(10 * 60);
const SWEEP_INTERVAL: Duration = Duration::from_secs(6 * 3600);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeSource {
    Archived,
    Closed,
}

/// Why a worktree is stale. Rendered by the review dialog and toasts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StaleReason {
    BranchMerged { base: String },
    PrMerged { number: u32 },
    Idle { days: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StaleWorktree {
    pub path: String,
    pub title: String,
    pub branch: Option<String>,
    /// Repository directory name, for grouping in the UI.
    pub project: String,
    pub source: WorktreeSource,
    pub reason: StaleReason,
    /// Why the worktree stays on disk even though it is stale. `None`
    /// means it is safe to remove.
    pub kept_because: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CleanupOutcome {
    pub removed: Vec<StaleWorktree>,
    /// Asked for but not removed; `kept_because` says why.
    pub kept: Vec<StaleWorktree>,
}

/// Payload of [`EVENT`]: what an automatic sweep removed, or what a
/// prompted sweep found newly removable.
#[derive(Debug, Clone, Serialize)]
pub struct CleanupEvent {
    pub removed: Vec<StaleWorktree>,
    pub available: Vec<StaleWorktree>,
}

/// A worktree this device closed without deleting it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ClosedWorktree {
    path: String,
    title: String,
    #[serde(default)]
    branch: Option<String>,
    closed_at: u64,
}

/// A worktree with no live workspace, before git has been asked about it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Candidate {
    path: PathBuf,
    title: String,
    branch: Option<String>,
    source: WorktreeSource,
    retired_at: u64,
}

/// Serializes scans and removals so a background sweep and a click on
/// "Remove" never act on the same worktree at once.
static RUN_LOCK: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));
/// Removable paths a prompted sweep already told the user about, so the
/// same list doesn't toast again every few hours.
static NOTIFIED: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
/// Wakes the sweep early, e.g. right after automatic cleanup is enabled.
static KICK: LazyLock<tokio::sync::Notify> = LazyLock::new(tokio::sync::Notify::new);

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Comparable spelling of a path: canonical when it exists, as given otherwise.
fn path_key(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn is_auto_enabled(db: &crate::database::DatabaseStore) -> bool {
    db.get_setting(AUTO_SETTING_KEY).as_deref() == Some("true")
}

fn load_closed(db: &crate::database::DatabaseStore) -> Vec<ClosedWorktree> {
    db.get_ui_state(CLOSED_KEY)
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn store_closed(db: &crate::database::DatabaseStore, closed: &[ClosedWorktree]) {
    let Ok(raw) = serde_json::to_string(closed) else { return };
    if let Err(error) = db.set_ui_state(CLOSED_KEY, &raw) {
        eprintln!("[worktree-cleanup] failed to record closed worktrees: {error}");
    }
}

/// Serializes read-modify-write of the closed list.
static CLOSED_LOCK: Mutex<()> = Mutex::new(());

/// Remember that `ws` was closed with its worktree left on disk, so a later
/// sweep can offer it once it goes stale. Only disposable local worktrees
/// are recorded; everything else is never a cleanup candidate.
pub(crate) fn remember_closed(db: &crate::database::DatabaseStore, ws: &WorkspaceSnapshot) {
    if crate::commands::workspace::refuse_worktree_removal(ws).is_some()
        || crate::commands::workspace::archive_refusal_reason(ws).is_some()
    {
        return;
    }
    let Some(path) = ws.worktree_path.clone() else { return };
    let _guard = CLOSED_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut closed = load_closed(db);
    closed.retain(|c| c.path != path);
    closed.push(ClosedWorktree {
        path,
        title: ws.title.clone(),
        branch: ws.git_branch.clone(),
        closed_at: now_secs(),
    });
    store_closed(db, &closed);
}

/// Every path a live workspace has open, in [`path_key`] spelling.
fn live_paths(snapshot: &AppStateSnapshot) -> HashSet<PathBuf> {
    snapshot
        .workspaces
        .iter()
        .flat_map(|ws| [Some(ws.cwd.as_str()), ws.worktree_path.as_deref()])
        .flatten()
        .map(|p| path_key(Path::new(p)))
        .collect()
}

/// True when a live workspace has `key` or a directory inside it open.
fn is_open(live: &HashSet<PathBuf>, key: &Path) -> bool {
    live.iter().any(|open| open.starts_with(key))
}

/// Archived and closed worktrees that still exist and that no live
/// workspace has open. An archived entry wins over a closed record for the
/// same path (archiving goes through the close path too).
fn gather_candidates(snapshot: &AppStateSnapshot, closed: &[ClosedWorktree]) -> Vec<Candidate> {
    let live = live_paths(snapshot);
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut push = |candidate: Candidate| {
        let key = path_key(&candidate.path);
        if candidate.path.is_dir() && !is_open(&live, &key) && seen.insert(key) {
            out.push(candidate);
        }
    };
    for entry in &snapshot.archived_workspaces {
        if crate::commands::workspace::refuse_archived_file_delete(entry).is_some() {
            continue;
        }
        let Some(path) = entry.worktree_path.as_deref() else { continue };
        push(Candidate {
            path: PathBuf::from(path),
            title: entry.title.clone(),
            branch: entry.git_branch.clone(),
            source: WorktreeSource::Archived,
            retired_at: entry.archived_at,
        });
    }
    for record in closed {
        push(Candidate {
            path: PathBuf::from(&record.path),
            title: record.title.clone(),
            branch: record.branch.clone(),
            source: WorktreeSource::Closed,
            retired_at: record.closed_at,
        });
    }
    out
}

/// Closed records still worth keeping: the directory exists and no live
/// workspace reopened it (a reopened worktree gets a new close time when
/// it closes again).
fn prune_closed(snapshot: &AppStateSnapshot, closed: &[ClosedWorktree]) -> Vec<ClosedWorktree> {
    let live = live_paths(snapshot);
    closed
        .iter()
        .filter(|c| {
            let path = Path::new(&c.path);
            path.is_dir() && !is_open(&live, &path_key(path))
        })
        .cloned()
        .collect()
}

/// Git without optional locks: a scan must not refresh the index (that
/// would also be a write inside a worktree nobody is using).
fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("--no-optional-locks")
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| format!("Failed to run git: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim_end().to_string())
}

fn ref_exists(dir: &Path, refname: &str) -> bool {
    git(dir, &["rev-parse", "--verify", "--quiet", refname]).is_ok()
}

/// What git says about a candidate. Gathered once, judged by [`judge`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Inspection {
    /// Not a linked worktree of a repository (or not a checkout at all).
    unmanaged: bool,
    head: Option<String>,
    /// Checked-out branch; `None` for a detached HEAD.
    branch: Option<String>,
    default_branch: Option<String>,
    /// The default branch HEAD is merged into, if any. Only set for a
    /// branch that gained commits of its own: a branch nobody committed to
    /// is trivially "merged" but was never worked on.
    merged_into: Option<String>,
    dirty_files: usize,
    /// Commits reachable from HEAD that no remote-tracking ref has.
    unpushed: usize,
}

fn inspect(path: &Path) -> Inspection {
    let unmanaged = Inspection { unmanaged: true, ..Default::default() };
    if !path.join(".git").is_file() {
        return unmanaged;
    }
    let Some(repo_root) = crate::config::workspace_config::find_git_root(path) else {
        return unmanaged;
    };
    if path_key(&repo_root) == path_key(path) {
        return unmanaged;
    }
    let Ok(head) = git(path, &["rev-parse", "--verify", "HEAD"]) else {
        return unmanaged;
    };
    let branch = git(path, &["symbolic-ref", "--quiet", "--short", "HEAD"]).ok();
    let dirty_files = git(
        path,
        &["status", "--porcelain", "--untracked-files=all", "--ignore-submodules=none"],
    )
    .map(|s| s.lines().filter(|l| !l.is_empty()).count())
    // A status that fails can't vouch for the tree.
    .unwrap_or(usize::MAX);
    let unpushed = git(path, &["rev-list", "--count", "HEAD", "--not", "--remotes"])
        .ok()
        .and_then(|n| n.trim().parse().ok())
        .unwrap_or(usize::MAX);
    let default_branch = crate::git::git_default_branch(&repo_root).ok();
    // More than the creation entry in the branch reflog: it was committed
    // to (or moved) after the worktree was made.
    let has_own_commits = branch.as_deref().is_some_and(|b| {
        git(path, &["reflog", "show", "--format=%H", &format!("refs/heads/{b}")])
            .is_ok_and(|log| log.lines().count() > 1)
    });
    let merged_into = default_branch.as_deref().filter(|_| has_own_commits).and_then(|base| {
        [format!("refs/remotes/origin/{base}"), format!("refs/heads/{base}")]
            .into_iter()
            .find(|target| {
                ref_exists(path, target)
                    && git(path, &["merge-base", "--is-ancestor", "HEAD", target]).is_ok()
            })
            .map(|_| base.to_string())
    });
    Inspection {
        unmanaged: false,
        head: Some(head),
        branch,
        default_branch,
        merged_into,
        dirty_files,
        unpushed,
    }
}

/// The merged pull request for a branch, as far as GitHub says.
#[derive(Debug, Clone, PartialEq, Eq)]
struct MergedPr {
    number: u32,
    head_oid: Option<String>,
}

fn merged_pr(path: &Path) -> Option<MergedPr> {
    let pr = crate::github::get_branch_pr(path).ok()??;
    (pr.state == "MERGED").then(|| MergedPr {
        number: pr.number,
        head_oid: pr.head_ref_oid,
    })
}

/// Pure verdict for one candidate. `pr` is consulted lazily because it
/// costs a GitHub request; `busy` is the process check. Returns `None` when
/// the worktree is not stale (or is not one Codemux may manage at all).
fn judge(
    candidate: &Candidate,
    inspection: &Inspection,
    now: u64,
    pr: &mut dyn FnMut() -> Option<MergedPr>,
    busy: bool,
    hermes_pending: bool,
) -> Option<StaleWorktree> {
    if inspection.unmanaged {
        return None;
    }
    // A checkout of the default branch itself is never disposable.
    if inspection.branch.is_some() && inspection.branch == inspection.default_branch {
        return None;
    }
    let idle_days = now.saturating_sub(candidate.retired_at) / 86_400;
    let idle = now.saturating_sub(candidate.retired_at) >= IDLE_AFTER_SECS;
    let mut pr_cache: Option<Option<MergedPr>> = None;
    let mut merged_pr = || pr_cache.get_or_insert_with(&mut *pr).clone();

    let reason = if let Some(base) = &inspection.merged_into {
        StaleReason::BranchMerged { base: base.clone() }
    } else if let Some(pr) = inspection.branch.as_ref().and_then(|_| merged_pr()) {
        StaleReason::PrMerged { number: pr.number }
    } else if idle {
        StaleReason::Idle { days: idle_days }
    } else {
        return None;
    };

    let kept_because = if busy {
        Some("A process is running inside it".to_string())
    } else if hermes_pending {
        Some("Hermes cleanup is still pending".to_string())
    } else if inspection.branch.is_none() {
        Some("HEAD is detached, so its commits have no branch".to_string())
    } else if inspection.dirty_files > 0 {
        Some(match inspection.dirty_files {
            usize::MAX => "Its git status could not be read".to_string(),
            1 => "1 uncommitted or untracked file".to_string(),
            n => format!("{n} uncommitted or untracked files"),
        })
    } else if inspection.unpushed > 0
        // Commits GitHub holds as the merged PR's head are pushed even
        // when the remote branch has since been deleted and pruned.
        && !merged_pr().is_some_and(|pr| pr.head_oid.is_some() && pr.head_oid == inspection.head)
    {
        Some(match inspection.unpushed {
            usize::MAX => "Its unpushed commits could not be counted".to_string(),
            1 => "1 commit isn't on any remote".to_string(),
            n => format!("{n} commits aren't on any remote"),
        })
    } else {
        None
    };

    Some(StaleWorktree {
        path: candidate.path.to_string_lossy().into_owned(),
        title: candidate.title.clone(),
        branch: inspection.branch.clone().or_else(|| candidate.branch.clone()),
        project: crate::config::workspace_config::find_git_root(&candidate.path)
            .and_then(|root| root.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_default(),
        source: candidate.source,
        reason,
        kept_because,
    })
}

/// Working directories of every process on the machine (Linux only; other
/// platforms rely on the no-live-workspace rule alone).
fn process_cwds() -> Vec<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        let Ok(entries) = std::fs::read_dir("/proc") else { return Vec::new() };
        entries
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().bytes().all(|b| b.is_ascii_digit()))
            .filter_map(|e| std::fs::read_link(e.path().join("cwd")).ok())
            .collect()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Vec::new()
    }
}

fn is_busy(path: &Path, cwds: &[PathBuf]) -> bool {
    let key = path_key(path);
    cwds.iter().any(|cwd| cwd.starts_with(&key))
}

fn hermes_pending(db: &crate::database::DatabaseStore, path: &Path) -> bool {
    // An unreadable hold counts as a hold.
    db.hermes_cleanup_pending_path(&path.to_string_lossy()).unwrap_or(true)
}

/// Evaluate every candidate. Blocking: shells out to git and GitHub.
fn evaluate(db: &crate::database::DatabaseStore, candidates: &[Candidate]) -> Vec<StaleWorktree> {
    let now = now_secs();
    let cwds = process_cwds();
    candidates
        .iter()
        .filter_map(|candidate| {
            let inspection = inspect(&candidate.path);
            judge(
                candidate,
                &inspection,
                now,
                &mut || merged_pr(&candidate.path),
                is_busy(&candidate.path, &cwds),
                hermes_pending(db, &candidate.path),
            )
        })
        .collect()
}

/// The current candidates, with the closed list pruned on the way.
fn current_candidates<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Vec<Candidate> {
    let db = app.state::<crate::database::DatabaseStore>();
    let snapshot = app.state::<AppStateStore>().snapshot();
    let _guard = CLOSED_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let closed = load_closed(&db);
    let kept = prune_closed(&snapshot, &closed);
    if kept != closed {
        store_closed(&db, &kept);
    }
    gather_candidates(&snapshot, &kept)
}

async fn scan_locked<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<Vec<StaleWorktree>, String> {
    let candidates = current_candidates(app);
    let app = app.clone();
    tokio::task::spawn_blocking(move || {
        evaluate(&app.state::<crate::database::DatabaseStore>(), &candidates)
    })
    .await
    .map_err(|e| format!("worktree cleanup scan failed: {e}"))
}

/// `git worktree remove` without `--force`, so git itself refuses a tree
/// that changed after the checks; untracked files count whatever the repo
/// config says. The branch is left alone.
fn remove_worktree(path: &Path) -> Result<(), String> {
    let repo = crate::config::workspace_config::find_git_root(path)
        .ok_or_else(|| "Its repository could not be found".to_string())?;
    git(
        &repo,
        &["-c", "status.showUntrackedFiles=all", "worktree", "remove", &path.to_string_lossy()],
    )
    .map(drop)
}

/// Remove the requested worktrees that are still stale and safe. Each one
/// is re-evaluated from fresh state right before its removal, so anything
/// reopened, edited or committed to since the review is kept.
async fn remove_locked<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    paths: &[String],
) -> CleanupOutcome {
    let mut outcome = CleanupOutcome::default();
    for requested in paths {
        let key = path_key(Path::new(requested));
        let Some(candidate) = current_candidates(app)
            .into_iter()
            .find(|c| path_key(&c.path) == key)
        else {
            continue;
        };
        // Same lock the close path holds around its delete, so a Hermes
        // start can't claim this directory between check and removal.
        let _hermes = crate::agent_provider::hermes::WORKTREE_LIFECYCLE.lock().await;
        let app_for_task = app.clone();
        let result = tokio::task::spawn_blocking(move || {
            let db = app_for_task.state::<crate::database::DatabaseStore>();
            let verdict = evaluate(&db, std::slice::from_ref(&candidate)).pop()?;
            if verdict.kept_because.is_some() {
                return Some(verdict);
            }
            Some(match remove_worktree(&candidate.path) {
                Ok(_) => verdict,
                Err(error) => StaleWorktree {
                    kept_because: Some(format!("git refused to remove it: {error}")),
                    ..verdict
                },
            })
        })
        .await;
        match result {
            Ok(Some(verdict)) if verdict.kept_because.is_none() => outcome.removed.push(verdict),
            Ok(Some(verdict)) => outcome.kept.push(verdict),
            // No longer stale: nothing to report.
            Ok(None) => {}
            Err(error) => eprintln!("[worktree-cleanup] removal task failed: {error}"),
        }
    }
    outcome
}

/// One background pass: remove everything removable when automatic cleanup
/// is on, otherwise tell the frontend about worktrees it hasn't shown yet.
async fn sweep<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    let _run = RUN_LOCK.lock().await;
    let stale = match scan_locked(app).await {
        Ok(stale) => stale,
        Err(error) => {
            eprintln!("[worktree-cleanup] {error}");
            return;
        }
    };
    let removable: Vec<_> = stale.into_iter().filter(|w| w.kept_because.is_none()).collect();
    if removable.is_empty() {
        return;
    }
    let auto = is_auto_enabled(&app.state::<crate::database::DatabaseStore>());
    let event = if auto {
        let paths: Vec<_> = removable.iter().map(|w| w.path.clone()).collect();
        let outcome = remove_locked(app, &paths).await;
        if outcome.removed.is_empty() {
            return;
        }
        CleanupEvent { removed: outcome.removed, available: Vec::new() }
    } else {
        let mut notified = NOTIFIED.lock().unwrap_or_else(|e| e.into_inner());
        if removable.iter().all(|w| notified.contains(&w.path)) {
            return;
        }
        notified.extend(removable.iter().map(|w| w.path.clone()));
        CleanupEvent { removed: Vec::new(), available: removable }
    };
    if let Err(error) = app.emit(EVENT, event) {
        eprintln!("[worktree-cleanup] failed to emit sweep result: {error}");
    }
}

/// Background sweep: once shortly after startup, then every few hours, and
/// whenever [`KICK`] fires.
pub fn spawn_sweeper<R: tauri::Runtime>(app: tauri::AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_SWEEP_DELAY).await;
        loop {
            sweep(&app).await;
            tokio::select! {
                _ = tokio::time::sleep(SWEEP_INTERVAL) => {}
                _ = KICK.notified() => {}
            }
        }
    });
}

/// Every stale worktree, removable or kept, for the review dialog.
#[tauri::command]
pub async fn worktree_cleanup_scan<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<Vec<StaleWorktree>, String> {
    let _run = RUN_LOCK.lock().await;
    scan_locked(&app).await
}

/// Remove the worktrees the user confirmed in the review dialog.
#[tauri::command]
pub async fn worktree_cleanup_remove<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    paths: Vec<String>,
) -> Result<CleanupOutcome, String> {
    let _run = RUN_LOCK.lock().await;
    let outcome = remove_locked(&app, &paths).await;
    crate::state::emit_app_state(&app);
    Ok(outcome)
}

/// Turn automatic cleanup on or off. Turning it on runs a sweep right away
/// instead of waiting for the next scheduled one.
#[tauri::command]
pub fn worktree_cleanup_set_auto(
    db: tauri::State<'_, crate::database::DatabaseStore>,
    enabled: bool,
) -> Result<(), String> {
    db.set_setting(AUTO_SETTING_KEY, if enabled { "true" } else { "false" })?;
    if enabled {
        KICK.notify_one();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(retired_at: u64) -> Candidate {
        Candidate {
            path: PathBuf::from("/tmp/wt"),
            title: "wt".into(),
            branch: Some("feature".into()),
            source: WorktreeSource::Archived,
            retired_at,
        }
    }

    fn clean() -> Inspection {
        Inspection {
            unmanaged: false,
            head: Some("abc".into()),
            branch: Some("feature".into()),
            default_branch: Some("main".into()),
            merged_into: None,
            dirty_files: 0,
            unpushed: 0,
        }
    }

    const NOW: u64 = 100 * 86_400;

    fn verdict(inspection: &Inspection, retired_at: u64, pr: Option<MergedPr>) -> Option<StaleWorktree> {
        judge(&candidate(retired_at), inspection, NOW, &mut || pr.clone(), false, false)
    }

    #[test]
    fn fresh_unmerged_worktree_is_not_stale() {
        assert_eq!(verdict(&clean(), NOW - 86_400, None), None);
    }

    #[test]
    fn merged_branch_is_stale_and_removable() {
        let inspection = Inspection { merged_into: Some("main".into()), ..clean() };
        let stale = verdict(&inspection, NOW, None).unwrap();
        assert_eq!(stale.reason, StaleReason::BranchMerged { base: "main".into() });
        assert_eq!(stale.kept_because, None);
    }

    #[test]
    fn merged_pr_is_stale() {
        let pr = MergedPr { number: 7, head_oid: Some("abc".into()) };
        let stale = verdict(&clean(), NOW, Some(pr)).unwrap();
        assert_eq!(stale.reason, StaleReason::PrMerged { number: 7 });
    }

    #[test]
    fn idle_for_thirty_days_is_stale() {
        let stale = verdict(&clean(), NOW - IDLE_AFTER_SECS, None).unwrap();
        assert_eq!(stale.reason, StaleReason::Idle { days: 30 });
    }

    #[test]
    fn default_branch_checkout_is_never_a_candidate() {
        let inspection = Inspection {
            branch: Some("main".into()),
            merged_into: Some("main".into()),
            ..clean()
        };
        assert_eq!(verdict(&inspection, 0, None), None);
    }

    #[test]
    fn unmanaged_path_is_never_a_candidate() {
        let inspection = Inspection { unmanaged: true, ..Default::default() };
        assert_eq!(verdict(&inspection, 0, None), None);
    }

    #[test]
    fn work_in_the_tree_keeps_it() {
        let merged = Inspection { merged_into: Some("main".into()), ..clean() };
        for inspection in [
            Inspection { dirty_files: 2, ..merged.clone() },
            Inspection { dirty_files: usize::MAX, ..merged.clone() },
            Inspection { branch: None, ..merged.clone() },
        ] {
            let stale = verdict(&inspection, NOW, None).unwrap();
            assert!(stale.kept_because.is_some(), "{inspection:?} must be kept");
        }
    }

    #[test]
    fn unpushed_commits_keep_it_unless_github_has_that_head() {
        let ahead = Inspection { unpushed: 3, ..clean() };
        let idle = NOW - IDLE_AFTER_SECS;
        assert!(verdict(&ahead, idle, None).unwrap().kept_because.is_some());

        let other_head = MergedPr { number: 1, head_oid: Some("def".into()) };
        assert!(verdict(&ahead, idle, Some(other_head)).unwrap().kept_because.is_some());

        let same_head = MergedPr { number: 1, head_oid: Some("abc".into()) };
        assert_eq!(verdict(&ahead, idle, Some(same_head)).unwrap().kept_because, None);
    }

    #[test]
    fn running_process_or_hermes_hold_keeps_it() {
        let merged = Inspection { merged_into: Some("main".into()), ..clean() };
        let busy = judge(&candidate(NOW), &merged, NOW, &mut || None, true, false).unwrap();
        assert!(busy.kept_because.unwrap().contains("process"));
        let held = judge(&candidate(NOW), &merged, NOW, &mut || None, false, true).unwrap();
        assert!(held.kept_because.unwrap().contains("Hermes"));
    }

    #[test]
    fn pr_is_not_requested_when_a_local_reason_suffices() {
        let merged = Inspection { merged_into: Some("main".into()), ..clean() };
        let mut asked = false;
        judge(&candidate(NOW), &merged, NOW, &mut || { asked = true; None }, false, false);
        assert!(!asked);
    }

    /// A repo with `main` mirrored to `origin/main` and a `feature`
    /// worktree branched from it.
    fn repo_with_worktree() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let wt = dir.path().join("wt");
        std::fs::create_dir(&repo).unwrap();
        let run = |dir: &Path, args: &[&str]| git(dir, args).unwrap();
        run(&repo, &["init", "-q", "-b", "main"]);
        for (key, value) in [
            ("user.name", "Test"),
            ("user.email", "test@example.com"),
            ("commit.gpgsign", "false"),
        ] {
            run(&repo, &["config", key, value]);
        }
        run(&repo, &["commit", "-q", "--allow-empty", "-m", "base"]);
        run(&repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
        run(&repo, &["symbolic-ref", "refs/remotes/origin/HEAD", "refs/remotes/origin/main"]);
        run(&repo, &["worktree", "add", "-q", "-b", "feature", &wt.to_string_lossy(), "main"]);
        (dir, repo, wt)
    }

    fn commit(dir: &Path, file: &str) {
        std::fs::write(dir.join(file), file).unwrap();
        git(dir, &["add", file]).unwrap();
        git(dir, &["commit", "-q", "-m", file]).unwrap();
    }

    #[test]
    fn inspect_untouched_branch_is_clean_but_not_merged() {
        let (_dir, _repo, wt) = repo_with_worktree();
        let inspection = inspect(&wt);
        assert!(!inspection.unmanaged);
        assert_eq!(inspection.branch.as_deref(), Some("feature"));
        assert_eq!(inspection.default_branch.as_deref(), Some("main"));
        assert_eq!(inspection.merged_into, None, "no commits of its own");
        assert_eq!((inspection.dirty_files, inspection.unpushed), (0, 0));
    }

    #[test]
    fn inspect_counts_unpushed_until_merged_upstream() {
        let (_dir, repo, wt) = repo_with_worktree();
        commit(&wt, "work.txt");
        let ahead = inspect(&wt);
        assert_eq!(ahead.unpushed, 1);
        assert_eq!(ahead.merged_into, None);

        git(&repo, &["merge", "-q", "--ff-only", "feature"]).unwrap();
        git(&repo, &["update-ref", "refs/remotes/origin/main", "main"]).unwrap();
        let merged = inspect(&wt);
        assert_eq!(merged.unpushed, 0);
        assert_eq!(merged.merged_into.as_deref(), Some("main"));
    }

    #[test]
    fn inspect_sees_untracked_files_hidden_by_config() {
        let (_dir, repo, wt) = repo_with_worktree();
        git(&repo, &["config", "status.showUntrackedFiles", "no"]).unwrap();
        std::fs::create_dir(wt.join("notes")).unwrap();
        std::fs::write(wt.join("notes/todo.txt"), "keep me").unwrap();
        assert_eq!(inspect(&wt).dirty_files, 1);
    }

    #[test]
    fn inspect_treats_the_main_checkout_as_unmanaged() {
        let (_dir, repo, _wt) = repo_with_worktree();
        assert!(inspect(&repo).unmanaged);
    }

    #[test]
    fn removal_keeps_the_branch_and_refuses_untracked_files() {
        let (_dir, repo, wt) = repo_with_worktree();
        commit(&wt, "work.txt");
        let tip = git(&wt, &["rev-parse", "HEAD"]).unwrap();

        git(&repo, &["config", "status.showUntrackedFiles", "no"]).unwrap();
        std::fs::write(wt.join("scratch.txt"), "local").unwrap();
        assert!(remove_worktree(&wt).is_err(), "untracked file must block removal");
        assert!(wt.join("scratch.txt").exists());

        std::fs::remove_file(wt.join("scratch.txt")).unwrap();
        remove_worktree(&wt).unwrap();
        assert!(!wt.exists());
        assert_eq!(git(&repo, &["rev-parse", "refs/heads/feature"]).unwrap(), tip);
    }

    #[test]
    fn candidates_skip_live_and_missing_worktrees() {
        let dir = tempfile::tempdir().unwrap();
        let (open, gone, closed) = (dir.path().join("open"), dir.path().join("gone"), dir.path().join("closed"));
        std::fs::create_dir_all(open.join("sub")).unwrap();
        std::fs::create_dir(&closed).unwrap();
        let mut snapshot = AppStateStore::default().snapshot();
        snapshot.workspaces.clear();
        snapshot.archived_workspaces.clear();
        let record = |path: &Path| ClosedWorktree {
            path: path.to_string_lossy().into_owned(),
            title: "t".into(),
            branch: None,
            closed_at: 0,
        };
        let closed_list = [record(&open), record(&gone), record(&closed)];

        let all = gather_candidates(&snapshot, &closed_list);
        assert_eq!(all.len(), 2, "a missing directory is never a candidate");

        let live_paths = HashSet::from([path_key(&open.join("sub"))]);
        assert!(is_open(&live_paths, &path_key(&open)));
        assert!(!is_open(&live_paths, &path_key(&closed)));
    }

    #[test]
    fn busy_matches_nested_cwds_only() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path().join("wt");
        std::fs::create_dir_all(wt.join("src")).unwrap();
        let key = path_key(&wt);
        assert!(is_busy(&wt, &[key.join("src")]));
        assert!(is_busy(&wt, &[key.clone()]));
        assert!(!is_busy(&wt, &[path_key(dir.path())]));
        assert!(!is_busy(&wt, &[PathBuf::from(format!("{}-other", key.display()))]));
    }
}
