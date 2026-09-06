use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Instant, SystemTime};

use tauri::Manager;

use crate::commands::SyncStatus;
use crate::database::DatabaseStore;

// fs is used by machine_id() and token_file_path() (migration support)
use std::fs;

use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use rand::RngCore;
use sha2::{Digest, Sha256};
use tauri::Emitter;

// ── Submodules ───────────────────────────────────────────────────

pub mod api;
/// Headless sign-in/out for the `codemux` CLI (`login` / `logout` /
/// `whoami`). Lives beside the storage helpers it drives so the CLI and
/// the GUI's Tauri commands persist the identical auth record.
pub mod cli_login;
pub mod derivation;

// Re-export the zero-knowledge auth derivation + API client at the
// module root so command handlers can write
// `crate::auth::{derive_auth_secret, login_email_api}` alongside the
// other auth helpers. API client helpers are `pub(crate)` — they're
// only meant for use inside this crate's Tauri commands and
// intentionally don't leak to consumers linking against codemux_lib.
pub(crate) use api::{login_email_api, signup_email_api};
pub use derivation::{derive_auth_secret, derive_login_credentials, AuthSecret, EncryptionKey};

// ── Types ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthUser {
    pub id: String,
    pub email: String,
    pub name: Option<String>,
    pub image: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthResponse {
    pub user: AuthUser,
    pub token: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthStatePayload {
    pub authenticated: bool,
    pub user: Option<AuthUser>,
}

/// What the API returns from /desktop/verify
#[derive(Debug, Deserialize)]
struct VerifyResponse {
    user: ApiUser,
    #[allow(dead_code)]
    session: ApiSession,
}

#[derive(Debug, Deserialize)]
struct ApiUser {
    id: String,
    name: Option<String>,
    email: String,
    image: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ApiSession {
    #[serde(rename = "expiresAt")]
    expires_at: String,
}

/// Stored on disk (encrypted)
#[derive(Debug, Serialize, Deserialize)]
struct StoredAuth {
    token: String,
    expires_at: String,
    #[serde(default)]
    user: Option<AuthUser>,
    /// `"email"` or `"github"`. Persisted so a cold start can
    /// restore which signin path produced this session — the
    /// Settings → Sync section needs it to choose between the
    /// SetupSyncPasswordForm (OAuth-only user, never set up sync)
    /// and the ProvidePasswordForm (had sync, local key lost).
    /// Field is `Option` for backward compat with auth tokens
    /// written before this field existed; `#[serde(default)]`
    /// means a missing field deserializes to `None`.
    #[serde(default)]
    auth_method: Option<String>,
}

// ── Auth State (managed by Tauri) ────────────────────────────────

pub struct AuthState {
    /// CSRF state tokens keyed to their creation time and launching session
    /// generation. Uses `SystemTime`
    /// rather than `Instant` because on Windows `Instant` is backed by
    /// `QueryPerformanceCounter`, and `Instant::now() - Duration::from_secs(600)`
    /// panics with "overflow when subtracting duration from instant" during
    /// the first ~10 minutes after VM boot (observed on GitHub Actions
    /// windows-latest runners when cargo test runs quickly after job setup).
    /// `SystemTime` is wall-clock and Unix-epoch-based, so subtracting 600s
    /// from current time (~56 years past epoch) never underflows. Tradeoff:
    /// `SystemTime` is not monotonic, so NTP adjustments or manual clock
    /// changes could briefly skew CSRF token validity — acceptable for a
    /// 10-minute OAuth state token.
    pub(crate) csrf_states: Mutex<HashMap<String, (SystemTime, u64)>>,
    callback_port: Mutex<Option<u16>>,
    /// Tracks which signin path produced the current session —
    /// `"email"` after email/password, `"github"` after OAuth, or
    /// `None` after a cold-start `check_auth` (the path is not
    /// persisted on disk; the frontend infers from sync state).
    /// Cleared on `sign_out`. Mirrors Vexis's `AuthStatus.authMethod`
    /// which drives the Settings → Sync section's UI fork.
    auth_method: Mutex<Option<String>>,
    /// Serializes local session replacement against after-paint verification.
    /// A remote response may arrive after sign-out or another account signs
    /// in; its generation must then be unable to rewrite that newer session.
    session_generation: Mutex<u64>,
}

impl Default for AuthState {
    fn default() -> Self {
        Self {
            csrf_states: Mutex::new(HashMap::new()),
            callback_port: Mutex::new(None),
            auth_method: Mutex::new(None),
            session_generation: Mutex::new(0),
        }
    }
}

impl AuthState {
    /// Generate a CSRF state token and store it with a timestamp.
    pub fn generate_csrf_state(&self) -> String {
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        let state = base64_url_encode(&bytes);

        // Pair the callback with the session that launched it. Holding the
        // generation guard until the CSRF record is installed closes the gap
        // where sign-out could otherwise happen between those two actions.
        let generation = self.session_generation.lock().unwrap();
        let mut states = self.csrf_states.lock().unwrap();
        // Clean up expired entries (older than 10 minutes). See the
        // csrf_states field comment for why this uses SystemTime.
        let cutoff = SystemTime::now() - std::time::Duration::from_secs(600);
        states.retain(|_, (created_at, _)| *created_at > cutoff);
        states.insert(state.clone(), (SystemTime::now(), *generation));

        state
    }

    /// Validate and consume a CSRF state token (one-time use).
    pub fn validate_csrf_state(&self, state: &str) -> bool {
        self.consume_csrf_state(state).is_some()
    }

    /// Validate and consume an OAuth state token, returning the exact session
    /// generation that launched the flow. The callback may replace a session
    /// only if that generation is still current.
    pub fn consume_csrf_state(&self, state: &str) -> Option<u64> {
        let mut states = self.csrf_states.lock().unwrap();
        let cutoff = SystemTime::now() - std::time::Duration::from_secs(600);
        let (created_at, generation) = states.remove(state)?;
        (created_at > cutoff).then_some(generation)
    }

    pub fn set_callback_port(&self, port: u16) {
        *self.callback_port.lock().unwrap() = Some(port);
    }

    pub fn take_callback_port(&self) -> Option<u16> {
        self.callback_port.lock().unwrap().take()
    }

    pub fn set_auth_method(&self, method: Option<&str>) {
        *self.auth_method.lock().unwrap() = method.map(|m| m.to_string());
    }

    pub fn auth_method(&self) -> Option<String> {
        self.auth_method.lock().unwrap().clone()
    }

    /// Read session-backed state and its generation atomically with respect to
    /// [`Self::replace_session`] and [`Self::commit_if_session_current`].
    pub fn session_snapshot<T>(&self, read: impl FnOnce() -> T) -> (u64, T) {
        let generation = self.session_generation.lock().unwrap();
        (*generation, read())
    }

    /// Commit a remote result only while the session that launched it remains
    /// current. The closure runs under the same guard used by sign-in/out.
    pub fn commit_if_session_current<T>(
        &self,
        expected_generation: u64,
        commit: impl FnOnce() -> T,
    ) -> Option<T> {
        let generation = self.session_generation.lock().unwrap();
        (*generation == expected_generation).then(commit)
    }

    /// Install or clear an account as one ordered local session replacement.
    pub fn replace_session<T>(&self, replace: impl FnOnce() -> T) -> T {
        self.replace_session_with_generation(replace).1
    }

    /// Install or clear an account and return the generation assigned to that
    /// replacement. Callers that launch follow-up remote work must carry this
    /// exact generation rather than taking a later snapshot: a sign-out can
    /// otherwise land in the gap and make stale work look current again.
    pub fn replace_session_with_generation<T>(&self, replace: impl FnOnce() -> T) -> (u64, T) {
        let mut generation = self.session_generation.lock().unwrap();
        *generation = generation.saturating_add(1);
        let assigned_generation = *generation;
        (assigned_generation, replace())
    }

    /// Conditionally replace the current session and advance its generation.
    /// This is the definitive-clear counterpart to
    /// [`Self::commit_if_session_current`]: expiry/401 handling must invalidate
    /// other remote work launched for the token it just removed.
    pub fn replace_session_if_current<T>(
        &self,
        expected_generation: u64,
        replace: impl FnOnce() -> T,
    ) -> Option<(u64, T)> {
        let mut generation = self.session_generation.lock().unwrap();
        if *generation != expected_generation {
            return None;
        }
        *generation = generation.saturating_add(1);
        let assigned_generation = *generation;
        Some((assigned_generation, replace()))
    }
}

// ── API base URL ─────────────────────────────────────────────────

pub fn api_base_url() -> String {
    std::env::var("CODEMUX_API_URL").unwrap_or_else(|_| "https://api.codemux.org".into())
}

// ── Encrypted token storage ──────────────────────────────────────

const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
// AES-GCM tag is appended by the aes-gcm crate inside the ciphertext

pub(crate) fn token_file_path() -> PathBuf {
    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local/share"));
    data_dir.join(crate::APP_DIR_NAME).join("auth-token.enc")
}

fn machine_id() -> Vec<u8> {
    // Try /etc/machine-id (Linux), then fallback to hostname
    if let Ok(id) = fs::read_to_string("/etc/machine-id") {
        return id.trim().as_bytes().to_vec();
    }
    if let Ok(id) = fs::read_to_string("/var/lib/dbus/machine-id") {
        return id.trim().as_bytes().to_vec();
    }
    // macOS: use IOPlatformUUID via sysctl or hostname
    if let Ok(output) = std::process::Command::new("sysctl")
        .args(["-n", "kern.uuid"])
        .output()
    {
        if output.status.success() {
            return String::from_utf8_lossy(&output.stdout)
                .trim()
                .as_bytes()
                .to_vec();
        }
    }
    // Last resort: hostname
    hostname::get()
        .map(|h| h.to_string_lossy().as_bytes().to_vec())
        .unwrap_or_else(|_| b"codemux-fallback-key".to_vec())
}

fn derive_key(salt: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(&machine_id());
    hasher.update(salt);
    hasher.finalize().into()
}

/// Machine-guarded symmetric encryption (AES-256-GCM, key derived from the
/// machine id + a random per-blob salt). Used for the encrypted auth-token
/// blob and reused by `web_remote::iroh` to seal the desktop's stable iroh
/// identity key at rest, so both device secrets share one machine-guard.
pub(crate) fn encrypt_data(plaintext: &[u8]) -> Result<Vec<u8>, String> {
    let mut salt = [0u8; SALT_LEN];
    OsRng.fill_bytes(&mut salt);

    let key = derive_key(&salt);
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| format!("cipher init: {e}"))?;

    let mut nonce_bytes = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| format!("encrypt: {e}"))?;

    // Format: salt (16) + nonce (12) + ciphertext (includes 16-byte tag)
    let mut out = Vec::with_capacity(SALT_LEN + NONCE_LEN + ciphertext.len());
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

pub(crate) fn decrypt_data(data: &[u8]) -> Result<Vec<u8>, String> {
    let min_len = SALT_LEN + NONCE_LEN + 16 + 1; // salt + nonce + tag + at least 1 byte
    if data.len() < min_len {
        return Err("data too short".into());
    }

    let salt = &data[..SALT_LEN];
    let nonce_bytes = &data[SALT_LEN..SALT_LEN + NONCE_LEN];
    let ciphertext = &data[SALT_LEN + NONCE_LEN..];

    let key = derive_key(salt);
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| format!("cipher init: {e}"))?;
    let nonce = Nonce::from_slice(nonce_bytes);

    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| format!("decrypt: {e}"))
}

