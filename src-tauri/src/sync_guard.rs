//! Keeping an account sync run on the account it started for, and within
//! its time.
//!
//! A run resolves the signed-in session once. After a sign-out or account
//! switch it must not send anything more with that session's token, nor
//! write what the old account answered into the new account's data — the
//! same contract `hosts_sync` keeps with its own session type.

use std::time::{Duration, Instant};

use crate::auth::AuthState;

/// How long one sync pass may keep starting requests. Requests are never
/// cut off part-way by it — each has its own timeout (see [`sync_client`]) —
/// so a pass holds its registry's queue for at most this plus one request.
pub(crate) const SYNC_BUDGET: Duration = Duration::from_secs(120);

/// An HTTP client whose every request gives up on its own: connecting
/// within 10 seconds, finishing within 30.
pub(crate) fn sync_client() -> reqwest::Client {
    sync_client_with(Duration::from_secs(30))
}

pub(crate) fn sync_client_with(timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10).min(timeout))
        .timeout(timeout)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// The session a sync run works for, and when it must stop starting
/// requests.
pub(crate) struct SyncGuard<'a> {
    /// The signed-in session at a generation, or `None` for runs with no
    /// session to follow: tests, and the host-side scheduler, which runs in
    /// its own process with a fixed token.
    session: Option<(&'a AuthState, u64)>,
    deadline: Option<Instant>,
}

impl<'a> SyncGuard<'a> {
    pub(crate) fn session(auth: &'a AuthState, generation: u64) -> Self {
        Self { session: Some((auth, generation)), deadline: None }
    }

    pub(crate) fn unchecked() -> Self {
        Self { session: None, deadline: None }
    }

    /// Stop starting requests once `budget` from now has passed.
    pub(crate) fn within(self, budget: Duration) -> Self {
        Self { deadline: Some(Instant::now() + budget), ..self }
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
        self.deadline.is_some_and(|deadline| Instant::now() >= deadline)
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
pub(crate) type Attempts = std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<i64, Instant>>>;

/// Sort `rows` (by id) so the least recently attempted come first, never
/// attempted before all. Entries for rows no longer waiting are dropped, so
/// the record stays as small as the backlog.
pub(crate) fn order_by_last_attempt<T>(attempts: &Attempts, rows: &mut [T], id: impl Fn(&T) -> i64) {
    let mut attempts = attempts.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
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
        Self { state: std::sync::Mutex::new((false, false)) }
    }

    /// A pass is wanted. `true`: the caller starts the runner; `false`: one
    /// is running and will make a follow-up pass.
    pub(crate) fn request(&self) -> bool {
        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
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
        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
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
    use super::Coalescer;

    #[test]
    fn requests_during_a_pass_collapse_into_one_follow_up() {
        let background = Coalescer::new();
        assert!(background.request(), "the first request starts the runner");
        for _ in 0..5 {
            assert!(!background.request(), "a running pass absorbs further requests");
        }
        assert!(background.finished_pass(), "they get exactly one follow-up pass");
        assert!(!background.finished_pass(), "and then the runner stops");
        assert!(background.request(), "the next request starts a new runner");
        assert!(!background.finished_pass());
    }
}
