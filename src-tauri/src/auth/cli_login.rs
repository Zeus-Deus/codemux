//! Headless account sign-in for the `codemux` CLI.
//!
//! Until this existed, signing a machine into a Codemux account was only
//! possible from the desktop GUI (the `signin_email` / `start_oauth_flow`
//! Tauri commands). That made relay-mode a chicken-and-egg problem on a
//! VPS reached over SSH: device registration needs a signed-in account,
//! but signing in needed a display. `codemux login` closes that loop —
//! it runs standalone (no GUI, no running instance, no control socket)
//! and writes **exactly** the state a GUI sign-in writes, so
//! `build_headless_app()`, `codemux serve`, and a later GUI launch all
//! read the session back through the ordinary `load_token` /
//! `load_cached_user` path.
//!
//! Persisted-state parity with `commands::auth::signin_email`:
//!
//! | step                                    | GUI | CLI |
//! |-----------------------------------------|-----|-----|
//! | `derive_auth_secret(password, email)`   | yes | yes |
//! | POST `/api/auth/desktop/signin`         | yes | yes |
//! | `save_auth(db, token, expires, user)`   | yes | yes |
//! | `save_stored_auth_method(db, "email")`  | yes | yes |
//! | `AuthState::set_auth_method` (in-memory)| yes | n/a — no live app |
//! | `auth-state-changed` / `sync-state-changed` events | yes | n/a — no webview |
//!
//! The two GUI-only steps are process-local (a `Mutex<Option<String>>` in
//! the running app and two webview events); neither is persisted, and the
//! persisted `auth_method` written here is what a cold-start `check_auth`
//! restores anyway. So the on-disk result is byte-for-byte the same record.
//!
//! Everything the raw password touches is confined to
//! [`resolve_password`] and [`sign_in`], which immediately stretch it into
//! an [`AuthSecret`](crate::auth::AuthSecret) and drop the original — the
//! typed boundary in `auth/api.rs` makes it a compile error to send the
//! raw value to the server.

use std::io::{BufRead, IsTerminal, Write};

use serde::Deserialize;

use crate::auth::{
    api_base_url, clear_token, derive_auth_secret, is_token_expired, load_cached_user,
    load_stored_auth_method, load_token, login_email_api, save_auth,
    save_stored_auth_method, AuthUser,
};
use crate::database::DatabaseStore;

/// Environment variable accepted in place of an interactive prompt. Meant
/// for container / CI harnesses that have no TTY; always warns, because a
/// password in the environment is visible to anything that can read
/// `/proc/<pid>/environ` and tends to end up in shell history.
pub const PASSWORD_ENV: &str = "CODEMUX_PASSWORD";

// ── Errors ───────────────────────────────────────────────────────

/// Why a CLI sign-in failed. Kept as a small enum (rather than a bare
/// `String`) so the *non-enumerating* mapping is testable: every
/// server-side rejection — wrong password, unknown address, unverified
/// address — collapses into the single [`LoginError::Credentials`]
/// message. Mirroring the API's stance matters because `codemux login`
/// is reachable from any shell, so a chatty error would turn the CLI
/// into an account-existence oracle.
#[derive(Debug)]
pub(crate) enum LoginError {
    /// The API was unreachable / the request never completed.
    Network(String),
    /// The API rejected the credentials. Deliberately carries no detail.
    Credentials,
    /// The API rejected a pasted bearer (`--token`). Like
    /// [`LoginError::Credentials`], carries no server detail — expired and
    /// invalid must be indistinguishable from the outside.
    Token,
    /// Something local went wrong (no TTY, derivation failure, DB write).
    Local(String),
}

impl LoginError {
    pub(crate) fn user_message(&self) -> String {
        match self {
            LoginError::Network(detail) => format!(
                "Could not reach the Codemux API at {base} ({detail}). \
                 Check the network connection, or set CODEMUX_API_URL if \
                 you are pointing at a different server.",
                base = crate::auth::api_base_url(),
            ),
            LoginError::Credentials => "Sign-in failed. Check the email address \
                 and password, and make sure the address has been verified."
                .to_string(),
            LoginError::Token => "That token was not accepted. Copy the session \
                 token again from a machine that is signed in — tokens expire, \
                 and a truncated paste looks the same as an invalid one."
                .to_string(),
            LoginError::Local(detail) => detail.clone(),
        }
    }
}

/// Map an `auth::api` error string onto a [`LoginError`].
///
/// `login_email_api` returns three shapes: `"Request failed: …"` for a
/// transport error, `"Parse response: …"` for a 2xx whose body did not
/// deserialize, and otherwise the server's own `error` field. Only the
/// transport case is safe to echo verbatim; the server's message is not,
/// because Better Auth distinguishes "email not verified" from "invalid
/// credentials" and repeating that would leak whether an account exists.
pub(crate) fn classify_api_error(raw: &str) -> LoginError {
    if let Some(detail) = raw.strip_prefix("Request failed: ") {
        return LoginError::Network(detail.to_string());
    }
    if let Some(detail) = raw.strip_prefix("Parse response: ") {
        return LoginError::Local(format!(
            "The Codemux API returned an unexpected response ({detail})."
        ));
    }
    LoginError::Credentials
}

