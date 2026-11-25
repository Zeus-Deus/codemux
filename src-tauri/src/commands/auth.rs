use std::time::Instant;
use tauri::{Emitter, Manager, State};

use crate::auth::{
    api_base_url, derive_auth_secret, is_token_expired, load_auth_record,
    login_email_api, save_auth_with_method, signup_email_api, AuthResponse, AuthState,
    AuthStatePayload, AuthUser,
};
use crate::database::DatabaseStore;

/// Sync state surfaced to the frontend. Skills sync is stored
/// server-side (no client-held key), so `sync_available` is simply
/// "the user is signed in" — there's no password to set up and no
/// device-local key to repair. `auth_method` is still reported so
/// the UI can tailor copy, and the shape stays identical to the
/// event payload the frontend already listens for.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub sync_available: bool,
    pub auth_method: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionBootstrap {
    pub authenticated: bool,
    pub user: Option<AuthUser>,
    pub settings: crate::settings_sync::UserSettings,
    pub auth_method: Option<String>,
    /// `local`, `pending-verification`, or `signed-out`.
    pub status: &'static str,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRefresh {
    pub authenticated: bool,
    pub user: Option<AuthUser>,
    pub settings: crate::settings_sync::UserSettings,
    pub auth_method: Option<String>,
    /// `local`, `verified`, `offline`, `degraded`, `pending-verification`,
    /// or `signed-out`.
    pub status: &'static str,
}

const AUTH_REMOTE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

enum VerifyOutcome {
    Verified(AuthUser),
    Unauthorized,
    Offline,
}

async fn verify_token(token: &str) -> VerifyOutcome {
    let client = match reqwest::Client::builder()
        .timeout(AUTH_REMOTE_TIMEOUT)
        .build()
    {
        Ok(client) => client,
        Err(_) => return VerifyOutcome::Offline,
    };
    let url = format!("{}/api/auth/desktop/verify", api_base_url());
    match client
        .get(url)
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => {
            let Ok(verify) = response.json::<VerifyResp>().await else {
                return VerifyOutcome::Offline;
            };
            VerifyOutcome::Verified(AuthUser {
                id: verify.user.id,
                email: verify.user.email,
                name: verify.user.name,
                image: verify.user.image,
            })
        }
        Ok(response) if response.status() == reqwest::StatusCode::UNAUTHORIZED => {
            VerifyOutcome::Unauthorized
        }
        Ok(_) | Err(_) => VerifyOutcome::Offline,
    }
}

/// A new backend process must not trust old account browser sessions until
/// the account API has validated this host's exact stored token. This also
/// keeps a rejected token suspended after a previous process could not write
/// its revocation to SQLite. Offline verification leaves browsers suspended.
pub(crate) async fn verify_stored_account_for_remote(db: &DatabaseStore) {
    let Some(record) = load_auth_record(db) else {
        return;
    };
    if record.stored.user.is_none() {
        return;
    }
    loop {
        if !record.is_current(db) {
            return;
        }
        let outcome = if is_token_expired(&record.stored.expires_at) {
            VerifyOutcome::Unauthorized
        } else {
            verify_token(&record.stored.token).await
        };
        match outcome {
            VerifyOutcome::Verified(user) => {
                if record.stored.user.as_ref().is_some_and(|cached| cached.id == user.id) {
                    db.mark_account_auth_verified_if_current(record.encrypted());
                    return;
                }
                // A different server-side owner invalidates this host's old
                // browser rows. Require a new sign-in for the new identity.
                if let Err(error) = db.clear_auth_token_if_current(record.encrypted()) {
                    log::warn!("[auth] Could not persist changed account revocation: {error}");
                    retry_rejected_auth_revocation(db, record.encrypted()).await;
                }
                return;
            }
            VerifyOutcome::Unauthorized => {
                if let Err(error) = db.clear_auth_token_if_current(record.encrypted()) {
                    log::warn!("[auth] Could not persist rejected account revocation: {error}");
                    retry_rejected_auth_revocation(db, record.encrypted()).await;
                }
                return;
            }
            VerifyOutcome::Offline => {}
        }
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
    }
}

