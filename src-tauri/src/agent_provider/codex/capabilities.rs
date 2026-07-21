//! Live capability harvest for the Codex provider.
//!
//! Stage 9 replaces the static fallback model list with a real
//! `model/list` JSON-RPC call against the `codex app-server` binary.
//! Mirrors the OpenCode harvest pattern (see
//! [`crate::agent_provider::opencode::capabilities`]) but over JSON-RPC
//! stdio instead of HTTP.
//!
//! # Lifecycle
//!
//! Each call spawns a short-lived `codex app-server` child, runs the
//! `initialize` → `account/read` → `model/list` handshake, and returns
//! the resulting [`ProviderChatCapabilities`]. The Tauri command layer
//! caches the result in memory so the picker does not pay this cost on
//! every render — see
//! [`commands::agent_chat::list_chat_provider_capabilities`].
//!
//! # Failure modes
//!
//! * Binary missing → [`HarvestError::NotInstalled`]; the picker shows
//!   an install hint, mirroring the OpenCode empty state.
//! * `account/read` reports no account while the active provider requires
//!   OpenAI auth → [`HarvestError::NotAuthenticated`]; picker shows a "run
//!   codex login" hint instead of an empty model list.
//! * Any other RPC failure → [`HarvestError::HarvestFailed`] with the
//!   underlying message — no static fallback, since drifting silently to
//!   a stale model list is what Stage 8 already burned us with.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::sync::Mutex;

use crate::agent_provider::{
    ChatModelInfo, EffortGranularity, PermissionModeOption, ProviderChatCapabilities,
};
use crate::json_rpc_child::{JsonRpcChild, SpawnConfig};

use super::protocol::{
    AccountReadResponse, Capabilities, ClientInfo, InitializeParams, ModelEntry,
    ModelListParams, ModelListResponse,
};

/// Hard cap on the harvest. Tuned to fit comfortably under the picker's
/// initial-render budget — the user is staring at a spinner the whole
/// time.
const HARVEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Error variants surfaced to the picker. Each one maps to a distinct
/// empty-state UX so the user knows whether to install Codex, log in, or
/// report a bug.
#[derive(Debug, Clone)]
pub enum HarvestError {
    /// `codex` binary not on PATH.
    NotInstalled { hint: String },
    /// Binary present but the user has not signed in.
    NotAuthenticated { hint: String },
    /// Spawn or RPC failure during harvest. Keep the original message
    /// so the picker error toast carries enough detail to diagnose.
    HarvestFailed { message: String },
}

impl HarvestError {
    /// One-line string the Tauri command surface can return verbatim to
    /// the frontend. The frontend pattern-matches the prefix so each
    /// failure type can render distinct UI.
    pub fn to_command_string(&self) -> String {
        match self {
            Self::NotInstalled { hint } => format!("codex_not_installed: {hint}"),
            Self::NotAuthenticated { hint } => format!("codex_not_authenticated: {hint}"),
            Self::HarvestFailed { message } => format!("codex_harvest_failed: {message}"),
        }
    }
}

/// Process-wide cache for the harvested capabilities. Populated on the
/// first call and reused for the rest of the app's lifetime; the user
/// can force a refresh via the eventual settings-panel button.
#[derive(Default)]
pub struct CodexCapabilityCache {
    inner: Mutex<Option<ProviderChatCapabilities>>,
}

impl CodexCapabilityCache {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }

    /// Return the cached value if present; otherwise run the live
    /// harvest, cache it, and return.
    pub async fn get_or_harvest(
        &self,
        binary_path: &std::path::Path,
        codex_home: Option<&std::path::Path>,
    ) -> Result<ProviderChatCapabilities, HarvestError> {
        {
            let guard = self.inner.lock().await;
            if let Some(cached) = guard.clone() {
                return Ok(cached);
            }
        }
        let fresh = harvest_codex_capabilities(binary_path, codex_home).await?;
        {
            let mut guard = self.inner.lock().await;
            *guard = Some(fresh.clone());
        }
        Ok(fresh)
    }

    /// Drop any cached value. The next call will re-harvest.
    pub async fn invalidate(&self) {
        let mut guard = self.inner.lock().await;
        *guard = None;
    }
}

