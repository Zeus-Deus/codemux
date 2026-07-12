//! Account-mode admission (Stage A).
//!
//! A second way to mint a `web_remote_sessions` row, alongside the pairing
//! token (`auth.rs`): a browser on a reachable endpoint (LAN / the user's own
//! Tailscale) proves it owns the **same** Codemux account the desktop is signed
//! into, and gets a session — no QR dance, no pairing code.
//!
//! Why the desktop proxies the sign-in. `api.codemux.org`'s CORS allowlist is
//! fixed and does **not** include the dynamic web-remote origin
//! `http://<desktop-ip>:4377`, so the browser cannot call the auth API
//! directly. Instead the browser derives the `codemux-api-*` **AuthSecret**
//! client-side (identical algorithm to `auth::derivation`, so the raw password
//! never leaves the browser) and sends `{email, auth_secret}` to the desktop's
//! own same-origin `POST /api/pair-account`. The desktop — which already talks
//! to `api.codemux.org` server-side with no CORS constraint — forwards the
//! secret to `/api/auth/desktop/signin`, resolves the presented credentials to
//! a `user.id`, and admits a session **iff** that id equals the desktop's own
//! signed-in user (`load_cached_user`). The raw password only ever exists in
//! the browser; the AuthSecret only ever travels browser → the user's own
//! desktop.
//!
//! This module is the pure verify-and-mint core; the HTTP surface (origin
//! check, rate limit, request parsing, and the error → status mapping) lives in
//! [`super::server`].

use std::sync::Arc;

use super::Shared;
use crate::database::DatabaseStore;

/// Why an account-pairing attempt was refused. [`super::server`] maps each
/// variant onto an HTTP response; kept free of axum types so the core logic
/// stays unit-testable without a running server.
#[derive(Debug)]
pub(crate) enum AccountPairError {
    /// The account-mode master toggle is off (`account_mode_enabled = false`).
    /// → 403. Account compromise must never reach a device that never opted in.
    Disabled,
    /// The desktop is not signed into any Codemux account, so there is no
    /// "same account" to verify against. → 403 with a clear, actionable message.
    DesktopSignedOut,
    /// The presented credentials did not authenticate against `api.codemux.org`
    /// (bad password / unverified email / network error). → 401, non-enumerating
    /// (never reveals whether the email exists).
    AuthFailed,
    /// The credentials authenticated, but to a *different* account than the one
    /// the desktop is signed into. → 403.
    Mismatch,
    /// An internal failure minting the session row. → 500.
    Internal(String),
}

/// A freshly minted account session, ready to return to the browser exactly
/// like `/api/pair` does (`{session_id, session_token, approved}` + cookie). The
/// verified account user id is persisted on the session row itself
/// (`source = "account"`, `account_user_id`), so it isn't repeated here.
pub(crate) struct AccountPairOutcome {
    pub session_id: String,
    /// Plaintext bearer token — returned to the browser once, only its SHA-256
    /// is persisted.
    pub session_token: String,
    /// Whether the session is admitted immediately or starts pending approval.
    pub approved: bool,
}

// Redact the plaintext session token so a test-failure `{:?}` (or any stray
// log) can't leak it, mirroring the `AuthSecret` Debug discipline.
impl std::fmt::Debug for AccountPairOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountPairOutcome")
            .field("session_id", &self.session_id)
            .field("session_token", &"***")
            .field("approved", &self.approved)
            .finish()
    }
}

