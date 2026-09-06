//! Ordered, coalesced persistence for the active workspace selection.
//!
//! Every full state emit passes through this stream. That makes sidebar clicks,
//! keyboard navigation, control/MCP activation, create/close fallback, and
//! archive restoration share one persistence path without holding the invoke
//! response open on SQLite.

use std::sync::{LazyLock, Mutex, MutexGuard};

use serde::Serialize;
use tauri::{Emitter, Manager};

#[derive(Clone, Debug, PartialEq, Eq)]
struct PersistRequest {
    generation: u64,
    workspace_id: String,
}

#[derive(Default)]
struct PersistQueue {
    next_generation: u64,
    latest_state_revision: u64,
    pending: Option<PersistRequest>,
    worker_running: bool,
    /// Latest accepted selection, retained independently of worker retry state.
    latest_desired: Option<String>,
    latest_requested: Option<String>,
    last_persisted: Option<String>,
    last_reported_failure: Option<String>,
}

impl PersistQueue {
    /// Returns true when the caller must start the one worker.
    fn enqueue(&mut self, workspace_id: &str, state_revision: u64) -> bool {
        // Snapshot serialization and event delivery can finish out of order.
        // Reject an older snapshot even when it reaches this queue after a
        // newer selection, otherwise a slow A emit can overwrite B in SQLite.
        if state_revision < self.latest_state_revision {
            return false;
        }
        if state_revision == self.latest_state_revision
            && self
                .latest_requested
                .as_deref()
                .is_some_and(|latest| latest != workspace_id)
        {
            return false;
        }
        self.latest_state_revision = state_revision;
        self.latest_desired = Some(workspace_id.to_owned());
        // Deduplicate only when the selection is genuinely covered: queued
        // or in flight with a live worker, or already durable with nothing
        // queued. After a worker abort neither holds for the request the
        // dead worker had taken, so a duplicate emit re-enqueues it instead
        // of assuming a dead worker will finish the write.
        let queued_or_in_flight = self.latest_requested.as_deref() == Some(workspace_id)
            && (self.worker_running || self.pending.is_some());
        let already_durable = !self.worker_running
            && self.pending.is_none()
            && self.last_persisted.as_deref() == Some(workspace_id);
        if queued_or_in_flight || already_durable {
            // A request that survived a worker abort still needs draining:
            // restart the worker without re-enqueueing.
            if !self.worker_running && self.pending.is_some() {
                self.worker_running = true;
                return true;
            }
            return false;
        }
        if self.last_reported_failure.as_deref() != Some(workspace_id) {
            self.last_reported_failure = None;
        }
        self.next_generation = self.next_generation.saturating_add(1);
        self.pending = Some(PersistRequest {
            generation: self.next_generation,
            workspace_id: workspace_id.to_owned(),
        });
        self.latest_requested = Some(workspace_id.to_owned());
        if self.worker_running {
            false
        } else {
            self.worker_running = true;
            true
        }
    }

    fn take(&mut self) -> Option<PersistRequest> {
        match self.pending.take() {
            Some(request) => Some(request),
            None => {
                self.worker_running = false;
                None
            }
        }
    }

    /// Returns true when this failure is still authoritative and has not
    /// already been surfaced. Obsolete generations never toast.
    fn complete(&mut self, request: &PersistRequest, succeeded: bool) -> bool {
        if succeeded {
            self.last_persisted = Some(request.workspace_id.clone());
            if self.last_reported_failure.as_deref() == Some(&request.workspace_id) {
                self.last_reported_failure = None;
            }
        }
        let authoritative = self
            .pending
            .as_ref()
            .map_or(true, |pending| pending.generation <= request.generation)
            && self.latest_requested.as_deref() == Some(request.workspace_id.as_str());
        let report_failure = !succeeded
            && authoritative
            && self.last_reported_failure.as_deref() != Some(&request.workspace_id);
        if report_failure {
            self.last_reported_failure = Some(request.workspace_id.clone());
        }
        if self.pending.is_none()
            && self.latest_requested.as_deref() == Some(request.workspace_id.as_str())
        {
            // A failed value must be retryable on the next state emit.
            self.latest_requested = succeeded.then(|| request.workspace_id.clone());
        }
        report_failure
    }

