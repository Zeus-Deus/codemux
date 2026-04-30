use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Instant, SystemTime};

use tauri::Manager;

use crate::commands::SyncStatus;
use crate::database::DatabaseStore;
use crate::encryption::EncryptionManager;

// fs is used by machine_id() and token_file_path() (migration support)
use std::fs;

use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use rand::RngCore;
use sha2::{Digest, Sha256};
use tauri::Emitter;

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
    /// CSRF state tokens keyed to their creation time. Uses `SystemTime`
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
    pub(crate) csrf_states: Mutex<HashMap<String, SystemTime>>,
    callback_port: Mutex<Option<u16>>,
    /// Tracks which signin path produced the current session —
    /// `"email"` after email/password, `"github"` after OAuth, or
    /// `None` after a cold-start `check_auth` (the path is not
    /// persisted on disk; the frontend infers from sync state).
    /// Cleared on `sign_out`. Mirrors Vexis's `AuthStatus.authMethod`
    /// which drives the Settings → Sync section's UI fork.
    auth_method: Mutex<Option<String>>,
}

impl Default for AuthState {
    fn default() -> Self {
        Self {
            csrf_states: Mutex::new(HashMap::new()),
            callback_port: Mutex::new(None),
            auth_method: Mutex::new(None),
        }
    }
}

impl AuthState {
    /// Generate a CSRF state token and store it with a timestamp.
    pub fn generate_csrf_state(&self) -> String {
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        let state = base64_url_encode(&bytes);

        let mut states = self.csrf_states.lock().unwrap();
        // Clean up expired entries (older than 10 minutes). See the
        // csrf_states field comment for why this uses SystemTime.
        let cutoff = SystemTime::now() - std::time::Duration::from_secs(600);
        states.retain(|_, ts| *ts > cutoff);
        states.insert(state.clone(), SystemTime::now());

        state
    }

