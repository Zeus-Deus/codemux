//! Live capability discovery for Grok Build's official ACP server.
//!
//! Grok's initialize response is the only model catalogue used here. Model
//! ids and reasoning-effort values deliberately remain opaque so a newer CLI
//! can publish new choices without requiring a Codemux release.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::agent_provider::acp::protocol::{current_model_id, grok_auth_method, initialize_params};
use crate::agent_provider::{
    ChatModelInfo, EffortGranularity, PermissionModeOption, ProviderChatCapabilities,
};
use crate::json_rpc_child::{JsonRpcChild, RpcChildError, SpawnConfig};

const HARVEST_TIMEOUT: Duration = Duration::from_secs(15);
const CAPABILITY_CACHE_TTL: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone)]
pub enum HarvestError {
    NotInstalled { hint: String },
    NotAuthenticated { hint: String },
    HarvestFailed { message: String },
}

impl HarvestError {
    pub fn to_command_string(&self) -> String {
        match self {
            Self::NotInstalled { hint } => format!("grok_not_installed: {hint}"),
            Self::NotAuthenticated { hint } => format!("grok_not_authenticated: {hint}"),
            Self::HarvestFailed { message } => format!("grok_harvest_failed: {message}"),
        }
    }
}

struct CachedCapabilities {
    harvested_at: Instant,
    capabilities: ProviderChatCapabilities,
}

#[derive(Default)]
pub struct GrokCapabilityCache {
    inner: Mutex<Option<CachedCapabilities>>,
}