async fn retry_rejected_auth_revocation(db: &DatabaseStore, expected: &[u8]) {
    let mut delay = std::time::Duration::from_secs(5);
    loop {
        tokio::time::sleep(delay).await;
        match db.clear_auth_token_if_current(expected) {
            Ok(_) => return,
            Err(error) => {
                log::warn!("[auth] Retrying rejected account revocation: {error}");
                // An in-memory fallback has no durable database to recover.
                if db.require_persistent_auth_storage().is_err() {
                    return;
                }
            }
        }
        delay = (delay * 2).min(std::time::Duration::from_secs(60));
    }
}

fn emit_signed_out<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    let _ = app.emit(
        "auth-state-changed",
        &AuthStatePayload {
            authenticated: false,
            user: None,
        },
    );
    let _ = app.emit(
        "sync-state-changed",
        &SyncStatus {
            sync_available: false,
            auth_method: None,
        },
    );
}

fn signed_out_refresh() -> SessionRefresh {
    SessionRefresh {
        authenticated: false,
        user: None,
        settings: Default::default(),
        auth_method: None,
        status: "signed-out",
    }
}

/// Refresh result when the verification endpoint could not be reached
/// (timeout, 5xx, transport failure) while the local token is still valid.
/// With a cached identity the session keeps working offline. Without one,
/// identity is not yet established and verification is still owed, so the
/// session stays `pending-verification` — the frontend's post-paint bounded
/// retry keys off that status, and reporting `offline` here would cancel the
/// retries and strand a valid token on the login frame.
fn unreachable_verify_refresh(
    user: Option<AuthUser>,
    settings: crate::settings_sync::UserSettings,
    auth_method: Option<String>,
) -> SessionRefresh {
    SessionRefresh {
        authenticated: user.is_some(),
        status: if user.is_some() {
            "offline"
        } else {
            "pending-verification"
        },
        user,
        settings,
        auth_method,
    }
}

/// Re-read the current persisted account after a stale remote operation.
fn current_local_refresh(db: &DatabaseStore, auth_state: &AuthState) -> SessionRefresh {
    auth_state
        .session_snapshot(|| {
            let record =
                load_auth_record(db).filter(|record| !is_token_expired(&record.stored.expires_at));
            let Some(record) = record else {
                auth_state.set_local_session(None);
                return signed_out_refresh();
            };
            auth_state.set_local_session(Some(&record.stored));
            let user = record.stored.user;
            SessionRefresh {
                authenticated: user.is_some(),
                settings: crate::settings_sync::load_cache().unwrap_or_default(),
                status: if user.is_some() {
                    "local"
                } else {
                    "pending-verification"
                },
                user,
                auth_method: record.stored.auth_method,
            }
        })
        .1
}

/// Local-only first-stage startup. No socket, subprocess, or remote request is
/// performed, so a valid cached session and settings can paint immediately.
#[tauri::command]
pub fn bootstrap_session(
    db: State<'_, DatabaseStore>,
    auth_state: State<'_, AuthState>,
) -> Result<SessionBootstrap, String> {
    let auth_started = Instant::now();
    loop {
        let (session_generation, record) = auth_state.session_snapshot(|| load_auth_record(&db));
        if let Some(record) = record
            .as_ref()
            .filter(|r| is_token_expired(&r.stored.expires_at))
        {
            if !auth_state.clear_session_if_current(&db, session_generation, record, || ())? {
                continue;
            }
            crate::diagnostics::record_perf_timing("auth.local-bootstrap", auth_started.elapsed());
            return Ok(SessionBootstrap {
                authenticated: false,
                user: None,
                settings: Default::default(),
                auth_method: None,
                status: "signed-out",
            });
        }
        let bootstrap = auth_state.commit_if_session_current(session_generation, || {
            let stored = record.map(|record| record.stored);
            auth_state.set_local_session(stored.as_ref());
            let settings_started = Instant::now();
            let settings = crate::settings_sync::load_cache().unwrap_or_default();
            crate::diagnostics::record_perf_timing(
                "settings.local-cache-load",
                settings_started.elapsed(),
            );
            let status = match &stored {
                Some(stored) if stored.user.is_some() => "local",
                Some(_) => "pending-verification",
                None => "signed-out",
            };
            let user = stored.as_ref().and_then(|stored| stored.user.clone());
            SessionBootstrap {
                authenticated: user.is_some(),
                user,
                settings,
                auth_method: stored.and_then(|stored| stored.auth_method),
                status,
            }
        });
        if let Some(bootstrap) = bootstrap {
            crate::diagnostics::record_perf_timing("auth.local-bootstrap", auth_started.elapsed());
            return Ok(bootstrap);
        }
    }
}