// ── Sign-in ──────────────────────────────────────────────────────

/// Sign in against `db`, persisting the same record the GUI persists.
///
/// Takes the password by value and drops it as soon as the `AuthSecret`
/// is derived, exactly like `commands::auth::signin_email`.
pub(crate) async fn sign_in(
    db: &DatabaseStore,
    email: &str,
    password: String,
) -> Result<AuthUser, LoginError> {
    let email = email.trim();
    if email.is_empty() || password.is_empty() {
        return Err(LoginError::Local(
            "Email and password are required.".to_string(),
        ));
    }

    // Same zero-knowledge derivation the GUI runs: the server only ever
    // sees the stretched secret. `login_email_api` takes `&AuthSecret`,
    // so the compiler refuses to let the raw password past this line.
    let auth_secret = derive_auth_secret(&password, email)
        .map_err(|e| LoginError::Local(format!("Could not derive the auth secret: {e}")))?;
    drop(password);

    let api_resp = login_email_api(email, &auth_secret)
        .await
        .map_err(|e| classify_api_error(&e))?;

    let user = AuthUser {
        id: api_resp.user.id.clone(),
        email: api_resp.user.email.clone(),
        name: api_resp.user.name.clone(),
        image: api_resp.user.image.clone(),
    };

    save_auth(db, &api_resp.token, &api_resp.expires_at, Some(&user))
        .map_err(|e| LoginError::Local(format!("Could not persist the session: {e}")))?;
    // Matches the GUI ordering: `save_auth` preserves any prior
    // auth_method, so the new value is stamped right after. Non-fatal for
    // the same reason it is in `signin_email` — the session itself is
    // already stored, and `auth_method` only tailors Settings copy.
    if let Err(err) = save_stored_auth_method(db, Some("email")) {
        eprintln!("[codemux login] persisting auth_method=email failed: {err}");
    }

    Ok(user)
}

// ── Bearer-token sign-in ─────────────────────────────────────────
//
// The escape hatch for an account that has no password at all: a
// GitHub-OAuth-only user cannot complete the email/password flow above,
// and the OAuth flow itself needs a browser + a loopback callback, which
// a headless VPS reached over SSH has neither of. `codemux login --token`
// closes that gap — sign in on a machine that *does* have a browser, copy
// the session bearer, and paste it here.
//
// The token is not trusted on faith: it is verified against
// `/api/auth/desktop/verify` (the same endpoint a cold-start `check_auth`
// uses) before anything is written, so a typo'd or expired paste fails
// loudly instead of leaving a session that only breaks later. The verify
// response is also where the profile and the real expiry come from — the
// user is never asked to supply either.

/// The `GET /api/auth/desktop/verify` response. Deliberately a local
/// shape rather than a re-export: `auth::mod`'s equivalent is private and
/// its only caller is the blocking OAuth-callback path, whose client
/// cannot be used from async code.
#[derive(Debug, Deserialize)]
struct VerifyResponse {
    user: VerifyUser,
    session: VerifySession,
}

#[derive(Debug, Deserialize)]
struct VerifyUser {
    id: String,
    email: String,
    name: Option<String>,
    image: Option<String>,
}

#[derive(Debug, Deserialize)]
struct VerifySession {
    #[serde(rename = "expiresAt")]
    expires_at: String,
}

/// Verify a pasted bearer and return the account it belongs to plus the
/// session's real expiry. Errors mirror [`classify_api_error`]'s stance:
/// transport problems are echoed, a rejected token is not (the CLI must
/// not become an oracle for which tokens are live).
async fn verify_bearer(token: &str) -> Result<(AuthUser, String), LoginError> {
    let base = api_base_url();
    let url = format!("{base}/api/auth/desktop/verify");
    let resp = reqwest::Client::new()
        .get(&url)
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| LoginError::Network(e.to_string()))?;

    if !resp.status().is_success() {
        return Err(LoginError::Token);
    }

    let verify: VerifyResponse = resp.json().await.map_err(|e| {
        LoginError::Local(format!(
            "The Codemux API returned an unexpected response ({e})."
        ))
    })?;
    Ok((
        AuthUser {
            id: verify.user.id,
            email: verify.user.email,
            name: verify.user.name,
            image: verify.user.image,
        },
        verify.session.expires_at,
    ))
}