    /// Validate and consume a CSRF state token (one-time use).
    pub fn validate_csrf_state(&self, state: &str) -> bool {
        let mut states = self.csrf_states.lock().unwrap();
        let cutoff = SystemTime::now() - std::time::Duration::from_secs(600);
        if let Some(ts) = states.remove(state) {
            ts > cutoff
        } else {
            false
        }
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
}

// ── API base URL ─────────────────────────────────────────────────

pub fn api_base_url() -> String {
    std::env::var("CODEMUX_API_URL")
        .unwrap_or_else(|_| "https://api.codemux.org".into())
}

// ── Encrypted token storage ──────────────────────────────────────

const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
// AES-GCM tag is appended by the aes-gcm crate inside the ciphertext

pub(crate) fn token_file_path() -> PathBuf {
    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local/share"));
    data_dir.join("codemux").join("auth-token.enc")
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
    let mut sysctl_cmd = std::process::Command::new("sysctl");
    sysctl_cmd.args(["-n", "kern.uuid"]);
    crate::execution::sanitize_gui_env_std(&mut sysctl_cmd);
    if let Ok(output) = sysctl_cmd.output() {
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

fn encrypt_data(plaintext: &[u8]) -> Result<Vec<u8>, String> {
    let mut salt = [0u8; SALT_LEN];
    OsRng.fill_bytes(&mut salt);

    let key = derive_key(&salt);
    let cipher =
        Aes256Gcm::new_from_slice(&key).map_err(|e| format!("cipher init: {e}"))?;

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

fn decrypt_data(data: &[u8]) -> Result<Vec<u8>, String> {
    let min_len = SALT_LEN + NONCE_LEN + 16 + 1; // salt + nonce + tag + at least 1 byte
    if data.len() < min_len {
        return Err("data too short".into());
    }

    let salt = &data[..SALT_LEN];
    let nonce_bytes = &data[SALT_LEN..SALT_LEN + NONCE_LEN];
    let ciphertext = &data[SALT_LEN + NONCE_LEN..];

    let key = derive_key(salt);
    let cipher =
        Aes256Gcm::new_from_slice(&key).map_err(|e| format!("cipher init: {e}"))?;
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

/// Update only the `auth_method` field of the stored auth record,
/// preserving token/expires_at/user. Called by the signin paths
/// (email, github) and by `setup_sync_password` so the value
/// survives a cold-start `check_auth` and the frontend can choose
/// between SetupSyncPasswordForm and ProvidePasswordForm correctly.
pub fn save_stored_auth_method(
    db: &DatabaseStore,
    method: Option<&str>,
) -> Result<(), String> {
    let data = db.load_auth_token().ok_or_else(|| "no stored auth".to_string())?;
    let decrypted = decrypt_data(&data).map_err(|e| format!("decrypt: {e}"))?;
    let mut stored: StoredAuth = serde_json::from_slice(&decrypted)
        .map_err(|e| format!("parse stored auth: {e}"))?;
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

// ── Sync encryption key (Stage 2) ────────────────────────────────
//
// The 32-byte symmetric `encryption_key` from
// `derive_login_credentials` is persisted as a separate file at
// `~/.local/share/codemux/sync-key.enc`, encrypted with the same
// machine-bound AES-256-GCM wrap as the auth token. Mirrors Vexis's
// `auth/token_store.rs::save_encryption_key` pattern.
//
// Why a separate file rather than alongside the auth token: the
// auth token's lifecycle (signin, refresh, logout) is independent
// of the sync key's lifecycle (setup, repair, reset_sync). Keeping
// them in separate files means a partially-corrupted token file
// doesn't take the sync key down with it and vice versa.
//
// Why the same machine-bound wrap: the threat model is identical —
// neither file should be useful when copied to a different machine.
// `derive_key(salt)` uses the host's `/etc/machine-id` (with macOS
// and hostname fallbacks) as the master input, so a stolen
// `sync-key.enc` from laptop A cannot be decrypted on laptop B.

pub(crate) fn sync_key_file_path() -> PathBuf {
    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local/share"));
    data_dir.join("codemux").join("sync-key.enc")
}

/// Encrypt `key` with the machine-bound wrap and write to
/// `sync-key.enc`. Returns the resolved path so callers can log it
/// at info level (the path is not sensitive — only the file
/// contents are, and those are already AES-GCM-wrapped).
pub fn save_encryption_key(key: &[u8; 32]) -> Result<PathBuf, String> {
    let path = sync_key_file_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create data dir: {e}"))?;
    }
    let encrypted = encrypt_data(key)?;
    fs::write(&path, &encrypted).map_err(|e| format!("write sync-key.enc: {e}"))?;
    Ok(path)
}

/// Read `sync-key.enc` if present and decrypt the 32-byte key.
/// Returns `Ok(None)` if the file does not exist (fresh install,
/// post-logout, or pre-setup state). Returns `Err` only on
/// genuinely unexpected errors — a missing file is a normal state.
///
/// A decrypt failure also returns `Ok(None)` rather than `Err`: the
/// most common cause is "file copied from another machine" or "the
/// machine's `/etc/machine-id` was regenerated", in which case the
/// only sane recovery is to treat the key as missing and prompt the
/// user to re-derive via `provide_password_for_sync`.
pub fn load_encryption_key() -> Result<Option<[u8; 32]>, String> {
    let path = sync_key_file_path();
    let data = match fs::read(&path) {
        Ok(d) => d,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("read sync-key.enc: {err}")),
    };
    let plaintext = match decrypt_data(&data) {
        Ok(b) => b,
        Err(_) => return Ok(None),
    };
    if plaintext.len() != 32 {
        // Wrong size means the file was tampered with or written by
        // a different version. Treat as missing — Stage 3 will
        // route the user through provide_password_for_sync.
        return Ok(None);
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&plaintext);
    Ok(Some(key))
}

/// Delete the persisted sync key. Used on logout and as part of
/// the `reset_sync` recovery flow. Missing-file is treated as
/// success — the caller wanted the file gone, and it is.
pub fn delete_encryption_key() -> Result<(), String> {
    let path = sync_key_file_path();
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!("remove sync-key.enc: {err}")),
    }
}

// ── Localhost callback server ────────────────────────────────────

pub fn start_callback_server(
    auth_state: std::sync::Arc<AuthState>,
    app_handle: tauri::AppHandle,
) -> Result<u16, String> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("bind: {e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();

    let auth_state_clone = auth_state.clone();
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

                    let path = first_line
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or("");
                    let url_str = format!("http://127.0.0.1{path}");
                    let parsed = match url::Url::parse(&url_str) {
                        Ok(u) => u,
                        Err(_) => continue,
                    };

                    let params: HashMap<String, String> =
                        parsed.query_pairs().map(|(k, v)| (k.to_string(), v.to_string())).collect();

                    let token = params.get("token");
                    let expires_at = params.get("expiresAt");
                    let state = params.get("state");

                    if let (Some(token), Some(expires_at), Some(state)) =
                        (token, expires_at, state)
                    {
                        if !auth_state_clone.validate_csrf_state(state) {
                            let body = r#"{"error":"Invalid or expired auth session"}"#;
                            let resp = format!(
                                "HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                                body.len(),
                                body
                            );
                            let _ = stream.write_all(resp.as_bytes());
                            continue;
                        }

                        let db: tauri::State<'_, DatabaseStore> = handle_clone.state();
                        if let Err(e) = save_token(&db, token, expires_at) {
                            eprintln!("[auth] Failed to save token in callback: {e}");
                        }

                        // Tag the session as a GitHub OAuth signin and
                        // persist that fact so a cold-start `check_auth`
                        // can restore it. Without this the Settings →
                        // Sync section sees `auth_method=null` after
                        // OAuth and falls through to ProvidePasswordForm
                        // (the repair flow) when it should show
                        // SetupSyncPasswordForm (the initial-setup flow)
                        // for users who have never set up sync.
                        let auth_state_local: tauri::State<'_, AuthState> =
                            handle_clone.state();
                        auth_state_local.set_auth_method(Some("github"));
                        if let Err(e) = save_stored_auth_method(&db, Some("github")) {
                            eprintln!(
                                "[auth] Failed to persist auth_method=github in callback: {e}"
                            );
                        }

                        // Emit auth event to frontend
                        emit_auth_state(&handle_clone, token, expires_at);

                        // Mirror the signin_email pattern: tell the
                        // frontend the sync state immediately. OAuth
                        // signin doesn't load an encryption key (no
                        // password was typed), so `sync_available`
                        // is whatever EncryptionManager reports. The
                        // important field for the Settings UI is
                        // `auth_method=github` — that's what selects
                        // SetupSyncPasswordForm.
                        let encryption_local: tauri::State<'_, EncryptionManager> =
                            handle_clone.state();
                        let _ = handle_clone.emit(
                            "sync-state-changed",
                            &SyncStatus {
                                sync_available: encryption_local.is_available(),
                                auth_method: Some("github".into()),
                            },
                        );

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

fn emit_auth_state(app: &tauri::AppHandle, token: &str, expires_at: &str) {
    // Fetch user data from API to populate the event
    let base = api_base_url();
    let url = format!("{base}/api/auth/desktop/verify");
    let client = reqwest::blocking::Client::new();
    let user = client
        .get(&url)
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .ok()
        .and_then(|r| r.json::<VerifyResponse>().ok())
        .map(|v| AuthUser {
            id: v.user.id,
            email: v.user.email,
            name: v.user.name,
            image: v.user.image,
        });

    // Cache user data for offline/network-error auth
    if let Some(ref u) = user {
        let db: tauri::State<'_, DatabaseStore> = app.state();
        let _ = save_auth(&db, token, expires_at, Some(u));
    }

    let payload = AuthStatePayload {
        authenticated: user.is_some(),
        user,
    };

    let _ = app.emit("auth-state-changed", &payload);
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

// ────────────────────────────────────────────────────────────────
// Zero-knowledge auth credential derivation (Step 10 — Skills Sync)
// ────────────────────────────────────────────────────────────────
//
// TODO: relocate this entire section to `auth/derivation.rs` once
// `feature/agent-chat` merges main, which carries the auth-module
// refactor (single `auth.rs` → `auth/{api,derivation,mod}.rs`).
// Until then, the derivation lives inline here so Stage 1 can ship
// without pulling in 71 commits of unrelated main work. The
// implementation matches `feature/skills-sync-stage-1`'s
// `src-tauri/src/auth/derivation.rs` byte-for-byte at the protocol
// level — both produce identical output for the same `(password,
// email)`, pinned by `auth_secret_matches_vexis_for_known_input`
// and `encryption_key_matches_vexis_for_known_input` below.
//
// Cross-product contract: `derive_login_credentials(password, email)`
// returns an `AuthSecret` (sent to Better Auth in place of the raw
// password) and a 32-byte `EncryptionKey` (kept on-device, used by
// `crate::encryption` to encrypt synced skills before upload). The
// HKDF info labels `codemux-api-master-v1`,
// `codemux-api-auth-secret-v1`, and `codemux-api-encryption-key-v1`
// are SHARED across every product authenticating to api.codemux.org
// — Vexis uses them too. Any rotation must bump the `vN` suffix in
// every product simultaneously.

use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine as _;
use hkdf::Hkdf;

const ARGON2_M_COST_KIB: u32 = 65_536; // 64 MiB
const ARGON2_T_COST: u32 = 3;
const ARGON2_P_COST: u32 = 4;
const AUTH_SECRET_LEN: usize = 32;
const ENCRYPTION_KEY_LEN: usize = 32;

const MASTER_SALT_DOMAIN: &[u8] = b"codemux-api-master-v1\0";
const AUTH_SECRET_INFO: &[u8] = b"codemux-api-auth-secret-v1";
const ENCRYPTION_KEY_INFO: &[u8] = b"codemux-api-encryption-key-v1";

/// A base64-encoded 32-byte authentication secret derived from the
/// user's password + email. Sent to Better Auth in place of the raw
/// password, so the server only ever bcrypt-hashes this stretched
/// value and never sees what the user actually typed.
///
/// The `Debug` impl deliberately redacts the value — a stray
/// `println!("{:?}", secret)` won't leak the auth credential.
#[derive(Clone)]
pub struct AuthSecret(String);

impl AuthSecret {
    /// Borrow the base64 secret for transmission to the auth API.
    /// `pub(crate)` so only code inside the codemux crate can read
    /// it; external callers can't accidentally log or leak it.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    /// Public escape hatch for integration test binaries (e.g. the
    /// `skills_smoke` example) that need to forward the secret to
    /// the live `/api/auth/desktop/signin` endpoint. Verbose name
    /// keeps the call sites greppable.
    pub fn expose_for_external_signin(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for AuthSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AuthSecret(***)")
    }
}

/// A 32-byte raw symmetric key derived alongside `AuthSecret` for
/// products that ship client-side end-to-end encryption (Step 10
/// skills sync, Vexis voice_*). Never sent to the server, never
/// persisted in plaintext on disk, never logged.
///
/// `Drop` zeroes the bytes via `write_volatile` so a stack frame
/// holding a transient `EncryptionKey` doesn't leave key material
/// in memory after it goes out of scope. Not as airtight as the
/// `zeroize` crate but sufficient for our threat model — brief
/// in-stack lifetime, never crosses FFI.
pub struct EncryptionKey([u8; ENCRYPTION_KEY_LEN]);

impl EncryptionKey {
    /// Borrow the raw key bytes for use with an AEAD cipher.
    /// `pub(crate)` so the bytes don't leak past crate boundaries;
    /// the public escape hatch below is for the smoke-test binary.
    #[allow(dead_code)] // used by tests + Stage 2 sync layer (forthcoming)
    pub(crate) fn as_bytes(&self) -> &[u8; ENCRYPTION_KEY_LEN] {
        &self.0
    }

    /// Public escape hatch for the Stage 1 `skills_smoke` example
    /// binary, which needs the raw bytes to feed
    /// `crate::encryption::encrypt`. Verbose name keeps the
    /// callsites greppable.
    pub fn expose_for_smoke_test(&self) -> &[u8; ENCRYPTION_KEY_LEN] {
        &self.0
    }
}

impl Drop for EncryptionKey {
    fn drop(&mut self) {
        for byte in self.0.iter_mut() {
            // Safety: writing to our own slot, no aliasing.
            unsafe { core::ptr::write_volatile(byte, 0) };
        }
    }
}

impl std::fmt::Debug for EncryptionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "EncryptionKey(***)")
    }
}

/// Derive both halves of the login credentials from a single
/// Argon2id stretch of `(password, email)`. Mirrors Vexis's
/// `derive_login_credentials` and produces byte-identical output
/// for the same inputs, which is what makes the shared Better Auth
/// account work across both apps.
///
/// ```text
/// master         = Argon2id(
///     password,
///     salt   = SHA256("codemux-api-master-v1\0" || normalize_email(email)),
///     m=64MiB, t=3, p=4, L=32,
/// )
/// auth_secret    = base64_no_pad( HKDF-Expand(master, "codemux-api-auth-secret-v1",    32) )
/// encryption_key =                HKDF-Expand(master, "codemux-api-encryption-key-v1", 32)
/// ```
///
/// HKDF domain separation: observing one half tells an attacker
/// nothing about the other. Server-stored AuthSecret cannot be
/// used to derive any user's EncryptionKey.
pub fn derive_login_credentials(
    password: &str,
    email: &str,
) -> Result<(AuthSecret, EncryptionKey), String> {
    let master = derive_master_material(password, email)?;
    let hk = Hkdf::<Sha256>::from_prk(&master)
        .map_err(|e| format!("hkdf from_prk: {e}"))?;

    let mut auth_bytes = [0u8; AUTH_SECRET_LEN];
    hk.expand(AUTH_SECRET_INFO, &mut auth_bytes)
        .map_err(|e| format!("hkdf expand auth: {e}"))?;
    let auth_secret = AuthSecret(
        base64::engine::general_purpose::STANDARD_NO_PAD.encode(auth_bytes),
    );

    let mut key_bytes = [0u8; ENCRYPTION_KEY_LEN];
    hk.expand(ENCRYPTION_KEY_INFO, &mut key_bytes)
        .map_err(|e| format!("hkdf expand encryption-key: {e}"))?;
    let encryption_key = EncryptionKey(key_bytes);

    Ok((auth_secret, encryption_key))
}

/// Convenience wrapper for callers that only need the AuthSecret
/// half (signin / signup / set-password). Discards the
/// EncryptionKey; the Argon2id cost is paid once regardless.
#[allow(dead_code)] // wired up in Stage 2 when settings UI calls it
pub fn derive_auth_secret(password: &str, email: &str) -> Result<AuthSecret, String> {
    let (auth, _key) = derive_login_credentials(password, email)?;
    Ok(auth)
}

fn derive_master_material(password: &str, email: &str) -> Result<[u8; AUTH_SECRET_LEN], String> {
    let normalized = normalize_email(email);

    let mut hasher = Sha256::new();
    hasher.update(MASTER_SALT_DOMAIN);
    hasher.update(normalized.as_bytes());
    let salt: [u8; 32] = hasher.finalize().into();

    let params = Params::new(
        ARGON2_M_COST_KIB,
        ARGON2_T_COST,
        ARGON2_P_COST,
        Some(AUTH_SECRET_LEN),
    )
    .map_err(|e| format!("invalid argon2 params: {e}"))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut out = [0u8; AUTH_SECRET_LEN];
    argon
        .hash_password_into(password.as_bytes(), &salt, &mut out)
        .map_err(|e| format!("argon2 derive: {e}"))?;
    Ok(out)
}

/// Lowercase + trim. Deliberately no NFKC/IDN normalization — Better
/// Auth stores emails as submitted, and as long as every client
/// uses this exact function multi-device determinism holds.
pub(crate) fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

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
    fn csrf_state_expired_token_fails() {
        let state = AuthState::default();
        {
            let mut states = state.csrf_states.lock().unwrap();
            // 660s > the 600s cutoff used by validate_csrf_state, so this
            // entry must be treated as expired. SystemTime subtraction is
            // safe here because current wall-clock time is ~56 years past
            // the Unix epoch — 660s subtraction never underflows.
            let expired = SystemTime::now() - std::time::Duration::from_secs(660);
            states.insert("expired-state".into(), expired);
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

    // ────────────────────────────────────────────────────────────
    // Derivation tests (Step 10 — Skills Sync)
    //
    // These pin the cross-product `codemux-api-*` derivation
    // protocol against Vexis. Any drift in either implementation
    // fails CI and signals that cross-product login or E2E sync
    // for shared-account users is broken.
    //
    // Mirrors the test suite in
    // `feature/skills-sync-stage-1`'s `auth/derivation.rs`. When
    // the auth-module split lands on agent-chat (via main merge),
    // these tests move along with the code.
    // ────────────────────────────────────────────────────────────

    use base64::engine::general_purpose::STANDARD_NO_PAD;
    use std::time::Instant;

    const PASS: &str = "correct horse battery staple";
    const PASS_WRONG: &str = "Tr0ub4dor&3";
    const EMAIL: &str = "alice@example.com";
    const EMAIL_OTHER: &str = "bob@example.com";

    #[test]
    fn auth_secret_is_valid_base64_of_32_bytes() {
        let secret = derive_auth_secret(PASS, EMAIL).expect("derive");
        let decoded = STANDARD_NO_PAD
            .decode(secret.as_str())
            .expect("auth_secret must be valid base64");
        assert_eq!(decoded.len(), 32);
    }

    #[test]
    fn auth_secret_is_deterministic() {
        let a = derive_auth_secret(PASS, EMAIL).unwrap();
        let b = derive_auth_secret(PASS, EMAIL).unwrap();
        assert_eq!(a.as_str(), b.as_str());
    }

    #[test]
    fn different_password_yields_different_auth_secret() {
        let a = derive_auth_secret(PASS, EMAIL).unwrap();
        let b = derive_auth_secret(PASS_WRONG, EMAIL).unwrap();
        assert_ne!(a.as_str(), b.as_str());
    }

    #[test]
    fn different_email_yields_different_auth_secret() {
        let a = derive_auth_secret(PASS, EMAIL).unwrap();
        let b = derive_auth_secret(PASS, EMAIL_OTHER).unwrap();
        assert_ne!(a.as_str(), b.as_str());
    }

    #[test]
    fn email_normalization_lowercases_and_trims() {
        // `Alice@Example.COM` and `  alice@example.com  ` must
        // produce the same AuthSecret — multi-device determinism.
        let a = derive_auth_secret(PASS, "Alice@Example.COM").unwrap();
        let b = derive_auth_secret(PASS, "  alice@example.com  ").unwrap();
        let c = derive_auth_secret(PASS, "alice@example.com").unwrap();
        assert_eq!(a.as_str(), b.as_str());
        assert_eq!(b.as_str(), c.as_str());
    }

    #[test]
    fn empty_password_does_not_panic() {
        let secret = derive_auth_secret("", EMAIL).expect("derive empty");
        let decoded = STANDARD_NO_PAD.decode(secret.as_str()).unwrap();
        assert_eq!(decoded.len(), 32);
    }

    #[test]
    fn auth_secret_does_not_contain_password_substring() {
        // HKDF output is indistinguishable from random — a leak of
        // the raw password into the encoded AuthSecret would mean
        // the derivation is broken.
        let password = "super-distinctive-plaintext-password-12345";
        let secret = derive_auth_secret(password, EMAIL).unwrap();
        assert!(!secret.as_str().contains(password));
    }

    #[test]
    fn auth_secret_debug_redacts() {
        let secret = derive_auth_secret(PASS, EMAIL).unwrap();
        let rendered = format!("{:?}", secret);
        assert!(rendered.contains("***"));
        assert!(!rendered.contains(secret.as_str()));
    }

    #[test]
    fn derivation_timing_at_least_100ms() {
        // Pins the Argon2id cost — drop below 100ms and dictionary
        // attacks on weak passwords become feasible.
        let start = Instant::now();
        let _ = derive_auth_secret(PASS, EMAIL).expect("derive");
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() >= 100,
            "argon2id derivation should take ≥ 100ms (got {}ms)",
            elapsed.as_millis()
        );
        assert!(elapsed.as_secs() < 30);
    }

    /// Cross-product AuthSecret pin — must match Vexis byte-for-byte.
    /// The expected value comes from the `codemux-api-infrastructure`
    /// skill's documented golden value.
    #[test]
    fn auth_secret_matches_vexis_for_known_input() {
        let secret = derive_auth_secret(
            "golden-test-password",
            "golden-test@example.com",
        )
        .unwrap();
        assert_eq!(
            secret.as_str(),
            "9FxAbaiRLQfRmjpB6x4d3FuamAUojg9bh9dVfPYRfyI",
            "Codemux derivation diverged from Vexis — cross-product login is broken"
        );
    }

    // ── Encryption key half ─────────────────────────────────────

    #[test]
    fn encryption_key_is_32_raw_bytes() {
        let (_auth, key) = derive_login_credentials(PASS, EMAIL).unwrap();
        assert_eq!(key.as_bytes().len(), 32);
    }

    #[test]
    fn encryption_key_is_deterministic() {
        let (_a_auth, a_key) = derive_login_credentials(PASS, EMAIL).unwrap();
        let (_b_auth, b_key) = derive_login_credentials(PASS, EMAIL).unwrap();
        assert_eq!(a_key.as_bytes(), b_key.as_bytes());
    }

    #[test]
    fn encryption_key_differs_from_auth_secret() {
        // HKDF domain separation: the two halves come from different
        // info labels and must produce different bytes. If they ever
        // coincide, observing the server-visible AuthSecret would
        // leak the client-only EncryptionKey — catastrophic.
        let (auth, key) = derive_login_credentials(PASS, EMAIL).unwrap();
        let auth_decoded = STANDARD_NO_PAD.decode(auth.as_str()).unwrap();
        assert_ne!(auth_decoded.as_slice(), key.as_bytes().as_slice());
    }

    #[test]
    fn encryption_key_changes_with_password() {
        let (_a, key_a) = derive_login_credentials(PASS, EMAIL).unwrap();
        let (_b, key_b) = derive_login_credentials(PASS_WRONG, EMAIL).unwrap();
        assert_ne!(key_a.as_bytes(), key_b.as_bytes());
    }

    #[test]
    fn encryption_key_changes_with_email() {
        let (_a, key_a) = derive_login_credentials(PASS, EMAIL).unwrap();
        let (_b, key_b) = derive_login_credentials(PASS, EMAIL_OTHER).unwrap();
        assert_ne!(key_a.as_bytes(), key_b.as_bytes());
    }

    #[test]
    fn encryption_key_email_normalization_matches() {
        let (_a, key_a) = derive_login_credentials(PASS, "Alice@Example.COM").unwrap();
        let (_b, key_b) = derive_login_credentials(PASS, "alice@example.com").unwrap();
        assert_eq!(key_a.as_bytes(), key_b.as_bytes());
    }

    #[test]
    fn encryption_key_debug_redacts() {
        let (_auth, key) = derive_login_credentials(PASS, EMAIL).unwrap();
        let rendered = format!("{:?}", key);
        assert!(rendered.contains("***"));
    }

    /// **Cross-product encryption-key pin.** Must match Vexis
    /// byte-for-byte. Expected hex from `codemux-api-infrastructure`
    /// skill: this is the canary that catches any drift between
    /// Codemux's and Vexis's encryption_key derivation.
    #[test]
    fn encryption_key_matches_vexis_for_known_input() {
        let (_auth, key) = derive_login_credentials(
            "golden-test-password",
            "golden-test@example.com",
        )
        .unwrap();
        let hex_str: String = key
            .as_bytes()
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect();
        assert_eq!(
            hex_str,
            "0b03b2541a15d39e7a422df4a6e6ade56b371453d5b41c1ec117efa62db54534",
            "Codemux encryption_key derivation diverged from Vexis — cross-product E2E sync is broken"
        );
    }
}