/// After-paint remote verification + settings refresh. Network failures retain
/// a non-expired local session; only expiry or a definitive 401 signs it out.
#[tauri::command]
pub async fn refresh_session<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    db: State<'_, DatabaseStore>,
    auth_state: State<'_, AuthState>,
) -> Result<SessionRefresh, String> {
    let (session_generation, record) = auth_state.session_snapshot(|| load_auth_record(&db));
    let Some(record) = record else {
        return Ok(current_local_refresh(&db, &auth_state));
    };
    let verify_outcome = if is_token_expired(&record.stored.expires_at) {
        VerifyOutcome::Unauthorized
    } else {
        let verify_started = Instant::now();
        let outcome = verify_token(&record.stored.token).await;
        crate::diagnostics::record_perf_timing("auth.remote-verify", verify_started.elapsed());
        if is_token_expired(&record.stored.expires_at) {
            VerifyOutcome::Unauthorized
        } else {
            outcome
        }
    };

    match verify_outcome {
        VerifyOutcome::Verified(user) => {
            let saved = auth_state
                .commit_if_session_current(session_generation, || {
                    let Some(saved) = record.save_verified_user(&db, &user)? else {
                        return Ok::<_, String>(None);
                    };
                    if !saved.is_current(&db) {
                        return Ok(None);
                    }
                    auth_state.set_local_session(Some(&saved.stored));
                    Ok(Some(saved))
                })
                .transpose()?
                .flatten();
            let Some(saved) = saved else {
                return Ok(current_local_refresh(&db, &auth_state));
            };
            let settings_sync_started = Instant::now();
            let settings_result =
                crate::settings_sync::sync_settings_for_owner(&saved.stored.token, &user.id).await;
            crate::diagnostics::record_perf_timing(
                "settings.remote-sync",
                settings_sync_started.elapsed(),
            );
            let refresh = auth_state
                .commit_if_session_current(session_generation, || {
                    if !saved.is_current(&db) || is_token_expired(&saved.stored.expires_at) {
                        return None;
                    }
                    let (settings, status) = match settings_result {
                        Ok(settings) => {
                            let _ = app.emit(
                                "settings-synced",
                                &crate::settings_sync::SettingsPersistence::synced(
                                    settings.clone(),
                                ),
                            );
                            (settings, "verified")
                        }
                        Err(_) => (
                            crate::settings_sync::load_cache().unwrap_or_default(),
                            "degraded",
                        ),
                    };
                    let auth_method = saved.stored.auth_method;
                    let _ = app.emit(
                        "sync-state-changed",
                        &SyncStatus {
                            sync_available: true,
                            auth_method: auth_method.clone(),
                        },
                    );
                    Some(SessionRefresh {
                        authenticated: true,
                        user: Some(user),
                        settings,
                        auth_method,
                        status,
                    })
                })
                .flatten();
            Ok(refresh.unwrap_or_else(|| current_local_refresh(&db, &auth_state)))
        }
        VerifyOutcome::Unauthorized => {
            let cleared = auth_state.clear_session_if_current(&db, session_generation, &record, || {
                emit_signed_out(&app);
            });
            match cleared {
                Ok(true) => Ok(signed_out_refresh()),
                Ok(false) => Ok(current_local_refresh(&db, &auth_state)),
                Err(error) => {
                    let app = app.clone();
                    let expected = record.encrypted().to_vec();
                    tauri::async_runtime::spawn(async move {
                        let db = app.state::<DatabaseStore>();
                        retry_rejected_auth_revocation(&db, &expected).await;
                    });
                    Err(error)
                }
            }
        }
        VerifyOutcome::Offline => {
            let refresh = auth_state
                .commit_if_session_current(session_generation, || {
                    if !record.is_current(&db) || is_token_expired(&record.stored.expires_at) {
                        return None;
                    }
                    auth_state.set_local_session(Some(&record.stored));
                    Some(unreachable_verify_refresh(
                        record.stored.user,
                        crate::settings_sync::load_cache().unwrap_or_default(),
                        record.stored.auth_method,
                    ))
                })
                .flatten();
            Ok(refresh.unwrap_or_else(|| current_local_refresh(&db, &auth_state)))
        }
    }
}