pub fn save_token(db: &DatabaseStore, token: &str, expires_at: &str) -> Result<(), String> {
    save_auth(db, token, expires_at, None)
}

pub fn save_auth(
    db: &DatabaseStore,
    token: &str,
    expires_at: &str,
    user: Option<&AuthUser>,
) -> Result<(), String> {
    // Preserve any previously-persisted auth_method across token
    // refreshes (signin → check_auth verify success → save_auth)
    // so the value set by the signin path doesn't get clobbered.
    // Callers that intend to set a new auth_method should follow
    // this with `save_stored_auth_method`.
    let existing_auth_method = load_stored_auth_method(db);
    let stored = StoredAuth {
        token: token.to_string(),
        expires_at: expires_at.to_string(),
        user: user.cloned(),
        auth_method: existing_auth_method,
    };
    let json = serde_json::to_vec(&stored).map_err(|e| format!("serialize: {e}"))?;
    let encrypted = encrypt_data(&json)?;
    db.save_auth_token(&encrypted)
}

/// Dev/test-only: seed an offline auth session so an automated end-to-end
/// harness can drive the app without a real Codemux account or any network
/// access.
///
/// Compiled **only into debug builds** (the `debug_assertions` gate) and, even
/// there, dormant unless `CODEMUX_DEV_OFFLINE_LOGIN=1` is set — so it can never
/// affect a shipped release. It writes a cached local user via the normal
/// [`save_auth`] path; combined with pointing `CODEMUX_API_URL` at an
/// unreachable endpoint, `check_auth` returns this user through its existing
/// offline/network-error fallback branch (`Ok(load_cached_user(..))`). No-ops
/// when a real session is already stored so it never clobbers a genuine login.
#[cfg(debug_assertions)]
pub fn seed_dev_offline_login(db: &DatabaseStore) {
    if std::env::var("CODEMUX_DEV_OFFLINE_LOGIN").ok().as_deref() != Some("1") {
        return;
    }
    if load_token(db).is_some() {
        return;
    }
    let user = AuthUser {
        id: "dev-offline".to_string(),
        email: "dev@localhost".to_string(),
        name: Some("Dev Mode".to_string()),
        image: None,
    };
    let expires_at = (chrono::Utc::now() + chrono::Duration::days(3650)).to_rfc3339();
    match save_auth(db, "dev-offline-token", &expires_at, Some(&user)) {
        Ok(()) => eprintln!("[codemux::auth] seeded offline dev login (debug/e2e affordance)"),
        Err(e) => eprintln!("[codemux::auth] seed_dev_offline_login failed: {e}"),
    }
}

