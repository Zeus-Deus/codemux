use crate::git::ensure_git_exclude;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
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
    /// `update_feature_flags` or by editing
    /// `.codemux/observability.json` to dogfood the scaffolding.
    #[serde(default)]
    pub enable_agent_chat: bool,
    /// Gates the lazy-workspace-creation path: sidebar-plus and
    /// boot-into-Home open a client-side chat draft instead of
    /// eagerly materialising a workspace. The draft is promoted to a
    /// real workspace on first message send.
    ///
    /// Defaults to `false`: today's eager flow is preserved. Flip to
    /// `true` via `update_feature_flags` or by editing
    /// `.codemux/observability.json`.
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
    let path = snapshot_path();
    match fs::read_to_string(&path) {
        Ok(contents) => match serde_json::from_str::<ObservabilitySnapshot>(&contents) {
            Ok(snapshot) => {
                return ObservabilityStore {
                    inner: Arc::new(Mutex::new(snapshot)),
                };
            }
            Err(error) => {
                // Don't swallow: partial/corrupt JSON silently defaulting to
                // `enable_agent_chat: false` cost us hours. Log and continue.
                eprintln!(
                    "[codemux::observability] Failed to parse {}: {error}. Falling back to defaults.",
                    path.display()
                );
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // First run on this machine — nothing to log.
        }
        Err(error) => {
            eprintln!(
                "[codemux::observability] Failed to read {}: {error}. Falling back to defaults.",
                path.display()
            );
        }
    }

    ObservabilityStore::default()
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
            enable_agent_chat: true,
            enable_lazy_workspace_creation: true,
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
    let path = snapshot_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create observability dir: {error}"))?;
    }
    if let Some(workspace_dir) = path.parent().and_then(|p| p.parent()) {
        ensure_git_exclude(workspace_dir, ".codemux");
    }

    let json = serde_json::to_string_pretty(snapshot)
        .map_err(|error| format!("Failed to serialize observability snapshot: {error}"))?;
    fs::write(&path, json).map_err(|error| {
        format!(
            "Failed to write observability snapshot {}: {error}",
            path.display()
        )
    })
}

fn snapshot_path() -> PathBuf {
    // Anchor to $HOME, not CWD. CWD drifts between build modes (`cargo
    // tauri dev` launches from `src-tauri/`, the installed binary from
    // wherever the user invoked it), so a CWD-relative path produces a
    // different file per launch — feature flags and safety config
    // appear to "not stick" across restarts. See feature/agent-chat
    // debugging for the incident.
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

    // Regresses the dev-vs-release drift bug: the old CWD-relative
    // `snapshot_path` resolved to `src-tauri/.codemux/observability.json`
    // under `cargo tauri dev` and `$HOME/.codemux/observability.json`
    // under the installed binary — so edits in one file never affected
    // the other launch mode. Pinning to `$HOME` keeps both in sync.
    #[test]
    #[serial_test::serial]
    fn snapshot_path_is_home_anchored_regardless_of_cwd() {
        let home = dirs::home_dir().expect("HOME must be set for this test");
        let tmp = tempfile::tempdir().expect("create tempdir");
        let prev_cwd = std::env::current_dir().expect("current_dir");
        std::env::set_current_dir(tmp.path()).expect("set_current_dir to tempdir");

        let path = snapshot_path();

        std::env::set_current_dir(&prev_cwd).expect("restore cwd");

        assert!(
            path.starts_with(&home),
            "snapshot_path() = {} should live under HOME = {}",
            path.display(),
            home.display()
        );
    }
}
