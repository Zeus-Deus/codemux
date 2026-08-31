use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use tokio::sync::Notify;

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
    pub source_control: SourceControlSettings,
    #[serde(default)]
    pub keyboard: KeyboardSettings,
    #[serde(default)]
    pub notifications: NotificationSettings,
    #[serde(default)]
    pub file_tree: FileTreeSettings,
    #[serde(default)]
    pub session_restore: SessionRestoreSettings,
    #[serde(default)]
    pub agent_chat: AgentChatSettings,
    #[serde(default)]
    pub browser: BrowserSettings,
}

/// Agent-browser behavior knobs.
///
/// `default_viewport` is the user's preferred starting viewport for
/// agent-browser sessions, as a spec string accepted by
/// `browser_viewport::parse_spec` — a preset name (`"desktop-large"`)
/// or custom `"WxH"` (`"2560x1440"`). When set, a freshly launched
/// agent-browser daemon gets this viewport applied before its first
/// action, and `viewport reset` returns to it instead of the built-in
/// `RESET_SPEC` baseline — so screenshots the agent takes match the
/// user's own screen proportions. `None` (the default) keeps today's
/// behavior everywhere (1280×800 baseline). Invalid strings are
/// treated as unset rather than erroring — settings blobs sync across
/// devices and a bad value must never break browser startup.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct BrowserSettings {
    #[serde(default)]
    pub default_viewport: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AppearanceSettings {
    #[serde(default = "default_theme")]
    pub theme: String,
    /// Versioned custom theme payloads. The frontend owns the extensible role
    /// schema; raw JSON keeps settings forward-compatible across clients.
    #[serde(default)]
    pub custom_themes: Vec<serde_json::Value>,
    #[serde(default)]
    pub shell_font: Option<String>,
    #[serde(default = "default_typography_mode")]
    pub typography_mode: String,
    #[serde(default)]
    pub interface_font_family: Option<String>,
    #[serde(default = "default_interface_font_size")]
    pub interface_font_size: f32,
    #[serde(default)]
    pub conversation_font_family: Option<String>,
    #[serde(default = "default_conversation_font_size")]
    pub conversation_font_size: f32,
    #[serde(default)]
    pub code_font_family: Option<String>,
    /// `None` is the migration marker for settings written before code and
    /// terminal typography were separated. The frontend resolves it from the
    /// existing terminal size, preserving large-text accessibility choices.
    #[serde(default)]
    pub code_font_size: Option<f32>,
    #[serde(default)]
    pub terminal_font_family: Option<String>,
    #[serde(default = "default_font_size")]
    pub terminal_font_size: f32,
    /// Show the resource monitor (CPU/memory) icon in the title bar.
    #[serde(default = "default_true")]
    pub show_resource_monitor: bool,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            custom_themes: Vec::new(),
            shell_font: None,
            typography_mode: default_typography_mode(),
            interface_font_family: None,
            interface_font_size: default_interface_font_size(),
            conversation_font_family: None,
            conversation_font_size: default_conversation_font_size(),
            code_font_family: None,
            code_font_size: Some(default_font_size()),
            terminal_font_family: None,
            terminal_font_size: default_font_size(),
            show_resource_monitor: true,
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

/// Git *hosting* knobs, as opposed to the local-git ones in
/// [`GitSettings`].
///
/// `custom_hosts` maps a bare hostname to a hosting product
/// (`"git.acme.internal" -> "gitlab"`), for self-hosted instances whose
/// domain gives nothing away. It is the highest-priority input to
/// provider detection — see `crate::git_provider::detect::classify_host`.
/// Values are read leniently: an entry naming a product this build does
/// not know is skipped rather than failing the whole settings blob,
/// because these sync across devices and versions.
///
/// `open_pr_links_in_browser` opts out of opening a host pull-request URL
/// on the Pull Requests page: with it set, such a link goes to the
/// system browser like any other. Defaults to `false` — the in-app page
/// is the better destination — and is only ever read by the frontend.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct SourceControlSettings {
    #[serde(default)]
    pub custom_hosts: HashMap<String, String>,
    #[serde(default)]
    pub open_pr_links_in_browser: bool,
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

/// Agent-chat behavior knobs. `checkpoints_enabled` is the opt-in for
/// provider-aware per-turn revert checkpoints: before a supported provider
/// dispatches a turn, Codemux snapshots the workspace so files, native
/// conversation history, and the transcript can be rewound together.
/// Defaults to OFF because snapshots write objects into the user's repo.
///
/// `background_browser_desktop_viewport` pins a GUI-mode background
/// browser's CDP viewport to a real desktop size (1280×800, matching
/// the `desktop` preset / `RESET_SPEC` in `browser_viewport.rs`) when
/// the peek popover (`BrowserPeekOverlay.tsx`) is showing it, instead
/// of shrinking the viewport to the tiny popover's pixel size — the
/// popover's canvas letterboxes the larger frame down to fit. Defaults
/// to ON — pages render at real desktop size out of the box and agents
/// don't have to re-send `viewport` each turn; blobs where the user
/// explicitly saved `false` keep the container-sync behavior.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AgentChatSettings {
    #[serde(default)]
    pub checkpoints_enabled: bool,
    #[serde(default = "default_true")]
    pub background_browser_desktop_viewport: bool,
}

impl Default for AgentChatSettings {
    fn default() -> Self {
        Self {
            checkpoints_enabled: false,
            background_browser_desktop_viewport: true,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SessionRestoreSettings {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_scrollback_lines")]
    pub scrollback_lines: u32,
    #[serde(default = "default_max_total_mb")]
    pub max_total_mb: u32,
}

impl Default for SessionRestoreSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            scrollback_lines: default_scrollback_lines(),
            max_total_mb: default_max_total_mb(),
        }
    }
}