/// Run the live harvest. Spawns a short-lived `codex app-server`,
/// performs `initialize` → `account/read` → `model/list`, and shapes the
/// result into the picker's [`ProviderChatCapabilities`].
///
/// `binary_path` is resolved by the caller (typically via
/// `which::which("codex")`); passing the path explicitly keeps the
/// harvest testable against fake binaries.
pub async fn harvest_codex_capabilities(
    binary_path: &std::path::Path,
    codex_home: Option<&std::path::Path>,
) -> Result<ProviderChatCapabilities, HarvestError> {
    let mut env = HashMap::new();
    if let Some(home) = codex_home {
        env.insert("CODEX_HOME".into(), home.to_string_lossy().to_string());
    }

    let child = tokio::time::timeout(
        HARVEST_TIMEOUT,
        JsonRpcChild::spawn(SpawnConfig {
            program: PathBuf::from(binary_path),
            args: vec!["app-server".into()],
            env,
            cwd: None,
            default_timeout: HARVEST_TIMEOUT,
        }),
    )
    .await
    .map_err(|_| HarvestError::HarvestFailed {
        message: "spawn timed out".into(),
    })?
    .map_err(|err| HarvestError::HarvestFailed {
        message: format!("spawn failed: {err}"),
    })?;
    let child = Arc::new(child);

    // Initialize handshake.
    let init_params = serde_json::to_value(InitializeParams {
        client_info: ClientInfo {
            name: "codemux-capability-harvest".into(),
            title: "Codemux".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        },
        capabilities: Capabilities {
            experimental_api: true,
        },
    })
    .map_err(|err| HarvestError::HarvestFailed {
        message: format!("initialize serialize failed: {err}"),
    })?;
    child
        .request("initialize", init_params)
        .await
        .map_err(|err| HarvestError::HarvestFailed {
            message: format!("initialize failed: {err}"),
        })?;
    child
        .notify("initialized", json!({}))
        .await
        .map_err(|err| HarvestError::HarvestFailed {
            message: format!("initialized notify failed: {err}"),
        })?;

    // Auth gate. `requires_openai_auth` describes the active provider, not
    // whether an account exists: the normal logged-in ChatGPT response has
    // both `account: Some(...)` and `requires_openai_auth: true`. Only the
    // combination of a missing account and a provider that requires OpenAI
    // auth is an unauthenticated state.
    let account_resp = child
        .request("account/read", json!({}))
        .await
        .map_err(|err| HarvestError::HarvestFailed {
            message: format!("account/read failed: {err}"),
        })?;
    let account: AccountReadResponse = serde_json::from_value(account_resp).map_err(|err| {
        HarvestError::HarvestFailed {
            message: format!("account/read decode failed: {err}"),
        }
    })?;
    if account.needs_login() {
        let _ = child.shutdown().await;
        return Err(HarvestError::NotAuthenticated {
            hint: "Run `codex login` and try again.".into(),
        });
    }

    // Model list. Single page is fine for the picker — Codex returns
    // ~5-10 models today and the cursor is rarely populated.
    let model_resp = child
        .request(
            "model/list",
            serde_json::to_value(ModelListParams::default()).unwrap(),
        )
        .await
        .map_err(|err| HarvestError::HarvestFailed {
            message: format!("model/list failed: {err}"),
        })?;
    let models: ModelListResponse =
        serde_json::from_value(model_resp).map_err(|err| HarvestError::HarvestFailed {
            message: format!("model/list decode failed: {err}"),
        })?;

    let _ = child.shutdown().await;

    Ok(build_capabilities(models.data))
}

/// Pure transformer — turn the SDK's `model/list` payload into the
/// picker's [`ProviderChatCapabilities`]. Pulled out so it's
/// independently testable against fixture data lifted from a real
/// harvest.
pub fn build_capabilities(entries: Vec<ModelEntry>) -> ProviderChatCapabilities {
    let models: Vec<ChatModelInfo> = entries
        .into_iter()
        .filter(|m| !m.hidden)
        .map(model_entry_to_chat_info)
        .collect();
    ProviderChatCapabilities {
        models,
        effort_granularity: EffortGranularity::PerTurn,
        effort_label_map: codex_effort_label_map(),
        permission_modes: codex_permission_modes(),
        default_permission_mode: Some("danger-full-access".into()),
        permission_granularity: EffortGranularity::PerSession,
    }
}

