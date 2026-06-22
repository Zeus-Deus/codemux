use tauri::{Emitter, State};

use crate::auth::{
    api_base_url, clear_token, derive_auth_secret, is_token_expired, load_cached_user,
    load_stored_auth_method, load_token, login_email_api, save_auth,
    save_stored_auth_method, signup_email_api, AuthResponse, AuthState, AuthStatePayload,
    AuthUser,
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

#[tauri::command]
pub async fn start_oauth_flow(
    app: tauri::AppHandle,
    auth_state: State<'_, AuthState>,
) -> Result<(), String> {
    let csrf_state = auth_state.generate_csrf_state();

    // Start localhost callback server
    let auth_arc = std::sync::Arc::new(AuthState::default());
    // Transfer the CSRF state to the server's state.
    // SystemTime (not Instant) — see AuthState::csrf_states for the reasoning.
    {
        let mut states = auth_arc.csrf_states.lock().unwrap();
        states.insert(csrf_state.clone(), std::time::SystemTime::now());
    }

    let port = crate::auth::start_callback_server(auth_arc, app.clone())?;

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
pub async fn signin_email(
    app: tauri::AppHandle,
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

    save_auth(&db, &api_resp.token, &api_resp.expires_at, Some(&user))?;
    // Persist auth_method so a cold-start `check_auth` can restore it
    // and keep the value going through `sync-state-changed` in sync
    // with the persisted record.
    auth_state.set_auth_method(Some("email"));
    if let Err(err) = save_stored_auth_method(&db, Some("email")) {
        eprintln!("[signin_email] persist auth_method=email failed: {err}");
    }

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
pub async fn signup_email(
    email: String,
    password: String,
    name: String,
) -> Result<(), String> {
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
pub async fn check_auth(
    app: tauri::AppHandle,
    db: State<'_, DatabaseStore>,
    auth_state: State<'_, AuthState>,
) -> Result<Option<AuthUser>, String> {
    let (token, expires_at) = match load_token(&db) {
        Some(t) => t,
        None => return Ok(None),
    };

    if is_token_expired(&expires_at) {
        clear_token(&db);
        auth_state.set_auth_method(None);
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
        return Ok(None);
    }

    // Restore auth_method from the stored auth so the Settings →
    // Sync section can tailor its copy on cold start. Legacy stored
    // auths from before this field existed deserialize to None.
    if let Some(method) = load_stored_auth_method(&db) {
        auth_state.set_auth_method(Some(&method));
    }

    let base = api_base_url();
    let url = format!("{base}/api/auth/desktop/verify");

    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            let verify: VerifyResp = r.json().await.map_err(|e| e.to_string())?;
            let user = AuthUser {
                id: verify.user.id,
                email: verify.user.email,
                name: verify.user.name,
                image: verify.user.image,
            };

            // Cache user data for offline/network-error auth
            let _ = save_auth(&db, &token, &expires_at, Some(&user));

            // Background-sync settings after successful auth (fetch + flush dirty)
            let settings_handle = app.clone();
            let settings_token = token.clone();
            tauri::async_runtime::spawn(async move {
                match crate::settings_sync::sync_settings(&settings_token).await {
                    Ok(s) => {
                        let _ = settings_handle.emit("settings-synced", &s);
                    }
                    Err(e) => {
                        eprintln!("[settings-sync] Background sync failed: {e}");
                    }
                }
            });

            // Notify the frontend that skills sync is available —
            // the user is signed in, and server-side sync needs no
            // device-local key.
            let _ = app.emit(
                "sync-state-changed",
                &SyncStatus {
                    sync_available: true,
                    auth_method: auth_state.auth_method(),
                },
            );

            Ok(Some(user))
        }
        Ok(r) if r.status() == reqwest::StatusCode::UNAUTHORIZED => {
            clear_token(&db);
            auth_state.set_auth_method(None);
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
            Ok(None)
        }
        Ok(_) => Ok(load_cached_user(&db)),
        Err(_) => Ok(load_cached_user(&db)),
    }
}

#[tauri::command]
pub fn sign_out(
    app: tauri::AppHandle,
    db: State<'_, DatabaseStore>,
    auth_state: State<'_, AuthState>,
) -> Result<(), String> {
    clear_token(&db);
    crate::settings_sync::clear_cache();

    // Clear the session's provider tag alongside auth.
    auth_state.set_auth_method(None);

    // Reset frontend settings store to defaults before auth-state-changed
    let _ = app.emit("settings-synced", &crate::settings_sync::UserSettings::default());

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
pub fn get_auth_token(db: State<'_, DatabaseStore>) -> Result<Option<String>, String> {
    match load_token(&db) {
        Some((token, expires_at)) => {
            if is_token_expired(&expires_at) {
                clear_token(&db);
                Ok(None)
            } else {
                Ok(Some(token))
            }
        }
        None => Ok(None),
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
    let signed_in = matches!(load_token(&db), Some((_t, ref expires)) if !is_token_expired(expires));
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