    fn seed_persisted(&mut self, workspace_id: &str) {
        if self.worker_running || self.pending.is_some() {
            return;
        }
        self.latest_requested = Some(workspace_id.to_owned());
        self.last_persisted = Some(workspace_id.to_owned());
        self.latest_desired = Some(workspace_id.to_owned());
    }

    /// Called when a worker thread exits without draining the queue (a
    /// panic, or any early bail-out). Only clears the running flag: a
    /// selection enqueued while the worker was unwinding survives untouched,
    /// so the next state emit starts a fresh worker that drains it and
    /// `flush_latest` still sees the `latest_desired`/`last_persisted`
    /// gap. The request the dead worker had already taken was never marked
    /// persisted either, so a duplicate emit re-enqueues it (see `enqueue`)
    /// and a shutdown flush re-persists it.
    fn reset_after_worker_abort(&mut self) {
        self.worker_running = false;
    }

    fn take_latest_for_flush(&mut self) -> Option<PersistRequest> {
        let latest = self.latest_desired.clone()?;
        if self.last_persisted.as_deref() == Some(&latest) && self.pending.is_none() {
            return None;
        }
        self.pending = None;
        self.next_generation = self.next_generation.saturating_add(1);
        Some(PersistRequest {
            generation: self.next_generation,
            workspace_id: latest,
        })
    }
}

static QUEUE: LazyLock<Mutex<PersistQueue>> =
    LazyLock::new(|| Mutex::new(PersistQueue::default()));
static WRITE_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// Both locks recover from poisoning instead of giving up: a single
/// panicked worker must never disable selection persistence for the rest
/// of the session.
///
/// This is sound for `WRITE_LOCK` because it guards exactly one SQLite
/// upsert. A worker that panicked mid-write left the row either updated or
/// untouched, and both are states the generation-ordered queue already
/// handles (a failed write retries on the next emit, an obsolete one is
/// superseded), so the poison carries no invariant worth honouring.
fn lock_writer() -> MutexGuard<'static, ()> {
    WRITE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// See [`lock_writer`]. For `QUEUE` the guarded section is a short pure
/// method on `PersistQueue`; the worst a mid-method panic can leave behind
/// is one extra or one skipped write, which the generation ordering and
/// the failed-write retry path already tolerate.
fn lock_queue() -> MutexGuard<'static, PersistQueue> {
    QUEUE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Clears the worker bookkeeping even when the worker thread unwinds.
/// Without this, a panic anywhere in the loop would leave `worker_running`
/// set and every future `enqueue` would decline to start a worker until
/// the process restarts.
struct WorkerExitGuard {
    armed: bool,
}