fn default_theme() -> String {
    "default".into()
}
fn default_typography_mode() -> String {
    "simple".into()
}
fn default_interface_font_size() -> f32 {
    16.0
}
fn default_conversation_font_size() -> f32 {
    14.0
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
fn default_scrollback_lines() -> u32 {
    10_000
}
fn default_max_total_mb() -> u32 {
    100
}

// ── Local Cache ─────────────────────────────────────────────────

fn cache_dir() -> PathBuf {
    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local/share"));
    data_dir.join(crate::APP_DIR_NAME)
}

fn cache_file_path() -> PathBuf {
    cache_dir().join("settings-cache.json")
}

fn dirty_flag_path() -> PathBuf {
    cache_dir().join("settings-dirty")
}

#[derive(Serialize, Deserialize)]
struct ScopedSettingsCache {
    user_id: Option<String>,
    settings: UserSettings,
}

static CACHE_OWNER: LazyLock<Mutex<Option<String>>> = LazyLock::new(|| Mutex::new(None));

/// Bumped on every settings cache write. All writes funnel through
/// [`save_cache_as_owner`], whose callers hold the `CACHE_OWNER` lock, so
/// comparing this counter under that lock is race-free. A fetch captures it
/// together with its dirty snapshot and refuses to install the server
/// response over a cache state it never observed — see
/// [`install_fetched_settings`].
static CACHE_WRITE_GENERATION: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct CacheScope(Option<String>);

/// Select the account that may read/write the settings cache. Changing owner
/// invalidates process-local derived values immediately; a cache owned by a
/// different account is treated as absent rather than leaked across sessions.
pub fn set_cache_owner(user_id: Option<&str>) {
    let next = user_id.map(str::to_owned);
    let changed = if let Ok(mut owner) = CACHE_OWNER.lock() {
        if *owner == next {
            false
        } else {
            *owner = next.clone();
            true
        }
    } else {
        false
    };
    if !changed {
        return;
    }

    CHECKPOINTS_ENABLED_CACHE.store(0, Ordering::Release);
    crate::git_provider::invalidate_detection_cache(None);

    // Upgrade an old unscoped cache only when a concrete cached identity is
    // known. Signed-out startup must never interpret legacy account settings
    // as a local/public profile.
    let Some(owner) = next else {
        return;
    };
    let path = cache_file_path();
    let Some(legacy) = fs::read_to_string(&path)
        .ok()
        .filter(|data| serde_json::from_str::<ScopedSettingsCache>(data).is_err())
        .and_then(|data| serde_json::from_str::<UserSettings>(&data).ok())
    else {
        return;
    };
    // The owner can change while the file is read (for example, an OAuth
    // callback racing sign-out). Re-check it at the write boundary so legacy
    // migration cannot overwrite the next account's cache.
    let _ = save_cache_for_scope(&legacy, &CacheScope(Some(owner)));
}

fn cache_owner() -> Option<String> {
    CACHE_OWNER.lock().ok().and_then(|owner| owner.clone())
}

fn cache_scope() -> CacheScope {
    CacheScope(cache_owner())
}

fn cache_scope_for_owner(user_id: &str) -> CacheScope {
    CacheScope(Some(user_id.to_owned()))
}

fn ensure_scope_current(scope: &CacheScope) -> Result<(), String> {
    let owner = CACHE_OWNER
        .lock()
        .map_err(|_| "settings cache owner lock poisoned".to_string())?;
    if scope.0 == *owner {
        Ok(())
    } else {
        Err("Settings account changed while request was in flight".into())
    }
}

// Turn dispatch is latency-sensitive. Reading and deserializing the complete
// settings file for every prompt is unnecessary because every in-process
// settings mutation funnels through `save_cache`/`clear_cache`. Zero means
// uninitialized, one means disabled, and two means enabled.
static CHECKPOINTS_ENABLED_CACHE: AtomicU8 = AtomicU8::new(0);

fn set_cached_checkpoints_enabled(enabled: bool) {
    CHECKPOINTS_ENABLED_CACHE.store(if enabled { 2 } else { 1 }, Ordering::Release);
}

pub fn agent_chat_checkpoints_enabled() -> bool {
    match CHECKPOINTS_ENABLED_CACHE.load(Ordering::Acquire) {
        1 => false,
        2 => true,
        _ => {
            let enabled = load_cache()
                .map(|settings| settings.agent_chat.checkpoints_enabled)
                .unwrap_or(false);
            set_cached_checkpoints_enabled(enabled);
            enabled
        }
    }
}

fn settings_from_cache_data(data: &str, owner: &Option<String>) -> Option<UserSettings> {
    if let Ok(scoped) = serde_json::from_str::<ScopedSettingsCache>(data) {
        return (scoped.user_id == *owner).then_some(scoped.settings);
    }
    // Legacy data is migrated by `set_cache_owner(Some(identity))`. It is
    // never readable through an unowned/signed-out scope.
    None
}

fn save_cache_as_owner(settings: &UserSettings, owner: &Option<String>) -> Result<(), String> {
    let path = cache_file_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
    }
    // Read before write: provider detection reads `source_control` out of
    // this file and caches its answer for a minute, so a new custom-host
    // mapping would otherwise not take effect until that lapsed. Every
    // settings write funnels through here, which is why the hook lives
    // here rather than at each call site.
    let host_mapping_changed = fs::read_to_string(&path)
        .ok()
        .and_then(|data| settings_from_cache_data(&data, owner))
        .map(|previous| previous.source_control != settings.source_control)
        .unwrap_or(true);

    let scoped = ScopedSettingsCache {
        user_id: owner.clone(),
        settings: settings.clone(),
    };
    let json = serde_json::to_string_pretty(&scoped).map_err(|e| format!("serialize: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("write cache: {e}"))?;
    CACHE_WRITE_GENERATION.fetch_add(1, Ordering::AcqRel);
    set_cached_checkpoints_enabled(settings.agent_chat.checkpoints_enabled);

    if host_mapping_changed {
        crate::git_provider::invalidate_detection_cache(None);
    }
    Ok(())
}

pub fn save_cache(settings: &UserSettings) -> Result<(), String> {
    let owner = CACHE_OWNER
        .lock()
        .map_err(|_| "settings cache owner lock poisoned".to_string())?;
    save_cache_as_owner(settings, &owner)
}

/// Save the settings and set the dirty marker as one step under the cache
/// owner lock. Commands that cannot reach the server (signed out, or a
/// pending unverified identity) use this instead of `save_cache` +
/// `set_dirty` so a concurrent fetch can never interleave between the write
/// and the marker: it either observes both (and stands down, see
/// [`install_fetched_settings`]) or neither.
pub fn save_cache_dirty(settings: &UserSettings) -> Result<(), String> {
    let owner = CACHE_OWNER
        .lock()
        .map_err(|_| "settings cache owner lock poisoned".to_string())?;
    save_cache_as_owner(settings, &owner)?;
    set_dirty(true);
    Ok(())
}

/// Commit a network response only if the account that launched the request is
/// still current. A sign-out or account switch may happen while an HTTP future
/// is suspended; without this guard, the old response would be relabelled with
/// the new global cache owner and exposed on that account's next local boot.
fn save_cache_for_scope(settings: &UserSettings, scope: &CacheScope) -> Result<bool, String> {
    let owner = CACHE_OWNER
        .lock()
        .map_err(|_| "settings cache owner lock poisoned".to_string())?;
    if scope.0 != *owner {
        return Ok(false);
    }
    save_cache_as_owner(settings, &owner)?;
    Ok(true)
}

/// Scope-guarded counterpart of [`save_cache_dirty`]: save an unpushed edit
/// and its dirty marker atomically, but only while `scope` still owns the
/// cache.
fn save_dirty_cache_for_scope(settings: &UserSettings, scope: &CacheScope) -> Result<bool, String> {
    let owner = CACHE_OWNER
        .lock()
        .map_err(|_| "settings cache owner lock poisoned".to_string())?;
    if scope.0 != *owner {
        return Ok(false);
    }
    save_cache_as_owner(settings, &owner)?;
    set_dirty(true);
    Ok(true)
}

fn clear_dirty_for_scope(scope: &CacheScope) -> bool {
    let Ok(owner) = CACHE_OWNER.lock() else {
        return false;
    };
    if scope.0 != *owner {
        return false;
    }
    set_dirty(false);
    true
}

/// Commit a fetched server snapshot to the cache — unless a local settings
/// write landed after `observed_write_generation` was captured. A
/// pending-identity edit can commit (save + dirty marker) while the GET is
/// in flight; overwriting it and clearing its marker would silently discard
/// the edit with no push. The fetch stands down instead and the sync path
/// re-reads and flushes the newer dirty state.
///
/// Returns `Ok(true)` when the snapshot was installed and the dirty marker
/// cleared, `Ok(false)` when a newer local write won, and `Err` when the
/// cache owner changed while the request was in flight.
fn install_fetched_settings(
    settings: &UserSettings,
    scope: &CacheScope,
    observed_write_generation: u64,
) -> Result<bool, String> {
    let owner = CACHE_OWNER
        .lock()
        .map_err(|_| "settings cache owner lock poisoned".to_string())?;
    if scope.0 != *owner {
        return Err("Settings account changed while refresh was in flight".into());
    }
    if CACHE_WRITE_GENERATION.load(Ordering::Acquire) != observed_write_generation {
        return Ok(false);
    }
    save_cache_as_owner(settings, &owner)?;
    set_dirty(false);
    Ok(true)
}

pub fn load_cache() -> Option<UserSettings> {
    let data = fs::read_to_string(cache_file_path()).ok()?;
    let owner = CACHE_OWNER.lock().ok()?;
    settings_from_cache_data(&data, &owner)
}

/// Capture the dirty local settings for `scope` together with the cache
/// write generation observed by the read, atomically under the owner lock.
/// The generation lets the fetch that follows prove no local write landed
/// between this snapshot and the moment the server response is installed —
/// see [`install_fetched_settings`].
fn load_dirty_cache_for_scope(
    scope: &CacheScope,
) -> Result<(u64, Option<UserSettings>), String> {
    let owner = CACHE_OWNER
        .lock()
        .map_err(|_| "settings cache owner lock poisoned".to_string())?;
    if scope.0 != *owner {
        return Err("Settings account changed while request was in flight".into());
    }
    let write_generation = CACHE_WRITE_GENERATION.load(Ordering::Acquire);
    if !dirty_flag_path().exists() {
        return Ok((write_generation, None));
    }
    let Some(data) = fs::read_to_string(cache_file_path()).ok() else {
        return Ok((write_generation, None));
    };
    if let Some(settings) = settings_from_cache_data(&data, &owner) {
        return Ok((write_generation, Some(settings)));
    }
    // Edits made while signed out are cached under owner `None` with the
    // dirty marker set. The first sign-in for a scope must adopt and flush
    // them — otherwise the initial fetch would overwrite the cache and clear
    // the dirty flag, silently discarding the user's pre-account edits. Only
    // the dirty (unpushed) case is adopted; a clean unscoped cache still
    // loses to the server fetch.
    if owner.is_some() {
        return Ok((write_generation, settings_from_cache_data(&data, &None)));
    }
    Ok((write_generation, None))
}

pub fn clear_cache() {
    let _ = fs::remove_file(cache_file_path());
    let _ = fs::remove_file(dirty_flag_path());
    set_cached_checkpoints_enabled(false);
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

const SETTINGS_REMOTE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

fn settings_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(SETTINGS_REMOTE_TIMEOUT)
        .build()
        .map_err(|e| format!("HTTP client: {e}"))
}

async fn fetch_settings_in_scope(
    token: &str,
    scope: &CacheScope,
    observed_write_generation: u64,
) -> Result<UserSettings, String> {
    ensure_scope_current(scope)?;
    let base = crate::auth::api_base_url();
    let client = settings_client()?;
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
    let settings: UserSettings = serde_json::from_value(api_resp.settings).unwrap_or_default();

    // A local edit that committed while the GET was in flight keeps the
    // cache and its dirty marker; the sync path re-reads and pushes it.
    install_fetched_settings(&settings, scope, observed_write_generation)?;
    Ok(settings)
}

pub async fn fetch_settings(token: &str) -> Result<UserSettings, String> {
    let scope = cache_scope();
    let observed_write_generation = CACHE_WRITE_GENERATION.load(Ordering::Acquire);
    fetch_settings_in_scope(token, &scope, observed_write_generation).await
}

async fn push_settings_in_scope(
    token: &str,
    settings: &UserSettings,
    scope: &CacheScope,
    accept_offline_local_write: bool,
) -> Result<UserSettings, String> {
    ensure_scope_current(scope)?;
    let base = crate::auth::api_base_url();
    let client = settings_client()?;

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
            let saved: UserSettings = serde_json::from_value(api_resp.settings).unwrap_or_default();
            if !save_cache_for_scope(&saved, scope)? {
                return Err("Settings account changed while update was in flight".into());
            }
            clear_dirty_for_scope(scope);
            Ok(saved)
        }
        Ok(r) => Err(format!("API error: {}", r.status())),
        Err(error) => {
            // Offline — save locally and mark dirty
            if !save_dirty_cache_for_scope(settings, scope)? {
                return Err("Settings account changed while update was in flight".into());
            }
            if accept_offline_local_write {
                Ok(settings.clone())
            } else {
                Err(format!("Network error: {error}"))
            }
        }
    }
}

