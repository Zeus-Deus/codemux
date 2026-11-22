//! Keeping an account sync run on the account it started for, and within
//! its time.
//!
//! A run resolves the signed-in session once. After a sign-out or account
//! switch it must not send anything more with that session's token, nor
//! write what the old account answered into the new account's data — the
//! same contract `hosts_sync` keeps with its own session type.

use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use crate::auth::{api_base_url, is_token_expired, load_token, AuthState};
use crate::database::DatabaseStore;

const LINKED_BUDGET: Duration = Duration::from_secs(60);
const LINKED_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// One first-upload batch per current session. A late receipt is guarded by
/// its original auth generation, so a new session can use its own slot.
pub(crate) struct CreateGate(Mutex<Option<CreateSlot>>);

pub(crate) struct CreatePermit;

struct CreateSlot {
    generation: Option<u64>,
    permit: Weak<CreatePermit>,
}

impl CreateGate {
    pub(crate) const fn new() -> Self {
        Self(Mutex::new(None))
    }

    /// Call under the registry lock. An unscoped, fixed-token caller shares
    /// the existing slot, but cannot displace it.
    pub(crate) fn try_acquire(&self, generation: Option<u64>) -> Option<Arc<CreatePermit>> {
        let mut slot = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(owner) = slot.as_ref() {
            let shares_slot = generation.is_none()
                || owner.generation.is_none()
                || owner.generation == generation;
            if shares_slot && owner.permit.strong_count() != 0 {
                return None;
            }
        }
        let permit = Arc::new(CreatePermit);
        *slot = Some(CreateSlot {
            generation,
            permit: Arc::downgrade(&permit),
        });
        Some(permit)
    }
}

pub(crate) fn signed_in_token(auth: &AuthState, db: &DatabaseStore) -> (u64, Option<String>) {
    auth.session_snapshot(|| {
        let (token, expires_at) = load_token(db)?;
        (!is_token_expired(&expires_at)).then_some(token)
    })
}

pub(crate) struct SyncRequest<'a> {
    token: &'a str,
    base: String,
    guard: SyncGuard<'a>,
    client: reqwest::Client,
}

impl<'a> SyncRequest<'a> {
    pub(crate) fn linked(token: &'a str, guard: SyncGuard<'a>) -> Self {
        Self::new(token, guard.within(LINKED_BUDGET))
    }

    pub(crate) fn new(token: &'a str, guard: SyncGuard<'a>) -> Self {
        Self {
            token,
            base: api_base_url(),
            guard,
            client: reqwest::Client::new(),
        }
    }

    pub(crate) fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let bounded = method != reqwest::Method::POST;
        let request = self
            .client
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(self.token);
        if bounded {
            request.timeout(LINKED_REQUEST_TIMEOUT)
        } else {
            // A first POST can commit before its receipt reaches us. Keep
            // reading the receipt even when it outlasts a linked pass.
            request
        }
    }

    pub(crate) async fn send(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, String> {
        self.guard.ensure_current()?;
        request
            .send()
            .await
            .map_err(|e| format!("Network error: {e}"))
    }

    pub(crate) fn commit<T>(&self, commit: impl FnOnce() -> T) -> Result<T, String> {
        self.guard.commit(commit)
    }

    pub(crate) fn expired(&self) -> bool {
        self.guard.expired()
    }
}

pub(crate) fn combine(pull: Result<(), String>, push: Result<(), String>) -> Result<(), String> {
    match (pull, push) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(p), Ok(())) => Err(format!("pull failed: {p}")),
        (Ok(()), Err(p)) => Err(format!("push failed: {p}")),
        (Err(a), Err(b)) => Err(format!("pull failed: {a}; push failed: {b}")),
    }
}

#[derive(Debug, serde::Deserialize)]
pub(crate) struct CreateReceipt {
    pub(crate) id: String,
}

/// The session a sync run works for, and when it must stop starting
/// requests.
#[derive(Clone, Copy)]
pub(crate) struct SyncGuard<'a> {
    /// The signed-in session at a generation, or `None` for runs with no
    /// session to follow: tests, and the host-side scheduler, which runs in
    /// its own process with a fixed token.
    session: Option<(&'a AuthState, u64)>,
    deadline: Option<Instant>,
}

impl<'a> SyncGuard<'a> {
    pub(crate) fn session(auth: &'a AuthState, generation: u64) -> Self {
        Self {
            session: Some((auth, generation)),
            deadline: None,
        }
    }

    pub(crate) fn unchecked() -> Self {
        Self {
            session: None,
            deadline: None,
        }
    }

    /// Stop starting requests once `budget` from now has passed.
    pub(crate) fn within(self, budget: Duration) -> Self {
        Self {
            deadline: Some(Instant::now() + budget),
            ..self
        }
    }