#[tauri::command]
pub async fn start_oauth_flow<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    db: State<'_, DatabaseStore>,
    auth_state: State<'_, AuthState>,
) -> Result<(), String> {
    let csrf_state = auth_state.generate_csrf_state(&db)?;

    // The callback server reads the managed AuthState through the AppHandle.
    // Keeping CSRF and session-generation state in that one instance makes an
    // OAuth callback an atomic replacement with respect to startup refresh.
    let port = crate::auth::start_callback_server(app.clone())?;

    let base = api_base_url();
    let url = format!(
        "{base}/api/auth/desktop/connect?provider=github&state={state}&port={port}",
        state = urlencoding::encode(&csrf_state),
    );

    // Open in system browser
    tauri_plugin_opener::open_url(&url, None::<&str>)
        .map_err(|e| format!("Failed to open browser: {e}"))?;

    Ok(())
}

#[tauri::command]
pub async fn signin_email<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    db: State<'_, DatabaseStore>,
    auth_state: State<'_, AuthState>,
    email: String,
    password: String,
) -> Result<AuthResponse, String> {
    if email.is_empty() || password.is_empty() {
        return Err("Email and password are required".into());
    }

    // Bitwarden-style zero-knowledge derivation: stretch
    // (password, email) locally into a high-entropy AuthSecret that
    // goes to Better Auth in place of the raw password. Must produce
    // byte-identical output to Vexis's derivation or cross-product
    // login breaks — pinned by `auth_secret_matches_vexis_for_known_input`
    // in auth/derivation.rs. Skills sync is server-side now, so we
    // no longer derive the `encryption_key` half.
    //
    // `login_email_api` takes `&AuthSecret`, not `&str` — the
    // compiler refuses to let us pass the raw password past this
    // point.
    let auth_secret = derive_auth_secret(&password, &email)?;
    drop(password); // raw password falls out of scope; only the derived secret proceeds.

    let api_resp = login_email_api(&email, &auth_secret).await?;

    let user = AuthUser {
        id: api_resp.user.id.clone(),
        email: api_resp.user.email.clone(),
        name: api_resp.user.name.clone(),
        image: api_resp.user.image.clone(),
    };

    auth_state.try_replace_session(|| {
        let record = save_auth_with_method(
            &db,
            &api_resp.token,
            &api_resp.expires_at,
            Some(&user),
            Some("email"),
        )?;
        auth_state.set_local_session(Some(&record.stored));
        let payload = AuthStatePayload {
            authenticated: true,
            user: Some(user.clone()),
        };
        let _ = app.emit("auth-state-changed", &payload);
        let _ = app.emit(
            "sync-state-changed",
            &SyncStatus {
                sync_available: true,
                auth_method: record.stored.auth_method,
            },
        );
        Ok(AuthResponse {
            token: api_resp.token,
            expires_at: api_resp.expires_at,
            user,
        })
    })
}