/// Sign in with an already-issued bearer. Persists exactly what
/// [`sign_in`] persists — same `save_auth` record, same cached profile —
/// so the rest of the app cannot tell the two apart.
pub(crate) async fn sign_in_with_token(
    db: &DatabaseStore,
    token: &str,
) -> Result<AuthUser, LoginError> {
    let token = token.trim();
    if token.is_empty() {
        return Err(LoginError::Local("A token is required.".to_string()));
    }

    let (user, expires_at) = verify_bearer(token).await?;

    save_auth(db, token, &expires_at, Some(&user))
        .map_err(|e| LoginError::Local(format!("Could not persist the session: {e}")))?;
    // `"token"` rather than `"email"`/`"github"`: the Settings → Sync
    // section keys off this to decide which password form to offer, and a
    // pasted bearer says nothing about whether the account has a sync
    // password. An unrecognised value makes it fall through to the
    // conservative branch, which is the correct answer here.
    if let Err(err) = save_stored_auth_method(db, Some("token")) {
        eprintln!("[codemux login] persisting auth_method=token failed: {err}");
    }

    Ok(user)
}

// ── Status ───────────────────────────────────────────────────────

/// What the cached auth record says about this machine.
#[derive(Debug)]
pub(crate) enum AuthStatusReport {
    /// No stored session (or a corrupt/undecryptable one).
    SignedOut,
    /// A session exists but its expiry has passed.
    Expired { expires_at: String },
    SignedIn {
        /// `None` for a record written by [`crate::auth::save_token`],
        /// which stores no profile. Rare, but possible on old records.
        user: Option<AuthUser>,
        expires_at: String,
        method: Option<String>,
    },
}

pub(crate) fn auth_status(db: &DatabaseStore) -> AuthStatusReport {
    let (_token, expires_at) = match load_token(db) {
        Some(pair) => pair,
        None => return AuthStatusReport::SignedOut,
    };
    if is_token_expired(&expires_at) {
        return AuthStatusReport::Expired { expires_at };
    }
    AuthStatusReport::SignedIn {
        user: load_cached_user(db),
        expires_at,
        method: load_stored_auth_method(db),
    }
}

/// Render a status report for a human at a terminal. Kept separate from
/// the printing so tests can assert on the exact text without capturing
/// stdout.
pub(crate) fn format_status(report: &AuthStatusReport) -> String {
    match report {
        AuthStatusReport::SignedOut => "Not signed in".to_string(),
        AuthStatusReport::Expired { expires_at } => format!(
            "Not signed in — the stored session expired at {expires_at}.\n\
             Run `codemux login` to sign in again."
        ),
        AuthStatusReport::SignedIn {
            user,
            expires_at,
            method,
        } => {
            let mut out = match user {
                Some(u) => format!("Signed in as {}\n", u.email),
                None => "Signed in (no cached profile on this machine)\n".to_string(),
            };
            if let Some(u) = user {
                out.push_str(&format!("User ID:  {}\n", u.id));
                if let Some(name) = u.name.as_deref().filter(|n| !n.is_empty()) {
                    out.push_str(&format!("Name:     {name}\n"));
                }
            }
            out.push_str(&format!(
                "Method:   {}\n",
                method.as_deref().unwrap_or("unknown")
            ));
            out.push_str(&format!("Expires:  {expires_at}"));
            out
        }
    }
}

/// Exit code for `codemux whoami` / `codemux login --status`: 0 only when
/// a live session exists, so shell scripts can branch on it.
pub(crate) fn status_exit_code(report: &AuthStatusReport) -> i32 {
    match report {
        AuthStatusReport::SignedIn { .. } => 0,
        _ => 1,
    }
}

// ── Sign-out ─────────────────────────────────────────────────────

/// Drop the encrypted auth record, reporting whether there was one.
///
/// Split out from [`sign_out`] so tests can exercise the part that owns
/// state without also deleting the *real* settings-sync cache: that path
/// resolves through `dirs::data_dir()` with no test seam, so calling it
/// from a unit test would clobber the running developer's cache.
pub(crate) fn clear_session_record(db: &DatabaseStore) -> bool {
    let had_session = load_token(db).is_some();
    clear_token(db);
    had_session
}

/// Clear the cached session. Mirrors `commands::auth::sign_out`'s
/// persistent half — the encrypted auth record plus the settings-sync
/// cache. (The GUI additionally resets its in-memory `AuthState` and
/// emits two webview events; neither exists in a CLI process, and
/// neither is persisted.)
///
/// Returns `true` when there was something to clear, so the command can
/// say "Signed out" versus "Already signed out" without a second read.
pub(crate) fn sign_out(db: &DatabaseStore) -> bool {
    let had_session = clear_session_record(db);
    crate::settings_sync::clear_cache();
    had_session
}

// ── Terminal input ───────────────────────────────────────────────

/// Where the password came from. Surfaced so the caller can warn about
/// the environment-variable path exactly once.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) enum PasswordSource {
    Env,
    Tty,
    PipedStdin,
}