pub async fn push_settings(token: &str, settings: &UserSettings) -> Result<UserSettings, String> {
    let scope = cache_scope();
    push_settings_in_scope(token, settings, &scope, true).await
}

pub async fn push_settings_for_owner(
    token: &str,
    user_id: &str,
    settings: &UserSettings,
) -> Result<UserSettings, String> {
    let scope = cache_scope_for_owner(user_id);
    push_settings_in_scope(token, settings, &scope, true).await
}

async fn patch_settings_in_scope(
    token: &str,
    partial: serde_json::Value,
    scope: &CacheScope,
) -> Result<UserSettings, String> {
    ensure_scope_current(scope)?;
    let base = crate::auth::api_base_url();
    let client = settings_client()?;

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
    let settings: UserSettings = serde_json::from_value(api_resp.settings).unwrap_or_default();

    if !save_cache_for_scope(&settings, scope)? {
        return Err("Settings account changed while update was in flight".into());
    }
    clear_dirty_for_scope(scope);
    Ok(settings)
}

pub async fn patch_settings(
    token: &str,
    partial: serde_json::Value,
) -> Result<UserSettings, String> {
    let scope = cache_scope();
    patch_settings_in_scope(token, partial, &scope).await
}

pub async fn patch_settings_for_owner(
    token: &str,
    user_id: &str,
    partial: serde_json::Value,
) -> Result<UserSettings, String> {
    let scope = cache_scope_for_owner(user_id);
    patch_settings_in_scope(token, partial, &scope).await
}

