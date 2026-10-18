//! Per-turn git diffs: capture around every dispatched agent turn, and the
//! command that serves one turn's diff.
//!
//! Capture rides the provider-agnostic dispatch hook
//! ([`TurnDispatchCheckpoint`]): every provider calls `prepare` right before
//! it dispatches a new turn (queued drains included), then `commit` or
//! `abort`. `prepare` snapshots the worktree; `commit` records that snapshot
//! as the turn's `before`. The turn's `after` is snapshotted when its
//! `TurnCompleted` arrives, or, if the next dispatch gets there first, the
//! next turn's `before` doubles as it: nothing runs between the two.
//!
//! Rows are keyed by the provider turn id the transcript (and the UI's turn
//! folds) use. The id is only known after dispatch, so rows carry the
//! dispatch's client nonce and are bound when the send path or the queued
//! dispatch event learns the id. Binding and completion are handled off the
//! event loop and may arrive in either order; a small per-thread buffer
//! holds whichever comes early.
//!
//! Unlike the opt-in revert checkpoints, capture is always on for local git
//! workspaces: a snapshot is a stat-cached `git add -A` into a private index
//! plus `commit-tree`, and only the newest [`KEEP_PER_THREAD`] turns keep
//! their refs.

use crate::agent_provider::{ProviderError, TurnDispatchCheckpoint};
use crate::database::turn_diffs::{AgentChatTurnDiffRecord, NewAgentChatTurnDiff};
use crate::database::DatabaseStore;
use crate::turn_diff::{TurnDiffFile, turn_diff_ref_name};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use tauri::{AppHandle, Manager, Runtime, State};

const KEEP_PER_THREAD: usize = 100;
/// Early bindings or completions remembered per thread. Anything older has
/// no row coming (a turn whose capture failed, a subagent turn).
const EARLY_EVENTS_PER_THREAD: usize = 8;

#[derive(Default)]
struct ThreadCapture {
    /// Serializes one thread's snapshot and bookkeeping steps.
    lock: Arc<Mutex<()>>,
    /// Turn ids that completed before any row was bound to them.
    settled: VecDeque<String>,
    /// `(client_nonce, turn_id)` bindings that arrived before their row.
    bindings: VecDeque<(String, String)>,
}

fn captures() -> &'static Mutex<HashMap<String, ThreadCapture>> {
    static CAPTURES: OnceLock<Mutex<HashMap<String, ThreadCapture>>> = OnceLock::new();
    CAPTURES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn thread_lock(thread_id: &str) -> Arc<Mutex<()>> {
    captures()
        .lock()
        .unwrap()
        .entry(thread_id.to_string())
        .or_default()
        .lock
        .clone()
}

fn push_bounded<T>(queue: &mut VecDeque<T>, value: T) {
    if queue.len() == EARLY_EVENTS_PER_THREAD {
        queue.pop_front();
    }
    queue.push_back(value);
}

fn remember_settled(thread_id: &str, turn_id: &str) {
    let mut captures = captures().lock().unwrap();
    let capture = captures.entry(thread_id.to_string()).or_default();
    push_bounded(&mut capture.settled, turn_id.to_string());
}

fn take_settled(thread_id: &str, turn_id: &str) -> bool {
    let mut captures = captures().lock().unwrap();
    let Some(capture) = captures.get_mut(thread_id) else {
        return false;
    };
    let before = capture.settled.len();
    capture.settled.retain(|id| id != turn_id);
    capture.settled.len() != before
}

fn remember_binding(thread_id: &str, client_nonce: &str, turn_id: &str) {
    let mut captures = captures().lock().unwrap();
    let capture = captures.entry(thread_id.to_string()).or_default();
    push_bounded(
        &mut capture.bindings,
        (client_nonce.to_string(), turn_id.to_string()),
    );
}

fn take_binding(thread_id: &str, client_nonce: &str) -> Option<String> {
    let mut captures = captures().lock().unwrap();
    let capture = captures.get_mut(thread_id)?;
    let index = capture
        .bindings
        .iter()
        .position(|(nonce, _)| nonce == client_nonce)?;
    capture.bindings.remove(index).map(|(_, turn_id)| turn_id)
}

fn unique_token() -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{millis}-{seq}")
}

/// A `before` snapshot taken by `prepare`, waiting for `commit` or `abort`.
#[derive(Debug, Clone)]
struct PendingSnapshot {
    repo_path: String,
    before_ref: String,
    before_commit: String,
    after_ref: String,
}