/// Decide where the password should be read from, given whether the env
/// var is set and whether stdin is a terminal. Split out from the actual
/// reading so the precedence rules are unit-testable without a TTY.
pub(crate) fn password_source(env_set: bool, stdin_is_tty: bool) -> PasswordSource {
    if env_set {
        PasswordSource::Env
    } else if stdin_is_tty {
        PasswordSource::Tty
    } else {
        PasswordSource::PipedStdin
    }
}

fn read_line_from_stdin() -> Result<String, String> {
    let mut line = String::new();
    let read = std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| format!("Could not read from stdin: {e}"))?;
    if read == 0 {
        return Err("Unexpected end of input.".to_string());
    }
    Ok(line.trim_end_matches(['\n', '\r']).to_string())
}

/// Prompt for the email address. Prompts go to stderr so that
/// `codemux login` can still have its stdout piped somewhere useful.
fn prompt_email() -> Result<String, String> {
    if !std::io::stdin().is_terminal() {
        return Err(
            "No terminal to prompt on — pass --email <address> (and supply the \
             password via CODEMUX_PASSWORD or on stdin)."
                .to_string(),
        );
    }
    eprint!("Email: ");
    let _ = std::io::stderr().flush();
    read_line_from_stdin()
}

/// Read the password without echoing it, or take it from the environment
/// / a pipe when there is no terminal.
fn resolve_password() -> Result<String, String> {
    let from_env = std::env::var(PASSWORD_ENV).ok().filter(|v| !v.is_empty());
    match password_source(from_env.is_some(), std::io::stdin().is_terminal()) {
        PasswordSource::Env => {
            eprintln!(
                "warning: using the password from ${PASSWORD_ENV}. Anything that \
                 can read this process's environment can read it — prefer the \
                 interactive prompt outside of automated runs."
            );
            Ok(from_env.unwrap_or_default())
        }
        PasswordSource::Tty => {
            eprint!("Password: ");
            let _ = std::io::stderr().flush();
            // ECHO off for the duration of the read, restored by rpassword
            // even if the read fails. The trailing newline the user typed
            // is not echoed either, hence the explicit one below.
            let password = rpassword::read_password()
                .map_err(|e| format!("Could not read the password: {e}"))?;
            eprintln!();
            Ok(password)
        }
        PasswordSource::PipedStdin => read_line_from_stdin(),
    }
}

// ── Command entry points ─────────────────────────────────────────

/// Open the same SQLite store the desktop app and `codemux serve` use.
/// Deliberately not the control socket: `login` must work with nothing
/// else running. When an instance *is* running, SQLite's WAL mode makes
/// this a normal concurrent writer.
fn open_db() -> Result<DatabaseStore, String> {
    crate::database::init_database()
}

/// `codemux login [--email <addr>] [--token <bearer>] [--status]`.
pub async fn run_login(
    email: Option<String>,
    token: Option<String>,
    status: bool,
) -> Result<(), String> {
    let db = open_db()?;

    if status {
        return print_status(&db);
    }

    let user = match token {
        Some(token) => sign_in_with_token(&db, &token)
            .await
            .map_err(|e| e.user_message())?,
        None => {
            let email = match email {
                Some(e) if !e.trim().is_empty() => e.trim().to_string(),
                _ => prompt_email()?,
            };
            if email.is_empty() {
                return Err("An email address is required.".to_string());
            }

            let password = resolve_password()?;

            sign_in(&db, &email, password)
                .await
                .map_err(|e| e.user_message())?
        }
    };

    println!("✓ Signed in as {}", user.email);

    // A GUI / `serve` process reads the account bearer and cached user
    // straight from SQLite on each use (`web_remote::registration`,
    // `web_remote::status`), so a live instance picks this session up for
    // relay registration without a restart. Its *webview* auth store,
    // though, is only hydrated by `check_auth` at startup — so say so
    // rather than let the window look signed-out. There is no control
    // command to push an auth refresh into a running instance today.
    if crate::control::control_server_is_running() {
        println!(
            "Note: a Codemux instance is already running on this machine. It will \
             use this session for account/relay features immediately; restart it \
             to refresh the signed-in state shown in its UI."
        );
    }

    Ok(())
}

/// Whether step 1 of `codemux connect` had anything to do.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SignInOutcome {
    /// A live session already existed; the account is named for confirmation.
    AlreadySignedIn(String),
    /// Credentials were collected and a session was created.
    SignedIn(String),
}

impl SignInOutcome {
    /// The account label either way — what the caller prints.
    pub(crate) fn identity(&self) -> &str {
        match self {
            SignInOutcome::AlreadySignedIn(email) | SignInOutcome::SignedIn(email) => email,
        }
    }
}

