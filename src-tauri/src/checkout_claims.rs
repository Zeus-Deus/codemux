//! Claims on checkouts by work that app state no longer shows: a closing
//! workspace until its chats confirm they stopped, and each chat's stop.
//! Every deletion of a checkout's files consults them, explicit ones and
//! worktree cleanup alike, so a checkout never looks unused while an agent
//! may still run in it. A claim whose stop failed stays until restart, which
//! ends every session Codemux started.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, MutexGuard};

use tokio::sync::watch;

use crate::state::WorkspaceSnapshot;

type Outcome = Option<Result<(), String>>;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Key {
    /// A closing workspace.
    Close(u64),
    /// A chat's stop, by thread id.
    Stop(String),
}

struct Claim {
    /// The workspace a chat's session was recorded under. `None` for a
    /// closing workspace, or when unknown.
    workspace_id: Option<String>,
    paths: Vec<PathBuf>,
    result: watch::Receiver<Outcome>,
}

impl Claim {
    fn within(&self, root: &Path) -> bool {
        self.paths.iter().any(|path| crate::workspace_paths::path_within(path, root))
    }
}

static CLAIMS: LazyLock<Mutex<HashMap<Key, Claim>>> = LazyLock::new(Default::default);

fn claims() -> MutexGuard<'static, HashMap<Key, Claim>> {
    CLAIMS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Outcomes of stops, for [`wait_for`].
#[derive(Clone, Default)]
pub struct Stops(Vec<watch::Receiver<Outcome>>);

impl Stops {
    pub(crate) fn extend(&mut self, other: Stops) {
        self.0.extend(other.0);
    }
}