impl GrokCapabilityCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn get_or_harvest(
        &self,
        binary_path: &Path,
    ) -> Result<ProviderChatCapabilities, HarvestError> {
        // Keep the lock through refresh so simultaneous picker mounts spawn a
        // single short-lived ACP process.
        let mut cached = self.inner.lock().await;
        if let Some(entry) = cached.as_ref() {
            if entry.harvested_at.elapsed() < CAPABILITY_CACHE_TTL {
                return Ok(entry.capabilities.clone());
            }
        }
        let fresh = harvest_grok_capabilities(binary_path).await?;
        *cached = Some(CachedCapabilities {
            harvested_at: Instant::now(),
            capabilities: fresh.clone(),
        });
        Ok(fresh)
    }

    pub async fn invalidate(&self) {
        *self.inner.lock().await = None;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrokHealthProbe {
    pub authenticated: bool,
    pub version: Option<String>,
}

pub async fn harvest_grok_capabilities(
    binary_path: &Path,
) -> Result<ProviderChatCapabilities, HarvestError> {
    let initialize =
        harvest_grok_initialize(binary_path, None, "codemux-capability-harvest").await?;
    build_capabilities(&initialize)
}

/// Run the safe initialize-only portion of Grok ACP discovery.
///
/// Slash commands can be project-sensitive, so callers may anchor the probe
/// at a chat cwd. Authentication intentionally happens after initialize in
/// ACP and is not needed for either the model or command catalogue.
pub(crate) async fn harvest_grok_initialize(
    binary_path: &Path,
    cwd: Option<PathBuf>,
    client_name: &str,
) -> Result<Value, HarvestError> {
    let child = Arc::new(spawn_grok(binary_path, cwd).await?);
    let result = child
        .request("initialize", initialize_params(client_name))
        .await
        .map_err(map_rpc_error);
    let _ = child.shutdown().await;
    result
}

/// Probe authentication without selecting Grok's browser-login method.
///
/// `initialize` chooses an advertised non-browser default; older CLIs fall
/// back to `cached_token` or an inherited `XAI_API_KEY`. Browser login is
/// never selected, so this can safely run while the desktop is locked.
pub async fn probe_grok_health(binary_path: &Path) -> Result<GrokHealthProbe, HarvestError> {
    let child = Arc::new(spawn_grok(binary_path, None).await?);
    let result = async {
        let initialize = child
            .request("initialize", initialize_params("codemux-health-probe"))
            .await
            .map_err(map_rpc_error)?;
        let version = string_at_pointer(&initialize, "/_meta/agentVersion");
        // `grok_auth_method` accepts only an advertised non-browser method,
        // including future headless methods selected by the CLI itself.
        let method_id = grok_auth_method(&initialize, &HashMap::new());
        let Some(method_id) = method_id else {
            return Ok(GrokHealthProbe {
                authenticated: false,
                version,
            });
        };
        child
            .request(
                "authenticate",
                json!({ "methodId": method_id, "_meta": { "headless": true } }),
            )
            .await
            .map_err(map_rpc_error)?;
        Ok(GrokHealthProbe {
            authenticated: true,
            version,
        })
    }
    .await;
    let _ = child.shutdown().await;
    result
}

async fn spawn_grok(
    binary_path: &Path,
    cwd: Option<PathBuf>,
) -> Result<JsonRpcChild, HarvestError> {
    JsonRpcChild::spawn(SpawnConfig {
        program: PathBuf::from(binary_path),
        args: vec![
            "--no-auto-update".into(),
            "agent".into(),
            "--no-leader".into(),
            "stdio".into(),
        ],
        env: HashMap::new(),
        cwd,
        default_timeout: HARVEST_TIMEOUT,
    })
    .await
    .map_err(|error| map_spawn_error(error, binary_path))
}

fn map_spawn_error(error: RpcChildError, binary_path: &Path) -> HarvestError {
    match error {
        RpcChildError::SpawnFailed(source) if source.kind() == std::io::ErrorKind::NotFound => {
            HarvestError::NotInstalled {
                hint: format!(
                    "Install Grok CLI and ensure `{}` is on PATH.",
                    binary_path.display()
                ),
            }
        }
        other => HarvestError::HarvestFailed {
            message: format!("Could not start Grok ACP: {other}"),
        },
    }
}

fn map_rpc_error(error: RpcChildError) -> HarvestError {
    let message = error.to_string();
    if crate::agent_provider::acp::protocol::looks_unauthenticated(&message) {
        HarvestError::NotAuthenticated {
            hint: "Run `grok login --device-auth` or set `XAI_API_KEY`, then try again.".into(),
        }
    } else {
        HarvestError::HarvestFailed { message }
    }
}

pub(crate) fn build_capabilities(
    initialize: &Value,
) -> Result<ProviderChatCapabilities, HarvestError> {
    let supports_images = initialize
        .pointer("/agentCapabilities/promptCapabilities/image")
        .or_else(|| initialize.pointer("/promptCapabilities/image"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let entries = available_models(initialize);
    if entries.is_empty() {
        return Err(HarvestError::HarvestFailed {
            message: "Grok initialize response did not contain a model catalogue.".into(),
        });
    }

    let mut models = Vec::new();
    let mut effort_label_map = HashMap::new();
    let mut seen_models = HashSet::new();
    for entry in entries {
        let Some(model_id) = string_field(&entry, &["modelId", "id", "value"]) else {
            continue;
        };
        if !seen_models.insert(model_id.clone()) {
            continue;
        }
        let label = string_field(&entry, &["name", "label"]).unwrap_or_else(|| model_id.clone());
        let description = string_field(&entry, &["description"]);
        let max_context_tokens = number_field(
            &entry,
            &[
                "totalContextTokens",
                "contextTokens",
                "contextWindowTokens",
                "contextWindow",
                "maxContextTokens",
            ],
        );

        let explicit_reasoning_support = bool_field(&entry, "supportsReasoningEffort");
        let effort_entries = if explicit_reasoning_support == Some(false) {
            Vec::new()
        } else {
            reasoning_efforts(&entry)
        };
        let mut effort_levels = Vec::new();
        let mut effort_descriptions = HashMap::new();
        let mut default_from_entry = None;
        let mut seen_efforts = HashSet::new();
        for effort in effort_entries {
            let Some(value) = effort_value(&effort) else {
                continue;
            };
            if !seen_efforts.insert(value.clone()) {
                continue;
            }
            let label = effort
                .as_object()
                .and_then(|_| string_field(&effort, &["label", "name"]))
                .unwrap_or_else(|| value.clone());
            effort_label_map.entry(value.clone()).or_insert(label);
            if let Some(description) = effort
                .as_object()
                .and_then(|_| string_field(&effort, &["description"]))
            {
                effort_descriptions.insert(value.clone(), description);
            }
            if default_from_entry.is_none()
                && effort.get("default").and_then(Value::as_bool) == Some(true)
            {
                default_from_entry = Some(value.clone());
            }
            effort_levels.push(value);
        }

        let advertised_default = string_field_from_meta_or_top(&entry, "reasoningEffort")
            .filter(|value| effort_levels.contains(value));
        let default_effort = default_from_entry
            .or(advertised_default)
            .or_else(|| effort_levels.first().cloned());

        models.push(ChatModelInfo {
            id: model_id,
            label,
            description,
            effort_levels,
            default_effort,
            effort_descriptions,
            prompt_injected_effort_levels: Vec::new(),
            context_window_options: Vec::new(),
            supports_adaptive_thinking: false,
            supports_thinking_toggle: false,
            supports_fast_mode: false,
            supports_images,
            sub_provider: None,
            is_free: false,
            max_context_tokens,
        });
    }

    if models.is_empty() {
        return Err(HarvestError::HarvestFailed {
            message: "Grok model catalogue contained no usable model ids.".into(),
        });
    }

    // The composer treats the first row as the provider default. Grok reports
    // that explicitly rather than promising catalogue order, so promote the
    // live current model while preserving the order of every other entry.
    if let Some(current) = current_model_id(initialize) {
        if let Some(index) = models.iter().position(|model| model.id == current) {
            if index != 0 {
                let current = models.remove(index);
                models.insert(0, current);
            }
        }
    }

    Ok(ProviderChatCapabilities {
        models,
        effort_granularity: EffortGranularity::PerTurn,
        effort_label_map,
        permission_modes: grok_permission_modes(),
        default_permission_mode: Some("agent".into()),
        permission_granularity: EffortGranularity::PerSession,
    })
}

pub(crate) fn grok_permission_modes() -> Vec<PermissionModeOption> {
    vec![
        PermissionModeOption {
            value: "ask".into(),
            label: "Ask first".into(),
            description: "Ask before commands or edits that need approval.".into(),
            is_default: false,
        },
        PermissionModeOption {
            value: "agent".into(),
            label: "Full access".into(),
            description: "Allow Grok to work without approval prompts.".into(),
            is_default: true,
        },
    ]
}

fn available_models(initialize: &Value) -> Vec<Value> {
    for pointer in [
        "/_meta/modelState/availableModels",
        "/_meta/modelState/models",
        "/models/availableModels",
        "/_meta/models/availableModels",
        "/availableModels",
        "/models",
        "/_meta/models",
    ] {
        if let Some(models) = initialize.pointer(pointer).and_then(Value::as_array) {
            return models.clone();
        }
    }

    // Tolerate registries encoded as an object keyed by model id. Preserve
    // that key when the value omits its own id.
    for pointer in ["/models", "/_meta/models"] {
        if let Some(models) = initialize.pointer(pointer).and_then(Value::as_object) {
            return models
                .iter()
                .map(|(id, model)| {
                    let mut model = model.clone();
                    if let Some(object) = model.as_object_mut() {
                        if !object.contains_key("modelId")
                            && !object.contains_key("id")
                            && !object.contains_key("value")
                        {
                            object.insert("modelId".into(), Value::String(id.clone()));
                        }
                    }
                    model
                })
                .collect();
        }
    }
    Vec::new()
}

fn reasoning_efforts(model: &Value) -> Vec<Value> {
    for pointer in [
        "/_meta/reasoningEfforts",
        "/reasoningEfforts",
        "/_meta/supportedReasoningEfforts",
        "/supportedReasoningEfforts",
    ] {
        if let Some(values) = model.pointer(pointer).and_then(Value::as_array) {
            return values.clone();
        }
    }
    Vec::new()
}

fn effort_value(effort: &Value) -> Option<String> {
    match effort {
        Value::String(value) => nonempty(value),
        Value::Object(_) => string_field(effort, &["value", "id"]),
        _ => None,
    }
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str).and_then(nonempty))
}

fn string_field_from_meta_or_top(value: &Value, key: &str) -> Option<String> {
    value
        .pointer(&format!("/_meta/{key}"))
        .or_else(|| value.get(key))
        .and_then(Value::as_str)
        .and_then(nonempty)
}

fn string_at_pointer(value: &Value, pointer: &str) -> Option<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .and_then(nonempty)
}