/// Snapshot `row`'s repository as the turn's end state and close the row.
fn settle(db: &DatabaseStore, row: &AgentChatTurnDiffRecord) -> Result<(), String> {
    if row.after_commit.is_some() {
        return Ok(());
    }
    let repo = Path::new(&row.repo_path);
    let Some(snapshot) =
        crate::git::git_checkpoint_create(repo, &row.after_ref, "codemux turn diff: after turn")?
    else {
        return Ok(());
    };
    db.close_agent_chat_turn_diff(row.id, &snapshot.snapshot_commit)?;
    Ok(())
}

fn snapshot_before_dispatch(
    db: &DatabaseStore,
    thread_id: &str,
    repo_path: &str,
) -> Result<Option<PendingSnapshot>, String> {
    let lock = thread_lock(thread_id);
    let _guard = lock.lock().unwrap();
    let token = unique_token();
    let before_ref = turn_diff_ref_name(thread_id, &token, "before");
    let Some(snapshot) = crate::git::git_checkpoint_create(
        Path::new(repo_path),
        &before_ref,
        "codemux turn diff: before turn",
    )?
    else {
        return Ok(None);
    };
    // A turn still open here ended without its completion being handled
    // yet. This snapshot is exactly its end state.
    for open in db.open_agent_chat_turn_diffs(thread_id) {
        if open.repo_path == repo_path {
            crate::turn_diff::point_ref(
                Path::new(repo_path),
                &open.after_ref,
                &snapshot.snapshot_commit,
            )?;
            db.close_agent_chat_turn_diff(open.id, &snapshot.snapshot_commit)?;
        } else {
            settle(db, &open)?;
        }
    }
    Ok(Some(PendingSnapshot {
        repo_path: repo_path.to_string(),
        before_ref,
        before_commit: snapshot.snapshot_commit,
        after_ref: turn_diff_ref_name(thread_id, &token, "after"),
    }))
}

fn record_accepted(
    db: &DatabaseStore,
    thread_id: &str,
    client_nonce: &str,
    pending: PendingSnapshot,
) -> Result<(), String> {
    let lock = thread_lock(thread_id);
    let _guard = lock.lock().unwrap();
    let (row, pruned) = db.insert_agent_chat_turn_diff(
        &NewAgentChatTurnDiff {
            thread_id: thread_id.to_string(),
            client_nonce: Some(client_nonce.to_string()),
            turn_id: take_binding(thread_id, client_nonce),
            repo_path: pending.repo_path,
            before_ref: pending.before_ref,
            before_commit: pending.before_commit,
            after_ref: pending.after_ref,
        },
        KEEP_PER_THREAD,
    )?;
    for (repo_path, ref_name) in pruned {
        let _ = crate::turn_diff::delete_ref(Path::new(&repo_path), &ref_name);
    }
    if row
        .turn_id
        .as_deref()
        .is_some_and(|turn_id| take_settled(thread_id, turn_id))
    {
        settle(db, &row)?;
    }
    Ok(())
}

fn bind_turn(
    db: &DatabaseStore,
    thread_id: &str,
    client_nonce: &str,
    turn_id: &str,
) -> Result<(), String> {
    let lock = thread_lock(thread_id);
    let _guard = lock.lock().unwrap();
    match db.bind_agent_chat_turn_diff(thread_id, client_nonce, turn_id)? {
        Some(row) if take_settled(thread_id, turn_id) => settle(db, &row),
        Some(_) => Ok(()),
        None => {
            remember_binding(thread_id, client_nonce, turn_id);
            Ok(())
        }
    }
}

fn turn_settled(db: &DatabaseStore, thread_id: &str, turn_id: &str) -> Result<(), String> {
    let lock = thread_lock(thread_id);
    let _guard = lock.lock().unwrap();
    match db.get_agent_chat_turn_diff(thread_id, turn_id) {
        Some(row) => settle(db, &row),
        None => {
            remember_settled(thread_id, turn_id);
            Ok(())
        }
    }
}

fn spawn_db_task<R: Runtime>(
    app: &AppHandle<R>,
    label: &'static str,
    task: impl FnOnce(&DatabaseStore) -> Result<(), String> + Send + 'static,
) {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let Some(db) = app.try_state::<DatabaseStore>() else {
            return;
        };
        if let Err(error) = task(&db) {
            eprintln!("[codemux::turn_diff] {label} failed: {error}");
        }
    });
}

