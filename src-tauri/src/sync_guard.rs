//! Keeping an account sync run on the account it started for.
//!
//! A run resolves the signed-in session once. After a sign-out or account
//! switch it must not send anything more with that session's token, nor
//! write what the old account answered into the new account's data — the
//! same contract `hosts_sync` keeps with its own session type.

use std::time::Duration;

use crate::auth::AuthState;

/// The longest one sync pass may hold its registry's queue. A request that
/// never answers would otherwise block every later sync, "Sync now"
/// included.
pub(crate) const SYNC_BUDGET: Duration = Duration::from_secs(120);

/// An HTTP client whose every request gives up on its own: connecting
/// within 10 seconds, finishing within 30.
pub(crate) fn sync_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// The session a sync run works for.
pub(crate) enum SyncGuard<'a> {
    /// Tied to the signed-in session at `generation`.
    Session { auth: &'a AuthState, generation: u64 },
    /// No session to follow: tests, and the host-side scheduler, which
    /// runs in its own process with a fixed token.
    Unchecked,
}

impl SyncGuard<'_> {
    /// Run `commit` only while the run's session is still current. Local
    /// writes made from the account's answers, and the read of what to
    /// upload with its token, go through here.
    pub(crate) fn commit<T>(&self, commit: impl FnOnce() -> T) -> Result<T, String> {
        match self {
            SyncGuard::Session { auth, generation } => auth
                .commit_if_session_current(*generation, commit)
                .ok_or_else(|| "the signed-in account changed during sync".to_string()),
            SyncGuard::Unchecked => Ok(commit()),
        }
    }

    /// Checked before each request, so a replaced session's token sends
    /// nothing more.
    pub(crate) fn ensure_current(&self) -> Result<(), String> {
        self.commit(|| ())
    }
}