fn bool_field(value: &Value, key: &str) -> Option<bool> {
    value
        .pointer(&format!("/_meta/{key}"))
        .or_else(|| value.get(key))
        .and_then(Value::as_bool)
}

fn number_field(value: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|key| {
        value
            .pointer(&format!("/_meta/{key}"))
            .or_else(|| value.get(*key))
            .and_then(|raw| {
                raw.as_u64().or_else(|| {
                    raw.as_str()
                        .map(str::trim)
                        .and_then(|text| text.parse().ok())
                })
            })
    })
}

fn nonempty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_official_model_state_without_normalizing_future_efforts() {
        let initialize = json!({
            "agentCapabilities": {
                "promptCapabilities": { "image": true }
            },
            "_meta": {
                "modelState": {
                    "currentModelId": "grok-4.6",
                    "availableModels": [{
                        "modelId": "grok-older",
                        "name": "Grok Older",
                        "_meta": { "supportsReasoningEffort": false }
                    }, {
                        "modelId": "grok-4.6",
                        "name": "Grok 4.6",
                        "description": "Latest frontier model",
                        "_meta": {
                            "totalContextTokens": 500000,
                            "supportsReasoningEffort": true,
                            "reasoningEffort": "future/provider-effort",
                            "reasoningEfforts": [
                                {
                                    "id": "xhigh-id",
                                    "value": "xhigh",
                                    "label": "Extra High Effort",
                                    "description": "Highest effort",
                                    "default": false
                                },
                                {
                                    "id": "future-id",
                                    "value": "future/provider-effort",
                                    "label": "Future Effort",
                                    "description": "A value unknown to Codemux",
                                    "default": true
                                }
                            ]
                        }
                    }]
                }
            }
        });

        let capabilities = build_capabilities(&initialize).unwrap();
        assert_eq!(capabilities.effort_granularity, EffortGranularity::PerTurn);
        assert_eq!(
            capabilities.permission_granularity,
            EffortGranularity::PerSession
        );
        assert_eq!(
            capabilities.default_permission_mode.as_deref(),
            Some("agent")
        );
        let model = &capabilities.models[0];
        assert_eq!(model.id, "grok-4.6");
        assert_eq!(capabilities.models[1].id, "grok-older");
        assert_eq!(model.max_context_tokens, Some(500_000));
        assert!(model.supports_images);
        assert_eq!(model.effort_levels, vec!["xhigh", "future/provider-effort"]);
        assert_eq!(
            model.default_effort.as_deref(),
            Some("future/provider-effort")
        );
        assert_eq!(
            capabilities
                .effort_label_map
                .get("future/provider-effort")
                .map(String::as_str),
            Some("Future Effort")
        );
        assert_eq!(
            model
                .effort_descriptions
                .get("future/provider-effort")
                .map(String::as_str),
            Some("A value unknown to Codemux")
        );
    }

    #[test]
    fn accepts_models_fallback_and_string_efforts() {
        let initialize = json!({
            "promptCapabilities": { "image": false },
            "models": [{
                "id": "grok-future",
                "label": "Grok Future",
                "contextWindow": "1000000",
                "supportsReasoningEffort": true,
                "reasoningEfforts": ["provider:deep", "provider:fast"],
                "reasoningEffort": "provider:fast"
            }]
        });

        let capabilities = build_capabilities(&initialize).unwrap();
        let model = &capabilities.models[0];
        assert_eq!(model.id, "grok-future");
        assert_eq!(model.label, "Grok Future");
        assert_eq!(model.max_context_tokens, Some(1_000_000));
        assert_eq!(model.effort_levels, vec!["provider:deep", "provider:fast"]);
        assert_eq!(model.default_effort.as_deref(), Some("provider:fast"));
        assert!(!model.supports_images);
    }

    #[test]
    fn accepts_object_catalog_and_honors_explicit_no_reasoning() {
        let initialize = json!({
            "models": {
                "grok-keyed": {
                    "name": "Keyed model",
                    "_meta": {
                        "supportsReasoningEffort": false,
                        "reasoningEfforts": [{ "value": "should-not-surface" }]
                    }
                }
            }
        });

        let capabilities = build_capabilities(&initialize).unwrap();
        let model = &capabilities.models[0];
        assert_eq!(model.id, "grok-keyed");
        assert!(model.effort_levels.is_empty());
        assert_eq!(model.default_effort, None);
    }

    #[test]
    fn missing_catalog_is_a_typed_harvest_failure() {
        let error = build_capabilities(&json!({})).unwrap_err();
        assert!(error
            .to_command_string()
            .starts_with("grok_harvest_failed:"));
    }

    #[test]
    fn authenticate_timeout_is_not_misreported_as_missing_credentials() {
        let error = map_rpc_error(RpcChildError::Timeout {
            method: "authenticate".into(),
            elapsed: Duration::from_secs(15),
        });
        assert!(matches!(error, HarvestError::HarvestFailed { .. }));
        assert!(error
            .to_command_string()
            .starts_with("grok_harvest_failed:"));
    }

    #[test]
    fn authenticate_rejection_still_maps_to_actionable_auth_error() {
        let error = map_rpc_error(RpcChildError::RpcError(crate::json_rpc_child::RpcError {
            code: -32000,
            message: "Authentication required".into(),
            data: None,
        }));
        assert!(matches!(error, HarvestError::NotAuthenticated { .. }));
        assert!(error
            .to_command_string()
            .starts_with("grok_not_authenticated:"));
    }
}