fn model_entry_to_chat_info(entry: ModelEntry) -> ChatModelInfo {
    let supports_images = entry
        .input_modalities
        .iter()
        .any(|m| m == "image");
    let supports_fast_mode = entry
        .additional_speed_tiers
        .iter()
        .any(|t| t == "fast");
    let effort_levels: Vec<String> = entry
        .supported_reasoning_efforts
        .iter()
        .map(|opt| opt.reasoning_effort.clone())
        .filter(|level| level != "none")
        .collect();
    let default_effort = if effort_levels.is_empty() {
        None
    } else if entry.default_reasoning_effort != "none"
        && effort_levels.contains(&entry.default_reasoning_effort)
    {
        Some(entry.default_reasoning_effort)
    } else {
        Some(effort_levels[0].clone())
    };
    let label = if entry.display_name.is_empty() {
        entry.id.clone()
    } else {
        entry.display_name
    };
    ChatModelInfo {
        id: entry.id,
        label,
        description: if entry.description.is_empty() {
            None
        } else {
            Some(entry.description)
        },
        effort_levels,
        default_effort,
        prompt_injected_effort_levels: vec![],
        context_window_options: vec![],
        supports_adaptive_thinking: false,
        supports_thinking_toggle: false,
        supports_fast_mode,
        supports_images,
        sub_provider: None,
        is_free: false,
    }
}