/// The provider reported the turn id a dispatch with `client_nonce` became.
/// Steered guidance joins an existing turn and must not be bound.
pub(crate) fn on_turn_bound<R: Runtime>(
    app: &AppHandle<R>,
    thread_id: &str,
    client_nonce: Option<&str>,
    turn_id: &str,
) {
    let Some(client_nonce) = client_nonce.filter(|nonce| !nonce.is_empty()) else {
        return;
    };
    let (thread_id, client_nonce, turn_id) =
        (thread_id.to_string(), client_nonce.to_string(), turn_id.to_string());
    spawn_db_task(app, "binding a turn", move |db| {
        bind_turn(db, &thread_id, &client_nonce, &turn_id)
    });
}

/// A turn settled (completed, failed or interrupted): snapshot its end state.
pub(crate) fn on_turn_completed<R: Runtime>(app: &AppHandle<R>, thread_id: &str, turn_id: &str) {
    let (thread_id, turn_id) = (thread_id.to_string(), turn_id.to_string());
    spawn_db_task(app, "settling a turn", move |db| {
        turn_settled(db, &thread_id, &turn_id)
    });
}

/// Repositories that may hold a thread's turn-diff refs. Read before the
/// thread's rows are deleted.
pub(crate) fn thread_repos(db: &DatabaseStore, thread_id: &str) -> Vec<String> {
    let mut repos = db.agent_chat_turn_diff_repos(thread_id);
    if let Some(cwd) = db.get_agent_chat_session(thread_id).and_then(|s| s.cwd) {
        if !repos.contains(&cwd) {
            repos.push(cwd);
        }
    }
    repos
}

/// Drop a deleted thread's refs and in-memory capture state.
pub(crate) fn forget_thread(thread_id: &str, repos: &[String]) {
    captures().lock().unwrap().remove(thread_id);
    for repo in repos {
        if let Err(error) = crate::turn_diff::delete_thread_refs(Path::new(repo), thread_id) {
            eprintln!("[codemux::turn_diff] failed to delete refs for {thread_id}: {error}");
        }
    }
}

/// Wraps a dispatch's (possibly absent) revert checkpoint so the turn is
/// also captured for its diff. The inner checkpoint keeps its own semantics.
pub(crate) struct TurnDiffCapture<R: Runtime> {
    app: AppHandle<R>,
    thread_id: String,
    repo_path: String,
    client_nonce: String,
    inner: Option<Arc<dyn TurnDispatchCheckpoint>>,
    pending: Mutex<Option<PendingSnapshot>>,
}

impl<R: Runtime> std::fmt::Debug for TurnDiffCapture<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TurnDiffCapture")
            .field("thread_id", &self.thread_id)
            .field("repo_path", &self.repo_path)
            .field("inner", &self.inner)
            .finish_non_exhaustive()
    }
}

/// Add diff capture to a dispatch. A device thread's files are out of
/// local git's reach, and without a workspace path or a client nonce to
/// bind by there is nothing to capture; those dispatches are left as they
/// were.
pub(crate) fn wrap_dispatch_checkpoint<R: Runtime>(
    app: &AppHandle<R>,
    thread_id: &str,
    repo_path: Option<String>,
    client_nonce: Option<&str>,
    inner: Option<Arc<dyn TurnDispatchCheckpoint>>,
) -> Option<Arc<dyn TurnDispatchCheckpoint>> {
    let (Some(repo_path), Some(client_nonce)) =
        (repo_path, client_nonce.filter(|nonce| !nonce.is_empty()))
    else {
        return inner;
    };
    if super::agent_chat::thread_runs_on_device_pub(app, thread_id) {
        return inner;
    }
    Some(Arc::new(TurnDiffCapture {
        app: app.clone(),
        thread_id: thread_id.to_string(),
        repo_path,
        client_nonce: client_nonce.to_string(),
        inner,
        pending: Mutex::new(None),
    }))
}

impl<R: Runtime> TurnDiffCapture<R> {
    fn discard(&self, pending: Option<PendingSnapshot>) {
        if let Some(pending) = pending {
            let _ = crate::turn_diff::delete_ref(Path::new(&pending.repo_path), &pending.before_ref);
        }
    }
}

#[async_trait::async_trait]
impl<R: Runtime> TurnDispatchCheckpoint for TurnDiffCapture<R> {
    fn authorize_dispatch(&self) -> Result<(), ProviderError> {
        match &self.inner {
            Some(inner) => inner.authorize_dispatch(),
            None => Ok(()),
        }
    }