/// Update only the `auth_method` field of the stored auth record,
/// preserving token/expires_at/user. Called by the signin paths
/// (email, github) and by `setup_sync_password` so the value
/// survives a cold-start `check_auth` and the frontend can choose
/// between SetupSyncPasswordForm and ProvidePasswordForm correctly.
pub fn save_stored_auth_method(db: &DatabaseStore, method: Option<&str>) -> Result<(), String> {
    let data = db
        .load_auth_token()
        .ok_or_else(|| "no stored auth".to_string())?;
    let decrypted = decrypt_data(&data).map_err(|e| format!("decrypt: {e}"))?;
    let mut stored: StoredAuth =
        serde_json::from_slice(&decrypted).map_err(|e| format!("parse stored auth: {e}"))?;
    stored.auth_method = method.map(|s| s.to_string());
    let json = serde_json::to_vec(&stored).map_err(|e| format!("serialize: {e}"))?;
    let encrypted = encrypt_data(&json)?;
    db.save_auth_token(&encrypted)
}

pub fn load_token(db: &DatabaseStore) -> Option<(String, String)> {
    let data = db.load_auth_token()?;
    let decrypted = decrypt_data(&data).ok()?;
    let stored: StoredAuth = serde_json::from_slice(&decrypted).ok()?;
    Some((stored.token, stored.expires_at))
}

