//! Bound the time spent syncing linked rows and keep each signed-in pass
//! tied to the session it started with.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use crate::auth::{api_base_url, is_token_expired, load_token, AuthState};
use crate::database::DatabaseStore;

const LINKED_BUDGET: Duration = Duration::from_secs(60);
const LINKED_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

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
    pub(crate) fn linked(token: &'a str, guard: SyncGuard<'a>) -> Result<Self, String> {
        let base = api_base_url()?;
        let client = crate::auth::api_client(&base)?;
        Ok(Self {
            token,
            base,
            guard: guard.within(LINKED_BUDGET),
            client,
        })
    }

    pub(crate) fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.client
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(self.token)
            .timeout(LINKED_REQUEST_TIMEOUT)
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

/// When each row became eligible or was last sent, so a pass can try first
/// the rows that have waited longest.
pub(crate) type Attempts =
    std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<i64, Instant>>>;

/// Sort oldest first, counting from admission for new work and from the last
/// attempt for retries. Entries for rows no longer waiting are dropped.
pub(crate) fn order_by_last_attempt<T>(attempts: &Attempts, rows: &mut [T], id: impl Fn(&T) -> i64) {
    let mut attempts = attempts
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let waiting: HashSet<i64> = rows.iter().map(&id).collect();
    attempts.retain(|row_id, _| waiting.contains(row_id));
    let now = Instant::now();
    for row_id in waiting {
        attempts.entry(row_id).or_insert(now);
    }
    rows.sort_by_key(|row| (attempts[&id(row)], id(row)));
}

/// Record that the row `id` is being sent now.
pub(crate) fn record_attempt(attempts: &Attempts, id: i64) {
    attempts
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(id, Instant::now());
}

#[cfg(test)]
mod tests {
    use super::{order_by_last_attempt, Attempts};
    use std::time::{Duration, Instant};

    #[test]
    fn fresh_rows_do_not_overtake_waiting_retries() {
        let attempts: Attempts = std::sync::LazyLock::new(Default::default);
        let now = Instant::now();
        attempts.lock().unwrap().extend([
            (1, now - Duration::from_secs(1)),
            (2, now - Duration::from_secs(2)),
        ]);

        let mut rows = [3, 2, 1];
        order_by_last_attempt(&attempts, &mut rows, |id| *id);
        assert_eq!(rows, [2, 1, 3]);
    }
}
