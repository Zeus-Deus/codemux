use tauri::{Emitter, State};

use crate::auth::{
    api_base_url, clear_token, delete_encryption_key, derive_auth_secret,
    derive_login_credentials, is_token_expired, load_cached_user, load_encryption_key,
    load_stored_auth_method, load_token, login_email_api, save_auth, save_encryption_key,
    save_stored_auth_method, signup_email_api, AuthResponse, AuthState, AuthStatePayload,
    AuthUser,
};
use crate::database::DatabaseStore;
use crate::encryption::EncryptionManager;

/// Minimum length for the sync password. Matches Vexis (8 chars)
/// for cross-product UX consistency. The research doc and Vexis
/// both consciously chose 8 over 12 to keep the bar low for new
/// users; the actual entropy floor lives in Argon2id, not in the
/// password length itself.
const MIN_SYNC_PASSWORD_LEN: usize = 8;

/// Sync state surfaced to the frontend. Mirrors Vexis's
/// `AuthStatus.{syncAvailable, authMethod}` shape so the Settings
/// → Sync section can fork on `(authMethod, syncAvailable)` to pick
/// between the setup form (GitHub user, no key) and the repair
/// form (email user, no key) without needing additional server
/// roundtrips.
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
    encryption: State<'_, EncryptionManager>,
    email: String,
    password: String,
) -> Result<AuthResponse, String> {
    if email.is_empty() || password.is_empty() {
        return Err("Email and password are required".into());
    }

    // Bitwarden-style zero-knowledge derivation: stretch
    // (password, email) locally into a high-entropy AuthSecret and
    // a per-user EncryptionKey. The AuthSecret goes to Better Auth
    // in place of the raw password; the EncryptionKey stays
    // on-device for Step 10 skills sync. Must produce byte-identical
    // output to Vexis's derivation or cross-product login + sync
    // break — pinned by `auth_secret_matches_vexis_for_known_input`
    // and `encryption_key_matches_vexis_for_known_input` in
    // auth/derivation.rs.
    //
    // `login_email_api` takes `&AuthSecret`, not `&str` — the
    // compiler refuses to let us pass the raw password past this
    // point.
    let (auth_secret, encryption_key) = derive_login_credentials(&password, &email)?;
    drop(password); // raw password falls out of scope; only the derived secrets proceed.

    let api_resp = login_email_api(&email, &auth_secret).await?;

    let user = AuthUser {
        id: api_resp.user.id.clone(),
        email: api_resp.user.email.clone(),
        name: api_resp.user.name.clone(),
        image: api_resp.user.image.clone(),
    };

    save_auth(&db, &api_resp.token, &api_resp.expires_at, Some(&user))?;
    // Persist auth_method so a cold-start `check_auth` can restore
    // it; without this, an email user whose `sync-key.enc` was lost
    // would still see the right form (ProvidePasswordForm) on cold
    // start, but the value going through `sync-state-changed` would
    // disagree with the persisted record. Keep them in sync.
    if let Err(err) = save_stored_auth_method(&db, Some("email")) {
        eprintln!("[signin_email] persist auth_method=email failed: {err}");
    }

    // Email/password users get sync set up automatically on signin.
    // The EncryptionKey was derived alongside the AuthSecret above
    // (single Argon2id stretch — paying the cost twice would be
    // wasteful), so we just persist + load it now. A failure here is
    // logged but does NOT fail signin — the user is signed in
    // regardless; they can repair via the Settings → Sync "re-enter
    // password" form if persistence failed (rare: read-only
    // filesystem, full disk).
    auth_state.set_auth_method(Some("email"));
    let key_bytes = *encryption_key.expose_for_smoke_test();
    drop(encryption_key); // raw key bytes out of scope after this.
    if let Err(err) = save_encryption_key(&key_bytes) {
        eprintln!("[signin_email] save sync-key.enc failed: {err}");
    } else if let Err(err) = encryption.set_key(key_bytes) {
        eprintln!("[signin_email] EncryptionManager.set_key failed: {err}");
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
    let _ = app.emit(
        "sync-state-changed",
        &SyncStatus {
            sync_available: encryption.is_available(),
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
    encryption: State<'_, EncryptionManager>,
) -> Result<Option<AuthUser>, String> {
    let (token, expires_at) = match load_token(&db) {
        Some(t) => t,
        None => return Ok(None),
    };

    if is_token_expired(&expires_at) {
        clear_token(&db);
        encryption.clear();
        let _ = delete_encryption_key();
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

    // Cold-start sync-key load: if a previous session wrote
    // `sync-key.enc`, decrypt it and populate the in-memory
    // EncryptionManager. This is the path that makes the password
    // prompt once-per-device rather than once-per-launch — the
    // user typed their password at signin (or sync setup) and we
    // saved the key machine-bound; here we just reload it.
    //
    // A decrypt failure (key file copied from another machine,
    // /etc/machine-id rotated, file corrupted) is silently swallowed
    // and `sync_available` reports false — Stage 3 will route the
    // user through `provide_password_for_sync` to repair.
    //
    // Restore auth_method from the stored auth so the Settings →
    // Sync section can choose between SetupSyncPasswordForm (OAuth
    // user who never set up sync) and ProvidePasswordForm (had sync,
    // local key lost) on cold start. Legacy stored auths from before
    // this field existed deserialize to None; the frontend treats
    // that the same way it always has (defaults to ProvidePassword
    // for back-compat with existing email-user installs).
    if let Some(method) = load_stored_auth_method(&db) {
        auth_state.set_auth_method(Some(&method));
    }

    if let Ok(Some(key_bytes)) = load_encryption_key() {
        if let Err(err) = encryption.set_key(key_bytes) {
            eprintln!("[check_auth] EncryptionManager.set_key failed: {err}");
        }
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

            // Notify the frontend whether sync came back online.
            // `auth_method` is None on cold start (we don't persist
            // the signin path on disk); the frontend reads
            // `sync_available` to decide whether to show the
            // "Sync ready" badge or the inline setup form.
            let _ = app.emit(
                "sync-state-changed",
                &SyncStatus {
                    sync_available: encryption.is_available(),
                    auth_method: auth_state.auth_method(),
                },
            );

            Ok(Some(user))
        }
        Ok(r) if r.status() == reqwest::StatusCode::UNAUTHORIZED => {
            clear_token(&db);
            encryption.clear();
            let _ = delete_encryption_key();
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
    encryption: State<'_, EncryptionManager>,
) -> Result<(), String> {
    clear_token(&db);
    crate::settings_sync::clear_cache();

    // Tear down sync state alongside auth: in-memory key zeroed,
    // persisted `sync-key.enc` deleted, auth_method cleared. Same
    // hygiene as Vexis's logout — no key material survives a
    // signed-out app.
    encryption.clear();
    let _ = delete_encryption_key();
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
// Sync password setup / repair (Stage 2)
// ────────────────────────────────────────────────────────────────

/// Get the current sync state. Cheap — just reads in-memory
/// AuthState + EncryptionManager. The Settings → Sync section
/// calls this on mount and after any setup/repair action so the
/// fork between "needs setup" / "needs repair" / "ready" reflects
/// the truth without depending on cached frontend state.
#[tauri::command]
pub fn get_sync_status(
    auth_state: State<'_, AuthState>,
    encryption: State<'_, EncryptionManager>,
) -> SyncStatus {
    SyncStatus {
        sync_available: encryption.is_available(),
        auth_method: auth_state.auth_method(),
    }
}

/// One-time setup for GitHub-OAuth users to attach a password to
/// their account and enable skills sync. Mirrors Vexis's
/// `setup_sync_password` flow:
///
///   1. Validate password length.
///   2. Derive `(auth_secret, encryption_key)` from
///      `(password, user.email)` via the shared `codemux-api-*`
///      protocol.
///   3. POST `/api/auth/set-password` with the AuthSecret as the
///      password — Better Auth bcrypts it like any other password,
///      so the server has no idea this is a derived value. Doing
///      this BEFORE persisting the local key means a server-side
///      failure leaves the device's key state untouched and the
///      user can retry cleanly.
///   4. Persist the encryption_key machine-bound at
///      `~/.local/share/codemux/sync-key.enc`.
///   5. Load the key into the in-process `EncryptionManager` so
///      Stage 3's sync layer finds it on the next call.
///
/// Returns the updated SyncStatus (sync_available=true on success).
#[tauri::command]
pub async fn setup_sync_password(
    app: tauri::AppHandle,
    db: State<'_, DatabaseStore>,
    auth_state: State<'_, AuthState>,
    encryption: State<'_, EncryptionManager>,
    password: String,
) -> Result<SyncStatus, String> {
    if password.len() < MIN_SYNC_PASSWORD_LEN {
        return Err(format!(
            "Password must be at least {MIN_SYNC_PASSWORD_LEN} characters."
        ));
    }

    // Bearer token + user email come from the persisted auth.
    // The email is the salt input — every device must derive with
    // the same email or cross-device sync breaks silently.
    let (token, _expires_at) = load_token(&db).ok_or_else(|| "Not signed in".to_string())?;
    let user = load_cached_user(&db)
        .ok_or_else(|| "No cached user; sign in again before setting up sync.".to_string())?;

    let (auth_secret, key) = derive_login_credentials(&password, &user.email)
        .map_err(|e| format!("Failed to derive sync credentials: {e}"))?;

    set_password_api(&token, auth_secret.as_str())
        .await
        .map_err(|e| format!("Server rejected password: {e}"))?;

    let key_bytes = *key.expose_for_smoke_test();
    save_encryption_key(&key_bytes).map_err(|e| format!("Failed to persist sync key: {e}"))?;
    encryption
        .set_key(key_bytes)
        .map_err(|e| format!("Failed to load sync key into memory: {e}"))?;

    // OAuth users are exactly the population that hits this flow,
    // so the auth_method is "github" by construction. (Email
    // users would have already had sync enabled at signin and
    // would never reach setup_sync_password.) Persist on disk too
    // so cold-start `check_auth` restores the same value.
    auth_state.set_auth_method(Some("github"));
    if let Err(err) = save_stored_auth_method(&db, Some("github")) {
        eprintln!("[setup_sync_password] persist auth_method=github failed: {err}");
    }

    let status = SyncStatus {
        sync_available: true,
        auth_method: Some("github".into()),
    };
    let _ = app.emit("sync-state-changed", &status);
    Ok(status)
}

/// Repair flow: the user is signed in but the local
/// `sync-key.enc` file is missing or undecryptable (manual
/// deletion, machine-id rotation, file copied across machines).
/// Re-derive from the password they're now typing, persist locally,
/// and load into memory. **No server call** — the server already
/// has the right bcrypt; we're only rebuilding device-local state.
///
/// Wrong password → wrong key → Stage 3's first sync attempt will
/// fail to decrypt the user's existing skills. That's acceptable:
/// we don't verify against the server here for the same reason
/// Vexis doesn't (see research §4.1, "lazy key verification").
/// The Settings → Sync recovery panel routes the user through
/// either re-entering the password OR running `reset_sync` to
/// wipe the server-side ciphertext and start fresh.
#[tauri::command]
pub async fn provide_password_for_sync(
    app: tauri::AppHandle,
    db: State<'_, DatabaseStore>,
    auth_state: State<'_, AuthState>,
    encryption: State<'_, EncryptionManager>,
    password: String,
) -> Result<SyncStatus, String> {
    if password.len() < MIN_SYNC_PASSWORD_LEN {
        return Err(format!(
            "Password must be at least {MIN_SYNC_PASSWORD_LEN} characters."
        ));
    }

    let user = load_cached_user(&db)
        .ok_or_else(|| "No cached user; sign in again before repairing sync.".to_string())?;

    let (_auth_secret, key) = derive_login_credentials(&password, &user.email)
        .map_err(|e| format!("Failed to derive sync credentials: {e}"))?;

    let key_bytes = *key.expose_for_smoke_test();
    save_encryption_key(&key_bytes).map_err(|e| format!("Failed to persist sync key: {e}"))?;
    encryption
        .set_key(key_bytes)
        .map_err(|e| format!("Failed to load sync key into memory: {e}"))?;

    // Repair preserves whatever auth_method the session already
    // had — typically `None` after a cold-start `check_auth`.
    let status = SyncStatus {
        sync_available: true,
        auth_method: auth_state.auth_method(),
    };
    let _ = app.emit("sync-state-changed", &status);
    Ok(status)
}

/// HTTP helper. POST `/api/auth/set-password` with the derived
/// AuthSecret in the `newPassword` field (Better Auth's documented
/// schema; the codemux-api server's custom route also accepts
/// `password` as a transitional alias). Better Auth bcrypts it
/// server-side; nothing else changes. Bearer-authenticated.
async fn set_password_api(token: &str, auth_secret: &str) -> Result<(), String> {
    let url = format!("{}/api/auth/set-password", api_base_url());
    let resp = reqwest::Client::new()
        .post(&url)
        .bearer_auth(token)
        .json(&serde_json::json!({ "newPassword": auth_secret }))
        .send()
        .await
        .map_err(|e| format!("network: {e}"))?;

    let status = resp.status();
    if status.is_success() {
        Ok(())
    } else if status == reqwest::StatusCode::UNAUTHORIZED {
        Err("authentication expired — sign in again".into())
    } else {
        let msg = resp
            .text()
            .await
            .unwrap_or_else(|_| format!("HTTP {status}"));
        Err(msg)
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