pub fn load_cached_user(db: &DatabaseStore) -> Option<AuthUser> {
    let data = db.load_auth_token()?;
    let decrypted = decrypt_data(&data).ok()?;
    let stored: StoredAuth = serde_json::from_slice(&decrypted).ok()?;
    stored.user
}

pub fn load_stored_auth_method(db: &DatabaseStore) -> Option<String> {
    let data = db.load_auth_token()?;
    let decrypted = decrypt_data(&data).ok()?;
    let stored: StoredAuth = serde_json::from_slice(&decrypted).ok()?;
    stored.auth_method
}

pub fn clear_token(db: &DatabaseStore) {
    use std::io::Write;
    let msg = format!(
        "[auth] CLEAR_TOKEN CALLED - backtrace:\n{}\n",
        std::backtrace::Backtrace::force_capture()
    );
    let _ = std::io::stderr().write_all(msg.as_bytes());
    let _ = std::io::stderr().flush();
    db.clear_auth_token();
}

pub fn is_token_expired(expires_at: &str) -> bool {
    chrono::DateTime::parse_from_rfc3339(expires_at)
        .map(|dt| dt < chrono::Utc::now())
        .unwrap_or(true)
}

// ── Localhost callback server ────────────────────────────────────

pub fn start_callback_server<R: tauri::Runtime>(
    app_handle: tauri::AppHandle<R>,
) -> Result<u16, String> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| format!("bind: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();

    let handle_clone = app_handle.clone();

    std::thread::spawn(move || {
        // Accept one connection (or timeout after 5 minutes)
        listener
            .set_nonblocking(false)
            .expect("set_nonblocking failed");
        let _ = listener.set_ttl(300);

        let deadline = Instant::now() + std::time::Duration::from_secs(300);

        loop {
            if Instant::now() > deadline {
                eprintln!("[auth] Callback server timed out after 5 minutes");
                break;
            }

            // Use a short accept timeout via SO_RCVTIMEO
            #[cfg(unix)]
            {
                use std::os::unix::io::AsRawFd;
                let fd = listener.as_raw_fd();
                let timeout = libc::timeval {
                    tv_sec: 5,
                    tv_usec: 0,
                };
                unsafe {
                    libc::setsockopt(
                        fd,
                        libc::SOL_SOCKET,
                        libc::SO_RCVTIMEO,
                        &timeout as *const _ as *const libc::c_void,
                        std::mem::size_of::<libc::timeval>() as libc::socklen_t,
                    );
                }
            }

            match listener.accept() {
                Ok((mut stream, _)) => {
                    use std::io::{Read, Write};
                    let mut buf = [0u8; 4096];
                    let n = match stream.read(&mut buf) {
                        Ok(n) => n,
                        Err(_) => continue,
                    };
                    let request = String::from_utf8_lossy(&buf[..n]);

                    // Parse GET /auth/callback?token=...&expiresAt=...&state=...
                    let first_line = request.lines().next().unwrap_or("");
                    if !first_line.starts_with("GET /auth/callback?") {
                        let resp = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n";
                        let _ = stream.write_all(resp.as_bytes());
                        continue;
                    }

                    let path = first_line.split_whitespace().nth(1).unwrap_or("");
                    let url_str = format!("http://127.0.0.1{path}");
                    let parsed = match url::Url::parse(&url_str) {
                        Ok(u) => u,
                        Err(_) => continue,
                    };

                    let params: HashMap<String, String> = parsed
                        .query_pairs()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect();

                    let token = params.get("token");
                    let expires_at = params.get("expiresAt");
                    let state = params.get("state");

                    if let (Some(token), Some(expires_at), Some(state)) = (token, expires_at, state)
                    {
                        let auth_state_local: tauri::State<'_, AuthState> = handle_clone.state();
                        let Some(oauth_launch_generation) =
                            auth_state_local.consume_csrf_state(state)
                        else {
                            let body = r#"{"error":"Invalid or expired auth session"}"#;
                            let resp = format!(
                                "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                                body.len(),
                                body
                            );
                            let _ = stream.write_all(resp.as_bytes());
                            continue;
                        };

                        let db: tauri::State<'_, DatabaseStore> = handle_clone.state();
                        // OAuth is a complete account replacement. Advance the
                        // same generation used by after-paint verification
                        // before writing anything, so a response launched for
                        // the previous account cannot commit over this token.
                        let Some((oauth_generation, replacement)) = auth_state_local
                            .replace_session_if_current(oauth_launch_generation, || {
                                // The new token has no trusted identity until the
                                // verify response arrives. Hide the previous
                                // account's settings during that interval.
                                crate::settings_sync::set_cache_owner(None);
                                save_token(&db, token, expires_at)?;
                                auth_state_local.set_auth_method(Some("github"));
                                save_stored_auth_method(&db, Some("github"))
                            })
                        else {
                            let body = r#"{"error":"Authentication changed before callback"}"#;
                            let resp = format!(
                                "HTTP/1.1 409 Conflict\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                                body.len(),
                                body
                            );
                            let _ = stream.write_all(resp.as_bytes());
                            break;
                        };
                        if let Err(e) = replacement {
                            eprintln!("[auth] Failed to persist OAuth callback: {e}");
                            let body = r#"{"error":"Failed to persist session"}"#;
                            let resp = format!(
                                "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                                body.len(),
                                body
                            );
                            let _ = stream.write_all(resp.as_bytes());
                            continue;
                        }

                        // Emit auth event to frontend
                        emit_auth_state(&handle_clone, token, expires_at, oauth_generation);

                        let html = SUCCESS_HTML;
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            html.len(),
                            html
                        );
                        let _ = stream.write_all(resp.as_bytes());
                        break; // Success — shut down server
                    } else {
                        let body = r#"{"error":"Missing auth params"}"#;
                        let resp = format!(
                            "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        let _ = stream.write_all(resp.as_bytes());
                    }
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    // Timeout — loop and check deadline
                    continue;
                }
                Err(_) => {
                    // Accept error — loop
                    continue;
                }
            }
        }
    });

    Ok(port)
}

