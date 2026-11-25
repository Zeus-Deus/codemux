use std::time::{Duration, Instant};
use tauri::{Emitter, Manager, State};

use crate::auth::{
    api_base_url, derive_auth_secret, is_token_expired, load_auth_record,
    login_email_api, save_auth_at_revision, signup_email_api, AuthResponse, AuthState,
    AuthStatePayload, AuthUser, SIGN_IN_SESSION_CHANGED,
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

/// A backend process must not trust account browser sessions until the account
/// API has validated this host's exact stored token. Keep watching the file
/// database: a CLI login can write a new record after this process starts,
/// but its verification state belongs to the CLI's separate DatabaseStore.
pub(crate) async fn verify_stored_account_for_remote<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) {
    let db = app.state::<DatabaseStore>();
    let auth_state = app.state::<AuthState>();
    let notifying_app = app.clone();
    let cleanup_app = app.clone();
    verify_stored_account_for_remote_with_poll(
        &db,
        &auth_state,
        Duration::from_secs(5),
        crate::database::ACCOUNT_AUTH_VERIFICATION_TTL / 2,
        move |db, generation, expected, cleanup_pending| {
            notify_rejected_local_session(&notifying_app, db, generation, expected, cleanup_pending)
        },
        move |db, assigned| {
            clear_rejected_local_session_warning(&cleanup_app, db, assigned);
        },
    )
    .await;
}

async fn verify_stored_account_for_remote_with_poll<F, C>(
    db: &DatabaseStore,
    auth_state: &AuthState,
    poll: Duration,
    renew_after: Duration,
    on_rejected: F,
    on_cleanup_complete: C,
) where
    F: Fn(&DatabaseStore, u64, &[u8], bool) -> Option<u64> + Send + Sync,
    C: Fn(&DatabaseStore, u64) + Send + Sync,
{
    let mut attempted_record: Option<Vec<u8>> = None;
    let mut retry_after = Instant::now();
    let mut retry_delay = poll;
    loop {
        let (generation, record) = auth_state.session_snapshot(|| load_auth_record(db));
        if let Some(record) = record.filter(|record| record.stored.user.is_some()) {
            if attempted_record.as_deref() != Some(record.encrypted()) {
                attempted_record = Some(record.encrypted().to_vec());
                retry_after = Instant::now();
                retry_delay = poll;
            }
            if Instant::now() >= retry_after {
                if !record.is_current(db) {
                    attempted_record = None;
                    tokio::time::sleep(poll).await;
                    continue;
                }
                let expired = is_token_expired(&record.stored.expires_at);
                let outcome = if expired {
                    VerifyOutcome::Unauthorized
                } else {
                    verify_token(&record.stored.token).await
                };
                match outcome {
                    VerifyOutcome::Verified(user) if record.stored.user.as_ref().is_some_and(|cached| cached.id == user.id) => {
                        if db.mark_account_auth_verified_if_current(record.encrypted()) {
                            // Renew before the database's verification lease expires.
                            retry_after = Instant::now() + renew_after;
                            retry_delay = poll;
                        } else {
                            attempted_record = None;
                        }
                    }
                    VerifyOutcome::Verified(_) | VerifyOutcome::Unauthorized => {
                        reject_stored_account(
                            db,
                            record.encrypted(),
                            generation,
                            &on_rejected,
                            &on_cleanup_complete,
                        ).await;
                        attempted_record = None;
                    }
                    VerifyOutcome::Offline => {
                        retry_after = Instant::now() + retry_delay;
                        retry_delay = (retry_delay * 2).min(Duration::from_secs(60));
                    }
                }
            }
        } else {
            attempted_record = None;
            retry_delay = poll;
        }
        tokio::time::sleep(poll).await;
    }
}