#[tauri::command]
pub async fn signup_email(email: String, password: String, name: String) -> Result<(), String> {
    if email.is_empty() || password.is_empty() {
        return Err("Email and password are required".into());
    }

    // Bitwarden-style zero-knowledge derivation: the server only
    // ever sees the stretched AuthSecret, never the raw password.
    // Must match Vexis's derivation byte-for-byte or a user who
    // signs up in Codemux can't later sign in from Vexis (and vice
    // versa). See auth/derivation.rs.
    //
    // `signup_email_api` takes `&AuthSecret` — the compiler refuses
    // to let the raw password reach the network.
    let auth_secret = derive_auth_secret(&password, &email)?;
    drop(password); // raw password falls out of scope; only the derived secret proceeds.

    signup_email_api(&email, &auth_secret, &name).await?;

    // Don't save token — user must verify email first, then sign in
    Ok(())
}

#[tauri::command]
pub async fn forgot_password(email: String) -> Result<(), String> {
    if email.is_empty() {
        return Err("Email is required".into());
    }

    let base = api_base_url();
    let url = format!("{base}/api/auth/desktop/forgot-password");

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .json(&serde_json::json!({ "email": email }))
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err("Failed to send reset link".into());
    }

    Ok(())
}

/// Re-send the sign-up verification email through Better Auth's own
/// endpoint. Without a session it answers success whether or not the address
/// exists (or is already verified), so this never leaks account existence.
#[tauri::command]
pub async fn resend_verification_email(email: String) -> Result<(), String> {
    if email.is_empty() {
        return Err("Email is required".into());
    }

    let base = api_base_url();
    let url = format!("{base}/api/auth/send-verification-email");

    let resp = reqwest::Client::new()
        .post(&url)
        .json(&serde_json::json!({ "email": email }))
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(resend_verification_error(resp.status()).into());
    }

    Ok(())
}

fn resend_verification_error(status: reqwest::StatusCode) -> &'static str {
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        "Too many requests. Wait a minute, then try again."
    } else {
        "Couldn't resend the verification email"
    }
}

#[tauri::command]
pub async fn check_auth<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    db: State<'_, DatabaseStore>,
    auth_state: State<'_, AuthState>,
) -> Result<Option<AuthUser>, String> {
    Ok(refresh_session(app, db, auth_state).await?.user)
}

#[tauri::command]
pub fn sign_out<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    db: State<'_, DatabaseStore>,
    auth_state: State<'_, AuthState>,
) -> Result<(), String> {
    auth_state.try_replace_session(|| {
        crate::auth::clear_token_for_sign_out(&db)?;
        auth_state.clear_local_session();
        let _ = app.emit(
            "settings-synced",
            &crate::settings_sync::SettingsPersistence::synced(
                crate::settings_sync::UserSettings::default(),
            ),
        );
        emit_signed_out(&app);
        Ok(())
    })
}

#[tauri::command]
pub fn get_auth_token(
    db: State<'_, DatabaseStore>,
    auth_state: State<'_, AuthState>,
) -> Result<Option<String>, String> {
    loop {
        let (session_generation, record) = auth_state.session_snapshot(|| load_auth_record(&db));
        let Some(record) = record else {
            return Ok(None);
        };
        if !is_token_expired(&record.stored.expires_at) {
            return Ok(Some(record.stored.token));
        }
        if auth_state.clear_session_if_current(&db, session_generation, &record, || ())? {
            return Ok(None);
        }
        // A separate sign-in or sign-out replaced the expired record.
    }
}

// ────────────────────────────────────────────────────────────────
// Sync status
// ────────────────────────────────────────────────────────────────

/// Get the current sync state. Skills sync is server-side, so this
/// is simply "is the user signed in with a live token." The
/// Settings → Sync section calls this on mount to decide whether to
/// show the sync dashboard.
#[tauri::command]
pub fn get_sync_status(
    db: State<'_, DatabaseStore>,
    auth_state: State<'_, AuthState>,
) -> SyncStatus {
    auth_state
        .session_snapshot(|| {
            let record =
                load_auth_record(&db).filter(|record| !is_token_expired(&record.stored.expires_at));
            SyncStatus {
                sync_available: record.is_some(),
                auth_method: record.and_then(|record| record.stored.auth_method),
            }
        })
        .1
}