enum CallbackVerifyOutcome {
    Verified(AuthUser),
    Unauthorized,
    Offline,
}

fn classify_callback_verify(
    status: reqwest::StatusCode,
    user: Option<AuthUser>,
) -> CallbackVerifyOutcome {
    if status == reqwest::StatusCode::UNAUTHORIZED {
        CallbackVerifyOutcome::Unauthorized
    } else if status.is_success() {
        user.map_or(
            CallbackVerifyOutcome::Offline,
            CallbackVerifyOutcome::Verified,
        )
    } else {
        CallbackVerifyOutcome::Offline
    }
}

fn emit_callback_auth_state<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    user: Option<AuthUser>,
    auth_method: Option<String>,
) {
    let authenticated = user.is_some();
    let _ = app.emit(
        "auth-state-changed",
        &AuthStatePayload {
            authenticated,
            user,
        },
    );
    let _ = app.emit(
        "sync-state-changed",
        &SyncStatus {
            sync_available: authenticated,
            auth_method,
        },
    );
}

fn emit_auth_state<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    token: &str,
    expires_at: &str,
    session_generation: u64,
) {
    let auth_state: tauri::State<'_, AuthState> = app.state();
    // Fetch user data from API to populate the event
    let base = api_base_url();
    let url = format!("{base}/api/auth/desktop/verify");
    let outcome = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
    {
        Ok(client) => match client
            .get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .send()
        {
            Ok(response) => {
                let status = response.status();
                let user = if status.is_success() {
                    response
                        .json::<VerifyResponse>()
                        .ok()
                        .map(|verified| AuthUser {
                            id: verified.user.id,
                            email: verified.user.email,
                            name: verified.user.name,
                            image: verified.user.image,
                        })
                } else {
                    None
                };
                classify_callback_verify(status, user)
            }
            Err(_) => CallbackVerifyOutcome::Offline,
        },
        Err(_) => CallbackVerifyOutcome::Offline,
    };

    // Cache and emit only if this is still the OAuth session that launched
    // the verify request. Sign-out or a subsequent sign-in advances the
    // generation and makes this stale callback a no-op.
    let committed = match outcome {
        CallbackVerifyOutcome::Verified(user) => auth_state
            .commit_if_session_current(session_generation, || {
                let db: tauri::State<'_, DatabaseStore> = app.state();
                if let Err(error) = save_auth(&db, token, expires_at, Some(&user)) {
                    eprintln!("[auth] Failed to cache verified OAuth user: {error}");
                    crate::settings_sync::set_cache_owner(None);
                    emit_callback_auth_state(app, None, Some("github".into()));
                    return;
                }
                crate::settings_sync::set_cache_owner(Some(&user.id));
                emit_callback_auth_state(app, Some(user), Some("github".into()));
            })
            .is_some(),
        CallbackVerifyOutcome::Unauthorized => auth_state
            .replace_session_if_current(session_generation, || {
                let db: tauri::State<'_, DatabaseStore> = app.state();
                clear_token(&db);
                crate::settings_sync::clear_cache();
                crate::settings_sync::set_cache_owner(None);
                auth_state.set_auth_method(None);
                emit_callback_auth_state(app, None, None);
            })
            .is_some(),
        CallbackVerifyOutcome::Offline => auth_state
            .commit_if_session_current(session_generation, || {
                // Keep the locally valid token pending for a bounded retry
                // after the login frame paints, but expose neither identity
                // nor account settings until verification succeeds.
                crate::settings_sync::set_cache_owner(None);
                emit_callback_auth_state(app, None, Some("github".into()));
            })
            .is_some(),
    };

    if !committed {
        eprintln!("[auth] Ignoring stale OAuth verification result");
    }
}