async fn delete_settings_in_scope(token: &str, scope: &CacheScope) -> Result<UserSettings, String> {
    ensure_scope_current(scope)?;
    let base = crate::auth::api_base_url();
    let client = settings_client()?;

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
    if !save_cache_for_scope(&defaults, scope)? {
        return Err("Settings account changed while reset was in flight".into());
    }
    clear_dirty_for_scope(scope);
    Ok(defaults)
}

pub async fn delete_settings(token: &str) -> Result<UserSettings, String> {
    let scope = cache_scope();
    delete_settings_in_scope(token, &scope).await
}

pub async fn delete_settings_for_owner(token: &str, user_id: &str) -> Result<UserSettings, String> {
    let scope = cache_scope_for_owner(user_id);
    delete_settings_in_scope(token, &scope).await
}

#[derive(Default)]
struct SyncFlight {
    result: Mutex<Option<Result<UserSettings, String>>>,
    notify: Notify,
}

impl SyncFlight {
    fn result_snapshot(&self) -> Result<Option<Result<UserSettings, String>>, String> {
        let guard = match self.result.lock() {
            Ok(guard) => guard,
            Err(_) => return Err("settings sync result lock poisoned".to_string()),
        };
        let ready = guard.clone();
        drop(guard);
        Ok(ready)
    }

    async fn wait(&self) -> Result<UserSettings, String> {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            // Register before checking the result. `notify_waiters` does not
            // retain a permit for a future that has not been polled yet, so
            // checking first leaves a completion-in-the-gap hang.
            notified.as_mut().enable();
            let ready = self.result_snapshot()?;
            if let Some(result) = ready {
                return result;
            }
            notified.await;
        }
    }
}

type TokenKey = [u8; 32];
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct SyncKey {
    token: TokenKey,
    scope: CacheScope,
}

static SYNC_FLIGHTS: LazyLock<Mutex<HashMap<SyncKey, (u64, Arc<SyncFlight>)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static NEXT_SYNC_FLIGHT_ID: AtomicU64 = AtomicU64::new(1);

fn token_key(token: &str) -> TokenKey {
    Sha256::digest(token.as_bytes()).into()
}