/// Canonical labels for Codex reasoning-effort strings. Static — the SDK
/// uses a fixed set that hasn't changed since the V2 release.
pub fn codex_effort_label_map() -> HashMap<String, String> {
    let pairs = [
        ("none", "None"),
        ("minimal", "Minimal"),
        ("low", "Low"),
        ("medium", "Medium"),
        ("high", "High"),
        ("xhigh", "Extra High"),
    ];
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Codex sandbox-policy / permission-mode list. The SDK does not
/// expose these as a queryable list (they're enforced by the codex
/// CLI's runtime config), so this stays as a static catalog. The Rust
/// adapter converts each value to the
/// `(approval_policy, sandbox)` pair the RPC expects via
/// [`super::session::codex_permission_mode_to_policy_pair`].
pub fn codex_permission_modes() -> Vec<PermissionModeOption> {
    vec![
        PermissionModeOption {
            value: "read-only".into(),
            label: "Read only".into(),
            description: "Allow reads, block writes and commands.".into(),
            is_default: false,
        },
        PermissionModeOption {
            value: "workspace-write".into(),
            label: "Workspace write".into(),
            description: "Allow edits within the workspace, block commands.".into(),
            is_default: false,
        },
        PermissionModeOption {
            value: "danger-full-access".into(),
            label: "Full access".into(),
            description: "Allow commands and edits without prompts.".into(),
            is_default: true,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_provider::types::EffortGranularity;
    use serde_json::json;

    fn entry(id: &str, hidden: bool, input_modalities: Vec<&str>) -> ModelEntry {
        serde_json::from_value(json!({
            "id": id,
            "model": id,
            "displayName": id,
            "description": "",
            "hidden": hidden,
            "isDefault": false,
            "defaultReasoningEffort": "medium",
            "supportedReasoningEfforts": [
                {"reasoningEffort": "minimal", "description": ""},
                {"reasoningEffort": "low", "description": ""},
                {"reasoningEffort": "medium", "description": ""},
                {"reasoningEffort": "high", "description": ""},
            ],
            "inputModalities": input_modalities,
            "additionalSpeedTiers": ["fast"],
        }))
        .unwrap()
    }

    #[test]
    fn build_capabilities_filters_hidden_models() {
        let caps = build_capabilities(vec![
            entry("gpt-5.4", false, vec!["text", "image"]),
            entry("gpt-secret-internal", true, vec!["text"]),
        ]);
        assert_eq!(caps.models.len(), 1);
        assert_eq!(caps.models[0].id, "gpt-5.4");
    }

    #[test]
    fn build_capabilities_extracts_image_support_from_modalities() {
        let caps = build_capabilities(vec![
            entry("multimodal", false, vec!["text", "image"]),
            entry("text-only", false, vec!["text"]),
        ]);
        let by_id: HashMap<_, _> =
            caps.models.iter().map(|m| (m.id.as_str(), m)).collect();
        assert!(by_id["multimodal"].supports_images);
        assert!(!by_id["text-only"].supports_images);
    }

    #[test]
    fn build_capabilities_skips_none_effort_in_levels() {
        let mut e = entry("m", false, vec!["text"]);
        e.supported_reasoning_efforts = vec![
            super::super::protocol::ReasoningEffortOption {
                reasoning_effort: "none".into(),
                description: "".into(),
            },
            super::super::protocol::ReasoningEffortOption {
                reasoning_effort: "high".into(),
                description: "".into(),
            },
        ];
        e.default_reasoning_effort = "high".into();
        let caps = build_capabilities(vec![e]);
        let m = &caps.models[0];
        assert!(!m.effort_levels.contains(&"none".to_string()));
        assert_eq!(m.default_effort.as_deref(), Some("high"));
    }

    #[test]
    fn build_capabilities_falls_back_to_first_effort_when_default_unsupported() {
        let mut e = entry("m", false, vec!["text"]);
        e.default_reasoning_effort = "made-up-effort".into();
        let caps = build_capabilities(vec![e]);
        let m = &caps.models[0];
        // Falls back to the first supported level.
        assert!(m.default_effort.is_some());
        assert!(m.effort_levels.contains(m.default_effort.as_ref().unwrap()));
    }

    #[test]
    fn build_capabilities_carries_fast_mode_flag() {
        let caps = build_capabilities(vec![entry("m", false, vec!["text"])]);
        assert!(caps.models[0].supports_fast_mode);
    }

    #[test]
    fn capabilities_keep_per_turn_effort_granularity() {
        let caps = build_capabilities(vec![entry("m", false, vec!["text"])]);
        assert_eq!(caps.effort_granularity, EffortGranularity::PerTurn);
    }

    #[test]
    fn capabilities_advertise_three_permission_modes_with_full_access_default() {
        let caps = build_capabilities(vec![]);
        assert_eq!(caps.permission_modes.len(), 3);
        let default = caps
            .permission_modes
            .iter()
            .find(|m| m.is_default)
            .unwrap();
        assert_eq!(default.value, "danger-full-access");
    }

    #[test]
    fn harvest_error_to_command_string_has_stable_prefix() {
        assert!(HarvestError::NotInstalled {
            hint: "x".into()
        }
        .to_command_string()
        .starts_with("codex_not_installed:"));
        assert!(HarvestError::NotAuthenticated {
            hint: "y".into()
        }
        .to_command_string()
        .starts_with("codex_not_authenticated:"));
        assert!(HarvestError::HarvestFailed {
            message: "z".into()
        }
        .to_command_string()
        .starts_with("codex_harvest_failed:"));
    }

    #[tokio::test]
    async fn capability_cache_returns_same_value_on_second_call() {
        let cache = CodexCapabilityCache::new();
        // Manually pre-populate so we don't depend on a live binary.
        {
            let mut guard = cache.inner.lock().await;
            *guard = Some(build_capabilities(vec![entry(
                "test",
                false,
                vec!["text"],
            )]));
        }
        // Pass a guaranteed-bogus path; if the cache works, it never
        // gets touched.
        let result = cache
            .get_or_harvest(std::path::Path::new("/no/such/codex"), None)
            .await
            .expect("cached value returned");
        assert_eq!(result.models.len(), 1);
    }

    #[tokio::test]
    async fn capability_cache_invalidate_clears_value() {
        let cache = CodexCapabilityCache::new();
        {
            let mut guard = cache.inner.lock().await;
            *guard = Some(build_capabilities(vec![entry(
                "test",
                false,
                vec!["text"],
            )]));
        }
        cache.invalidate().await;
        let guard = cache.inner.lock().await;
        assert!(guard.is_none());
    }
}
