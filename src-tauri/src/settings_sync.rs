use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

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

/// If settings were changed while offline, push them now.
pub async fn flush_dirty(token: &str) -> Result<(), String> {
    if !is_dirty() {
        return Ok(());
    }
    if let Some(cached) = load_cache() {
        push_settings(token, &cached).await?;
    }
    Ok(())
}

// ── Tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

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
    fn dirty_flag_toggle() {
        clear_cache(); // ensure clean state

        assert!(!is_dirty());
        set_dirty(true);
        assert!(is_dirty());
        set_dirty(false);
        assert!(!is_dirty());
    }

    #[test]
    fn clear_cache_removes_dirty_flag() {
        set_dirty(true);
        assert!(is_dirty());
        clear_cache();
        assert!(!is_dirty());
    }

    #[test]
    fn load_cache_returns_none_when_no_file() {
        clear_cache();
        assert!(load_cache().is_none());
    }

    /// Every field round-trips through JSON serialize/deserialize.
    #[test]
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
        };

        let json = serde_json::to_string(&s).unwrap();
        let back: UserSettings = serde_json::from_str(&json).unwrap();

        assert_eq!(back.appearance.theme, "dark");
        assert_eq!(back.appearance.shell_font.as_deref(), Some("Fira Code"));
        assert_eq!(back.appearance.terminal_font_size, 18.5);
        assert_eq!(back.editor.default_ide.as_deref(), Some("cursor"));
        assert_eq!(back.terminal.scrollback_limit, 2000);
        assert_eq!(back.terminal.cursor_style, "underline");
        assert_eq!(back.git.default_base_branch, "develop");
        assert_eq!(back.keyboard.shortcuts.len(), 2);
        assert_eq!(back.keyboard.shortcuts.get("ctrl+s").unwrap(), "save");
        assert!(!back.notifications.sound_enabled);
        assert!(!back.notifications.desktop_enabled);
    }

    /// Patching one section preserves all other sections when round-tripped through cache.
    #[test]
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
}