impl WorkerExitGuard {
    /// The normal exit path clears `worker_running` inside `take()` while
    /// holding the queue lock — atomically with observing an empty queue —
    /// and a replacement worker may already be running by the time this
    /// thread finishes unwinding its stack. The guard must therefore be
    /// disarmed on that path and only fire on abnormal exits.
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for WorkerExitGuard {
    fn drop(&mut self) {
        if self.armed {
            lock_queue().reset_after_worker_abort();
        }
    }
}

pub fn seed_persisted(workspace_id: &str) {
    lock_queue().seed_persisted(workspace_id);
}

/// Apply the authoritative selection record to a restored layout body. A
/// stale id is ignored because close/archive may have removed its workspace
/// after the last successful selection commit.
pub fn apply_restored_selection(
    snapshot: &mut crate::state::AppStateSnapshot,
    persisted_workspace_id: Option<&str>,
) -> bool {
    let Some(workspace_id) = persisted_workspace_id else {
        return false;
    };
    if !snapshot
        .workspaces
        .iter()
        .any(|workspace| workspace.workspace_id.0 == workspace_id)
    {
        return false;
    }
    snapshot.active_workspace_id = crate::state::WorkspaceId(workspace_id.to_owned());
    true
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PersistFailure {
    generation: u64,
    error: String,
}

pub fn schedule<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    workspace_id: &str,
    state_revision: u64,
) {
    if workspace_id.is_empty() {
        return;
    }
    let should_start = lock_queue().enqueue(workspace_id, state_revision);
    if !should_start {
        return;
    }

    let app = app.clone();
    std::thread::spawn(move || {
        let mut exit_guard = WorkerExitGuard { armed: true };
        loop {
            // Lock order everywhere: WRITE_LOCK, then QUEUE.
            let _write_guard = lock_writer();
            let request = match lock_queue().take() {
                Some(request) => request,
                None => {
                    // `take()` already cleared `worker_running` under the
                    // queue lock; see `WorkerExitGuard::disarm`.
                    exit_guard.disarm();
                    return;
                }
            };
            let started = std::time::Instant::now();
            // `try_state` instead of the panicking `state`: during early
            // startup or teardown the store may not be managed yet, and a
            // panic here would poison the write lock and strand the worker
            // bookkeeping. An unmanaged store is just a failed write, which
            // the next state emit retries.
            let result = match app.try_state::<crate::database::DatabaseStore>() {
                Some(db) => db.set_ui_state("active_workspace", &request.workspace_id),
                None => Err("database store not initialized".to_string()),
            };
            crate::diagnostics::record_perf_timing(
                "state.active-workspace-persist",
                started.elapsed(),
            );
            let report_failure = lock_queue().complete(&request, result.is_ok());
            if report_failure {
                if let Err(error) = result {
                    let _ = app.emit(
                        "active-workspace-persist-failed",
                        PersistFailure {
                            generation: request.generation,
                            error,
                        },
                    );
                }
            }
        }
    });
}

/// Shutdown-only synchronous drain. Regular activation never waits for this
/// lock or SQLite; close/Exit use it so the latest optimistic A→B→C selection
/// is durable before the process can disappear.
pub fn flush_latest<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<(), String> {
    let _write_guard = lock_writer();
    let request = lock_queue().take_latest_for_flush();
    let Some(request) = request else {
        return Ok(());
    };
    let result = match app.try_state::<crate::database::DatabaseStore>() {
        Some(db) => db.set_ui_state("active_workspace", &request.workspace_id),
        None => Err("database store not initialized".to_string()),
    };
    lock_queue().complete(&request, result.is_ok());
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rapid_requests_coalesce_to_the_latest_generation() {
        let mut queue = PersistQueue::default();
        assert!(queue.enqueue("a", 1));
        assert!(!queue.enqueue("b", 2));
        assert!(!queue.enqueue("c", 3));
        let request = queue.take().unwrap();
        assert_eq!(request.workspace_id, "c");
        assert_eq!(request.generation, 3);
        queue.complete(&request, true);
        assert!(queue.take().is_none());
        assert_eq!(queue.last_persisted.as_deref(), Some("c"));
    }

    #[test]
    fn same_selection_is_a_noop_but_a_failed_write_retries() {
        let mut queue = PersistQueue::default();
        assert!(queue.enqueue("a", 1));
        let first = queue.take().unwrap();
        queue.complete(&first, true);
        assert!(queue.take().is_none());
        assert!(!queue.enqueue("a", 2));

        assert!(queue.enqueue("b", 3));
        let failed = queue.take().unwrap();
        queue.complete(&failed, false);
        assert!(queue.take().is_none());
        assert!(queue.enqueue("b", 4));
    }

    #[test]
    fn in_flight_commit_is_followed_by_the_latest_rapid_cycle() {
        let mut queue = PersistQueue::default();
        assert!(queue.enqueue("a", 1));
        let in_flight = queue.take().unwrap();

        assert!(!queue.enqueue("b", 2));
        assert!(!queue.enqueue("c", 3));
        queue.complete(&in_flight, true);

        let latest = queue.take().unwrap();
        assert_eq!(latest.workspace_id, "c");
        assert!(latest.generation > in_flight.generation);
        queue.complete(&latest, true);
        assert!(queue.take().is_none());
        assert_eq!(queue.last_persisted.as_deref(), Some("c"));
    }

    #[test]
    fn shutdown_flush_takes_latest_pending_selection() {
        let mut queue = PersistQueue::default();
        assert!(queue.enqueue("a", 1));
        let in_flight = queue.take().unwrap();
        assert!(!queue.enqueue("b", 2));
        assert!(!queue.enqueue("c", 3));
        queue.complete(&in_flight, true);

        let flush = queue.take_latest_for_flush().unwrap();
        assert_eq!(flush.workspace_id, "c");
        queue.complete(&flush, true);
        assert!(queue.take_latest_for_flush().is_none());
    }

    #[test]
    fn shutdown_flush_retries_a_failed_final_selection_without_another_emit() {
        let mut queue = PersistQueue::default();
        queue.seed_persisted("a");
        assert!(queue.enqueue("b", 1));
        let failed = queue.take().unwrap();
        assert!(queue.complete(&failed, false));
        assert!(queue.take().is_none());

        let flush = queue
            .take_latest_for_flush()
            .expect("failed selection must survive");
        assert_eq!(flush.workspace_id, "b");
        assert!(flush.generation > failed.generation);
        assert!(
            !queue.complete(&flush, false),
            "do not repeat the failure toast"
        );
        let retry = queue
            .take_latest_for_flush()
            .expect("failed flush must also retry");
        assert_eq!(retry.workspace_id, "b");
        assert!(retry.generation > flush.generation);
        queue.complete(&retry, true);
        assert_eq!(queue.last_persisted.as_deref(), Some("b"));
        assert!(queue.take_latest_for_flush().is_none());
    }

    #[test]
    fn returning_to_durable_selection_after_failure_cancels_shutdown_retry() {
        let mut queue = PersistQueue::default();
        queue.seed_persisted("a");
        assert!(queue.enqueue("b", 1));
        let failed = queue.take().unwrap();
        queue.complete(&failed, false);
        assert!(queue.take().is_none());

        assert!(
            !queue.enqueue("a", 2),
            "already durable selection needs no worker"
        );
        assert!(queue.take_latest_for_flush().is_none());
    }

    #[test]
    fn failed_selection_can_retry_before_worker_exits_without_accepting_stale_emit() {
        let mut queue = PersistQueue::default();
        queue.seed_persisted("a");
        assert!(queue.enqueue("b", 2));
        let failed = queue.take().unwrap();
        queue.complete(&failed, false);

        assert!(!queue.enqueue("a", 1));
        assert!(
            !queue.enqueue("b", 2),
            "reuse the worker that has not exited yet"
        );
        let retry = queue.take().expect("same-revision retry must be queued");
        assert_eq!(retry.workspace_id, "b");
        assert!(retry.generation > failed.generation);
        queue.complete(&retry, true);
        assert!(queue.take().is_none());
        assert!(queue.take_latest_for_flush().is_none());
    }

    #[test]
    fn obsolete_and_duplicate_failures_are_not_reported() {
        let mut queue = PersistQueue::default();
        assert!(queue.enqueue("a", 1));
        let obsolete = queue.take().unwrap();
        assert!(!queue.enqueue("b", 2));
        assert!(!queue.complete(&obsolete, false));
        let latest = queue.take().unwrap();
        assert!(queue.complete(&latest, false));
        assert!(queue.take().is_none());
        assert!(queue.enqueue("b", 3));
        let retry = queue.take().unwrap();
        assert!(!queue.complete(&retry, false));
    }

    #[test]
    fn stale_snapshot_cannot_overtake_a_newer_selection() {
        let mut queue = PersistQueue::default();
        assert!(queue.enqueue("workspace-b", 2));

        // Snapshot A began first but finished serializing after snapshot B.
        assert!(!queue.enqueue("workspace-a", 1));

        let request = queue.take().unwrap();
        assert_eq!(request.workspace_id, "workspace-b");
        queue.complete(&request, true);
        assert!(queue.take().is_none());
        assert_eq!(queue.last_persisted.as_deref(), Some("workspace-b"));
    }

    #[test]
    fn worker_abort_leaves_the_queue_able_to_start_a_new_worker() {
        let mut queue = PersistQueue::default();
        assert!(queue.enqueue("a", 1));
        let _in_flight = queue.take().unwrap();

        // The worker died mid-write (this is what WorkerExitGuard runs).
        queue.reset_after_worker_abort();
        assert!(!queue.worker_running);

        // The interrupted selection is not "already in flight": the next
        // state emit re-enqueues it and starts a fresh worker.
        assert!(queue.enqueue("a", 2));
        let retry = queue.take().unwrap();
        assert_eq!(retry.workspace_id, "a");
        queue.complete(&retry, true);
        assert!(queue.take().is_none());
        assert_eq!(queue.last_persisted.as_deref(), Some("a"));
    }

    /// A selection scheduled while the panicked worker was still unwinding
    /// (so `worker_running` declined to start a replacement) must survive
    /// the abort cleanup: the next emit — even a duplicate that would
    /// normally coalesce to a no-op — starts a fresh worker that drains it.
    #[test]
    fn selection_enqueued_during_worker_unwind_survives_the_abort() {
        let mut queue = PersistQueue::default();
        assert!(queue.enqueue("a", 1));
        let _died = queue.take().unwrap();

        assert!(!queue.enqueue("b", 2));
        queue.reset_after_worker_abort();

        assert!(queue.enqueue("b", 3));
        let survivor = queue.take().unwrap();
        assert_eq!(survivor.workspace_id, "b");
        queue.complete(&survivor, true);
        assert!(queue.take().is_none());
        assert_eq!(queue.last_persisted.as_deref(), Some("b"));
    }

    /// `flush_latest` also recovers what an aborted worker left behind:
    /// both a selection that arrived during the unwind and the request the
    /// dead worker had taken but never persisted.
    #[test]
    fn flush_persists_selections_stranded_by_a_worker_abort() {
        let mut queue = PersistQueue::default();
        assert!(queue.enqueue("a", 1));
        let _died = queue.take().unwrap();
        assert!(!queue.enqueue("b", 2));
        queue.reset_after_worker_abort();

        let flush = queue.take_latest_for_flush().unwrap();
        assert_eq!(flush.workspace_id, "b");
        queue.complete(&flush, true);
        assert!(queue.take_latest_for_flush().is_none());

        let mut queue = PersistQueue::default();
        assert!(queue.enqueue("c", 1));
        let _died = queue.take().unwrap();
        queue.reset_after_worker_abort();

        let flush = queue.take_latest_for_flush().unwrap();
        assert_eq!(flush.workspace_id, "c");
        queue.complete(&flush, true);
        assert!(queue.take_latest_for_flush().is_none());
    }

    /// End to end: a worker panic that poisoned `WRITE_LOCK` and left
    /// `worker_running` set must not disable persistence — a later
    /// `schedule` starts a new worker and the value reaches SQLite, and
    /// `flush_latest` keeps working too.
    #[test]
    fn persistence_survives_a_poisoned_write_lock_and_a_leaked_worker_flag() {
        // Poison WRITE_LOCK exactly the way a worker panicking mid-write
        // would.
        let _ = std::thread::spawn(|| {
            let _guard = WRITE_LOCK.lock().unwrap();
            panic!("simulated worker panic while holding the write lock");
        })
        .join();
        assert!(WRITE_LOCK.lock().is_err(), "lock should really be poisoned");

        // Simulate the stranded bookkeeping of the dead worker, then the
        // exit-guard cleanup that now runs on unwind.
        lock_queue().worker_running = true;
        lock_queue().reset_after_worker_abort();

        let app = tauri::test::mock_app();
        app.manage(crate::database::DatabaseStore::new_in_memory());

        // High revision so this test cannot race an unrelated user of the
        // global queue.
        schedule(app.handle(), "workspace-after-panic", u64::MAX);

        let db = app.state::<crate::database::DatabaseStore>();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if db.get_ui_state("active_workspace").as_deref()
                == Some("workspace-after-panic")
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "worker never persisted after the poisoned lock"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        assert_eq!(flush_latest(app.handle()), Ok(()));
    }

    #[test]
    fn authoritative_selection_overrides_a_stale_layout_on_restart() {
        let store = crate::state::AppStateStore::default();
        let mut snapshot = store.snapshot();
        let layout_selection = snapshot.active_workspace_id.clone();
        let mut latest_workspace = snapshot.workspaces[0].clone();
        latest_workspace.workspace_id = crate::state::WorkspaceId("workspace-c".into());
        snapshot.workspaces.push(latest_workspace);

        assert!(apply_restored_selection(
            &mut snapshot,
            Some("workspace-c")
        ));
        assert_eq!(snapshot.active_workspace_id.0, "workspace-c");
        assert_ne!(snapshot.active_workspace_id, layout_selection);

        assert!(!apply_restored_selection(
            &mut snapshot,
            Some("removed-workspace")
        ));
        assert_eq!(snapshot.active_workspace_id.0, "workspace-c");
    }
}