fn base64_url_encode(data: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(data)
}

const SUCCESS_HTML: &str = r#"<!DOCTYPE html>
<html><head><meta charset="utf-8"><title>Codemux</title>
<style>
body{font-family:system-ui,-apple-system,sans-serif;display:flex;justify-content:center;align-items:center;min-height:100vh;margin:0;background:#0a0a0a;color:#fafafa}
.c{text-align:center;max-width:400px;padding:2rem}
h2{margin-bottom:.5rem;font-size:1.25rem}
p{opacity:.6;font-size:.9rem}
</style>
</head><body>
<div class="c">
<h2>Signed in successfully</h2>
<p>You can close this tab and return to the desktop app.</p>
</div>
</body></html>"#;

// Zero-knowledge auth credential derivation now lives in
// `auth/derivation.rs` (re-exported above). The Step 10 — Skills
// Sync inline copy that used to live here — `AuthSecret`,
// `EncryptionKey`, `derive_login_credentials`, the cross-product
// hex pin tests — was relocated when this branch absorbed main's
// `auth.rs` → `auth/{api,derivation,mod}.rs` refactor.

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> DatabaseStore {
        DatabaseStore::new_in_memory()
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let plaintext = b"hello world token data";
        let encrypted = encrypt_data(plaintext).unwrap();
        let decrypted = decrypt_data(&encrypted).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn encrypted_data_is_not_plaintext() {
        let plaintext = b"secret-token-value-12345";
        let encrypted = encrypt_data(plaintext).unwrap();
        let plaintext_str = std::str::from_utf8(plaintext).unwrap();
        let encrypted_str = String::from_utf8_lossy(&encrypted);
        assert!(!encrypted_str.contains(plaintext_str));
        assert!(encrypted.windows(plaintext.len()).all(|w| w != plaintext));
    }

    #[test]
    fn decrypt_with_wrong_key_returns_error() {
        let plaintext = b"some secret data";
        let mut encrypted = encrypt_data(plaintext).unwrap();
        encrypted[0] ^= 0xff;
        encrypted[1] ^= 0xff;
        assert!(decrypt_data(&encrypted).is_err());
    }

    #[test]
    fn corrupted_data_returns_error() {
        assert!(decrypt_data(&[0u8; 10]).is_err());
        let garbage = vec![0xdeu8; 100];
        assert!(decrypt_data(&garbage).is_err());
        assert!(decrypt_data(&[]).is_err());
    }

    #[test]
    fn token_save_load_roundtrip() {
        let db = test_db();
        let token = "test-token-abc123";
        let expires = "2099-01-01T00:00:00Z";
        save_token(&db, token, expires).unwrap();

        let loaded = load_token(&db);
        assert!(loaded.is_some());
        let (t, e) = loaded.unwrap();
        assert_eq!(t, token);
        assert_eq!(e, expires);

        clear_token(&db);
        assert!(load_token(&db).is_none());
    }

    #[test]
    fn auth_method_persists_across_load() {
        // Setup-state bug regression: an OAuth callback persists
        // auth_method="github" so a cold-start `check_auth` can
        // restore it. Without persistence, `Settings → Sync` falls
        // through to ProvidePasswordForm for a brand-new OAuth user
        // who has never set up sync.
        let db = test_db();
        save_token(&db, "tok-1", "2099-01-01T00:00:00Z").unwrap();
        assert!(load_stored_auth_method(&db).is_none());

        save_stored_auth_method(&db, Some("github")).unwrap();
        assert_eq!(load_stored_auth_method(&db).as_deref(), Some("github"));

        // save_auth on token refresh must NOT clobber auth_method.
        save_auth(&db, "tok-2", "2099-02-02T00:00:00Z", None).unwrap();
        assert_eq!(load_stored_auth_method(&db).as_deref(), Some("github"));

        save_stored_auth_method(&db, Some("email")).unwrap();
        assert_eq!(load_stored_auth_method(&db).as_deref(), Some("email"));

        // Clearing the token wipes everything including auth_method.
        clear_token(&db);
        assert!(load_stored_auth_method(&db).is_none());
    }

    #[test]
    fn csrf_state_generate_and_validate() {
        let state = AuthState::default();
        let token = state.generate_csrf_state();
        assert!(!token.is_empty());
        assert!(state.validate_csrf_state(&token));
        assert!(!state.validate_csrf_state(&token));
    }

    #[test]
    fn csrf_state_invalid_token_fails() {
        let state = AuthState::default();
        assert!(!state.validate_csrf_state("nonexistent-state"));
    }

    #[test]
    fn oauth_state_cannot_replace_a_session_that_changed_after_launch() {
        let state = AuthState::default();
        let token = state.generate_csrf_state();
        state.replace_session(|| "explicit-sign-out");

        let launch_generation = state
            .consume_csrf_state(&token)
            .expect("the CSRF state itself remains valid");
        assert!(state
            .replace_session_if_current(launch_generation, || "stale-oauth-callback")
            .is_none());
    }

    #[test]
    fn oauth_callback_distinguishes_unauthorized_from_offline() {
        assert!(matches!(
            classify_callback_verify(reqwest::StatusCode::UNAUTHORIZED, None),
            CallbackVerifyOutcome::Unauthorized
        ));
        assert!(matches!(
            classify_callback_verify(reqwest::StatusCode::SERVICE_UNAVAILABLE, None),
            CallbackVerifyOutcome::Offline
        ));
        assert!(matches!(
            classify_callback_verify(reqwest::StatusCode::OK, None),
            CallbackVerifyOutcome::Offline
        ));

        let user = AuthUser {
            id: "oauth-user".into(),
            email: "oauth@example.test".into(),
            name: None,
            image: None,
        };
        assert!(matches!(
            classify_callback_verify(reqwest::StatusCode::OK, Some(user)),
            CallbackVerifyOutcome::Verified(_)
        ));
    }

    #[test]
    fn stale_session_generation_cannot_commit_after_account_replacement() {
        let state = AuthState::default();
        let (generation, value) = state.session_snapshot(|| "user-a");
        assert_eq!(value, "user-a");

        let (replacement_generation, replacement) =
            state.replace_session_with_generation(|| "user-b-installed");
        assert_eq!(replacement, "user-b-installed");
        assert_eq!(replacement_generation, generation + 1);
        assert!(state
            .commit_if_session_current(generation, || "stale-user-a")
            .is_none());
        assert_eq!(
            state.commit_if_session_current(replacement_generation, || "user-b"),
            Some("user-b")
        );

        let (cleared_generation, cleared) = state
            .replace_session_if_current(replacement_generation, || "signed-out")
            .expect("the current session can be definitively cleared");
        assert_eq!(cleared, "signed-out");
        assert_eq!(cleared_generation, replacement_generation + 1);
        assert!(state
            .commit_if_session_current(replacement_generation, || "late-user-b")
            .is_none());

        let (current_generation, ()) = state.session_snapshot(|| ());
        assert_eq!(
            state.commit_if_session_current(current_generation, || "user-b"),
            Some("user-b")
        );
    }

    #[test]
    fn csrf_state_expired_token_fails() {
        let state = AuthState::default();
        {
            let mut states = state.csrf_states.lock().unwrap();
            // 660s > the 600s cutoff used by validate_csrf_state, so this
            // entry must be treated as expired. SystemTime subtraction is
            // safe here because current wall-clock time is ~56 years past
            // the Unix epoch — 660s subtraction never underflows.
            let expired = SystemTime::now() - std::time::Duration::from_secs(660);
            states.insert("expired-state".into(), (expired, 0));
        }
        assert!(!state.validate_csrf_state("expired-state"));
    }

    #[test]
    fn token_expiry_check() {
        assert!(!is_token_expired("2099-12-31T23:59:59Z"));
        assert!(is_token_expired("2000-01-01T00:00:00Z"));
        assert!(is_token_expired("not-a-date"));
    }

    // ── Security tests for cached user data ─────────────────────────

    fn test_user() -> AuthUser {
        AuthUser {
            id: "usr-sec-test-9283".into(),
            email: "sectest@example.com".into(),
            name: Some("Security Test User".into()),
            image: None,
        }
    }

    #[test]
    fn encryption_integrity_with_user_data() {
        let db = test_db();
        let token = "sec-token-integrity-xK9mZ";
        let expires = "2099-01-01T00:00:00Z";
        let user = test_user();
        save_auth(&db, token, expires, Some(&user)).unwrap();

        // Read raw encrypted bytes from SQLite — must NOT be valid JSON
        let raw = db.load_auth_token().unwrap();
        assert!(
            serde_json::from_slice::<serde_json::Value>(&raw).is_err(),
            "raw encrypted bytes must not be valid JSON"
        );
    }

    #[test]
    fn no_plaintext_leakage_in_encrypted_data() {
        let db = test_db();
        let token = "sec-token-leakcheck-Qw7pR";
        let expires = "2099-01-01T00:00:00Z";
        let user = test_user();
        save_auth(&db, token, expires, Some(&user)).unwrap();

        let raw = db.load_auth_token().unwrap();

        let sensitive = [
            token.as_bytes(),
            user.email.as_bytes(),
            user.name.as_ref().unwrap().as_bytes(),
            user.id.as_bytes(),
        ];
        for secret in &sensitive {
            assert!(
                raw.windows(secret.len()).all(|w| w != *secret),
                "plaintext leaked in encrypted data: {:?}",
                std::str::from_utf8(secret).unwrap()
            );
        }
    }

    #[test]
    fn decryption_roundtrip_with_user() {
        let db = test_db();
        let token = "sec-token-roundtrip-Lm3nB";
        let expires = "2099-06-15T12:00:00Z";
        let user = test_user();
        save_auth(&db, token, expires, Some(&user)).unwrap();

        let (t, e) = load_token(&db).unwrap();
        assert_eq!(t, token);
        assert_eq!(e, expires);

        let cached = load_cached_user(&db).unwrap();
        assert_eq!(cached.id, user.id);
        assert_eq!(cached.email, user.email);
        assert_eq!(cached.name, user.name);
        assert_eq!(cached.image, user.image);
    }

    #[test]
    fn save_token_without_user() {
        let db = test_db();
        let token = "sec-token-compat-Hj8kW";
        let expires = "2099-01-01T00:00:00Z";

        save_token(&db, token, expires).unwrap();

        let (t, e) = load_token(&db).unwrap();
        assert_eq!(t, token);
        assert_eq!(e, expires);

        // No user was saved
        assert!(load_cached_user(&db).is_none());
    }

    #[test]
    fn corrupted_data_returns_none_gracefully() {
        let db = test_db();

        // Write garbage bytes directly into SQLite
        let garbage: Vec<u8> = (0u32..300).map(|i| (i.wrapping_mul(0xDE)) as u8).collect();
        db.save_auth_token(&garbage).unwrap();

        assert!(load_token(&db).is_none());
        assert!(load_cached_user(&db).is_none());
    }

    #[test]
    fn missing_data_returns_none_gracefully() {
        let db = test_db();

        // Empty database — no token stored
        assert!(load_token(&db).is_none());
        assert!(load_cached_user(&db).is_none());
    }

    // The cross-product derivation pin tests live in
    // `auth/derivation.rs::tests` — that's the file the algorithm
    // itself lives in, and the canary belongs next to the code.
}
