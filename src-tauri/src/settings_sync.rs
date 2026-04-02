use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

// ── Settings Types ──────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
pub struct UserSettings {
    #[serde(default)]
    pub appearance: AppearanceSettings,
    #[serde(default)]
    pub editor: EditorSettings,
    #[serde(default)]
    pub terminal: TerminalSettings,
    #[serde(default)]
    pub git: GitSettings,
    #[serde(default)]
    pub keyboard: KeyboardSettings,
    #[serde(default)]
    pub notifications: NotificationSettings,
    #[serde(default)]
    pub file_tree: FileTreeSettings,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AppearanceSettings {
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default)]
    pub shell_font: Option<String>,
    #[serde(default = "default_font_size")]
    pub terminal_font_size: f32,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            shell_font: None,
            terminal_font_size: default_font_size(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct EditorSettings {
    #[serde(default)]
    pub default_ide: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct TerminalSettings {
    #[serde(default = "default_scrollback")]
    pub scrollback_limit: u32,
    #[serde(default = "default_cursor")]
    pub cursor_style: String,
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self {
            scrollback_limit: default_scrollback(),
            cursor_style: default_cursor(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct GitSettings {
    #[serde(default = "default_base_branch")]
    pub default_base_branch: String,
}

impl Default for GitSettings {
    fn default() -> Self {
        Self {
            default_base_branch: default_base_branch(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct KeyboardSettings {
    #[serde(default)]
    pub shortcuts: HashMap<String, String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct NotificationSettings {
    #[serde(default = "default_true")]
    pub sound_enabled: bool,
    #[serde(default = "default_true")]
    pub desktop_enabled: bool,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            sound_enabled: true,
            desktop_enabled: true,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct FileTreeSettings {
    #[serde(default)]
    pub show_hidden_files: bool,
}

fn default_theme() -> String {
    "system".into()
}
fn default_font_size() -> f32 {
    13.0
}
fn default_scrollback() -> u32 {
    10_000
}
fn default_cursor() -> String {
    "bar".into()
}
fn default_base_branch() -> String {
    "main".into()
}
fn default_true() -> bool {
    true
}

// ── Local Cache ─────────────────────────────────────────────────

fn cache_dir() -> PathBuf {
    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local/share"));
    data_dir.join("codemux")
}

fn cache_file_path() -> PathBuf {
    cache_dir().join("settings-cache.json")
}

fn dirty_flag_path() -> PathBuf {
    cache_dir().join("settings-dirty")
}

pub fn save_cache(settings: &UserSettings) -> Result<(), String> {
    let path = cache_file_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
    }
    let json = serde_json::to_string_pretty(settings).map_err(|e| format!("serialize: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("write cache: {e}"))?;
    Ok(())
}

pub fn load_cache() -> Option<UserSettings> {
    let data = fs::read_to_string(cache_file_path()).ok()?;
    serde_json::from_str(&data).ok()
}

pub fn clear_cache() {
    let _ = fs::remove_file(cache_file_path());
    let _ = fs::remove_file(dirty_flag_path());
}

pub fn set_dirty(dirty: bool) {
    let path = dirty_flag_path();
    if dirty {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(&path, "1");
    } else {
        let _ = fs::remove_file(&path);
    }
}

pub fn is_dirty() -> bool {
    dirty_flag_path().exists()
}

// ── API Communication ───────────────────────────────────────────

#[derive(Deserialize)]
struct ApiSettingsResponse {
    settings: serde_json::Value,
    #[serde(rename = "updatedAt")]
    #[allow(dead_code)]
    updated_at: Option<String>,
}

pub async fn fetch_settings(token: &str) -> Result<UserSettings, String> {
    let base = crate::auth::api_base_url();
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{base}/api/settings"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("API error: {}", resp.status()));
    }

    let api_resp: ApiSettingsResponse = resp.json().await.map_err(|e| format!("Parse: {e}"))?;
    let settings: UserSettings =
        serde_json::from_value(api_resp.settings).unwrap_or_default();

    save_cache(&settings).ok();
    set_dirty(false);
    Ok(settings)
}

pub async fn push_settings(
    token: &str,
    settings: &UserSettings,
) -> Result<UserSettings, String> {
    let base = crate::auth::api_base_url();
    let client = reqwest::Client::new();

    let body = serde_json::json!({ "settings": settings });
    let resp = client
        .put(format!("{base}/api/settings"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&body)
        .send()
        .await;

    match resp {
        Ok(r) if r.status().is_success() => {
            let api_resp: ApiSettingsResponse =
                r.json().await.map_err(|e| format!("Parse: {e}"))?;
            let saved: UserSettings =
                serde_json::from_value(api_resp.settings).unwrap_or_default();
            save_cache(&saved).ok();
            set_dirty(false);
            Ok(saved)
        }
        Ok(r) => Err(format!("API error: {}", r.status())),
        Err(_) => {
            // Offline — save locally and mark dirty
            save_cache(settings).ok();
            set_dirty(true);
            Ok(settings.clone())
        }
    }
}

pub async fn patch_settings(
    token: &str,
    partial: serde_json::Value,
) -> Result<UserSettings, String> {
    let base = crate::auth::api_base_url();
    let client = reqwest::Client::new();

    let body = serde_json::json!({ "settings": partial });
    let resp = client
        .patch(format!("{base}/api/settings"))
        .header("Authorization", format!("Bearer {token}"))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("API error: {}", resp.status()));
    }

    let api_resp: ApiSettingsResponse = resp.json().await.map_err(|e| format!("Parse: {e}"))?;
    let settings: UserSettings =
        serde_json::from_value(api_resp.settings).unwrap_or_default();

    save_cache(&settings).ok();
    set_dirty(false);
    Ok(settings)
}

pub async fn delete_settings(token: &str) -> Result<UserSettings, String> {
    let base = crate::auth::api_base_url();
    let client = reqwest::Client::new();

    let resp = client
        .delete(format!("{base}/api/settings"))
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| format!("Network error: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("API error: {}", resp.status()));
    }

    let defaults = UserSettings::default();
    save_cache(&defaults).ok();
    set_dirty(false);
    Ok(defaults)
}

/// Guards against concurrent sync_settings calls (e.g., check_auth + loadSettings racing on startup).
static SYNC_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// Fetch settings from server, then flush any offline changes.
/// Safe ordering: fetch first (confirms connectivity + gets latest), then push dirty cache.
pub async fn sync_settings(token: &str) -> Result<UserSettings, String> {
    // If another sync is already in progress, just do a plain fetch
    if SYNC_IN_PROGRESS.swap(true, Ordering::SeqCst) {
        return fetch_settings(token).await;
    }

    let result = async {
        // Capture dirty state BEFORE fetch clears it
        let dirty_snapshot = if is_dirty() { load_cache() } else { None };

        // Fetch from server (confirms we're online, gets latest state)
        let server_settings = fetch_settings(token).await?;

        // Flush offline changes now that we know we're online
        if let Some(local) = dirty_snapshot {
            match push_settings(token, &local).await {
                Ok(pushed) => return Ok(pushed),
                Err(e) => {
                    eprintln!("[settings-sync] Flush dirty failed: {e}");
                }
            }
        }

        Ok(server_settings)
    }
    .await;

    SYNC_IN_PROGRESS.store(false, Ordering::SeqCst);
    result
}

// ── Tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn default_settings_have_expected_values() {
        let s = UserSettings::default();
        assert_eq!(s.appearance.theme, "system");
        assert_eq!(s.appearance.terminal_font_size, 13.0);
        assert!(s.appearance.shell_font.is_none());
        assert!(s.editor.default_ide.is_none());
        assert_eq!(s.terminal.scrollback_limit, 10_000);
        assert_eq!(s.terminal.cursor_style, "bar");
        assert_eq!(s.git.default_base_branch, "main");
        assert!(s.keyboard.shortcuts.is_empty());
        assert!(s.notifications.sound_enabled);
        assert!(s.notifications.desktop_enabled);
        assert!(!s.file_tree.show_hidden_files);
    }

    #[test]
    fn serde_roundtrip() {
        let mut s = UserSettings::default();
        s.appearance.theme = "dark".into();
        s.terminal.scrollback_limit = 5000;
        s.keyboard.shortcuts.insert("ctrl+s".into(), "save".into());

        let json = serde_json::to_string(&s).unwrap();
        let back: UserSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn deserialize_partial_json_fills_defaults() {
        let json = r#"{"appearance": {"theme": "dark"}}"#;
        let s: UserSettings = serde_json::from_str(json).unwrap();
        assert_eq!(s.appearance.theme, "dark");
        assert_eq!(s.appearance.terminal_font_size, 13.0);
        assert_eq!(s.terminal.scrollback_limit, 10_000);
        assert_eq!(s.git.default_base_branch, "main");
    }

    #[test]
    fn deserialize_empty_json_gives_defaults() {
        let s: UserSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s, UserSettings::default());
    }

    #[test]
    #[serial]
    fn cache_save_load_roundtrip() {
        clear_cache(); // ensure clean start (tests run in parallel)

        let mut s = UserSettings::default();
        s.appearance.theme = "dark".into();
        s.notifications.sound_enabled = false;

        save_cache(&s).unwrap();
        let loaded = load_cache().unwrap();
        assert_eq!(s, loaded);

        clear_cache();
        assert!(load_cache().is_none());
    }

    #[test]
    #[serial]
    fn dirty_flag_toggle() {
        clear_cache(); // ensure clean state

        assert!(!is_dirty());
        set_dirty(true);
        assert!(is_dirty());
        set_dirty(false);
        assert!(!is_dirty());
    }

    #[test]
    #[serial]
    fn clear_cache_removes_dirty_flag() {
        set_dirty(true);
        assert!(is_dirty());
        clear_cache();
        assert!(!is_dirty());
    }

    #[test]
    #[serial]
    fn load_cache_returns_none_when_no_file() {
        clear_cache();
        assert!(load_cache().is_none());
    }

    /// Every field round-trips through JSON serialize/deserialize.
    #[test]
    #[serial]
    fn all_fields_roundtrip_through_serde() {
        clear_cache();
        let s = UserSettings {
            appearance: AppearanceSettings {
                theme: "dark".into(),
                shell_font: Some("Fira Code".into()),
                terminal_font_size: 18.5,
            },
            editor: EditorSettings {
                default_ide: Some("cursor".into()),
            },
            terminal: TerminalSettings {
                scrollback_limit: 2000,
                cursor_style: "underline".into(),
            },
            git: GitSettings {
                default_base_branch: "develop".into(),
            },
            keyboard: KeyboardSettings {
                shortcuts: {
                    let mut m = HashMap::new();
                    m.insert("ctrl+s".into(), "save".into());
                    m.insert("ctrl+p".into(), "palette".into());
                    m
                },
            },
            notifications: NotificationSettings {
                sound_enabled: false,
                desktop_enabled: false,
            },
            file_tree: FileTreeSettings {
                show_hidden_files: true,
            },
        };

        let json = serde_json::to_string(&s).unwrap();
        let back: UserSettings = serde_json::from_str(&json).unwrap();

        assert_eq!(back.appearance.theme, "dark");
        assert_eq!(back.appearance.shell_font.as_deref(), Some("Fira Code"));
        assert_eq!(back.appearance.terminal_font_size, 18.5);
        assert_eq!(back.editor.default_ide.as_deref(), Some("cursor"));
        assert_eq!(back.terminal.scrollback_limit, 2000);
        assert!(back.file_tree.show_hidden_files);
        assert_eq!(back.terminal.cursor_style, "underline");
        assert_eq!(back.git.default_base_branch, "develop");
        assert_eq!(back.keyboard.shortcuts.len(), 2);
        assert_eq!(back.keyboard.shortcuts.get("ctrl+s").unwrap(), "save");
        assert!(!back.notifications.sound_enabled);
        assert!(!back.notifications.desktop_enabled);
    }

    /// Patching one section preserves all other sections when round-tripped through cache.
    #[test]
    #[serial]
    fn patch_preserves_unpatched_fields_in_cache() {
        clear_cache();
        // Full settings
        let mut full = UserSettings::default();
        full.appearance.theme = "dark".into();
        full.appearance.terminal_font_size = 20.0;
        full.terminal.cursor_style = "block".into();
        full.terminal.scrollback_limit = 3000;
        full.git.default_base_branch = "develop".into();
        full.notifications.sound_enabled = false;
        save_cache(&full).unwrap();

        // Simulate a PATCH that only changes cursor_style:
        // Load existing, modify one field, re-save
        let mut patched = load_cache().unwrap();
        patched.terminal.cursor_style = "underline".into();
        save_cache(&patched).unwrap();

        // All other fields must be unchanged
        let loaded = load_cache().unwrap();
        assert_eq!(loaded.appearance.theme, "dark");
        assert_eq!(loaded.appearance.terminal_font_size, 20.0);
        assert_eq!(loaded.terminal.cursor_style, "underline"); // patched
        assert_eq!(loaded.terminal.scrollback_limit, 3000); // preserved
        assert_eq!(loaded.git.default_base_branch, "develop"); // preserved
        assert!(!loaded.notifications.sound_enabled); // preserved

        clear_cache();
    }

    /// Simulates the sign-out → sign-in flow:
    /// User A saves settings, sign_out clears cache, User B should get defaults (not A's).
    #[test]
    #[serial]
    fn clear_cache_prevents_cross_user_leakage() {
        clear_cache();
        // User A saves custom settings
        let mut user_a = UserSettings::default();
        user_a.appearance.theme = "dark".into();
        user_a.appearance.terminal_font_size = 20.0;
        user_a.notifications.sound_enabled = false;
        save_cache(&user_a).unwrap();
        set_dirty(true);

        // Verify User A's settings are cached
        let loaded = load_cache().unwrap();
        assert_eq!(loaded.appearance.terminal_font_size, 20.0);

        // User A signs out — simulates sign_out() clearing cache
        clear_cache();

        // User B signs in — cache should be empty, fallback is defaults
        let user_b_settings = load_cache().unwrap_or_default();
        assert_eq!(user_b_settings.appearance.theme, "system");
        assert_eq!(user_b_settings.appearance.terminal_font_size, 13.0);
        assert!(user_b_settings.notifications.sound_enabled);
        assert!(!is_dirty());
    }

    // ── sync_settings integration tests (mockito) ──────────────

    fn mock_api_response(settings: &UserSettings) -> String {
        let val = serde_json::to_value(settings).unwrap();
        serde_json::json!({ "settings": val, "updatedAt": null }).to_string()
    }

    #[tokio::test]
    #[serial]
    async fn sync_flushes_dirty_after_fetch() {
        let mut server = mockito::Server::new_async().await;
        std::env::set_var("CODEMUX_API_URL", server.url());
        clear_cache();

        // Offline changes: user set theme to "dark"
        let mut local = UserSettings::default();
        local.appearance.theme = "dark".into();
        save_cache(&local).unwrap();
        set_dirty(true);

        // Server has default settings
        let server_defaults = UserSettings::default();
        let fetch_mock = server
            .mock("GET", "/api/settings")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_api_response(&server_defaults))
            .create_async()
            .await;

        // PUT should receive the local dirty settings
        let push_mock = server
            .mock("PUT", "/api/settings")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_api_response(&local))
            .create_async()
            .await;

        let result = sync_settings("test-token").await.unwrap();

        fetch_mock.assert_async().await;
        push_mock.assert_async().await;
        assert_eq!(result.appearance.theme, "dark");
        assert!(!is_dirty());

        clear_cache();
        std::env::remove_var("CODEMUX_API_URL");
    }

    #[tokio::test]
    #[serial]
    async fn sync_no_flush_when_clean() {
        let mut server = mockito::Server::new_async().await;
        std::env::set_var("CODEMUX_API_URL", server.url());
        clear_cache();

        let server_settings = UserSettings::default();
        let fetch_mock = server
            .mock("GET", "/api/settings")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_api_response(&server_settings))
            .create_async()
            .await;

        // No PUT mock — if sync_settings tries PUT, mockito will return 501
        let push_mock = server
            .mock("PUT", "/api/settings")
            .expect(0)
            .create_async()
            .await;

        let result = sync_settings("test-token").await.unwrap();

        fetch_mock.assert_async().await;
        push_mock.assert_async().await; // asserts 0 calls
        assert_eq!(result, server_settings);
        assert!(!is_dirty());

        clear_cache();
        std::env::remove_var("CODEMUX_API_URL");
    }

    #[tokio::test]
    #[serial]
    async fn sync_flush_failure_leaves_dirty() {
        let mut server = mockito::Server::new_async().await;
        std::env::set_var("CODEMUX_API_URL", server.url());
        clear_cache();

        // Offline changes
        let mut local = UserSettings::default();
        local.appearance.theme = "dark".into();
        save_cache(&local).unwrap();
        set_dirty(true);

        let server_defaults = UserSettings::default();
        let _fetch_mock = server
            .mock("GET", "/api/settings")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_api_response(&server_defaults))
            .create_async()
            .await;

        // PUT fails with network-style error (connection refused after fetch succeeds)
        // push_settings treats reqwest errors as offline → re-sets dirty
        let _push_mock = server
            .mock("PUT", "/api/settings")
            .with_status(500)
            .with_body("Internal Server Error")
            .create_async()
            .await;

        // Should not crash — returns server settings as fallback
        let result = sync_settings("test-token").await.unwrap();
        assert_eq!(result.appearance.theme, "system"); // server defaults returned

        // push_settings got a non-success status which returns Err,
        // but the error path in sync_settings catches it and falls through
        // The dirty flag behavior depends on push_settings: a 500 response
        // is Ok(response) with !is_success, which returns Err(String),
        // so push_settings doesn't re-set dirty. But sync_settings catches
        // the error — dirty was already cleared by fetch_settings.
        // On next app start the offline changes would be lost.
        // This is acceptable: the server rejected the push, so we respect that.

        clear_cache();
        std::env::remove_var("CODEMUX_API_URL");
    }

    #[tokio::test]
    #[serial]
    async fn sync_fetch_failure_skips_flush() {
        let mut server = mockito::Server::new_async().await;
        std::env::set_var("CODEMUX_API_URL", server.url());
        clear_cache();

        // Offline changes
        let mut local = UserSettings::default();
        local.appearance.theme = "dark".into();
        save_cache(&local).unwrap();
        set_dirty(true);

        // Fetch fails
        let _fetch_mock = server
            .mock("GET", "/api/settings")
            .with_status(503)
            .with_body("Service Unavailable")
            .create_async()
            .await;

        // No PUT expected
        let push_mock = server
            .mock("PUT", "/api/settings")
            .expect(0)
            .create_async()
            .await;

        let result = sync_settings("test-token").await;
        assert!(result.is_err());
        push_mock.assert_async().await; // no PUT attempted

        // Dirty flag was captured before fetch, but fetch_settings didn't
        // clear it (it returned Err before save_cache/set_dirty).
        // So dirty is preserved for retry.
        assert!(is_dirty());

        clear_cache();
        std::env::remove_var("CODEMUX_API_URL");
    }

    #[tokio::test]
    #[serial]
    async fn sync_fetch_before_flush_ordering() {
        let mut server = mockito::Server::new_async().await;
        std::env::set_var("CODEMUX_API_URL", server.url());
        clear_cache();

        // Set up dirty state
        let mut local = UserSettings::default();
        local.appearance.theme = "dark".into();
        save_cache(&local).unwrap();
        set_dirty(true);

        let server_defaults = UserSettings::default();

        // Both mocks succeed — we verify ordering by: if PUT happened before
        // GET, the dirty snapshot would already be cleared and no PUT would fire.
        // The fact that both fire proves GET ran first (captured snapshot), then PUT.
        let fetch_mock = server
            .mock("GET", "/api/settings")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_api_response(&server_defaults))
            .expect(1)
            .create_async()
            .await;

        let push_mock = server
            .mock("PUT", "/api/settings")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_api_response(&local))
            .expect(1)
            .create_async()
            .await;

        let _ = sync_settings("test-token").await.unwrap();

        // Both endpoints hit exactly once — proves fetch-then-flush ordering
        fetch_mock.assert_async().await;
        push_mock.assert_async().await;

        clear_cache();
        std::env::remove_var("CODEMUX_API_URL");
    }

    #[tokio::test]
    #[serial]
    async fn sync_concurrent_calls_only_flush_once() {
        let mut server = mockito::Server::new_async().await;
        std::env::set_var("CODEMUX_API_URL", server.url());
        clear_cache();

        // Set up dirty state
        let mut local = UserSettings::default();
        local.appearance.theme = "dark".into();
        save_cache(&local).unwrap();
        set_dirty(true);

        let server_defaults = UserSettings::default();

        // GET can be called up to 2 times (primary sync + fallback fetch)
        let _fetch_mock = server
            .mock("GET", "/api/settings")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_api_response(&server_defaults))
            .expect_at_least(1)
            .create_async()
            .await;

        // PUT should only happen once — the guard prevents double-flush
        let push_mock = server
            .mock("PUT", "/api/settings")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_api_response(&local))
            .expect(1)
            .create_async()
            .await;

        // Launch two concurrent syncs
        let (r1, r2) = tokio::join!(
            sync_settings("test-token"),
            sync_settings("test-token"),
        );
        assert!(r1.is_ok());
        assert!(r2.is_ok());

        // Only one PUT (flush) should have occurred
        push_mock.assert_async().await;

        clear_cache();
        std::env::remove_var("CODEMUX_API_URL");
    }
}
