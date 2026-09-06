use std::time::Instant;
use tauri::{Emitter, State};

use crate::auth::{
    api_base_url, clear_token, derive_auth_secret, is_token_expired, load_cached_user,
    load_stored_auth_method, load_token, login_email_api, save_auth, save_stored_auth_method,
    signup_email_api, AuthResponse, AuthState, AuthStatePayload, AuthUser,
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

/// Re-read whichever local account is current after a stale remote operation.
/// The session-generation guard is held while the token/user/cache owner are
/// paired, so an old refresh can never return user A with user B's settings.
fn current_local_refresh(db: &DatabaseStore, auth_state: &AuthState) -> SessionRefresh {
    let (_, refresh) = auth_state.session_snapshot(|| {
        let Some((_token, expires_at)) = load_token(db) else {
            crate::settings_sync::set_cache_owner(None);
            auth_state.set_auth_method(None);
            return signed_out_refresh();
        };
        if is_token_expired(&expires_at) {
            crate::settings_sync::set_cache_owner(None);
            auth_state.set_auth_method(None);
            return signed_out_refresh();
        }
        let user = load_cached_user(db);
        crate::settings_sync::set_cache_owner(user.as_ref().map(|user| user.id.as_str()));
        let auth_method = load_stored_auth_method(db);
        auth_state.set_auth_method(auth_method.as_deref());
        // A valid token with no cached user has not established identity
        // yet; report it pending so post-paint verification keeps retrying.
        SessionRefresh {
            authenticated: user.is_some(),
            settings: crate::settings_sync::load_cache().unwrap_or_default(),
            status: if user.is_some() {
                "local"
            } else {
                "pending-verification"
            },
            user,
            auth_method,
        }
    });
    refresh
}

/// Local-only first-stage startup. No socket, subprocess, or remote request is
/// performed, so a valid cached session and settings can paint immediately.
#[tauri::command]
pub fn bootstrap_session(
    db: State<'_, DatabaseStore>,
    auth_state: State<'_, AuthState>,
) -> SessionBootstrap {
    let auth_started = Instant::now();
    loop {
        let (session_generation, stored_token) =
            auth_state.session_snapshot(|| load_token(&db));
        let bootstrap = match stored_token {
            None => auth_state.commit_if_session_current(session_generation, || {
                crate::settings_sync::set_cache_owner(None);
                auth_state.set_auth_method(None);
                let settings_started = Instant::now();
                let settings = crate::settings_sync::load_cache().unwrap_or_default();
                crate::diagnostics::record_perf_timing(
                    "settings.local-cache-load",
                    settings_started.elapsed(),
                );
                SessionBootstrap {
                    authenticated: false,
                    user: None,
                    settings,
                    auth_method: None,
                    status: "signed-out",
                }
            }),
            Some((_token, expires_at)) if is_token_expired(&expires_at) => auth_state
                .replace_session_if_current(session_generation, || {
                    clear_token(&db);
                    crate::settings_sync::clear_cache();
                    crate::settings_sync::set_cache_owner(None);
                    auth_state.set_auth_method(None);
                    SessionBootstrap {
                        authenticated: false,
                        user: None,
                        settings: Default::default(),
                        auth_method: None,
                        status: "signed-out",
                    }
                })
                .map(|(_, bootstrap)| bootstrap),
            Some(_) => auth_state.commit_if_session_current(session_generation, || {
                // Pair identity, owner scope, auth method, and cached settings
                // while the same generation guard is held. A concurrent
                // sign-out/sign-in cannot make bootstrap return user A with
                // account B's settings or reset the global cache owner to A.
                let user = load_cached_user(&db);
                crate::settings_sync::set_cache_owner(
                    user.as_ref().map(|user| user.id.as_str()),
                );
                let auth_method = load_stored_auth_method(&db);
                auth_state.set_auth_method(auth_method.as_deref());
                let settings_started = Instant::now();
                let settings = crate::settings_sync::load_cache().unwrap_or_default();
                crate::diagnostics::record_perf_timing(
                    "settings.local-cache-load",
                    settings_started.elapsed(),
                );
                SessionBootstrap {
                    authenticated: user.is_some(),
                    status: if user.is_some() {
                        "local"
                    } else {
                        "pending-verification"
                    },
                    user,
                    settings,
                    auth_method,
                }
            }),
        };
        if let Some(bootstrap) = bootstrap {
            crate::diagnostics::record_perf_timing(
                "auth.local-bootstrap",
                auth_started.elapsed(),
            );
            return bootstrap;
        }
        // The account changed between the token read and the guarded commit.
        // Re-read the new generation instead of publishing mixed local state.
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
    let (session_generation, (stored_token, auth_method)) =
        auth_state.session_snapshot(|| (load_token(&db), load_stored_auth_method(&db)));
    let Some((token, expires_at)) = stored_token else {
        return Ok(auth_state
            .commit_if_session_current(session_generation, || {
                crate::settings_sync::set_cache_owner(None);
                auth_state.set_auth_method(None);
                signed_out_refresh()
            })
            .unwrap_or_else(|| current_local_refresh(&db, &auth_state)));
    };
    if is_token_expired(&expires_at) {
        let cleared = auth_state.replace_session_if_current(session_generation, || {
            clear_token(&db);
            crate::settings_sync::clear_cache();
            crate::settings_sync::set_cache_owner(None);
            auth_state.set_auth_method(None);
            emit_signed_out(&app);
            signed_out_refresh()
        });
        return Ok(cleared
            .map(|(_, refresh)| refresh)
            .unwrap_or_else(|| current_local_refresh(&db, &auth_state)));
    }
    let verify_started = Instant::now();
    let verify_outcome = verify_token(&token).await;
    crate::diagnostics::record_perf_timing("auth.remote-verify", verify_started.elapsed());

    match verify_outcome {
        VerifyOutcome::Verified(user) => {
            let committed = auth_state.commit_if_session_current(session_generation, || {
                // Remote verification is authoritative. A transient local
                // DB/keyring write failure must not sign out a session the
                // server just accepted; log it and proceed verified.
                if let Err(err) = save_auth(&db, &token, &expires_at, Some(&user)) {
                    eprintln!("[refresh_session] persist verified session failed: {err}");
                }
                crate::settings_sync::set_cache_owner(Some(&user.id));
            });
            if committed.is_none() {
                return Ok(current_local_refresh(&db, &auth_state));
            }
            let settings_sync_started = Instant::now();
            let settings_result =
                crate::settings_sync::sync_settings_for_owner(&token, &user.id).await;
            crate::diagnostics::record_perf_timing(
                "settings.remote-sync",
                settings_sync_started.elapsed(),
            );
            let (settings, status, settings_synced) = match settings_result {
                Ok(settings) => (settings, "verified", true),
                Err(_) => (
                    crate::settings_sync::load_cache().unwrap_or_default(),
                    "degraded",
                    false,
                ),
            };
            Ok(auth_state
                .commit_if_session_current(session_generation, || {
                    if settings_synced {
                        let _ = app.emit("settings-synced", &settings);
                    }
                    let _ = app.emit(
                        "sync-state-changed",
                        &SyncStatus {
                            sync_available: true,
                            auth_method: auth_method.clone(),
                        },
                    );
                    SessionRefresh {
                        authenticated: true,
                        user: Some(user),
                        settings,
                        auth_method,
                        status,
                    }
                })
                .unwrap_or_else(|| current_local_refresh(&db, &auth_state)))
        }
        VerifyOutcome::Unauthorized => Ok(auth_state
            .replace_session_if_current(session_generation, || {
                clear_token(&db);
                crate::settings_sync::clear_cache();
                crate::settings_sync::set_cache_owner(None);
                auth_state.set_auth_method(None);
                emit_signed_out(&app);
                signed_out_refresh()
            })
            .map(|(_, refresh)| refresh)
            .unwrap_or_else(|| current_local_refresh(&db, &auth_state))),
        VerifyOutcome::Offline => Ok(auth_state
            .commit_if_session_current(session_generation, || {
                let user = load_cached_user(&db);
                crate::settings_sync::set_cache_owner(user.as_ref().map(|user| user.id.as_str()));
                unreachable_verify_refresh(
                    user,
                    crate::settings_sync::load_cache().unwrap_or_default(),
                    auth_method,
                )
            })
            .unwrap_or_else(|| current_local_refresh(&db, &auth_state))),
    }
}

#[tauri::command]
pub async fn start_oauth_flow<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    auth_state: State<'_, AuthState>,
) -> Result<(), String> {
    let csrf_state = auth_state.generate_csrf_state();

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

    auth_state.replace_session(|| -> Result<(), String> {
        save_auth(&db, &api_resp.token, &api_resp.expires_at, Some(&user))?;
        crate::settings_sync::set_cache_owner(Some(&user.id));
        // Persist auth_method so a cold-start `check_auth` can restore it
        // and keep the value going through `sync-state-changed` in sync
        // with the persisted record.
        auth_state.set_auth_method(Some("email"));
        if let Err(err) = save_stored_auth_method(&db, Some("email")) {
            eprintln!("[signin_email] persist auth_method=email failed: {err}");
        }
        Ok(())
    })?;

    let auth_response = AuthResponse {
        token: api_resp.token.clone(),
        expires_at: api_resp.expires_at.clone(),
        user: user.clone(),
    };

    let payload = AuthStatePayload {
        authenticated: true,
        user: Some(auth_response.user.clone()),
    };
    let _ = app.emit("auth-state-changed", &payload);
    // Signed in → skills sync is available (server-side, no key).
    let _ = app.emit(
        "sync-state-changed",
        &SyncStatus {
            sync_available: true,
            auth_method: auth_state.auth_method(),
        },
    );

    Ok(auth_response)
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
    auth_state.replace_session(|| {
        clear_token(&db);
        crate::settings_sync::clear_cache();
        crate::settings_sync::set_cache_owner(None);

        // Clear the session's provider tag alongside auth.
        auth_state.set_auth_method(None);
    });

    // Reset frontend settings store to defaults before auth-state-changed
    let _ = app.emit(
        "settings-synced",
        &crate::settings_sync::UserSettings::default(),
    );

    let payload = AuthStatePayload {
        authenticated: false,
        user: None,
    };
    let _ = app.emit("auth-state-changed", &payload);
    let _ = app.emit(
        "sync-state-changed",
        &SyncStatus {
            sync_available: false,
            auth_method: None,
        },
    );
    Ok(())
}

#[tauri::command]
pub fn get_auth_token(
    db: State<'_, DatabaseStore>,
    auth_state: State<'_, AuthState>,
) -> Result<Option<String>, String> {
    loop {
        let (session_generation, stored_token) = auth_state.session_snapshot(|| load_token(&db));
        match stored_token {
            Some((_token, expires_at)) if is_token_expired(&expires_at) => {
                if auth_state
                    .replace_session_if_current(session_generation, || {
                        clear_token(&db);
                        crate::settings_sync::clear_cache();
                        crate::settings_sync::set_cache_owner(None);
                        auth_state.set_auth_method(None);
                    })
                    .is_some()
                {
                    return Ok(None);
                }
                // A signin replaced the expired record after our snapshot.
                // Re-read instead of clearing or returning the old account.
            }
            Some((token, _)) => return Ok(Some(token)),
            None => return Ok(None),
        }
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
    let signed_in =
        matches!(load_token(&db), Some((_t, ref expires)) if !is_token_expired(expires));
    SyncStatus {
        sync_available: signed_in,
        auth_method: auth_state.auth_method(),
    }
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

    fn cached_user() -> AuthUser {
        AuthUser {
            id: "u1".into(),
            email: "u1@example.com".into(),
            name: None,
            image: None,
        }
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