async fn sync_settings_once(token: &str, scope: CacheScope) -> Result<UserSettings, String> {
    // Capture dirty state BEFORE fetch clears it, paired to the account that
    // owns the token rather than whichever account happens to be current when
    // the HTTP future is first polled. The write generation captured with it
    // is what lets the fetch prove no edit landed in between.
    let (write_generation, dirty_snapshot) = load_dirty_cache_for_scope(&scope)?;

    // Fetch from server (confirms we're online, gets latest state).
    let server_settings = fetch_settings_in_scope(token, &scope, write_generation).await?;

    // An edit that committed while the GET was in flight bumped the write
    // generation, so the fetch left the local cache and dirty marker alone.
    // Re-read after the fetch: the freshest dirty state supersedes the
    // pre-fetch snapshot (any raced edit merged over it), so this same sync
    // pushes it instead of leaving it stranded.
    let (_, post_fetch_dirty) = load_dirty_cache_for_scope(&scope)?;

    // Flush offline changes now that we know we're online.
    if let Some(local) = post_fetch_dirty.or(dirty_snapshot) {
        match push_settings_in_scope(token, &local, &scope, false).await {
            Ok(pushed) => return Ok(pushed),
            Err(error) => {
                // The GET may have installed the server snapshot. Restore the
                // unflushed local edit and its dirty marker so a transient
                // server rejection cannot silently discard offline work.
                if !save_dirty_cache_for_scope(&local, &scope)? {
                    return Err("Settings account changed while sync was in flight".into());
                }
                return Err(format!("Flush dirty settings failed: {error}"));
            }
        }
    }

    Ok(server_settings)
}

/// Fetch settings from server, then flush any offline changes.
/// Safe ordering: fetch first (confirms connectivity + gets latest), then push dirty cache.
async fn sync_settings_in_scope(token: &str, scope: CacheScope) -> Result<UserSettings, String> {
    ensure_scope_current(&scope)?;
    let key = SyncKey {
        token: token_key(token),
        scope,
    };
    let (flight_id, flight, created) = {
        let mut flights = SYNC_FLIGHTS
            .lock()
            .map_err(|_| "settings sync lock poisoned".to_string())?;
        if let Some((id, flight)) = flights.get(&key) {
            (*id, Arc::clone(flight), false)
        } else {
            let id = NEXT_SYNC_FLIGHT_ID.fetch_add(1, Ordering::Relaxed);
            let flight = Arc::new(SyncFlight::default());
            flights.insert(key.clone(), (id, Arc::clone(&flight)));
            (id, flight, true)
        }
    };

    if created {
        let owned_token = token.to_owned();
        let owned_flight = Arc::clone(&flight);
        let owned_scope = key.scope.clone();
        tauri::async_runtime::spawn(async move {
            let result = sync_settings_once(&owned_token, owned_scope).await;
            if let Ok(mut slot) = owned_flight.result.lock() {
                *slot = Some(result);
            }
            owned_flight.notify.notify_waiters();
            if let Ok(mut flights) = SYNC_FLIGHTS.lock() {
                if flights.get(&key).is_some_and(|(id, _)| *id == flight_id) {
                    flights.remove(&key);
                }
            }
        });
    }

    flight.wait().await
}

pub async fn sync_settings(token: &str) -> Result<UserSettings, String> {
    sync_settings_in_scope(token, cache_scope()).await
}

pub async fn sync_settings_for_owner(token: &str, user_id: &str) -> Result<UserSettings, String> {
    sync_settings_in_scope(token, cache_scope_for_owner(user_id)).await
}

// ── Tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn default_settings_have_expected_values() {
        let s = UserSettings::default();
        assert_eq!(s.appearance.theme, "default");
        assert_eq!(s.appearance.typography_mode, "simple");
        assert_eq!(s.appearance.interface_font_size, 16.0);
        assert_eq!(s.appearance.conversation_font_size, 14.0);
        assert_eq!(s.appearance.code_font_size, Some(13.0));
        assert_eq!(s.appearance.terminal_font_size, 13.0);
        assert!(s.appearance.shell_font.is_none());
        assert!(s.appearance.interface_font_family.is_none());
        assert!(s.appearance.conversation_font_family.is_none());
        assert!(s.appearance.code_font_family.is_none());
        assert!(s.appearance.terminal_font_family.is_none());
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
        assert_eq!(s.appearance.typography_mode, "simple");
        assert_eq!(s.appearance.interface_font_size, 16.0);
        assert_eq!(s.appearance.conversation_font_size, 14.0);
        assert!(s.appearance.code_font_size.is_none());
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
    fn checkpoint_flag_cache_tracks_settings_writes_and_clear() {
        clear_cache();
        assert!(!agent_chat_checkpoints_enabled());

        let mut settings = UserSettings::default();
        settings.agent_chat.checkpoints_enabled = true;
        save_cache(&settings).unwrap();
        assert!(agent_chat_checkpoints_enabled());

        settings.agent_chat.checkpoints_enabled = false;
        save_cache(&settings).unwrap();
        assert!(!agent_chat_checkpoints_enabled());

        settings.agent_chat.checkpoints_enabled = true;
        save_cache(&settings).unwrap();
        clear_cache();
        assert!(!agent_chat_checkpoints_enabled());
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
                custom_themes: Vec::new(),
                shell_font: Some("Fira Code".into()),
                typography_mode: "advanced".into(),
                interface_font_family: Some("Atkinson Hyperlegible".into()),
                interface_font_size: 17.0,
                conversation_font_family: Some("Source Sans 3".into()),
                conversation_font_size: 15.0,
                code_font_family: Some("Iosevka".into()),
                code_font_size: Some(14.0),
                terminal_font_family: Some("Berkeley Mono".into()),
                terminal_font_size: 18.5,
                show_resource_monitor: false,
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
            source_control: SourceControlSettings {
                custom_hosts: HashMap::from([("git.acme.internal".into(), "gitlab".into())]),
                open_pr_links_in_browser: false,
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
            session_restore: SessionRestoreSettings {
                enabled: false,
                scrollback_lines: 5000,
                max_total_mb: 50,
            },
            agent_chat: AgentChatSettings {
                checkpoints_enabled: true,
                background_browser_desktop_viewport: false,
            },
            browser: BrowserSettings {
                default_viewport: Some("2560x1440".into()),
            },
        };

        let json = serde_json::to_string(&s).unwrap();
        let back: UserSettings = serde_json::from_str(&json).unwrap();

        assert_eq!(back.appearance.theme, "dark");
        assert_eq!(back.appearance.shell_font.as_deref(), Some("Fira Code"));
        assert_eq!(back.appearance.typography_mode, "advanced");
        assert_eq!(
            back.appearance.interface_font_family.as_deref(),
            Some("Atkinson Hyperlegible")
        );
        assert_eq!(back.appearance.interface_font_size, 17.0);
        assert_eq!(
            back.appearance.conversation_font_family.as_deref(),
            Some("Source Sans 3")
        );
        assert_eq!(back.appearance.conversation_font_size, 15.0);
        assert_eq!(back.appearance.code_font_family.as_deref(), Some("Iosevka"));
        assert_eq!(back.appearance.code_font_size, Some(14.0));
        assert_eq!(
            back.appearance.terminal_font_family.as_deref(),
            Some("Berkeley Mono")
        );
        assert_eq!(back.appearance.terminal_font_size, 18.5);
        assert_eq!(back.editor.default_ide.as_deref(), Some("cursor"));
        assert_eq!(back.terminal.scrollback_limit, 2000);
        assert!(back.file_tree.show_hidden_files);
        assert_eq!(back.terminal.cursor_style, "underline");
        assert_eq!(back.git.default_base_branch, "develop");
        assert_eq!(
            back.source_control.custom_hosts.get("git.acme.internal"),
            Some(&"gitlab".to_string())
        );
        assert_eq!(back.keyboard.shortcuts.len(), 2);
        assert_eq!(back.keyboard.shortcuts.get("ctrl+s").unwrap(), "save");
        assert!(!back.notifications.sound_enabled);
        assert!(!back.notifications.desktop_enabled);
        assert!(!back.session_restore.enabled);
        assert_eq!(back.session_restore.scrollback_lines, 5000);
        assert_eq!(back.session_restore.max_total_mb, 50);
        assert!(back.agent_chat.checkpoints_enabled);
        assert!(!back.agent_chat.background_browser_desktop_viewport);
        assert_eq!(back.browser.default_viewport.as_deref(), Some("2560x1440"));
    }

    /// A settings blob saved before the `browser` section existed still
    /// deserializes — `default_viewport` stays unset (built-in baseline).
    #[test]
    #[serial]
    fn missing_browser_section_defaults_to_no_viewport() {
        let legacy = r#"{"appearance":{"theme":"dark"}}"#;
        let parsed: UserSettings = serde_json::from_str(legacy).unwrap();
        assert_eq!(parsed.browser.default_viewport, None);

        let explicit = r#"{"browser":{"default_viewport":"1920x1080"}}"#;
        let parsed: UserSettings = serde_json::from_str(explicit).unwrap();
        assert_eq!(
            parsed.browser.default_viewport.as_deref(),
            Some("1920x1080")
        );
    }

    /// A blob saved before source control had a section still
    /// deserializes — no custom host mappings, so provider detection
    /// runs on its built-in heuristics alone.
    #[test]
    #[serial]
    fn missing_source_control_section_defaults_to_no_custom_hosts() {
        let legacy = r#"{"appearance":{"theme":"dark"}}"#;
        let parsed: UserSettings = serde_json::from_str(legacy).unwrap();
        assert!(parsed.source_control.custom_hosts.is_empty());

        let explicit = r#"{"source_control":{"custom_hosts":{"git.acme.internal":"gitlab"}}}"#;
        let parsed: UserSettings = serde_json::from_str(explicit).unwrap();
        assert_eq!(
            parsed.source_control.custom_hosts.get("git.acme.internal"),
            Some(&"gitlab".to_string())
        );
    }

    /// A settings blob saved before the agent_chat section existed
    /// still deserializes — the checkpoint opt-in stays OFF and the
    /// desktop-viewport pin gets the ON default.
    #[test]
    #[serial]
    fn missing_agent_chat_section_defaults_to_checkpoints_off() {
        let legacy = r#"{"appearance":{"theme":"dark"}}"#;
        let parsed: UserSettings = serde_json::from_str(legacy).unwrap();
        assert!(!parsed.agent_chat.checkpoints_enabled);
        assert!(parsed.agent_chat.background_browser_desktop_viewport);
    }

    /// A settings blob saved before `background_browser_desktop_viewport`
    /// existed (but with an `agent_chat` section already present) still
    /// deserializes — the new field defaults to ON via serde default,
    /// not a deserialize error. An explicit saved `false` still wins.
    #[test]
    #[serial]
    fn missing_desktop_viewport_field_defaults_on() {
        let legacy = r#"{"agent_chat":{"checkpoints_enabled":true}}"#;
        let parsed: UserSettings = serde_json::from_str(legacy).unwrap();
        assert!(parsed.agent_chat.checkpoints_enabled);
        assert!(parsed.agent_chat.background_browser_desktop_viewport);

        let explicit_off = r#"{"agent_chat":{"checkpoints_enabled":true,"background_browser_desktop_viewport":false}}"#;
        let parsed: UserSettings = serde_json::from_str(explicit_off).unwrap();
        assert!(!parsed.agent_chat.background_browser_desktop_viewport);
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
        assert_eq!(user_b_settings.appearance.theme, "default");
        assert_eq!(user_b_settings.appearance.terminal_font_size, 13.0);
        assert!(user_b_settings.notifications.sound_enabled);
        assert!(!is_dirty());
    }

    #[test]
    #[serial]
    fn scoped_cache_rejects_another_accounts_settings() {
        clear_cache();
        set_cache_owner(Some("user-a"));
        let mut user_a = UserSettings::default();
        user_a.appearance.theme = "user-a-secret-theme".into();
        save_cache(&user_a).unwrap();
        assert_eq!(
            load_cache().unwrap().appearance.theme,
            "user-a-secret-theme"
        );

        set_cache_owner(Some("user-b"));
        assert!(load_cache().is_none());

        set_cache_owner(None);
        clear_cache();
    }

    #[test]
    #[serial]
    fn legacy_cache_is_migrated_only_with_a_concrete_identity() {
        clear_cache();
        set_cache_owner(None);
        let mut legacy = UserSettings::default();
        legacy.appearance.theme = "legacy-private-theme".into();
        std::fs::create_dir_all(cache_dir()).unwrap();
        std::fs::write(cache_file_path(), serde_json::to_string(&legacy).unwrap()).unwrap();

        // Signed-out startup cannot see an unscoped pre-upgrade account blob.
        assert!(load_cache().is_none());

        // A cached authenticated identity claims and rewrites it exactly once.
        set_cache_owner(Some("cached-user"));
        assert_eq!(
            load_cache().unwrap().appearance.theme,
            "legacy-private-theme"
        );
        let envelope: ScopedSettingsCache =
            serde_json::from_str(&std::fs::read_to_string(cache_file_path()).unwrap()).unwrap();
        assert_eq!(envelope.user_id.as_deref(), Some("cached-user"));

        set_cache_owner(None);
        assert!(load_cache().is_none());
        clear_cache();
    }

    #[test]
    #[serial]
    fn stale_network_scope_cannot_commit_into_the_next_account() {
        clear_cache();
        set_cache_owner(Some("user-a"));
        let user_a_request = cache_scope();

        set_cache_owner(Some("user-b"));
        let mut stale = UserSettings::default();
        stale.appearance.theme = "user-a-private-theme".into();
        assert!(!save_cache_for_scope(&stale, &user_a_request).unwrap());
        assert!(load_cache().is_none());

        set_cache_owner(None);
        clear_cache();
    }

    #[tokio::test]
    #[serial]
    async fn token_owner_mismatch_is_rejected_before_the_settings_get() {
        let mut server = mockito::Server::new_async().await;
        std::env::set_var("CODEMUX_API_URL", server.url());
        clear_cache();
        set_cache_owner(Some("user-b"));

        let fetch_mock = server
            .mock("GET", "/api/settings")
            .expect(0)
            .create_async()
            .await;

        let result = sync_settings_for_owner("user-a-token", "user-a").await;
        assert!(result.is_err());
        fetch_mock.assert_async().await;
        assert!(load_cache().is_none());

        set_cache_owner(None);
        clear_cache();
        std::env::remove_var("CODEMUX_API_URL");
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

        // The settings service rejects the offline edit after GET succeeds.
        let _push_mock = server
            .mock("PUT", "/api/settings")
            .with_status(500)
            .with_body("Internal Server Error")
            .create_async()
            .await;

        let result = sync_settings("test-token").await;
        assert!(result.is_err());
        assert_eq!(load_cache().unwrap().appearance.theme, "dark");
        assert!(is_dirty());

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

        // Every waiter receives the same shared result: exactly one GET.
        let fetch_mock = server
            .mock("GET", "/api/settings")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_api_response(&server_defaults))
            .expect(1)
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
        let (r1, r2) = tokio::join!(sync_settings("test-token"), sync_settings("test-token"),);
        assert!(r1.is_ok());
        assert!(r2.is_ok());

        fetch_mock.assert_async().await;
        // Only one PUT (flush) should have occurred.
        push_mock.assert_async().await;

        clear_cache();
        std::env::remove_var("CODEMUX_API_URL");
    }

    // ── Signed-out dirty edits across first sign-in ─────────────

    /// Settings edited while signed out are cached under owner `None` with
    /// the dirty flag set. The first sign-in must adopt those edits into the
    /// new owner's scope and push them — not let the initial fetch clobber
    /// them.
    #[tokio::test]
    #[serial]
    async fn sync_adopts_and_pushes_signed_out_dirty_edits_on_first_sign_in() {
        let mut server = mockito::Server::new_async().await;
        std::env::set_var("CODEMUX_API_URL", server.url());
        clear_cache();

        // Signed out: user customizes settings locally.
        set_cache_owner(None);
        let mut local = UserSettings::default();
        local.appearance.theme = "pre-account-theme".into();
        local
            .keyboard
            .shortcuts
            .insert("ctrl+k".into(), "palette".into());
        save_cache(&local).unwrap();
        set_dirty(true);

        // Sign-in happens; the account has never stored settings.
        set_cache_owner(Some("new-user"));

        let fetch_mock = server
            .mock("GET", "/api/settings")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_api_response(&UserSettings::default()))
            .expect(1)
            .create_async()
            .await;

        // The PUT must carry the signed-out edits.
        let push_mock = server
            .mock("PUT", "/api/settings")
            .match_body(mockito::Matcher::PartialJsonString(
                r#"{"settings":{"appearance":{"theme":"pre-account-theme"},"keyboard":{"shortcuts":{"ctrl+k":"palette"}}}}"#
                    .into(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_api_response(&local))
            .expect(1)
            .create_async()
            .await;

        let result = sync_settings_for_owner("test-token", "new-user")
            .await
            .unwrap();

        fetch_mock.assert_async().await;
        push_mock.assert_async().await;
        assert_eq!(result.appearance.theme, "pre-account-theme");
        assert_eq!(
            result.keyboard.shortcuts.get("ctrl+k").map(String::as_str),
            Some("palette")
        );
        // Edits are retained locally under the new owner and no longer dirty.
        assert_eq!(load_cache().unwrap().appearance.theme, "pre-account-theme");
        assert!(!is_dirty());

        set_cache_owner(None);
        clear_cache();
        std::env::remove_var("CODEMUX_API_URL");
    }

    /// A clean (non-dirty) unscoped cache has nothing unpushed, so sign-in
    /// keeps today's behavior: the server fetch wins and no PUT happens.
    #[tokio::test]
    #[serial]
    async fn sync_server_wins_over_clean_unscoped_cache_on_sign_in() {
        let mut server = mockito::Server::new_async().await;
        std::env::set_var("CODEMUX_API_URL", server.url());
        clear_cache();

        set_cache_owner(None);
        let mut local = UserSettings::default();
        local.appearance.theme = "stale-local-theme".into();
        save_cache(&local).unwrap();
        // No set_dirty — the cache is clean.

        set_cache_owner(Some("new-user"));

        let mut server_settings = UserSettings::default();
        server_settings.appearance.theme = "server-theme".into();
        let fetch_mock = server
            .mock("GET", "/api/settings")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_api_response(&server_settings))
            .expect(1)
            .create_async()
            .await;
        let push_mock = server
            .mock("PUT", "/api/settings")
            .expect(0)
            .create_async()
            .await;

        let result = sync_settings_for_owner("test-token", "new-user")
            .await
            .unwrap();

        fetch_mock.assert_async().await;
        push_mock.assert_async().await; // asserts 0 calls
        assert_eq!(result.appearance.theme, "server-theme");
        assert_eq!(load_cache().unwrap().appearance.theme, "server-theme");
        assert!(!is_dirty());

        set_cache_owner(None);
        clear_cache();
        std::env::remove_var("CODEMUX_API_URL");
    }

    /// If the flush of adopted signed-out edits fails, the edits and the
    /// dirty marker survive for retry — the flag clears only after a
    /// successful push.
    #[tokio::test]
    #[serial]
    async fn sign_in_flush_failure_keeps_signed_out_edits_dirty() {
        let mut server = mockito::Server::new_async().await;
        std::env::set_var("CODEMUX_API_URL", server.url());
        clear_cache();

        set_cache_owner(None);
        let mut local = UserSettings::default();
        local.appearance.theme = "pre-account-theme".into();
        save_cache(&local).unwrap();
        set_dirty(true);

        set_cache_owner(Some("new-user"));

        let _fetch_mock = server
            .mock("GET", "/api/settings")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_api_response(&UserSettings::default()))
            .create_async()
            .await;
        let _push_mock = server
            .mock("PUT", "/api/settings")
            .with_status(500)
            .with_body("Internal Server Error")
            .create_async()
            .await;

        let result = sync_settings_for_owner("test-token", "new-user").await;
        assert!(result.is_err());
        // The edits are preserved (now under the signing-in owner) and still
        // marked dirty so the next sync retries the flush.
        assert_eq!(load_cache().unwrap().appearance.theme, "pre-account-theme");
        assert!(is_dirty());

        set_cache_owner(None);
        clear_cache();
        std::env::remove_var("CODEMUX_API_URL");
    }

    // ── Edits racing an in-flight fetch ─────────────────────────

    /// A dirty edit committed after the fetch captured its snapshot must not
    /// be overwritten by the server response, and its marker must not be
    /// cleared — the fetch stands down.
    #[test]
    #[serial]
    fn fetch_install_stands_down_for_an_edit_committed_after_capture() {
        clear_cache();
        set_cache_owner(Some("race-user"));
        clear_cache();
        let scope = cache_scope();
        let (observed, dirty) = load_dirty_cache_for_scope(&scope).unwrap();
        assert!(dirty.is_none());

        // The edit commits while the GET is in flight (e.g. a
        // pending-identity command whose session snapshot predates
        // verification).
        let mut edit = UserSettings::default();
        edit.appearance.theme = "raced-edit-theme".into();
        save_cache_dirty(&edit).unwrap();

        let mut server_settings = UserSettings::default();
        server_settings.appearance.theme = "server-theme".into();
        assert!(!install_fetched_settings(&server_settings, &scope, observed).unwrap());
        assert_eq!(load_cache().unwrap().appearance.theme, "raced-edit-theme");
        assert!(is_dirty(), "the raced edit must stay queued for a push");

        // A capture with no interleaved write installs normally and clears
        // the marker.
        let (observed, dirty) = load_dirty_cache_for_scope(&scope).unwrap();
        assert_eq!(dirty.unwrap().appearance.theme, "raced-edit-theme");
        assert!(install_fetched_settings(&server_settings, &scope, observed).unwrap());
        assert_eq!(load_cache().unwrap().appearance.theme, "server-theme");
        assert!(!is_dirty());

        set_cache_owner(None);
        clear_cache();
    }

    /// End to end: an edit that commits while the sync's GET is in flight
    /// survives and is pushed by that same sync — not clobbered by the
    /// fetched snapshot and not stranded until some later sync.
    #[tokio::test]
    #[serial]
    async fn sync_pushes_an_edit_committed_while_the_fetch_was_in_flight() {
        let mut server = mockito::Server::new_async().await;
        std::env::set_var("CODEMUX_API_URL", server.url());
        clear_cache();
        set_cache_owner(Some("race-user"));
        clear_cache();

        let mut edit = UserSettings::default();
        edit.appearance.theme = "raced-edit-theme".into();

        // The GET body callback runs while the fetch is being served —
        // commit the dirty edit right there, after the sync captured its
        // (clean) dirty snapshot but before the response is installed.
        let edit_during_get = edit.clone();
        let fetch_mock = server
            .mock("GET", "/api/settings")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body_from_request(move |_| {
                save_cache_dirty(&edit_during_get).unwrap();
                mock_api_response(&UserSettings::default()).into_bytes()
            })
            .expect(1)
            .create_async()
            .await;

        // The raced edit must be flushed by this sync.
        let push_mock = server
            .mock("PUT", "/api/settings")
            .match_body(mockito::Matcher::PartialJsonString(
                r#"{"settings":{"appearance":{"theme":"raced-edit-theme"}}}"#.into(),
            ))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(mock_api_response(&edit))
            .expect(1)
            .create_async()
            .await;

        let result = sync_settings_for_owner("test-token", "race-user")
            .await
            .unwrap();

        fetch_mock.assert_async().await;
        push_mock.assert_async().await;
        assert_eq!(result.appearance.theme, "raced-edit-theme");
        assert_eq!(load_cache().unwrap().appearance.theme, "raced-edit-theme");
        assert!(!is_dirty(), "the push flushed the edit, nothing is pending");

        set_cache_owner(None);
        clear_cache();
        std::env::remove_var("CODEMUX_API_URL");
    }
}