/// Verify the browser owns the same account the desktop is signed into, then
/// mint an account-tagged `web_remote_sessions` row.
///
/// `auth_secret` is the client-derived `codemux-api-*` AuthSecret (base64), not
/// a raw password — the browser stretched it locally. The desktop forwards it
/// to `api.codemux.org/api/auth/desktop/signin` (server-side, no CORS), reads
/// the resolved `user.id`, and admits only when it matches
/// [`crate::auth::load_cached_user`].
pub(crate) async fn verify_and_mint(
    db: &DatabaseStore,
    shared: &Arc<Shared>,
    email: &str,
    auth_secret: String,
    device_name: Option<String>,
    user_agent: Option<String>,
) -> Result<AccountPairOutcome, AccountPairError> {
    // Snapshot the config values up front so the non-Send `std::sync::Mutex`
    // guard is never held across the `.await` below.
    let (account_mode_enabled, approved) = {
        let cfg = shared.config.lock().unwrap();
        (
            cfg.account_mode_enabled,
            super::account_session_approved(&cfg),
        )
    };

    // 1. Master toggle. Off → refuse before touching the account at all.
    if !account_mode_enabled {
        return Err(AccountPairError::Disabled);
    }

    // 2. The desktop must be signed in — account mode verifies "same account"
    //    against the desktop's own cached user, and there's nothing to compare
    //    to when it's signed out.
    let cached = crate::auth::load_cached_user(db).ok_or(AccountPairError::DesktopSignedOut)?;

    // 3. Server-side sign-in with the client-derived secret. The typed
    //    `AuthSecret` boundary is preserved: a derived secret, never a raw
    //    password, reaches the API.
    let secret = crate::auth::AuthSecret::from_web_remote_derived(auth_secret);
    let resolved = crate::auth::login_email_api(email, &secret)
        .await
        .map_err(|_| AccountPairError::AuthFailed)?;

    // 4. Same-account gate. Credentials for a *different* account get nothing —
    //    a browser reaches only devices under its own account.
    if resolved.user.id != cached.id {
        return Err(AccountPairError::Mismatch);
    }

    // 5. Mint the session, tagged `source = "account"` + the verified user id,
    //    returning the plaintext token once (only its hash is stored).
    let session_id = uuid::Uuid::new_v4().to_string();
    let session_token = super::auth::random_token();
    let token_hash = super::auth::sha256_hex(&session_token);
    let name = device_name.filter(|s| !s.trim().is_empty());
    db.web_remote_insert_account_session(
        &session_id,
        name.as_deref(),
        user_agent.as_deref(),
        &token_hash,
        approved,
        &cached.id,
    )
    .map_err(AccountPairError::Internal)?;

    Ok(AccountPairOutcome {
        session_id,
        session_token,
        approved,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{save_auth, AuthUser};
    use crate::database::init_test_database;
    use crate::web_remote::auth::{authenticate, sha256_hex};

    // Every test here mocks `api.codemux.org` via `mockito` + `CODEMUX_API_URL`
    // and NEVER hits the real API. The `mockserver` serial group serializes the
    // env-var mutation with the auth-api tests that share the same technique.

    fn seed_cached_user(db: &DatabaseStore, id: &str) {
        let user = AuthUser {
            id: id.to_string(),
            email: "desktop@example.com".to_string(),
            name: Some("Desktop User".to_string()),
            image: None,
        };
        save_auth(db, "desktop-token", "2099-01-01T00:00:00Z", Some(&user)).unwrap();
    }

    fn shared_with_account_mode(enabled: bool, trust: bool) -> Arc<Shared> {
        let shared = Arc::new(Shared::default());
        {
            let mut cfg = shared.config.lock().unwrap();
            cfg.enabled = true;
            cfg.account_mode_enabled = enabled;
            cfg.trust_account_browsers = trust;
            // Pairing-path approval is off; account sessions must still pend
            // unless `trust_account_browsers` is set — that's the whole point.
            cfg.require_approval = false;
        }
        shared
    }

    /// Mock `POST /api/auth/desktop/signin` to succeed and resolve to `user_id`.
    async fn mock_signin_ok(server: &mut mockito::Server, user_id: &str) -> mockito::Mock {
        server
            .mock("POST", "/api/auth/desktop/signin")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::json!({
                    "token": "resolved-token",
                    "expiresAt": "2099-01-01T00:00:00Z",
                    "user": { "id": user_id, "email": "x@example.com", "name": null, "image": null }
                })
                .to_string(),
            )
            .create_async()
            .await
    }

    #[tokio::test]
    #[serial_test::serial(mockserver)]
    async fn mints_session_on_same_user() {
        let db = init_test_database();
        seed_cached_user(&db, "user-match-1");
        let shared = shared_with_account_mode(true, false);

        let mut server = mockito::Server::new_async().await;
        let mock = mock_signin_ok(&mut server, "user-match-1").await;
        let url = server.url();

        let outcome = run_verify(&url, &db, &shared, "browser@example.com")
            .await
            .expect("same-account pairing mints a session");

        mock.assert_async().await;
        // Pending by default (approval on for account sessions).
        assert!(!outcome.approved, "account session pends until approved");

        // The row is tagged as an account session, records the verified account
        // user id (equal to the desktop's own), and its token authenticates.
        let row = db.web_remote_get_session(&outcome.session_id).unwrap();
        assert_eq!(row.source, "account");
        assert_eq!(row.account_user_id.as_deref(), Some("user-match-1"));
        assert_eq!(row.token_hash, sha256_hex(&outcome.session_token));
        assert!(
            authenticate(&db, &outcome.session_token).is_some(),
            "minted token authenticates like any session"
        );
    }

    #[tokio::test]
    #[serial_test::serial(mockserver)]
    async fn trusted_account_browser_is_approved_immediately() {
        let db = init_test_database();
        seed_cached_user(&db, "user-trust");
        let shared = shared_with_account_mode(true, true); // trust opt-out ON

        let mut server = mockito::Server::new_async().await;
        let _mock = mock_signin_ok(&mut server, "user-trust").await;
        let url = server.url();

        let outcome = run_verify(&url, &db, &shared, "user-trust-browser@example.com").await;
        let outcome = outcome.expect("mints");
        assert!(
            outcome.approved,
            "trust_account_browsers admits account sessions immediately"
        );
    }

    #[tokio::test]
    #[serial_test::serial(mockserver)]
    async fn rejects_on_user_mismatch() {
        let db = init_test_database();
        seed_cached_user(&db, "desktop-owner");
        let shared = shared_with_account_mode(true, false);

        let mut server = mockito::Server::new_async().await;
        // Credentials authenticate, but to a DIFFERENT account.
        let _mock = mock_signin_ok(&mut server, "some-other-account").await;
        let url = server.url();

        let err = run_verify(&url, &db, &shared, "intruder@example.com")
            .await
            .expect_err("a different account must be rejected");
        assert!(matches!(err, AccountPairError::Mismatch));
        // No session was minted for the mismatched account.
        assert!(
            db.web_remote_list_sessions().is_empty(),
            "no session row on mismatch"
        );
    }

    #[tokio::test]
    #[serial_test::serial(mockserver)]
    async fn refuses_when_account_mode_disabled() {
        let db = init_test_database();
        seed_cached_user(&db, "user-x");
        let shared = shared_with_account_mode(false, false); // account mode OFF

        // No mock needed — the toggle short-circuits before any network call.
        let err = verify_and_mint(&db, &shared, "x@example.com", "derived".into(), None, None)
            .await
            .expect_err("account mode off must refuse");
        assert!(matches!(err, AccountPairError::Disabled));
        assert!(db.web_remote_list_sessions().is_empty());
    }

    #[tokio::test]
    #[serial_test::serial(mockserver)]
    async fn errors_when_desktop_signed_out() {
        let db = init_test_database(); // no cached user
        let shared = shared_with_account_mode(true, false);

        let err = verify_and_mint(&db, &shared, "x@example.com", "derived".into(), None, None)
            .await
            .expect_err("account mode needs the desktop signed in");
        assert!(matches!(err, AccountPairError::DesktopSignedOut));
    }

    #[tokio::test]
    #[serial_test::serial(mockserver)]
    async fn auth_failure_maps_to_auth_failed() {
        let db = init_test_database();
        seed_cached_user(&db, "user-y");
        let shared = shared_with_account_mode(true, false);

        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("POST", "/api/auth/desktop/signin")
            .with_status(401)
            .with_header("content-type", "application/json")
            .with_body(r#"{"error":"Invalid email or password"}"#)
            .create_async()
            .await;
        let url = server.url();

        let err = run_verify(&url, &db, &shared, "wrongpass@example.com")
            .await
            .expect_err("bad credentials must fail");
        assert!(matches!(err, AccountPairError::AuthFailed));
        assert!(db.web_remote_list_sessions().is_empty());
    }

    // Small async env-var helper. `login_email_api` reads `CODEMUX_API_URL`
    // synchronously at call time, so setting it immediately before the await
    // (all within one serialized test) is sufficient; we restore it after.
    async fn run_verify(
        api_url: &str,
        db: &DatabaseStore,
        shared: &Arc<Shared>,
        email: &str,
    ) -> Result<AccountPairOutcome, AccountPairError> {
        let prev = std::env::var("CODEMUX_API_URL").ok();
        std::env::set_var("CODEMUX_API_URL", api_url);
        let out = verify_and_mint(db, shared, email, "derived-secret".into(), None, None).await;
        match prev {
            Some(v) => std::env::set_var("CODEMUX_API_URL", v),
            None => std::env::remove_var("CODEMUX_API_URL"),
        }
        out
    }
}