    async fn prepare(&self) {
        if let Some(inner) = &self.inner {
            inner.prepare().await;
        }
        let app = self.app.clone();
        let thread_id = self.thread_id.clone();
        let repo_path = self.repo_path.clone();
        let snapshot = tauri::async_runtime::spawn_blocking(move || {
            let db = app.try_state::<DatabaseStore>()?;
            snapshot_before_dispatch(&db, &thread_id, &repo_path)
                .inspect_err(|error| {
                    eprintln!("[codemux::turn_diff] before-turn snapshot failed: {error}")
                })
                .ok()
                .flatten()
        })
        .await
        .ok()
        .flatten();
        let previous = std::mem::replace(&mut *self.pending.lock().unwrap(), snapshot);
        self.discard(previous);
    }

    async fn commit(&self) {
        if let Some(inner) = &self.inner {
            inner.commit().await;
        }
        let Some(pending) = self.pending.lock().unwrap().take() else {
            return;
        };
        let app = self.app.clone();
        let thread_id = self.thread_id.clone();
        let client_nonce = self.client_nonce.clone();
        // Awaited so the row exists before the send path binds its turn id.
        let recorded = tauri::async_runtime::spawn_blocking(move || {
            let db = app
                .try_state::<DatabaseStore>()
                .ok_or_else(|| "database unavailable".to_string())?;
            let fallback = pending.clone();
            record_accepted(&db, &thread_id, &client_nonce, pending)
                .inspect_err(|_| {
                    let _ = crate::turn_diff::delete_ref(
                        Path::new(&fallback.repo_path),
                        &fallback.before_ref,
                    );
                })
        })
        .await;
        match recorded {
            Ok(Ok(())) => {}
            Ok(Err(error)) => eprintln!("[codemux::turn_diff] recording a turn failed: {error}"),
            Err(error) => eprintln!("[codemux::turn_diff] recording task failed: {error}"),
        }
    }

