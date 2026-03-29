use tauri::{Emitter, State};

use crate::auth::{
    api_base_url, clear_token, is_token_expired, load_cached_user, load_token, save_auth,
    AuthResponse, AuthState, AuthStatePayload, AuthUser,
};
use crate::database::DatabaseStore;

#[tauri::command]
pub async fn start_oauth_flow(
    app: tauri::AppHandle,
    auth_state: State<'_, AuthState>,
) -> Result<(), String> {
    let csrf_state = auth_state.generate_csrf_state();

    // Start localhost callback server
    let auth_arc = std::sync::Arc::new(AuthState::default());
    // Transfer the CSRF state to the server's state
    {
        let mut states = auth_arc.csrf_states.lock().unwrap();
        states.insert(csrf_state.clone(), std::time::Instant::now());
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
    email: String,
    password: String,
) -> Result<AuthResponse, String> {
    if email.is_empty() || password.is_empty() {
        return Err("Email and password are required".into());
    }

    let base = api_base_url();
    let url = format!("{base}/api/auth/desktop/signin");

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .json(&serde_json::json!({ "email": email, "password": password }))
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;

    if !resp.status().is_success() {
        let body: serde_json::Value = resp.json().await.unwrap_or_default();
        let msg = body["error"]
            .as_str()
            .unwrap_or("Authentication failed");
        return Err(msg.to_string());
    }

    let api_resp: ApiAuthResp = resp
        .json()
        .await
        .map_err(|e| format!("Parse response: {e}"))?;

    let user = AuthUser {
        id: api_resp.user.id.clone(),
        email: api_resp.user.email.clone(),
        name: api_resp.user.name.clone(),
        image: api_resp.user.image.clone(),
    };

    save_auth(&db, &api_resp.token, &api_resp.expires_at, Some(&user))?;

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

    let base = api_base_url();
    let url = format!("{base}/api/auth/desktop/signup");

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .json(&serde_json::json!({
            "email": email,
            "password": password,
            "name": name,
        }))
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;

    if !resp.status().is_success() {
        let body: serde_json::Value = resp.json().await.unwrap_or_default();
        let msg = body["error"]
            .as_str()
            .unwrap_or("Sign-up failed");
        return Err(msg.to_string());
    }

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
) -> Result<Option<AuthUser>, String> {
    let (token, expires_at) = match load_token(&db) {
        Some(t) => t,
        None => return Ok(None),
    };

    if is_token_expired(&expires_at) {
        clear_token(&db);
        let payload = AuthStatePayload {
            authenticated: false,
            user: None,
        };
        let _ = app.emit("auth-state-changed", &payload);
        return Ok(None);
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

            // Background-fetch synced settings after successful auth
            let settings_handle = app.clone();
            let settings_token = token.clone();
            tauri::async_runtime::spawn(async move {
                match crate::settings_sync::fetch_settings(&settings_token).await {
                    Ok(s) => {
                        let _ = settings_handle.emit("settings-synced", &s);
                    }
                    Err(e) => {
                        eprintln!("[settings-sync] Background fetch failed: {e}");
                    }
                }
            });

            Ok(Some(user))
        }
        Ok(r) if r.status() == reqwest::StatusCode::UNAUTHORIZED => {
            clear_token(&db);
            let payload = AuthStatePayload {
                authenticated: false,
                user: None,
            };
            let _ = app.emit("auth-state-changed", &payload);
            Ok(None)
        }
        Ok(_) => Ok(load_cached_user(&db)),
        Err(_) => Ok(load_cached_user(&db)),
    }
}

#[tauri::command]
pub fn sign_out(app: tauri::AppHandle, db: State<'_, DatabaseStore>) -> Result<(), String> {
    clear_token(&db);
    crate::settings_sync::clear_cache();

    // Reset frontend settings store to defaults before auth-state-changed
    let _ = app.emit("settings-synced", &crate::settings_sync::UserSettings::default());

    let payload = AuthStatePayload {
        authenticated: false,
        user: None,
    };
    let _ = app.emit("auth-state-changed", &payload);
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

// ── Internal types for API deserialization ────────────────────────

#[derive(Debug, serde::Deserialize)]
struct ApiAuthResp {
    token: String,
    #[serde(rename = "expiresAt")]
    expires_at: String,
    user: ApiUserResp,
}

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