/// Wait for `stops` to finish. Errs when any of them failed.
pub(crate) async fn wait_for(stops: Stops) -> Result<(), String> {
    let mut first_error = None;
    for mut result in stops.0 {
        let outcome = match result.wait_for(Option::is_some).await {
            Ok(outcome) => outcome.clone().unwrap_or(Ok(())),
            Err(_) => Err("agent shutdown ended without reporting".to_string()),
        };
        if let Err(error) = outcome {
            first_error.get_or_insert(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

/// A closing workspace's claim on its folders. Take it before the workspace
/// leaves app state: from then on nothing else shows they are in use.
/// Deletion waits on it until [`CloseClaim::stopped`], cleanup until it
/// drops. One that fails, or drops before either, stays until restart.
pub(crate) struct CloseClaim {
    id: u64,
    done: watch::Sender<Outcome>,
}

/// A remote-only workspace claims no folder here: its paths are its host's.
pub(crate) fn claim_checkout(ws: &WorkspaceSnapshot) -> CloseClaim {
    let mut paths = Vec::new();
    if !ws.is_remote_only() {
        paths.push(PathBuf::from(&ws.cwd));
        paths.extend(ws.worktree_path.as_ref().map(PathBuf::from));
    }
    claim_paths(paths)
}

fn claim_paths(paths: Vec<PathBuf>) -> CloseClaim {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let (done, result) = watch::channel(None);
    claims().insert(Key::Close(id), Claim { workspace_id: None, paths, result });
    CloseClaim { id, done }
}

impl CloseClaim {
    /// The workspace's chats stopped.
    pub(crate) fn stopped(&self) {
        self.done.send_if_modified(|outcome| {
            let unset = outcome.is_none();
            if unset {
                *outcome = Some(Ok(()));
            }
            unset
        });
    }

    /// A chat did not stop: the folders stay claimed until restart.
    pub(crate) fn failed(self, error: String) {
        self.done.send_replace(Some(Err(error)));
    }

    /// The workspace never left app state, so nothing ran unclaimed.
    pub(crate) fn release(self) {
        self.stopped();
    }
}

impl Drop for CloseClaim {
    fn drop(&mut self) {
        // Dropped before its chats confirmed they stopped, by a panic or a
        // cancelled close: they may still run, so the folders stay claimed.
        self.done.send_if_modified(|outcome| {
            let unset = outcome.is_none();
            if unset {
                *outcome = Some(Err("closing the workspace ended before its agents stopped".into()));
            }
            unset
        });
        if self.done.borrow().as_ref().is_some_and(Result::is_ok) {
            claims().remove(&Key::Close(self.id));
        }
    }
}

/// Stops about to start, each claimed in its session's folder. Settle each
/// with [`StopBatch::settle`]; one still unsettled when the batch drops
/// failed.
pub(crate) struct StopBatch(HashMap<String, watch::Sender<Outcome>>);

/// Claim the stops of `sessions` (thread id, recorded workspace id and
/// folder), replacing an earlier claim of the same thread. Register before
/// the stop starts, so a deletion that begins right after still finds it.
pub(crate) fn begin_stops(sessions: Vec<(String, Option<String>, Option<String>)>) -> (StopBatch, Stops) {
    let mut batch = HashMap::new();
    let mut stops = Stops::default();
    let mut claims = claims();
    for (thread_id, workspace_id, cwd) in sessions {
        let (done, result) = watch::channel(None);
        stops.0.push(result.clone());
        let paths = cwd.map(PathBuf::from).into_iter().collect();
        claims.insert(Key::Stop(thread_id.clone()), Claim { workspace_id, paths, result });
        batch.insert(thread_id, done);
    }
    (StopBatch(batch), stops)
}

impl StopBatch {
    pub(crate) fn settle(&mut self, thread_id: &str, outcome: Result<(), String>) {
        let Some(done) = self.0.remove(thread_id) else { return };
        let stopped = outcome.is_ok();
        done.send_replace(Some(outcome));
        let key = Key::Stop(thread_id.to_string());
        let mut claims = claims();
        let Some(claim) = claims.get(&key).filter(|claim| claim.result.same_channel(&done.subscribe())) else {
            return;
        };
        // A failure nothing can match to a checkout cannot keep one.
        if stopped || (claim.paths.is_empty() && claim.workspace_id.is_none()) {
            claims.remove(&key);
        }
    }
}

impl Drop for StopBatch {
    fn drop(&mut self) {
        let unsettled: Vec<String> = self.0.keys().cloned().collect();
        for thread_id in unsettled {
            self.settle(&thread_id, Err("agent shutdown ended without reporting".into()));
        }
    }
}

/// Stops of sessions recorded under `workspace_id`, in flight or failed,
/// and stops in flight of sessions whose workspace is unknown. Take this
/// as soon as the workspace is closed.
pub(crate) fn stops_of_workspace(workspace_id: &str) -> Stops {
    Stops(
        claims()
            .iter()
            .filter(|(key, _)| matches!(key, Key::Stop(_)))
            .filter(|(_, claim)| match claim.workspace_id.as_deref() {
                Some(id) => id == workspace_id,
                None => claim.result.borrow().is_none(),
            })
            .map(|(_, claim)| claim.result.clone())
            .collect(),
    )
}

/// Claims on folders inside `root`, except the deleting workspace's own,
/// for a deletion to wait on before its final check.
pub(crate) fn claims_in(root: &Path, own: Option<&CloseClaim>) -> Stops {
    Stops(
        claims()
            .iter()
            .filter(|(key, claim)| !is_own(key, own) && claim.within(root))
            .map(|(_, claim)| claim.result.clone())
            .collect(),
    )
}

fn is_own(key: &Key, own: Option<&CloseClaim>) -> bool {
    own.is_some_and(|own| *key == Key::Close(own.id))
}

const UNCONFIRMED: &str = "An agent that worked in this checkout did not confirm it stopped, so its \
                           files stay until Codemux restarts. No files were deleted.";

/// Refuses deleting the files at `root` while an agent that worked inside
/// them never confirmed it stopped.
pub(crate) fn refuse_failed_in(root: &Path) -> Result<(), String> {
    let failed = claims()
        .values()
        .any(|claim| claim.within(root) && claim.result.borrow().as_ref().is_some_and(Result::is_err));
    if failed {
        return Err(UNCONFIRMED.to_string());
    }
    Ok(())
}

/// [`refuse_failed_in`], and also while something inside `root`, other
/// than the deleting workspace's own claim, is still stopping.
pub(crate) fn refuse_in(root: &Path, own: Option<&CloseClaim>) -> Result<(), String> {
    refuse_failed_in(root)?;
    let stopping = claims()
        .iter()
        .any(|(key, claim)| !is_own(key, own) && claim.within(root) && claim.result.borrow().is_none());
    if stopping {
        return Err("An agent that worked in this checkout is still stopping. Try again in a \
                    moment; no files were deleted."
            .to_string());
    }
    Ok(())
}

/// Whether anything claims a folder inside `root`, settled or not.
pub(crate) fn held_in(root: &Path) -> bool {
    claims().values().any(|claim| claim.within(root))
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    /// Claim a stop of `thread_id` in `workspace_id` with no folder, settled
    /// through the returned sender.
    pub(crate) fn claim_stop(thread_id: &str, workspace_id: &str) -> watch::Sender<Outcome> {
        let (done, result) = watch::channel(None);
        let claim = Claim { workspace_id: Some(workspace_id.into()), paths: Vec::new(), result };
        claims().insert(Key::Stop(thread_id.into()), claim);
        done
    }

    pub(crate) fn forget_stop(thread_id: &str) {
        claims().remove(&Key::Stop(thread_id.into()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_close_claim_holds_its_folders_until_it_drops_unless_it_failed() {
        let dir = tempfile::tempdir().unwrap();
        let (released, kept) = (dir.path().join("a"), dir.path().join("b"));
        let claim = claim_paths(vec![released.join("src")]);
        assert!(held_in(&released), "a folder inside counts");
        claim.stopped();
        assert!(held_in(&released), "cleanup waits until it drops");
        assert!(refuse_in(&released, None).is_ok(), "deletion does not");
        drop(claim);
        assert!(!held_in(&released));

        claim_paths(vec![kept.clone()]).failed("did not stop".into());
        assert!(held_in(&kept));
        assert!(refuse_failed_in(&kept).unwrap_err().contains("did not confirm"));
        assert!(!held_in(&dir.path().join("bc")), "only folders inside");

        let never_detached = dir.path().join("c");
        claim_paths(vec![never_detached.clone()]).release();
        assert!(!held_in(&never_detached), "a close that removed nothing keeps nothing");
    }

    /// A close that unwinds after its workspace left app state, before its
    /// chats confirmed they stopped: they may still run.
    #[tokio::test]
    async fn a_close_claim_dropped_unsettled_by_a_panic_keeps_its_folders() {
        let dir = tempfile::tempdir().unwrap();
        let claim = claim_paths(vec![dir.path().to_path_buf()]);
        let pending_delete = claims_in(dir.path(), None);
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _claim = claim;
            panic!("after detach, before the chats were told to stop");
        }));
        assert!(panicked.is_err());
        assert!(wait_for(pending_delete).await.is_err(), "a waiting deletion is refused");
        assert!(refuse_in(dir.path(), None).unwrap_err().contains("did not confirm"));
        assert!(held_in(dir.path()), "cleanup keeps it");
    }

    #[tokio::test]
    async fn a_close_claim_dropped_unsettled_by_cancellation_keeps_its_folders() {
        let dir = tempfile::tempdir().unwrap();
        let claim = claim_paths(vec![dir.path().to_path_buf()]);
        let pending_delete = claims_in(dir.path(), None);
        let (entered, reached) = tokio::sync::oneshot::channel();
        let close = tokio::spawn(async move {
            let _claim = claim;
            entered.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        reached.await.unwrap();
        close.abort();
        assert!(close.await.unwrap_err().is_cancelled());
        assert!(wait_for(pending_delete).await.is_err(), "a waiting deletion is refused");
        assert!(refuse_in(dir.path(), None).is_err());
        assert!(held_in(dir.path()), "cleanup keeps it");
    }

    #[tokio::test]
    async fn deletion_waits_for_other_claims_and_skips_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let own = claim_paths(vec![dir.path().to_path_buf()]);
        let other = claim_paths(vec![dir.path().join("child")]);
        assert!(refuse_in(dir.path(), Some(&own)).unwrap_err().contains("still stopping"));
        let waiting = claims_in(dir.path(), Some(&own));
        assert_eq!(waiting.0.len(), 1);
        other.stopped();
        wait_for(waiting).await.unwrap();
        assert!(refuse_in(dir.path(), Some(&own)).is_ok());
        assert!(refuse_in(dir.path(), None).is_err(), "only the deleting workspace's own claim is skipped");
    }

    #[tokio::test]
    async fn a_failed_stop_stays_claimed_until_the_thread_stops_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = Some(dir.path().display().to_string());
        let (thread, workspace) = ("claims-test-thread", "claims-test-workspace");
        let session = || vec![(thread.to_string(), Some(workspace.to_string()), cwd.clone())];
        let (mut batch, stops) = begin_stops(session());
        assert!(refuse_in(dir.path(), None).unwrap_err().contains("still stopping"));
        batch.settle(thread, Err("no".into()));
        assert_eq!(wait_for(stops).await.unwrap_err(), "no");
        assert!(refuse_failed_in(dir.path()).is_err());
        assert_eq!(stops_of_workspace(workspace).0.len(), 1, "the workspace's close sees it");

        let (mut batch, stops) = begin_stops(session());
        batch.settle(thread, Ok(()));
        wait_for(stops).await.unwrap();
        assert!(!held_in(dir.path()));

        // Dropped unsettled: it failed, but with no folder or workspace
        // nothing could match it, so it is not kept.
        let (batch, stops) = begin_stops(vec![("claims-test-unknown".into(), None, None)]);
        drop(batch);
        assert!(wait_for(stops).await.is_err());
        assert!(!claims().contains_key(&Key::Stop("claims-test-unknown".into())));
    }
}