    async fn abort(&self) {
        if let Some(inner) = &self.inner {
            inner.abort().await;
        }
        let pending = self.pending.lock().unwrap().take();
        self.discard(pending);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentChatTurnDiffStatus {
    Ready,
    /// The turn has not settled; `files` is empty.
    InProgress,
}

/// What one agent turn changed in its workspace, as a git diff between the
/// snapshots taken right before and right after it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentChatTurnDiff {
    pub thread_id: String,
    pub turn_id: String,
    pub status: AgentChatTurnDiffStatus,
    pub repo_path: String,
    pub before_commit: String,
    pub after_commit: Option<String>,
    pub files: Vec<TurnDiffFile>,
    pub files_changed: usize,
    pub additions: u64,
    pub deletions: u64,
    pub truncated: bool,
}

/// One turn's diff. `None` when the turn was not captured: a folder that is
/// not a git repository, a device thread, or a turn older than capture or
/// pruned since.
#[tauri::command]
pub async fn agent_chat_get_turn_diff(
    db: State<'_, DatabaseStore>,
    thread_id: String,
    turn_id: String,
) -> Result<Option<AgentChatTurnDiff>, String> {
    let Some(record) = db.get_agent_chat_turn_diff(&thread_id, &turn_id) else {
        return Ok(None);
    };
    if !Path::new(&record.repo_path).is_dir() {
        return Ok(None);
    }
    let mut result = AgentChatTurnDiff {
        thread_id,
        turn_id,
        status: AgentChatTurnDiffStatus::InProgress,
        repo_path: record.repo_path.clone(),
        before_commit: record.before_commit.clone(),
        after_commit: record.after_commit.clone(),
        files: Vec::new(),
        files_changed: 0,
        additions: 0,
        deletions: 0,
        truncated: false,
    };
    let Some(after) = record.after_commit else {
        return Ok(Some(result));
    };
    let diff = tauri::async_runtime::spawn_blocking(move || {
        crate::turn_diff::diff_snapshots(Path::new(&record.repo_path), &record.before_commit, &after)
    })
    .await
    .map_err(|e| format!("agent_chat_get_turn_diff task join failed: {e}"))??;
    result.status = AgentChatTurnDiffStatus::Ready;
    result.files = diff.files;
    result.files_changed = diff.files_changed;
    result.additions = diff.additions;
    result.deletions = diff.deletions;
    result.truncated = diff.truncated;
    Ok(Some(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn git(repo: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(repo)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    fn setup(thread_id: &str) -> (tempfile::TempDir, String, DatabaseStore) {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path();
        git(repo, &["init", "-q", "-b", "main"]);
        git(repo, &["config", "user.name", "Test"]);
        git(repo, &["config", "user.email", "test@example.com"]);
        fs::write(repo.join("a.txt"), "a\n").unwrap();
        git(repo, &["add", "-A"]);
        git(repo, &["commit", "-q", "-m", "init"]);
        let repo_path = repo.to_string_lossy().to_string();
        let db = DatabaseStore::new_in_memory();
        db.upsert_agent_chat_session(thread_id, "ws", Some(&repo_path), "claude")
            .unwrap();
        (dir, repo_path, db)
    }

    fn dispatch(db: &DatabaseStore, thread_id: &str, repo: &str, nonce: &str) {
        let pending = snapshot_before_dispatch(db, thread_id, repo).unwrap().unwrap();
        record_accepted(db, thread_id, nonce, pending).unwrap();
    }

    fn diff(db: &DatabaseStore, thread_id: &str, turn_id: &str) -> Vec<(String, u64, u64)> {
        let row = db.get_agent_chat_turn_diff(thread_id, turn_id).unwrap();
        let after = row.after_commit.expect("turn settled");
        crate::turn_diff::diff_snapshots(Path::new(&row.repo_path), &row.before_commit, &after)
            .unwrap()
            .files
            .into_iter()
            .map(|f| (f.path, f.additions, f.deletions))
            .collect()
    }

    #[test]
    fn each_turn_owns_only_its_changes() {
        let (_dir, repo, db) = setup("t-own");
        let path = Path::new(&repo);

        dispatch(&db, "t-own", &repo, "n1");
        bind_turn(&db, "t-own", "n1", "turn-1").unwrap();
        fs::write(path.join("a.txt"), "a\nturn one\n").unwrap();
        turn_settled(&db, "t-own", "turn-1").unwrap();

        // The user edits between turns; neither turn may claim it.
        fs::write(path.join("user.txt"), "mine\n").unwrap();

        dispatch(&db, "t-own", &repo, "n2");
        bind_turn(&db, "t-own", "n2", "turn-2").unwrap();
        fs::write(path.join("b.txt"), "turn two\n").unwrap();
        turn_settled(&db, "t-own", "turn-2").unwrap();

        assert_eq!(diff(&db, "t-own", "turn-1"), vec![("a.txt".into(), 1, 0)]);
        assert_eq!(diff(&db, "t-own", "turn-2"), vec![("b.txt".into(), 1, 0)]);
    }

    #[test]
    fn next_dispatch_settles_a_turn_whose_completion_is_late() {
        let (_dir, repo, db) = setup("t-late");
        let path = Path::new(&repo);

        dispatch(&db, "t-late", &repo, "n1");
        bind_turn(&db, "t-late", "n1", "turn-1").unwrap();
        fs::write(path.join("one.txt"), "1\n").unwrap();
        // A queued follow-up dispatches before turn-1's completion is handled.
        dispatch(&db, "t-late", &repo, "n2");
        fs::write(path.join("two.txt"), "2\n").unwrap();
        // turn-1's completion lands after turn-2 started editing: ignored.
        turn_settled(&db, "t-late", "turn-1").unwrap();
        bind_turn(&db, "t-late", "n2", "turn-2").unwrap();
        turn_settled(&db, "t-late", "turn-2").unwrap();

        assert_eq!(diff(&db, "t-late", "turn-1"), vec![("one.txt".into(), 1, 0)]);
        assert_eq!(diff(&db, "t-late", "turn-2"), vec![("two.txt".into(), 1, 0)]);
    }

    #[test]
    fn completion_and_binding_may_arrive_in_either_order() {
        let (_dir, repo, db) = setup("t-order");
        let path = Path::new(&repo);

        // Completion before the binding.
        dispatch(&db, "t-order", &repo, "n1");
        fs::write(path.join("one.txt"), "1\n").unwrap();
        turn_settled(&db, "t-order", "turn-1").unwrap();
        bind_turn(&db, "t-order", "n1", "turn-1").unwrap();
        assert_eq!(diff(&db, "t-order", "turn-1"), vec![("one.txt".into(), 1, 0)]);

        // Binding (and completion) before the row is even recorded.
        let pending = snapshot_before_dispatch(&db, "t-order", &repo).unwrap().unwrap();
        fs::write(path.join("two.txt"), "2\n").unwrap();
        bind_turn(&db, "t-order", "n2", "turn-2").unwrap();
        turn_settled(&db, "t-order", "turn-2").unwrap();
        record_accepted(&db, "t-order", "n2", pending).unwrap();
        assert_eq!(diff(&db, "t-order", "turn-2"), vec![("two.txt".into(), 1, 0)]);
    }

    #[test]
    fn non_git_folders_are_not_captured() {
        let dir = tempfile::tempdir().unwrap();
        let db = DatabaseStore::new_in_memory();
        let folder = dir.path().to_string_lossy().to_string();
        assert!(snapshot_before_dispatch(&db, "t-plain", &folder)
            .unwrap()
            .is_none());
    }
}
