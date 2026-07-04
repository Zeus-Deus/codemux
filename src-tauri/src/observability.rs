use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredLogEntry {
    pub entry_id: String,
    pub source: String,
    pub level: LogLevel,
    pub message: String,
    pub metadata: Vec<(String, String)>,
    pub created_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsSnapshot {
    pub startup_count: u64,
    pub pane_count: u64,
    pub browser_operation_count: u64,
    pub openflow_run_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureFlags {
    pub unstable_openflow: bool,
    pub unstable_browser_automation: bool,
    pub unstable_indexing: bool,
    /// Gates the agent-chat pane kind, its Tauri command surface, and
    /// the runtime provider registry.
    ///
    /// Defaults to `false`: the chat pane is not selectable in the UI,
    /// lifecycle commands return `FeatureDisabled`, and the provider
    /// registry is not initialised at startup. Flip to `true` via
    /// `update_feature_flags` or by editing the persisted
    /// `observability.json` (see [`snapshot_path`]) to dogfood the
    /// scaffolding.
    #[serde(default)]
    pub enable_agent_chat: bool,
    /// Gates the lazy-workspace-creation path: sidebar-plus and
    /// boot-into-Home open a client-side chat draft instead of
    /// eagerly materialising a workspace. The draft is promoted to a
    /// real workspace on first message send.
    ///
    /// Defaults to `false`: today's eager flow is preserved. Flip to
    /// `true` via `update_feature_flags` or by editing the persisted
    /// `observability.json` (see [`snapshot_path`]).
    #[serde(default)]
    pub enable_lazy_workspace_creation: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionPolicy {
    pub require_risky_action_approval: bool,
    pub allow_destructive_actions: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayRecord {
    pub replay_id: String,
    pub title: String,
    pub summary: String,
    pub created_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SafetyConfig {
    pub model_budget_usd: f32,
    pub max_concurrency: u32,
    pub auto_apply: bool,
    pub approval_required_for_completion: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservabilitySnapshot {
    #[serde(default)]
    pub logs: Vec<StructuredLogEntry>,
    #[serde(default = "default_metrics_snapshot")]
    pub metrics: MetricsSnapshot,
    #[serde(default = "default_feature_flags")]
    pub feature_flags: FeatureFlags,
    #[serde(default = "default_permission_policy")]
    pub permission_policy: PermissionPolicy,
    #[serde(default)]
    pub replay_records: Vec<ReplayRecord>,
    #[serde(default = "default_safety_config")]
    pub safety_config: SafetyConfig,
}

fn default_metrics_snapshot() -> MetricsSnapshot {
    default_snapshot().metrics
}

fn default_feature_flags() -> FeatureFlags {
    default_snapshot().feature_flags
}

fn default_permission_policy() -> PermissionPolicy {
    default_snapshot().permission_policy
}

fn default_safety_config() -> SafetyConfig {
    default_snapshot().safety_config
}

pub struct ObservabilityStore {
    inner: Arc<Mutex<ObservabilitySnapshot>>,
}

impl Default for ObservabilityStore {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(default_snapshot())),
        }
    }
}

impl ObservabilityStore {
    pub fn snapshot(&self) -> ObservabilitySnapshot {
        self.inner.lock().unwrap().clone()
    }

    pub fn log(
        &self,
        source: &str,
        level: LogLevel,
        message: String,
        metadata: Vec<(String, String)>,
    ) {
        let mut snapshot = self.inner.lock().unwrap();
        let entry_id = format!("log-{}", snapshot.logs.len() + 1);
        snapshot.logs.push(StructuredLogEntry {
            entry_id,
            source: source.into(),
            level,
            message,
            metadata,
            created_at_ms: current_time_ms(),
        });
        trim_logs(&mut snapshot.logs, 300);
        let _ = save_snapshot(&snapshot);
    }

    pub fn increment_metric(&self, key: &str) {
        let mut snapshot = self.inner.lock().unwrap();
        match key {
            "startup_count" => snapshot.metrics.startup_count += 1,
            "pane_count" => snapshot.metrics.pane_count += 1,
            "browser_operation_count" => snapshot.metrics.browser_operation_count += 1,
            "openflow_run_count" => snapshot.metrics.openflow_run_count += 1,
            _ => {}
        }
        let _ = save_snapshot(&snapshot);
    }

    /// Read the current feature-flag snapshot.
    ///
    /// Callers that only need one flag should use the explicit
    /// accessor (e.g. [`agent_chat_enabled`](Self::agent_chat_enabled))
    /// so the call site documents which flag it depends on.
    pub fn feature_flags(&self) -> FeatureFlags {
        self.inner.lock().unwrap().feature_flags.clone()
    }

    /// Whether the agent-chat pane and provider registry are enabled.
    ///
    /// This is a plain boolean read, cheap to call from command
    /// entry-points. The gate defaults to `false`; see
    /// [`FeatureFlags::enable_agent_chat`] for how to flip it.
    pub fn agent_chat_enabled(&self) -> bool {
        self.inner.lock().unwrap().feature_flags.enable_agent_chat
    }

    pub fn set_feature_flags(&self, flags: FeatureFlags) {
        let mut snapshot = self.inner.lock().unwrap();
        snapshot.feature_flags = flags;
        let _ = save_snapshot(&snapshot);
    }

    pub fn set_permission_policy(&self, policy: PermissionPolicy) {
        let mut snapshot = self.inner.lock().unwrap();
        snapshot.permission_policy = policy;
        let _ = save_snapshot(&snapshot);
    }

    pub fn set_safety_config(&self, config: SafetyConfig) {
        let mut snapshot = self.inner.lock().unwrap();
        snapshot.safety_config = config;
        let _ = save_snapshot(&snapshot);
    }

    pub fn add_replay_record(&self, title: String, summary: String) {
        let mut snapshot = self.inner.lock().unwrap();
        let replay_id = format!("replay-{}", snapshot.replay_records.len() + 1);
        snapshot.replay_records.push(ReplayRecord {
            replay_id,
            title,
            summary,
            created_at_ms: current_time_ms(),
        });
        trim_replays(&mut snapshot.replay_records, 50);
        let _ = save_snapshot(&snapshot);
    }
}

pub fn load_observability_store() -> ObservabilityStore {
    let snapshot = load_or_migrate_snapshot(&snapshot_path(), &legacy_snapshot_path());
    ObservabilityStore {
        inner: Arc::new(Mutex::new(snapshot)),
    }
}

/// Outcome of trying to read a snapshot file, distinguishing "file is
/// absent" (safe to look elsewhere) from "file exists but is broken"
/// (fall back to defaults loudly — do NOT resurrect state from another
/// location, which could silently flip feature flags).
enum ReadOutcome {
    Loaded(Box<ObservabilitySnapshot>),
    Missing,
    Unreadable,
}

fn read_snapshot(path: &Path) -> ReadOutcome {
    match fs::read_to_string(path) {
        Ok(contents) => match serde_json::from_str::<ObservabilitySnapshot>(&contents) {
            Ok(snapshot) => ReadOutcome::Loaded(Box::new(snapshot)),
            Err(error) => {
                // Don't swallow: partial/corrupt JSON silently defaulting to
                // `enable_agent_chat: false` cost us hours. Log and continue.
                eprintln!(
                    "[codemux::observability] Failed to parse {}: {error}. Falling back to defaults.",
                    path.display()
                );
                ReadOutcome::Unreadable
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => ReadOutcome::Missing,
        Err(error) => {
            eprintln!(
                "[codemux::observability] Failed to read {}: {error}. Falling back to defaults.",
                path.display()
            );
            ReadOutcome::Unreadable
        }
    }
}

/// Load the snapshot from the per-build `path`; when it doesn't exist
/// yet, fall back to the legacy machine-shared location and copy it
/// forward once.
///
/// The copy (rather than a move) keeps the legacy file intact so an
/// older installed release that still reads `~/.codemux/` keeps
/// working after a downgrade. After migration each build owns its own
/// file, so they diverge from that point on — which is the fix for
/// the shared-flags bug (see [`snapshot_path`]).
fn load_or_migrate_snapshot(path: &Path, legacy_path: &Path) -> ObservabilitySnapshot {
    match read_snapshot(path) {
        ReadOutcome::Loaded(snapshot) => *snapshot,
        ReadOutcome::Unreadable => default_snapshot(),
        ReadOutcome::Missing => match read_snapshot(legacy_path) {
            ReadOutcome::Loaded(snapshot) => {
                if let Err(error) = save_snapshot_to(path, &snapshot) {
                    // Non-fatal: the next mutation re-attempts the write via
                    // `save_snapshot`. Until then we serve the legacy state.
                    eprintln!(
                        "[codemux::observability] Failed to migrate legacy snapshot {} -> {}: {error}",
                        legacy_path.display(),
                        path.display()
                    );
                }
                *snapshot
            }
            _ => default_snapshot(),
        },
    }
}

fn default_snapshot() -> ObservabilitySnapshot {
    ObservabilitySnapshot {
        logs: vec![],
        metrics: MetricsSnapshot {
            startup_count: 0,
            pane_count: 0,
            browser_operation_count: 0,
            openflow_run_count: 0,
        },
        feature_flags: FeatureFlags {
            unstable_openflow: true,
            unstable_browser_automation: true,
            unstable_indexing: true,
            // Step 13 — Agent Chat Beta is OFF by default. Existing users
            // with a persisted observability.json that has these keys set
            // to `true` keep their state (the persisted file wins over
            // these literals). The `Settings → Beta Features → Agent Chat`
            // toggle flips both flags atomically — they're paired in every
            // production read site, so the user-facing decision is one
            // switch. See docs/plans/step-13-beta-toggle-research.md §1.
            enable_agent_chat: false,
            enable_lazy_workspace_creation: false,
        },
        permission_policy: PermissionPolicy {
            require_risky_action_approval: true,
            allow_destructive_actions: false,
        },
        replay_records: vec![],
        safety_config: SafetyConfig {
            model_budget_usd: 25.0,
            max_concurrency: 4,
            auto_apply: false,
            approval_required_for_completion: true,
        },
    }
}

fn save_snapshot(snapshot: &ObservabilitySnapshot) -> Result<(), String> {
    save_snapshot_to(&snapshot_path(), snapshot)
}

fn save_snapshot_to(path: &Path, snapshot: &ObservabilitySnapshot) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create observability dir: {error}"))?;
    }

    let json = serde_json::to_string_pretty(snapshot)
        .map_err(|error| format!("Failed to serialize observability snapshot: {error}"))?;
    fs::write(path, json).map_err(|error| {
        format!(
            "Failed to write observability snapshot {}: {error}",
            path.display()
        )
    })
}

/// Per-build snapshot location:
/// `~/.local/share/codemux/observability.json` for release builds,
/// `~/.local/share/codemux-dev/observability.json` for debug builds
/// (platform-specific `dirs::data_dir()` on Windows/macOS).
///
/// Two hard-won constraints:
///
/// 1. Anchor to the XDG data dir, not CWD. CWD drifts between build
///    modes (`cargo tauri dev` launches from `src-tauri/`, the
///    installed binary from wherever the user invoked it), so a
///    CWD-relative path produces a different file per launch —
///    feature flags and safety config appear to "not stick" across
///    restarts. See feature/agent-chat debugging for the incident.
/// 2. Scope by `APP_DIR_NAME`, like every other piece of local state
///    (sqlite db, auth tokens, settings cache). The previous location,
///    `~/.codemux/observability.json`, was shared by every Codemux
///    build on the machine, so flipping Settings → Beta Features →
///    Agent Chat in a dev build also flipped it in the installed
///    release (and vice versa) — feature flags leaked across
///    otherwise-isolated instances and accounts.
fn snapshot_path() -> PathBuf {
    data_root().join("observability.json")
}

/// Mirrors `settings_sync::cache_dir()` — the canonical per-build
/// local-state root.
fn data_root() -> PathBuf {
    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local/share"));
    data_dir.join(crate::APP_DIR_NAME)
}

/// Pre-migration location, shared by every build on the machine.
/// Read-only from here on: consulted once when the per-build file
/// doesn't exist yet, never written to. Left in place so older
/// installed versions that still read it keep working.
fn legacy_snapshot_path() -> PathBuf {
    let root = dirs::home_dir()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    root.join(".codemux").join("observability.json")
}

fn trim_logs(logs: &mut Vec<StructuredLogEntry>, max: usize) {
    if logs.len() > max {
        let remove_count = logs.len() - max;
        logs.drain(0..remove_count);
    }
}

fn trim_replays(replays: &mut Vec<ReplayRecord>, max: usize) {
    if replays.len() > max {
        let remove_count = replays.len() - max;
        replays.drain(0..remove_count);
    }
}

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Regresses two path bugs:
    //
    // 1. CWD drift: an early CWD-relative `snapshot_path` resolved to
    //    `src-tauri/.codemux/observability.json` under `cargo tauri
    //    dev` and a different file under the installed binary — flags
    //    appeared to "not stick". The path must not depend on CWD.
    // 2. Cross-build leak: the follow-up `$HOME/.codemux/` anchor was
    //    shared by every build on the machine, so the Agent Chat Beta
    //    toggle flipped in BOTH the installed release and a dev build
    //    at once. The path must be scoped by `APP_DIR_NAME` like the
    //    rest of local state.
    #[test]
    #[serial_test::serial]
    fn snapshot_path_is_build_scoped_and_cwd_independent() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let prev_cwd = std::env::current_dir().expect("current_dir");

        let path_before = snapshot_path();
        std::env::set_current_dir(tmp.path()).expect("set_current_dir to tempdir");
        let path_after = snapshot_path();

        std::env::set_current_dir(&prev_cwd).expect("restore cwd");

        assert_eq!(
            path_before, path_after,
            "snapshot_path() must not depend on CWD"
        );
        assert!(
            path_after.ends_with(
                Path::new(crate::APP_DIR_NAME).join("observability.json")
            ),
            "snapshot_path() = {} must be scoped by APP_DIR_NAME = {} so dev and release builds don't share feature flags",
            path_after.display(),
            crate::APP_DIR_NAME
        );
        assert!(
            !path_after
                .components()
                .any(|c| c.as_os_str() == ".codemux"),
            "snapshot_path() = {} must not resolve to the legacy machine-shared ~/.codemux location",
            path_after.display()
        );
    }

    /// First launch after the path change: the per-build file doesn't
    /// exist, but the legacy machine-shared one does. The legacy state
    /// must be adopted (users who opted into the Beta keep it) and
    /// copied into the per-build location so subsequent flips no
    /// longer touch shared state. The legacy file itself stays intact
    /// for older installed versions.
    #[test]
    fn load_migrates_legacy_snapshot_once() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let legacy = tmp.path().join(".codemux").join("observability.json");
        let new = tmp.path().join("data").join("observability.json");

        let mut snapshot = default_snapshot();
        snapshot.feature_flags.enable_agent_chat = true;
        snapshot.feature_flags.enable_lazy_workspace_creation = true;
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        fs::write(&legacy, serde_json::to_string_pretty(&snapshot).unwrap()).unwrap();

        let loaded = load_or_migrate_snapshot(&new, &legacy);

        assert!(loaded.feature_flags.enable_agent_chat, "legacy opt-in must survive the path move");
        assert!(new.exists(), "legacy snapshot must be copied to the per-build path");
        assert!(legacy.exists(), "legacy file must be left in place for older versions");

        let copied: ObservabilitySnapshot =
            serde_json::from_str(&fs::read_to_string(&new).unwrap()).unwrap();
        assert!(copied.feature_flags.enable_agent_chat);
    }

    /// Once the per-build file exists it is authoritative — the legacy
    /// shared file must never override it again, otherwise the
    /// cross-build leak comes back through the migration path.
    #[test]
    fn per_build_snapshot_wins_over_legacy() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let legacy = tmp.path().join(".codemux").join("observability.json");
        let new = tmp.path().join("data").join("observability.json");

        let mut legacy_snapshot = default_snapshot();
        legacy_snapshot.feature_flags.enable_agent_chat = true;
        legacy_snapshot.feature_flags.enable_lazy_workspace_creation = true;
        fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        fs::write(&legacy, serde_json::to_string_pretty(&legacy_snapshot).unwrap()).unwrap();

        let new_snapshot = default_snapshot(); // Beta off
        fs::create_dir_all(new.parent().unwrap()).unwrap();
        fs::write(&new, serde_json::to_string_pretty(&new_snapshot).unwrap()).unwrap();

        let loaded = load_or_migrate_snapshot(&new, &legacy);

        assert!(
            !loaded.feature_flags.enable_agent_chat,
            "the per-build snapshot must win over the legacy shared file"
        );
    }

    /// Fresh machine: neither file exists. Defaults apply and nothing
    /// is written until the first real mutation.
    #[test]
    fn load_defaults_when_no_snapshot_exists() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let legacy = tmp.path().join(".codemux").join("observability.json");
        let new = tmp.path().join("data").join("observability.json");

        let loaded = load_or_migrate_snapshot(&new, &legacy);

        assert!(!loaded.feature_flags.enable_agent_chat);
        assert!(!new.exists(), "load must not create files when there is nothing to migrate");
    }

    /// Step 13 — Agent Chat is OFF by default for new users. The
    /// `default_snapshot()` literals and `#[serde(default)]` for the
    /// flag fields must agree, otherwise a fresh-install user sees
    /// the Beta surface without opting in.
    #[test]
    fn default_snapshot_disables_agent_chat_beta() {
        let snap = default_snapshot();
        assert!(
            !snap.feature_flags.enable_agent_chat,
            "default_snapshot must have enable_agent_chat = false; new users opt in via Settings → Beta Features"
        );
        assert!(
            !snap.feature_flags.enable_lazy_workspace_creation,
            "default_snapshot must have enable_lazy_workspace_creation = false; paired with enable_agent_chat as the unified Beta toggle"
        );
    }

    /// `#[serde(default)]` on the flag fields must produce the same
    /// off-default as `default_snapshot()`. Drift between the two
    /// produces "fresh install OFF, partial-config user ON" surprises
    /// — the bug Step 13 was scoped to fix.
    #[test]
    fn serde_default_for_agent_chat_flags_is_false() {
        // FeatureFlags requires the unstable_* fields, but the agent-
        // chat fields are `#[serde(default)]`. A partial JSON missing
        // those fields must default them to false, matching
        // `default_snapshot()`'s off-state.
        let json = r#"{
            "unstable_openflow": true,
            "unstable_browser_automation": true,
            "unstable_indexing": true
        }"#;
        let parsed: FeatureFlags = serde_json::from_str(json)
            .expect("FeatureFlags missing only the agent-chat keys parses with serde defaults");
        assert!(
            !parsed.enable_agent_chat,
            "serde(default) for enable_agent_chat must be false to match default_snapshot()"
        );
        assert!(
            !parsed.enable_lazy_workspace_creation,
            "serde(default) for enable_lazy_workspace_creation must be false to match default_snapshot()"
        );
    }

    /// An existing user with a persisted observability.json that opted
    /// into the Beta during dogfooding should keep their state — the
    /// persisted file wins over the off-default. Pins the merge
    /// promise made in docs/plans/step-13-beta-toggle-research.md §6.
    #[test]
    fn persisted_agent_chat_true_survives_default_flip() {
        let json = r#"{
            "unstable_openflow": true,
            "unstable_browser_automation": true,
            "unstable_indexing": true,
            "enable_agent_chat": true,
            "enable_lazy_workspace_creation": true
        }"#;
        let parsed: FeatureFlags = serde_json::from_str(json).expect("full flags parse");
        assert!(parsed.enable_agent_chat);
        assert!(parsed.enable_lazy_workspace_creation);
    }

    /// `set_agent_chat_beta` must flip both fields together. The Step
    /// 6–12 surface assumes both flags are true to function — leaving
    /// one of them flipped without the other lights up half the
    /// surface and breaks composer routing. Pin the contract at the
    /// store level so any future setter that bypasses the command
    /// still gets caught.
    #[test]
    fn agent_chat_flags_can_be_flipped_atomically_via_store() {
        let store = ObservabilityStore::default();
        let starting = store.feature_flags();
        assert!(!starting.enable_agent_chat, "default is off");
        assert!(!starting.enable_lazy_workspace_creation, "default is off");

        // Mirror what `commands::set_agent_chat_beta(enabled=true)` does.
        let mut next = store.feature_flags();
        next.enable_agent_chat = true;
        next.enable_lazy_workspace_creation = true;
        store.set_feature_flags(next);

        let after_on = store.feature_flags();
        assert!(after_on.enable_agent_chat);
        assert!(after_on.enable_lazy_workspace_creation);

        // And the symmetric off flip.
        let mut next = store.feature_flags();
        next.enable_agent_chat = false;
        next.enable_lazy_workspace_creation = false;
        store.set_feature_flags(next);

        let after_off = store.feature_flags();
        assert!(!after_off.enable_agent_chat);
        assert!(!after_off.enable_lazy_workspace_creation);

        // Other flags untouched by the toggle.
        assert!(after_off.unstable_openflow);
        assert!(after_off.unstable_browser_automation);
        assert!(after_off.unstable_indexing);
    }
}