/// Step 1 of `codemux connect`: make sure this machine has a live session,
/// prompting for credentials only when it doesn't.
///
/// Takes the caller's `DatabaseStore` rather than opening its own — `connect`
/// already holds one for the config write, and two handles to the same SQLite
/// file for one logical operation is a needless invitation to a lock. The
/// prompting, the `CODEMUX_PASSWORD` affordance, and the non-enumerating error
/// mapping are all the same code `codemux login` runs.
pub(crate) async fn ensure_signed_in(
    db: &DatabaseStore,
    email: Option<String>,
) -> Result<SignInOutcome, String> {
    if let AuthStatusReport::SignedIn { user, .. } = auth_status(db) {
        // `save_token` (the OAuth callback's first write) stores no profile,
        // so a rare-but-real record has a live session and no email to name.
        let label = user
            .map(|u| u.email)
            .unwrap_or_else(|| "the account stored on this machine".to_string());
        return Ok(SignInOutcome::AlreadySignedIn(label));
    }

    let email = match email {
        Some(e) if !e.trim().is_empty() => e.trim().to_string(),
        // An OAuth-only account has no password to prompt for, so name the
        // escape hatch here rather than after a failed attempt.
        _ => prompt_email().map_err(|e| {
            format!("{e}\nFor an account with no password, run `codemux login --token <token>` first.")
        })?,
    };
    let password = resolve_password()?;
    let user = sign_in(db, &email, password)
        .await
        .map_err(|e| e.user_message())?;
    Ok(SignInOutcome::SignedIn(user.email))
}

/// `codemux logout`.
pub fn run_logout() -> Result<(), String> {
    let db = open_db()?;
    if sign_out(&db) {
        println!("✓ Signed out. The cached session on this machine was cleared.");
    } else {
        println!("Already signed out — no cached session on this machine.");
    }
    Ok(())
}

/// `codemux whoami`. Exits 1 (without the generic CLI error prefix) when
/// there is no live session, so scripts can test it directly.
pub fn run_whoami() -> Result<(), String> {
    let db = open_db()?;
    print_status(&db)
}