async fn reject_stored_account<F, C>(
    db: &DatabaseStore,
    expected: &[u8],
    generation: u64,
    on_rejected: &F,
    on_cleanup_complete: &C,
) where
    F: Fn(&DatabaseStore, u64, &[u8], bool) -> Option<u64>,
    C: Fn(&DatabaseStore, u64),
{
    match db.clear_auth_token_if_current(expected) {
        Ok(true) => {
            on_rejected(db, generation, expected, false);
        }
        Ok(false) => {} // Sign-out or replacement already changed this record.
        Err(error) => {
            let assigned = on_rejected(db, generation, expected, true);
            log::warn!("[auth] Could not persist account session cleanup: {error}");
            retry_auth_revocation(db, expected).await;
            if let Some(assigned) = assigned {
                if matches!(db.load_raw_auth_token(), Ok(None)) {
                    on_cleanup_complete(db, assigned);
                }
            }
        }
    }
}

async fn retry_auth_revocation(db: &DatabaseStore, expected: &[u8]) {
    let mut delay = std::time::Duration::from_secs(5);
    loop {
        tokio::time::sleep(delay).await;
        // A new CLI or GUI sign-in owns the row now. The old rejection must
        // never clear it, even if this process still cannot start a write.
        match db.load_raw_auth_token() {
            Ok(None) => return,
            Ok(Some(current)) if current != expected => return,
            _ => {}
        }
        match db.clear_auth_token_if_current(expected) {
            Ok(_) => return,
            Err(error) => {
                log::warn!("[auth] Retrying account session cleanup: {error}");
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
    emit_signed_out_with_storage_error(app, None);
}

fn emit_auth_storage_error_cleared<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    let _ = app.emit("auth-storage-error-cleared", AUTH_CLEANUP_PENDING);
}

fn emit_signed_out_with_storage_error<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    storage_error: Option<String>,
) {
    let _ = app.emit(
        "auth-state-changed",
        &AuthStatePayload {
            authenticated: false,
            user: None,
            storage_error,
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

const AUTH_CLEANUP_PENDING: &str =
    "Account access is disabled, but local sign-out cleanup could not be saved. Codemux will retry.";

fn notify_rejected_local_session<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    db: &DatabaseStore,
    generation: u64,
    expected: &[u8],
    cleanup_pending: bool,
) -> Option<u64> {
    let auth_state = app.state::<AuthState>();
    match finish_rejected_local_session(
        app, db, &auth_state, generation, expected, cleanup_pending,
    ) {
        Ok(assigned) => assigned,
        Err(error) => {
            log::warn!("[auth] Could not finish rejected local sign-out: {error}");
            None
        }
    }
}

fn clear_rejected_local_session_warning<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    db: &DatabaseStore,
    assigned: u64,
) {
    let auth_state = app.state::<AuthState>();
    auth_state.commit_if_session_current(assigned, || {
        if matches!(db.load_raw_auth_token(), Ok(None)) {
            emit_auth_storage_error_cleared(app);
        }
    });
}

/// A definitive rejection ends this local session even if SQLite could not
/// commit its revocation. Inspect the raw row because the rejection set hides
/// the old record; a different row belongs to a newer CLI sign-in.
fn finish_rejected_local_session<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    db: &DatabaseStore,
    auth_state: &AuthState,
    generation: u64,
    expected: &[u8],
    cleanup_pending: bool,
) -> Result<Option<u64>, String> {
    auth_state
        .replace_session_if_current(generation, || {
            // A read error cannot prove that another account replaced this
            // one. Fail closed for the rejected local session and keep the
            // cleanup warning until storage can be inspected again.
            let (persisted, inspection_failed) = match db.load_raw_auth_token() {
                Ok(persisted) => (persisted, false),
                Err(error) => {
                    log::warn!("[auth] Could not inspect rejected auth record: {error}");
                    (Some(expected.to_vec()), true)
                }
            };
            if persisted.as_deref().is_some_and(|current| current != expected) {
                return Ok(None);
            }
            auth_state.clear_local_session();
            emit_signed_out_with_storage_error(
                app,
                ((cleanup_pending || inspection_failed) && persisted.is_some())
                    .then(|| AUTH_CLEANUP_PENDING.to_string()),
            );
            Ok(Some(()))
        })
        .map(|replaced| replaced.map(|(assigned, ())| assigned))
}

/// End a definitively rejected local session now and retry its exact-record
/// SQLite revocation in the background. The assigned generation owns only the
/// pending-cleanup warning, so a retry cannot sign out a replacement account.
pub(crate) fn start_rejected_auth_cleanup_retry<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    db: &DatabaseStore,
    auth_state: &AuthState,
    generation: u64,
    expected: Vec<u8>,
) -> Result<Option<u64>, String> {
    let local_transition = finish_rejected_local_session(
        app, db, auth_state, generation, &expected, true,
    );
    let retry_transition = local_transition.clone();
    let retry_app = app.clone();
    tauri::async_runtime::spawn(async move {
        let db = retry_app.state::<DatabaseStore>();
        retry_auth_revocation(&db, &expected).await;
        let auth_state = retry_app.state::<AuthState>();
        match db.load_raw_auth_token() {
            Ok(None) => {
                if let Ok(Some(assigned)) = retry_transition {
                    clear_rejected_local_session_warning(&retry_app, &db, assigned);
                } else if let Err(error) = finish_rejected_local_session(
                    &retry_app, &db, &auth_state, generation, &expected, false,
                ) {
                    log::warn!("[auth] Could not finish rejected local sign-out: {error}");
                }
            }
            Ok(Some(current)) if current == expected => {
                // The retry stopped without durable cleanup. Keep the warning
                // until the row is actually gone.
                if !matches!(retry_transition, Ok(Some(_))) {
                    if let Err(error) = finish_rejected_local_session(
                        &retry_app, &db, &auth_state, generation, &expected, true,
                    ) {
                        log::warn!("[auth] Could not finish rejected local sign-out: {error}");
                    }
                }
            }
            Ok(Some(_)) => {} // A newer CLI sign-in owns the row.
            Err(error) => log::warn!("[auth] Could not inspect auth cleanup: {error}"),
        }
    });
    local_transition
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
                settings: auth_state.cached_settings().unwrap_or_default(),
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
            let settings = auth_state.cached_settings().unwrap_or_default();
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
                            auth_state.cached_settings().unwrap_or_default(),
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
                    match start_rejected_auth_cleanup_retry(
                        &app,
                        &db,
                        &auth_state,
                        session_generation,
                        record.encrypted().to_vec(),
                    ) {
                        Ok(Some(_)) => Ok(signed_out_refresh()),
                        Ok(None) => Ok(current_local_refresh(&db, &auth_state)),
                        Err(inspect_error) => Err(format!("{error}; {inspect_error}")),
                    }
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
                        auth_state.cached_settings().unwrap_or_default(),
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
    let (csrf_state, cleaned_rejection) = auth_state.generate_csrf_state(&db)?;
    if cleaned_rejection {
        emit_auth_storage_error_cleared(&app);
    }

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

    let (generation, launch) = auth_state.session_snapshot(|| {
        let cleaned_rejection = db.settle_rejected_auth_before_sign_in()?;
        Ok::<_, String>((db.auth_session_revision()?, cleaned_rejection))
    });
    let (revision, cleaned_rejection) = launch?;
    if cleaned_rejection {
        emit_auth_storage_error_cleared(&app);
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

    let result = auth_state.replace_session_if_current(generation, || {
        let Some(record) = save_auth_at_revision(
            &db,
            revision,
            &api_resp.token,
            &api_resp.expires_at,
            Some(&user),
            "email",
        )?
        else {
            return Ok(None);
        };
        auth_state.set_local_session(Some(&record.stored));
        let payload = AuthStatePayload {
            authenticated: true,
            user: Some(user.clone()),
            storage_error: None,
        };
        let _ = app.emit("auth-state-changed", &payload);
        let _ = app.emit(
            "sync-state-changed",
            &SyncStatus {
                sync_available: true,
                auth_method: record.stored.auth_method,
            },
        );
        Ok(Some(AuthResponse {
            token: api_resp.token,
            expires_at: api_resp.expires_at,
            user,
        }))
    })?;
    result
        .map(|(_, response)| response)
        .ok_or_else(|| SIGN_IN_SESSION_CHANGED.into())
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

    async fn wait_for_account_browser(db: &DatabaseStore, browser_token: &str) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if crate::web_remote::auth::authenticate(db, browser_token).is_some() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("host should verify the account browser");
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

        let reopened = std::sync::Arc::new(DatabaseStore::test_file(&path));
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
        let verifier_db = reopened.clone();
        let verifier = tokio::spawn(async move {
            let auth_state = AuthState::default();
            verify_stored_account_for_remote_with_poll(
                &verifier_db, &auth_state, Duration::from_millis(20), Duration::from_secs(30),
                |_, _, _, _| None, |_, _| {},
            ).await;
        });
        wait_for_account_browser(&reopened, browser_token).await;
        verifier.abort();
        let _ = verifier.await;
        mock.assert_async().await;
        assert!(crate::web_remote::auth::authenticate(&reopened, browser_token).is_some());
    }

    #[tokio::test]
    #[serial_test::serial(mockserver)]
    async fn running_host_verifies_cli_login_without_reopening_its_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.sqlite");
        let host = std::sync::Arc::new(DatabaseStore::test_file(&path));
        let cli = DatabaseStore::test_file(&path);
        let browser_token = "new-cli-browser";
        assert!(load_auth_record(&host).is_none());

        let mut server = mockito::Server::new_async().await;
        let mock = server.mock("GET", "/api/auth/desktop/verify")
            .match_header("authorization", "Bearer cli-host-token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::json!({
                "user": { "id": "u1", "email": "u1@example.com", "name": null, "image": null }
            }).to_string())
            .create_async().await;
        let _api = ApiUrlGuard::set(&server.url());
        let verifier_db = host.clone();
        let verifier = tokio::spawn(async move {
            let auth_state = AuthState::default();
            verify_stored_account_for_remote_with_poll(
                &verifier_db, &auth_state, Duration::from_millis(20), Duration::from_secs(30),
                |_, _, _, _| None, |_, _| {},
            ).await;
        });
        tokio::time::sleep(Duration::from_millis(50)).await;

        crate::auth::save_auth(
            &cli, "cli-host-token", "2099-01-01T00:00:00Z", Some(&cached_user()),
        ).unwrap();
        cli.web_remote_insert_account_session(
            "cli-account-browser", None, None,
            &crate::web_remote::auth::sha256_hex(browser_token), true, "u1",
        ).unwrap();
        assert!(crate::web_remote::auth::authenticate(&host, browser_token).is_none());

        wait_for_account_browser(&host, browser_token).await;
        verifier.abort();
        let _ = verifier.await;
        mock.assert_async().await;
    }

    #[tokio::test]
    #[serial_test::serial(mockserver)]
    async fn running_host_renews_proof_after_peer_rejects_without_a_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.sqlite");
        let host = std::sync::Arc::new(DatabaseStore::test_file(&path));
        let peer = DatabaseStore::test_file(&path);
        crate::auth::save_auth(
            &host, "host-token", "2099-01-01T00:00:00Z", Some(&cached_user()),
        ).unwrap();
        let record = host.load_auth_token().unwrap();
        let browser_token = "peer-rejection-browser";
        host.web_remote_insert_account_session(
            "account-browser", None, None,
            &crate::web_remote::auth::sha256_hex(browser_token), true, "u1",
        ).unwrap();

        let mut valid_server = mockito::Server::new_async().await;
        let _valid = valid_server.mock("GET", "/api/auth/desktop/verify")
            .match_header("authorization", "Bearer host-token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(serde_json::json!({
                "user": { "id": "u1", "email": "u1@example.com", "name": null, "image": null }
            }).to_string())
            .create_async().await;
        let _api = ApiUrlGuard::set(&valid_server.url());
        let verifier_db = host.clone();
        let verifier = tokio::spawn(async move {
            let auth_state = AuthState::default();
            verify_stored_account_for_remote_with_poll(
                &verifier_db, &auth_state, Duration::from_millis(20), Duration::from_millis(100),
                |_, _, _, _| None, |_, _| {},
            ).await;
        });
        wait_for_account_browser(&host, browser_token).await;

        peer.test_set_query_only(true);
        assert!(peer.clear_auth_token_if_current(&record).is_err());
        assert!(crate::web_remote::auth::authenticate(&host, browser_token).is_some());

        let mut rejecting_server = mockito::Server::new_async().await;
        let rejected = rejecting_server.mock("GET", "/api/auth/desktop/verify")
            .match_header("authorization", "Bearer host-token")
            .with_status(401)
            .create_async().await;
        std::env::set_var("CODEMUX_API_URL", rejecting_server.url());
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if host.load_raw_auth_token().unwrap().is_none()
                    && crate::web_remote::auth::authenticate(&host, browser_token).is_none()
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }).await.expect("running host should reject the token on renewal");
        verifier.abort();
        let _ = verifier.await;
        rejected.assert_async().await;
    }

    #[tokio::test]
    #[serial_test::serial(mockserver)]
    async fn rejected_session_signs_out_locally_while_sqlite_cleanup_retries() {
        use tauri::Listener;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.sqlite");
        let settings_dir = dir.path().join("settings");
        std::fs::create_dir_all(&settings_dir).unwrap();
        let cache = settings_dir.join("settings-cache.json");
        let dirty = settings_dir.join("settings-dirty");
        let unrelated = dir.path().join("unrelated.json");
        std::fs::write(&cache, "cached settings").unwrap();
        std::fs::write(&dirty, "dirty").unwrap();
        std::fs::write(&unrelated, "keep").unwrap();
        let app = tauri::test::mock_app();
        assert!(app.manage(DatabaseStore::test_file(&path)));
        assert!(app.manage(AuthState::with_test_settings_cache_dir(settings_dir)));
        let db = app.state::<DatabaseStore>();
        let auth_state = app.state::<AuthState>();
        let record = crate::auth::save_auth_with_method(
            &db, "rejected-token", "2099-01-01T00:00:00Z", Some(&cached_user()), Some("email"),
        ).unwrap();
        auth_state.set_auth_method(Some("email"));

        let received = std::sync::Arc::new(std::sync::Mutex::new(Vec::<AuthStatePayload>::new()));
        let captured = received.clone();
        app.listen("auth-state-changed", move |event| {
            captured.lock().unwrap().push(serde_json::from_str(event.payload()).unwrap());
        });
        let warnings_cleared = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let cleared = warnings_cleared.clone();
        app.listen("auth-storage-error-cleared", move |_| {
            cleared.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        let mut server = mockito::Server::new_async().await;
        let mock = server.mock("GET", "/api/auth/desktop/verify")
            .match_header("authorization", "Bearer rejected-token")
            .with_status(401)
            .create_async().await;
        let _api = ApiUrlGuard::set(&server.url());
        db.test_set_query_only(true);

        let result = refresh_session(
            app.handle().clone(), app.state::<DatabaseStore>(), app.state::<AuthState>(),
        ).await.unwrap();
        mock.assert_async().await;
        assert_eq!(result.status, "signed-out");
        assert!(!result.authenticated);
        assert_eq!(auth_state.auth_method(), None);
        assert!(!cache.exists());
        assert!(!dirty.exists());
        assert!(unrelated.exists());
        assert!(db.load_auth_token().is_none());
        assert_eq!(db.load_raw_auth_token().unwrap().as_deref(), Some(record.encrypted()));
        assert_eq!(received.lock().unwrap().last().unwrap().storage_error.as_deref(), Some(AUTH_CLEANUP_PENDING));

        db.test_set_query_only(false);
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                if db.load_raw_auth_token().unwrap().is_none()
                    && warnings_cleared.load(std::sync::atomic::Ordering::SeqCst) > 0
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }).await.expect("background revocation should persist after write recovery");
        assert_eq!(received.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    #[serial_test::serial(mockserver)]
    async fn background_rejection_signs_out_an_offline_session_and_clears_its_warning() {
        use tauri::Listener;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.sqlite");
        let settings_dir = dir.path().join("settings");
        std::fs::create_dir_all(&settings_dir).unwrap();
        let cache = settings_dir.join("settings-cache.json");
        let dirty = settings_dir.join("settings-dirty");
        std::fs::write(&cache, "cached settings").unwrap();
        std::fs::write(&dirty, "dirty").unwrap();
        let app = tauri::test::mock_app();
        assert!(app.manage(DatabaseStore::test_file(&path)));
        assert!(app.manage(AuthState::with_test_settings_cache_dir(settings_dir)));
        let db = app.state::<DatabaseStore>();
        let auth_state = app.state::<AuthState>();
        let record = crate::auth::save_auth_with_method(
            &db, "rejected-token", "2099-01-01T00:00:00Z", Some(&cached_user()), Some("email"),
        ).unwrap();
        auth_state.set_auth_method(Some("email"));
        let received = std::sync::Arc::new(std::sync::Mutex::new(Vec::<AuthStatePayload>::new()));
        let captured = received.clone();
        app.listen("auth-state-changed", move |event| {
            captured.lock().unwrap().push(serde_json::from_str(event.payload()).unwrap());
        });
        let warnings_cleared = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let cleared = warnings_cleared.clone();
        app.listen("auth-storage-error-cleared", move |_| {
            cleared.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });

        let mut offline_server = mockito::Server::new_async().await;
        let offline_mock = offline_server.mock("GET", "/api/auth/desktop/verify")
            .match_header("authorization", "Bearer rejected-token")
            .with_status(503).create_async().await;
        {
            let _api = ApiUrlGuard::set(&offline_server.url());
            let result = refresh_session(
                app.handle().clone(), app.state::<DatabaseStore>(), app.state::<AuthState>(),
            ).await.unwrap();
            assert_eq!(result.status, "offline");
            assert!(result.authenticated);
        }
        offline_mock.assert_async().await;
        assert_eq!(auth_state.auth_method().as_deref(), Some("email"));

        let mut rejecting_server = mockito::Server::new_async().await;
        let rejecting_mock = rejecting_server.mock("GET", "/api/auth/desktop/verify")
            .match_header("authorization", "Bearer rejected-token")
            .with_status(401).create_async().await;
        let _api = ApiUrlGuard::set(&rejecting_server.url());
        db.test_set_query_only(true);
        let verifier_app = app.handle().clone();
        let verifier = tokio::spawn(async move {
            let db = verifier_app.state::<DatabaseStore>();
            let auth_state = verifier_app.state::<AuthState>();
            let notifying_app = verifier_app.clone();
            let cleanup_app = verifier_app.clone();
            verify_stored_account_for_remote_with_poll(
                &db, &auth_state, Duration::from_millis(20), Duration::from_secs(30),
                move |db, generation, expected, pending| {
                    notify_rejected_local_session(&notifying_app, db, generation, expected, pending)
                },
                move |db, assigned| clear_rejected_local_session_warning(&cleanup_app, db, assigned),
            ).await;
        });
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if received.lock().unwrap().last().is_some_and(|event| {
                    event.storage_error.as_deref() == Some(AUTH_CLEANUP_PENDING)
                }) {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }).await.expect("watcher should sign out locally after a 401");
        let signed_out_generation = auth_state.session_snapshot(|| ()).0;
        assert_eq!(auth_state.auth_method(), None);
        assert!(db.load_auth_token().is_none());
        assert_eq!(db.load_raw_auth_token().unwrap().as_deref(), Some(record.encrypted()));
        assert!(!cache.exists());
        assert!(!dirty.exists());

        db.test_set_query_only(false);
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                if db.load_raw_auth_token().unwrap().is_none()
                    && warnings_cleared.load(std::sync::atomic::Ordering::SeqCst) > 0
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }).await.expect("watcher should clear the warning after revocation persists");
        assert_eq!(received.lock().unwrap().len(), 1);
        assert_eq!(auth_state.session_snapshot(|| ()).0, signed_out_generation);
        verifier.abort();
        let _ = verifier.await;
        rejecting_mock.assert_async().await;
    }

    #[tokio::test]
    async fn stale_watcher_rejection_after_sign_out_preserves_new_oauth_launch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.sqlite");
        let settings_dir = dir.path().join("settings");
        std::fs::create_dir_all(&settings_dir).unwrap();
        let app = tauri::test::mock_app();
        assert!(app.manage(DatabaseStore::test_file(&path)));
        assert!(app.manage(AuthState::with_test_settings_cache_dir(settings_dir.clone())));
        let db = app.state::<DatabaseStore>();
        let auth_state = app.state::<AuthState>();
        let old = crate::auth::save_auth_with_method(
            &db, "old-token", "2099-01-01T00:00:00Z", Some(&cached_user()), Some("email"),
        ).unwrap();
        auth_state.set_auth_method(Some("email"));
        let (old_generation, ()) = auth_state.session_snapshot(|| ());

        // The verifier captured the old record before the remote 401 arrived.
        auth_state.try_replace_session(|| {
            db.clear_auth_token()?;
            auth_state.clear_local_session();
            Ok(())
        }).unwrap();
        let (csrf, _) = auth_state.generate_csrf_state(&db).unwrap();
        let (oauth_generation, ()) = auth_state.session_snapshot(|| ());
        let new_cache = settings_dir.join("settings-cache.json");
        std::fs::write(&new_cache, "new flow").unwrap();

        reject_stored_account(
            &db,
            old.encrypted(),
            old_generation,
            &|db, generation, expected, pending| {
                notify_rejected_local_session(app.handle(), db, generation, expected, pending)
            },
            &|db, assigned| clear_rejected_local_session_warning(app.handle(), db, assigned),
        ).await;

        assert_eq!(auth_state.session_snapshot(|| ()).0, oauth_generation);
        assert!(new_cache.exists());
        assert!(auth_state.validate_csrf_state(&csrf));
        assert!(auth_state.replace_session_if_current(oauth_generation, || Ok(Some(()))).unwrap().is_some());
        assert!(db.load_raw_auth_token().unwrap().is_none());
    }

    #[test]
    #[serial_test::serial(mockserver)]
    fn rejected_record_does_not_sign_out_a_replacement_cli_account() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("auth.sqlite");
        let app = tauri::test::mock_app();
        assert!(app.manage(DatabaseStore::test_file(&path)));
        assert!(app.manage(AuthState::with_test_settings_cache_dir(dir.path().join("settings"))));
        let db = app.state::<DatabaseStore>();
        let auth_state = app.state::<AuthState>();
        let old = crate::auth::save_auth_with_method(
            &db, "old-token", "2099-01-01T00:00:00Z", Some(&cached_user()), Some("email"),
        ).unwrap();
        auth_state.set_auth_method(Some("email"));
        let (generation, ()) = auth_state.session_snapshot(|| ());
        db.test_set_query_only(true);
        assert!(db.clear_auth_token_if_current(old.encrypted()).is_err());

        let cli = DatabaseStore::test_file(&path);
        let replacement = crate::auth::save_auth_with_method(
            &cli, "new-token", "2099-01-01T00:00:00Z", Some(&cached_user()), Some("email"),
        ).unwrap();
        assert_eq!(finish_rejected_local_session(
            app.handle(), &db, &auth_state, generation, old.encrypted(), true,
        ).unwrap(), None);
        assert_eq!(auth_state.auth_method().as_deref(), Some("email"));
        assert_eq!(db.load_raw_auth_token().unwrap().as_deref(), Some(replacement.encrypted()));
        db.test_set_query_only(false);
        assert!(!db.clear_auth_token_if_current(old.encrypted()).unwrap());
        assert_eq!(db.load_raw_auth_token().unwrap().as_deref(), Some(replacement.encrypted()));
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