    /// Run `commit` only while the run's session is still current. Local
    /// writes made from the account's answers, and the read of what to
    /// upload with its token, go through here. An answer already received
    /// is applied even past the deadline: it is real.
    pub(crate) fn commit<T>(&self, commit: impl FnOnce() -> T) -> Result<T, String> {
        match self.session {
            Some((auth, generation)) => auth
                .commit_if_session_current(generation, commit)
                .ok_or_else(|| "the signed-in account changed during sync".to_string()),
            None => Ok(commit()),
        }
    }

    /// Whether the pass is past its deadline and should start nothing new.
    pub(crate) fn expired(&self) -> bool {
        self.deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
    }

    /// Checked before each request: a replaced session's token sends
    /// nothing more, and a pass past its deadline starts nothing new.
    pub(crate) fn ensure_current(&self) -> Result<(), String> {
        if self.expired() {
            return Err("sync ran out of time; the rest waits for the next pass".to_string());
        }
        self.commit(|| ())
    }
}

/// When each row of a registry was last sent, so a pass can try first the
/// rows that waited longest. Rows that keep stalling then cannot keep a
/// healthy row behind them from ever being reached before the budget ends.
pub(crate) type Attempts =
    std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<i64, Instant>>>;

/// Sort `rows` (by id) so the least recently attempted come first, never
/// attempted before all. Entries for rows no longer waiting are dropped, so
/// the record stays as small as the backlog.
pub(crate) fn order_by_last_attempt<T>(
    attempts: &Attempts,
    rows: &mut [T],
    id: impl Fn(&T) -> i64,
) {
    let mut attempts = attempts
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let waiting: std::collections::HashSet<i64> = rows.iter().map(&id).collect();
    attempts.retain(|row_id, _| waiting.contains(row_id));
    rows.sort_by_key(|row| (attempts.get(&id(row)).copied(), id(row)));
}

/// Record that the row `id` is being sent now.
pub(crate) fn record_attempt(attempts: &Attempts, id: i64) {
    attempts
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(id, Instant::now());
}

/// Coalesces fire-and-forget sync requests: one pass runs at a time, and
/// any number of requests made meanwhile collapse into a single follow-up
/// pass, which starts after the last of them — so nothing a request was
/// made for is missed, and nothing piles up.
#[derive(Default)]
pub(crate) struct Coalescer {
    state: std::sync::Mutex<(bool, bool)>,
}

impl Coalescer {
    pub(crate) const fn new() -> Self {
        Self {
            state: std::sync::Mutex::new((false, false)),
        }
    }

    /// A pass is wanted. `true`: the caller starts the runner; `false`: one
    /// is running and will make a follow-up pass.
    pub(crate) fn request(&self) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (running, again) = &mut *state;
        if *running {
            *again = true;
            false
        } else {
            *running = true;
            *again = false;
            true
        }
    }

    /// The runner finished a pass. `true`: run another, for requests made
    /// during it; `false`: the runner is done.
    pub(crate) fn finished_pass(&self) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (running, again) = &mut *state;
        if *again {
            *again = false;
            true
        } else {
            *running = false;
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AuthState, Coalescer, CreateGate, SyncGuard};

    #[test]
    fn requests_during_a_pass_collapse_into_one_follow_up() {
        let background = Coalescer::new();
        assert!(background.request(), "the first request starts the runner");
        for _ in 0..5 {
            assert!(
                !background.request(),
                "a running pass absorbs further requests"
            );
        }
        assert!(
            background.finished_pass(),
            "they get exactly one follow-up pass"
        );
        assert!(!background.finished_pass(), "and then the runner stops");
        assert!(background.request(), "the next request starts a new runner");
        assert!(!background.finished_pass());
    }

    #[test]
    fn a_new_session_replaces_the_create_slot_and_rejects_the_old_commit() {
        let auth = AuthState::default();
        let gate = CreateGate::new();
        let (old_generation, ()) = auth.session_snapshot(|| ());
        let old_guard = SyncGuard::session(&auth, old_generation);
        let old_permit = gate.try_acquire(Some(old_generation)).unwrap();
        assert!(gate.try_acquire(Some(old_generation)).is_none());
        assert!(
            gate.try_acquire(None).is_none(),
            "fixed calls share the slot"
        );

        let (new_generation, ()) = auth.replace_session_with_generation(|| ());
        let new_guard = SyncGuard::session(&auth, new_generation);
        let new_permit = gate.try_acquire(Some(new_generation)).unwrap();
        assert!(old_guard.commit(|| panic!("stale commit")).is_err());
        assert!(new_guard.commit(|| ()).is_ok());
        drop(old_permit);
        assert!(gate.try_acquire(Some(new_generation)).is_none());
        drop(new_permit);
        assert!(gate.try_acquire(Some(new_generation)).is_some());
    }
}