fn print_status(db: &DatabaseStore) -> Result<(), String> {
    let report = auth_status(db);
    let text = format_status(&report);
    let code = status_exit_code(&report);
    if code == 0 {
        println!("{text}");
        Ok(())
    } else {
        eprintln!("{text}");
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
        std::process::exit(code);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> DatabaseStore {
        DatabaseStore::new_in_memory()
    }

    fn signin_mock_body(email: &str) -> String {
        serde_json::json!({
            "token": "cli-token-abc123",
            "expiresAt": "2099-01-01T00:00:00Z",
            "user": {
                "id": "usr_cli_1",
                "email": email,
                "name": "CLI Tester",
                "image": null,
            }
        })
        .to_string()
    }

    /// Point `api_base_url()` at a mock for the duration of a test and
    /// restore whatever was there before. Callers must be `#[serial]`
    /// (the `mockserver` group) because this mutates process env.
    struct ApiUrlGuard(Option<String>);
    impl ApiUrlGuard {
        fn set(url: &str) -> Self {
            let prev = std::env::var("CODEMUX_API_URL").ok();
            std::env::set_var("CODEMUX_API_URL", url);
            Self(prev)
        }
    }
    impl Drop for ApiUrlGuard {
        fn drop(&mut self) {
            match self.0.take() {
                Some(v) => std::env::set_var("CODEMUX_API_URL", v),
                None => std::env::remove_var("CODEMUX_API_URL"),
            }
        }
    }

    // ── Persisted-state parity ───────────────────────────────────

    #[tokio::test]
    #[serial_test::serial(mockserver)]
    async fn login_persists_a_session_the_app_can_load_back() {
        let email = "cli-login@example.com";
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/auth/desktop/signin")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(signin_mock_body(email))
            .create_async()
            .await;
        let _guard = ApiUrlGuard::set(&server.url());

        let db = test_db();
        let user = sign_in(&db, email, "correct horse battery".to_string())
            .await
            .expect("mocked signin succeeds");
        mock.assert_async().await;

        assert_eq!(user.email, email);

        // Everything the GUI's `signin_email` persists, read back through
        // the very functions `check_auth` / `serve` use.
        let (token, expires_at) = load_token(&db).expect("token stored");
        assert_eq!(token, "cli-token-abc123");
        assert_eq!(expires_at, "2099-01-01T00:00:00Z");

        let cached = load_cached_user(&db).expect("profile cached");
        assert_eq!(cached.id, "usr_cli_1");
        assert_eq!(cached.email, email);
        assert_eq!(cached.name.as_deref(), Some("CLI Tester"));
        assert_eq!(cached.image, None);

        assert_eq!(
            load_stored_auth_method(&db).as_deref(),
            Some("email"),
            "CLI login must stamp auth_method exactly like the GUI signin"
        );
    }

    #[tokio::test]
    #[serial_test::serial(mockserver)]
    async fn login_sends_the_derived_secret_not_the_raw_password() {
        // Complements the structural leak tests in `auth/api.rs` by
        // pinning the CLI's own call path: the mock only answers a body
        // whose `password` is exactly `derive_auth_secret(password,
        // email)`. Sending the raw password — or deriving with the
        // arguments swapped — leaves the request unmatched, mockito
        // answers 501, and `sign_in` fails.
        let email = "cli-derive@example.com";
        let password = "distinctive-cli-password-9182";
        let expected = derive_auth_secret(password, email).unwrap();

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/auth/desktop/signin")
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "email": email,
                "password": expected.as_str(),
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(signin_mock_body(email))
            .create_async()
            .await;
        let _guard = ApiUrlGuard::set(&server.url());

        let db = test_db();
        sign_in(&db, email, password.to_string())
            .await
            .expect("request body matches the derived AuthSecret");
        mock.assert_async().await;
    }

    #[tokio::test]
    #[serial_test::serial(mockserver)]
    async fn rejected_credentials_do_not_reveal_whether_the_email_exists() {
        let mut server = mockito::Server::new_async().await;
        // Deliberately the *enumerating* server message; the CLI must not
        // pass it through.
        let mock = server
            .mock("POST", "/api/auth/desktop/signin")
            .with_status(403)
            .with_header("content-type", "application/json")
            .with_body(r#"{"error":"Email not verified for user 42"}"#)
            .create_async()
            .await;
        let _guard = ApiUrlGuard::set(&server.url());

        let db = test_db();
        let err = sign_in(&db, "ghost@example.com", "pw".to_string())
            .await
            .expect_err("403 is an error");
        mock.assert_async().await;

        assert!(matches!(err, LoginError::Credentials));
        let msg = err.user_message();
        assert!(!msg.contains("not verified"), "leaked server detail: {msg}");
        assert!(!msg.contains("42"), "leaked server detail: {msg}");
        assert!(msg.starts_with("Sign-in failed."), "unexpected copy: {msg}");

        // A failed sign-in must not leave a half-written session behind.
        assert!(load_token(&db).is_none());
        assert!(load_cached_user(&db).is_none());
    }

    #[tokio::test]
    #[serial_test::serial(mockserver)]
    async fn unreachable_api_reports_a_network_error() {
        // Point at a closed port on loopback: connect fails immediately.
        let _guard = ApiUrlGuard::set("http://127.0.0.1:1");
        let db = test_db();
        let err = sign_in(&db, "someone@example.com", "pw".to_string())
            .await
            .expect_err("connection refused");
        assert!(matches!(err, LoginError::Network(_)), "got {err:?}");
        assert!(err.user_message().contains("Could not reach the Codemux API"));
        assert!(load_token(&db).is_none());
    }

    #[test]
    fn error_classification_splits_transport_from_credentials() {
        assert!(matches!(
            classify_api_error("Request failed: error sending request"),
            LoginError::Network(_)
        ));
        assert!(matches!(
            classify_api_error("Parse response: missing field `token`"),
            LoginError::Local(_)
        ));
        for server_msg in [
            "Invalid email or password",
            "Email not verified",
            "User not found",
            "Authentication failed",
        ] {
            assert!(
                matches!(classify_api_error(server_msg), LoginError::Credentials),
                "{server_msg} should collapse into the generic credential error"
            );
        }
    }

    #[tokio::test]
    async fn empty_inputs_fail_before_any_network_call() {
        let db = test_db();
        // No ApiUrlGuard on purpose — if this hit the network it would
        // reach the real API, so the assertion doubles as a guard.
        assert!(matches!(
            sign_in(&db, "  ", "pw".to_string()).await,
            Err(LoginError::Local(_))
        ));
        assert!(matches!(
            sign_in(&db, "a@b.c", String::new()).await,
            Err(LoginError::Local(_))
        ));
    }

    // ── Bearer-token sign-in (--token) ───────────────────────────

    fn verify_mock_body(email: &str) -> String {
        serde_json::json!({
            "user": {
                "id": "usr_token_1",
                "email": email,
                "name": "Token Tester",
                "image": null,
            },
            "session": { "expiresAt": "2099-06-07T08:09:10Z" }
        })
        .to_string()
    }

    #[tokio::test]
    #[serial_test::serial(mockserver)]
    async fn token_login_verifies_before_it_persists_anything() {
        let email = "oauth-only@example.com";
        let mut server = mockito::Server::new_async().await;
        // The token must ride as a bearer on the same endpoint a cold-start
        // `check_auth` uses — otherwise a token this CLI accepts could still
        // be one the app rejects on its next launch.
        let mock = server
            .mock("GET", "/api/auth/desktop/verify")
            .match_header("authorization", "Bearer sess_pasted_token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(verify_mock_body(email))
            .create_async()
            .await;
        let _guard = ApiUrlGuard::set(&server.url());

        let db = test_db();
        let user = sign_in_with_token(&db, "sess_pasted_token")
            .await
            .expect("a verified token signs in");
        mock.assert_async().await;

        assert_eq!(user.email, email);
        // The persisted record is the one the app reads back, and the expiry
        // comes from the server's session, never from the user.
        let (token, expires_at) = load_token(&db).expect("token stored");
        assert_eq!(token, "sess_pasted_token");
        assert_eq!(expires_at, "2099-06-07T08:09:10Z");
        assert_eq!(load_cached_user(&db).map(|u| u.id).as_deref(), Some("usr_token_1"));
        assert_eq!(load_stored_auth_method(&db).as_deref(), Some("token"));
        // And it reads as a live session to `whoami` / `connect`.
        assert!(matches!(auth_status(&db), AuthStatusReport::SignedIn { .. }));
    }

    #[tokio::test]
    #[serial_test::serial(mockserver)]
    async fn token_login_surrounding_whitespace_is_trimmed() {
        // Pasting a token into a terminal routinely brings a trailing newline
        // or a leading space; sending that verbatim would 401 for no visible
        // reason.
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/api/auth/desktop/verify")
            .match_header("authorization", "Bearer sess_clean")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(verify_mock_body("a@b.co"))
            .create_async()
            .await;
        let _guard = ApiUrlGuard::set(&server.url());

        let db = test_db();
        sign_in_with_token(&db, "  sess_clean\n").await.unwrap();
        mock.assert_async().await;
        assert_eq!(load_token(&db).unwrap().0, "sess_clean");
    }

    #[tokio::test]
    #[serial_test::serial(mockserver)]
    async fn a_rejected_token_persists_nothing_and_reveals_nothing() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/api/auth/desktop/verify")
            .with_status(401)
            .with_header("content-type", "application/json")
            .with_body(r#"{"error":"Session expired for user 42"}"#)
            .create_async()
            .await;
        let _guard = ApiUrlGuard::set(&server.url());

        let db = test_db();
        let err = sign_in_with_token(&db, "sess_stale")
            .await
            .expect_err("a 401 is an error");
        mock.assert_async().await;

        assert!(matches!(err, LoginError::Token));
        let msg = err.user_message();
        assert!(!msg.contains("42"), "leaked server detail: {msg}");
        assert!(!msg.contains("expired for"), "leaked server detail: {msg}");
        assert!(msg.contains("not accepted"), "unexpected copy: {msg}");
        // No half-written session.
        assert!(load_token(&db).is_none());
        assert!(load_stored_auth_method(&db).is_none());
    }

    #[tokio::test]
    #[serial_test::serial(mockserver)]
    async fn a_malformed_verify_response_is_a_local_error_not_a_session() {
        // A 200 whose body doesn't carry the session expiry must NOT be
        // stored: `check_auth` treats an unparseable expiry as expired and
        // would silently sign the machine back out on its next launch.
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("GET", "/api/auth/desktop/verify")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"user":{"id":"u","email":"a@b.co"}}"#)
            .create_async()
            .await;
        let _guard = ApiUrlGuard::set(&server.url());

        let db = test_db();
        let err = sign_in_with_token(&db, "sess_weird")
            .await
            .expect_err("a body without a session is not a sign-in");
        assert!(matches!(err, LoginError::Local(_)), "got {err:?}");
        assert!(load_token(&db).is_none());
    }

    #[tokio::test]
    async fn an_empty_token_fails_before_any_network_call() {
        // No ApiUrlGuard: reaching the network here would hit the real API,
        // so the assertion doubles as a guard.
        let db = test_db();
        assert!(matches!(
            sign_in_with_token(&db, "   ").await,
            Err(LoginError::Local(_))
        ));
        assert!(load_token(&db).is_none());
    }

    #[tokio::test]
    #[serial_test::serial(mockserver)]
    async fn an_unreachable_api_reports_a_network_error_for_tokens_too() {
        let _guard = ApiUrlGuard::set("http://127.0.0.1:1");
        let db = test_db();
        let err = sign_in_with_token(&db, "sess_any")
            .await
            .expect_err("connection refused");
        assert!(matches!(err, LoginError::Network(_)), "got {err:?}");
        assert!(err.user_message().contains("Could not reach the Codemux API"));
    }

    // ── connect's step 1 ─────────────────────────────────────────

    #[tokio::test]
    async fn ensure_signed_in_is_a_no_op_when_a_session_exists() {
        // `codemux connect` on an already-signed-in box must not prompt —
        // it has to be safe to re-run from a script.
        let db = test_db();
        let user = AuthUser {
            id: "usr_c".to_string(),
            email: "connect@example.com".to_string(),
            name: None,
            image: None,
        };
        save_auth(&db, "tok", "2099-01-01T00:00:00Z", Some(&user)).unwrap();

        let outcome = ensure_signed_in(&db, None)
            .await
            .expect("an existing session short-circuits");
        assert_eq!(
            outcome,
            SignInOutcome::AlreadySignedIn("connect@example.com".to_string())
        );
        assert_eq!(outcome.identity(), "connect@example.com");
    }

    #[tokio::test]
    #[serial_test::serial(mockserver)]
    async fn ensure_signed_in_signs_in_when_the_session_expired() {
        // An expired record must be treated as signed out, not reused.
        let email = "reconnect@example.com";
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/api/auth/desktop/signin")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(signin_mock_body(email))
            .create_async()
            .await;
        let _guard = ApiUrlGuard::set(&server.url());
        std::env::set_var(PASSWORD_ENV, "from-the-environment");

        let db = test_db();
        save_auth(&db, "old", "2000-01-01T00:00:00Z", None).unwrap();
        let outcome = ensure_signed_in(&db, Some(email.to_string())).await;
        std::env::remove_var(PASSWORD_ENV);

        assert_eq!(
            outcome.expect("signs in again"),
            SignInOutcome::SignedIn(email.to_string())
        );
        assert_eq!(load_token(&db).unwrap().0, "cli-token-abc123");
    }

    // ── Sign-out ─────────────────────────────────────────────────

    #[tokio::test]
    #[serial_test::serial(mockserver)]
    async fn logout_clears_everything_login_wrote() {
        let email = "cli-logout@example.com";
        let mut server = mockito::Server::new_async().await;
        server
            .mock("POST", "/api/auth/desktop/signin")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(signin_mock_body(email))
            .create_async()
            .await;
        let _guard = ApiUrlGuard::set(&server.url());

        let db = test_db();
        sign_in(&db, email, "pw".to_string()).await.unwrap();
        assert!(load_token(&db).is_some());

        // `clear_session_record`, not `sign_out`: the latter also wipes the
        // settings-sync cache, whose path is the real `dirs::data_dir()`
        // with no test seam. The session record is the part that matters
        // here — everything `codemux login` wrote.
        assert!(
            clear_session_record(&db),
            "reports that a session was cleared"
        );
        assert!(load_token(&db).is_none());
        assert!(load_cached_user(&db).is_none());
        assert!(load_stored_auth_method(&db).is_none());
        assert!(matches!(auth_status(&db), AuthStatusReport::SignedOut));

        assert!(
            !clear_session_record(&db),
            "second logout reports nothing to clear"
        );
    }

    // ── Status / whoami ──────────────────────────────────────────

    #[test]
    fn status_is_signed_out_on_a_fresh_machine() {
        let db = test_db();
        let report = auth_status(&db);
        assert!(matches!(report, AuthStatusReport::SignedOut));
        assert_eq!(format_status(&report), "Not signed in");
        assert_eq!(status_exit_code(&report), 1);
    }

    #[test]
    fn status_reports_the_cached_identity() {
        let db = test_db();
        let user = AuthUser {
            id: "usr_whoami".to_string(),
            email: "who@example.com".to_string(),
            name: Some("Who Ami".to_string()),
            image: None,
        };
        save_auth(&db, "tok", "2099-03-04T05:06:07Z", Some(&user)).unwrap();
        save_stored_auth_method(&db, Some("email")).unwrap();

        let report = auth_status(&db);
        assert_eq!(status_exit_code(&report), 0);
        let text = format_status(&report);
        assert!(text.starts_with("Signed in as who@example.com"), "{text}");
        assert!(text.contains("usr_whoami"), "{text}");
        assert!(text.contains("Who Ami"), "{text}");
        assert!(text.contains("email"), "{text}");
        assert!(text.contains("2099-03-04T05:06:07Z"), "{text}");
    }

    #[test]
    fn status_treats_an_expired_session_as_signed_out() {
        let db = test_db();
        let user = AuthUser {
            id: "usr_old".to_string(),
            email: "old@example.com".to_string(),
            name: None,
            image: None,
        };
        save_auth(&db, "tok", "2000-01-01T00:00:00Z", Some(&user)).unwrap();

        let report = auth_status(&db);
        assert!(matches!(report, AuthStatusReport::Expired { .. }));
        assert_eq!(status_exit_code(&report), 1);
        let text = format_status(&report);
        assert!(text.starts_with("Not signed in"), "{text}");
        assert!(text.contains("2000-01-01T00:00:00Z"), "{text}");
    }

    #[test]
    fn status_handles_a_record_with_no_cached_profile() {
        // `save_token` (used by the OAuth callback before the verify
        // round-trip) stores no user; whoami must not panic on it.
        let db = test_db();
        crate::auth::save_token(&db, "tok", "2099-01-01T00:00:00Z").unwrap();
        let report = auth_status(&db);
        assert_eq!(status_exit_code(&report), 0);
        assert!(format_status(&report).contains("no cached profile"));
    }

    // ── Password source precedence ───────────────────────────────

    #[test]
    fn password_source_precedence() {
        assert_eq!(password_source(true, true), PasswordSource::Env);
        assert_eq!(password_source(true, false), PasswordSource::Env);
        assert_eq!(password_source(false, true), PasswordSource::Tty);
        assert_eq!(
            password_source(false, false),
            PasswordSource::PipedStdin,
            "a headless harness with no TTY and no env var reads stdin"
        );
    }
}