// ── Internal types for API deserialization ────────────────────────
//
// `check_auth` still hand-rolls its `/desktop/verify` HTTP call
// here (no password involved, so it doesn't need the typed
// AuthSecret boundary). The signin/signup response types live in
// `auth/api.rs` alongside the typed helpers that return them.

#[derive(Debug, serde::Deserialize)]
struct ApiUserResp {
    id: String,
    email: String,
    name: Option<String>,
    image: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct VerifyResp {
    user: ApiUserResp,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ApiUrlGuard(Option<std::ffi::OsString>);

    impl ApiUrlGuard {
        fn set(url: &str) -> Self {
            let previous = std::env::var_os("CODEMUX_API_URL");
            std::env::set_var("CODEMUX_API_URL", url);
            Self(previous)
        }
    }

    impl Drop for ApiUrlGuard {
        fn drop(&mut self) {
            if let Some(previous) = &self.0 {
                std::env::set_var("CODEMUX_API_URL", previous);
            } else {
                std::env::remove_var("CODEMUX_API_URL");
            }
        }
    }

    fn cached_user() -> AuthUser {
        AuthUser {
            id: "u1".into(),
            email: "u1@example.com".into(),
            name: None,
            image: None,
        }
    }

    #[tokio::test]
    #[serial_test::serial(mockserver)]
    async fn fresh_host_verification_restores_account_browser_after_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.sqlite");
        let browser_token = "browser-token";
        let db = DatabaseStore::test_file(&path);
        crate::auth::save_auth(
            &db, "host-token", "2099-01-01T00:00:00Z", Some(&cached_user()),
        ).unwrap();
        db.web_remote_insert_account_session(
            "account-browser", None, None,
            &crate::web_remote::auth::sha256_hex(browser_token), true, "u1",
        ).unwrap();
        drop(db);

        let reopened = DatabaseStore::test_file(&path);
        assert!(crate::web_remote::auth::authenticate(&reopened, browser_token).is_none());
        let mut server = mockito::Server::new_async().await;
        let mock = server.mock("GET", "/api/auth/desktop/verify")
            .match_header("authorization", "Bearer host-token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::json!({
                "user": { "id": "u1", "email": "u1@example.com", "name": null, "image": null }
            }).to_string())
            .create_async().await;
        let _api = ApiUrlGuard::set(&server.url());
        verify_stored_account_for_remote(&reopened).await;
        mock.assert_async().await;
        assert!(crate::web_remote::auth::authenticate(&reopened, browser_token).is_some());
    }

    #[test]
    fn resend_verification_rate_limit_says_to_wait() {
        assert!(resend_verification_error(reqwest::StatusCode::TOO_MANY_REQUESTS)
            .contains("Wait a minute"));
        assert_eq!(
            resend_verification_error(reqwest::StatusCode::BAD_GATEWAY),
            "Couldn't resend the verification email"
        );
    }

    #[test]
    fn unreachable_verify_with_cached_user_stays_offline() {
        let refresh = unreachable_verify_refresh(
            Some(cached_user()),
            Default::default(),
            Some("email".into()),
        );
        assert_eq!(refresh.status, "offline");
        assert!(refresh.authenticated);
        assert_eq!(refresh.user.as_ref().map(|u| u.id.as_str()), Some("u1"));
    }

    #[test]
    fn unreachable_verify_without_cached_user_stays_pending() {
        // A timed-out/5xx verify over a valid token with no cached identity
        // must not resolve as `offline`: the frontend applies the resolved
        // status directly, and only `pending-verification` keeps its bounded
        // retry loop re-verifying instead of leaving the login screen painted
        // over a valid token.
        let refresh =
            unreachable_verify_refresh(None, Default::default(), Some("github".into()));
        assert_eq!(refresh.status, "pending-verification");
        assert!(!refresh.authenticated);
        assert!(refresh.user.is_none());
    }
}
