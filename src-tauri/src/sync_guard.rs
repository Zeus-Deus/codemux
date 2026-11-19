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

    /// Checked before each request: a replaced session's token sends
    /// nothing more, and a pass past its deadline starts nothing new.
    pub(crate) fn ensure_current(&self) -> Result<(), String> {
        if self.deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Err("sync ran out of time; the rest waits for the next pass".to_string());
        }
        self.commit(|| ())
    }
}
