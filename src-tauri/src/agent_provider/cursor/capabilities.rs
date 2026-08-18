//! Runtime capability discovery for Cursor Agent.
//!
//! Cursor's ACP server is the source of truth.  The provider deliberately
//! carries no maintained model catalogue: every refresh asks the installed
//! CLI for `cursor/list_available_models`, then projects each model's ACP
//! session config options onto Codemux's existing reasoning, context, fast,
//! and thinking controls.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::agent_provider::{
    ChatModelInfo, ContextWindowOption, EffortGranularity, PermissionModeOption,
    ProviderChatCapabilities,
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
            Self::NotInstalled { hint } => format!("cursor_not_installed: {hint}"),
            Self::NotAuthenticated { hint } => format!("cursor_not_authenticated: {hint}"),
            Self::HarvestFailed { message } => format!("cursor_harvest_failed: {message}"),
        }
    }
}

struct CachedCapabilities {
    harvested_at: Instant,
    capabilities: ProviderChatCapabilities,
}

#[derive(Default)]
pub struct CursorCapabilityCache {
    inner: Mutex<Option<CachedCapabilities>>,
}

impl CursorCapabilityCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn get_or_harvest(
        &self,
        binary_path: &Path,
    ) -> Result<ProviderChatCapabilities, HarvestError> {
        // Hold the mutex through harvest: capability refreshes are rare and
        // this makes concurrent picker mounts a single-flight operation
        // instead of spawning duplicate Cursor ACP processes.
        let mut cached = self.inner.lock().await;
        if let Some(entry) = cached.as_ref() {
            if entry.harvested_at.elapsed() < CAPABILITY_CACHE_TTL {
                return Ok(entry.capabilities.clone());
            }
        }
        let fresh = harvest_cursor_capabilities(binary_path).await?;
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

#[derive(Debug, Deserialize)]
struct AvailableModelsResponse {
    models: Vec<AvailableModel>,
}

#[derive(Debug, Deserialize)]
struct AvailableModel {
    value: String,
    name: String,
    #[serde(default, rename = "configOptions")]
    config_options: Vec<Value>,
}

pub(crate) fn initialize_params(client_name: &str) -> Value {
    json!({
        "protocolVersion": 1,
        "clientCapabilities": {
            "session": { "configOptions": { "boolean": {} } },
            "_meta": { "parameterizedModelPicker": true }
        },
        "clientInfo": {
            "name": client_name,
            "title": "Codemux",
            "version": env!("CARGO_PKG_VERSION")
        }
    })
}

pub async fn harvest_cursor_capabilities(
    binary_path: &Path,
) -> Result<ProviderChatCapabilities, HarvestError> {
    let child = JsonRpcChild::spawn(SpawnConfig {
        program: PathBuf::from(binary_path),
        args: vec!["acp".into()],
        env: HashMap::new(),
        cwd: None,
        default_timeout: HARVEST_TIMEOUT,
    })
    .await
    .map_err(|err| map_spawn_error(err, binary_path))?;
    let child = Arc::new(child);

    let result = async {
        child
            .request(
                "initialize",
                initialize_params("codemux-capability-harvest"),
            )
            .await
            .map_err(map_rpc_error)?;
        child
            .request("authenticate", json!({ "methodId": "cursor_login" }))
            .await
            .map_err(map_auth_error)?;
        let raw = child
            .request("cursor/list_available_models", json!({}))
            .await
            .map_err(map_rpc_error)?;
        let response: AvailableModelsResponse =
            serde_json::from_value(raw).map_err(|err| HarvestError::HarvestFailed {
                message: format!("Cursor returned an invalid model catalogue: {err}"),
            })?;
        Ok(build_capabilities(response.models))
    }
    .await;

    let _ = child.shutdown().await;
    result
}

fn map_spawn_error(err: RpcChildError, binary_path: &Path) -> HarvestError {
    match err {
        RpcChildError::SpawnFailed(source) if source.kind() == std::io::ErrorKind::NotFound => {
            HarvestError::NotInstalled {
                hint: format!(
                    "Install Cursor Agent and ensure `{}` is on PATH.",
                    binary_path.display()
                ),
            }
        }
        other => HarvestError::HarvestFailed {
            message: format!("Could not start Cursor ACP: {other}"),
        },
    }
}

fn map_auth_error(err: RpcChildError) -> HarvestError {
    let message = err.to_string();
    if looks_unauthenticated(&message) {
        HarvestError::NotAuthenticated {
            hint: "Run `cursor-agent login` and try again.".into(),
        }
    } else {
        HarvestError::HarvestFailed {
            message: format!("Cursor authentication probe failed: {message}"),
        }
    }
}

fn map_rpc_error(err: RpcChildError) -> HarvestError {
    let message = err.to_string();
    if looks_unauthenticated(&message) {
        HarvestError::NotAuthenticated {
            hint: "Run `cursor-agent login` and try again.".into(),
        }
    } else {
        HarvestError::HarvestFailed { message }
    }
}

pub(crate) fn looks_unauthenticated(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("authentication required")
        || lower.contains("not authenticated")
        || lower.contains("not logged in")
        || lower.contains("login required")
}

fn build_capabilities(entries: Vec<AvailableModel>) -> ProviderChatCapabilities {
    let effort_label_map = dynamic_effort_label_map(&entries);
    let mut seen = HashSet::new();
    let models = entries
        .into_iter()
        .filter_map(|entry| {
            let id = entry.value.trim().to_string();
            if id.is_empty() || !seen.insert(id.clone()) {
                return None;
            }
            let label = match entry.name.trim() {
                "" => id.clone(),
                value => value.to_string(),
            };
            Some(model_from_options(id, label, &entry.config_options))
        })
        .collect();

    ProviderChatCapabilities {
        models,
        effort_granularity: EffortGranularity::PerTurn,
        effort_label_map,
        permission_modes: cursor_permission_modes(),
        default_permission_mode: Some("agent".into()),
        // ACP config options can be changed on a live session; Codemux sends
        // the selected mode with the next turn rather than restarting Cursor.
        permission_granularity: EffortGranularity::PerTurn,
    }
}

fn model_from_options(id: String, label: String, options: &[Value]) -> ChatModelInfo {
    let effort = find_effort_option(options);
    let thinking = find_thinking_option(options);
    let mut effort_levels = effort.map(select_values).unwrap_or_default();
    if thinking.is_some() && !effort_levels.iter().any(|value| value == "none") {
        effort_levels.insert(0, "none".into());
    }
    let default_effort = if thinking.and_then(current_boolean_like) == Some(false) {
        Some("none".into())
    } else {
        effort
            .and_then(current_string)
            .and_then(normalize_effort)
            .filter(|value| effort_levels.contains(value))
            .or_else(|| effort_levels.first().cloned())
    };

    let context = options.iter().find(|option| {
        category(option) == "model_config" && option_matches(option, &["context", "context_size"])
    });
    let context_window_options = context
        .map(|option| {
            let current = current_string(option);
            select_entries(option)
                .into_iter()
                .map(|(value, label)| ContextWindowOption {
                    context_window_tokens: parse_context_tokens(&value)
                        .or_else(|| parse_context_tokens(&label)),
                    is_default: current.as_deref() == Some(value.as_str()),
                    value,
                    label,
                })
                .collect()
        })
        .unwrap_or_default();

    let supports_fast_mode = options.iter().any(|option| {
        category(option) == "model_config"
            && option_matches(option, &["fast"])
            && boolean_like(option)
    });
    let supports_thinking_toggle = thinking.is_some();

    ChatModelInfo {
        id,
        label,
        description: None,
        effort_levels,
        default_effort,
        effort_descriptions: HashMap::new(),
        prompt_injected_effort_levels: vec![],
        context_window_options,
        supports_adaptive_thinking: false,
        supports_thinking_toggle,
        supports_fast_mode,
        // Cursor's ACP initialize response advertises image prompts globally.
        // The extension does not currently narrow this per model.
        supports_images: true,
        sub_provider: None,
        is_free: false,
        max_context_tokens: None,
    }
}

pub(crate) fn cursor_permission_modes() -> Vec<PermissionModeOption> {
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
            description: "Allow Cursor Agent to work without approval prompts.".into(),
            is_default: true,
        },
    ]
}

fn dynamic_effort_label_map(entries: &[AvailableModel]) -> HashMap<String, String> {
    let mut labels = HashMap::new();
    // Prefer an actual effort/reasoning label (for example "None") over a
    // synthesized thinking-toggle label (for example "Off"), regardless of
    // model catalogue order.
    for entry in entries {
        if let Some(effort) = find_effort_option(&entry.config_options) {
            for (value, label) in select_entries(effort) {
                if let Some(value) = normalize_effort(value) {
                    labels.entry(value).or_insert(label);
                }
            }
        }
    }
    for entry in entries {
        if let Some(thinking) = find_thinking_option(&entry.config_options) {
            let off_label = select_entries(thinking)
                .into_iter()
                .find_map(|(value, label)| value.eq_ignore_ascii_case("false").then_some(label))
                .unwrap_or_else(|| "None".into());
            labels.entry("none".into()).or_insert(off_label);
        }
    }
    labels
}

fn string_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)?
        .as_str()
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

fn category(option: &Value) -> String {
    string_field(option, "category")
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn option_matches(option: &Value, needles: &[&str]) -> bool {
    let id = string_field(option, "id")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let name = string_field(option, "name")
        .unwrap_or_default()
        .to_ascii_lowercase();
    needles
        .iter()
        .any(|needle| id == *needle || name == *needle || name.contains(needle))
}

fn find_effort_option(options: &[Value]) -> Option<&Value> {
    let candidates = options
        .iter()
        .filter(|option| {
            string_field(option, "type") == Some("select")
                && option_matches(option, &["effort", "reasoning"])
        })
        .collect::<Vec<_>>();
    candidates
        .iter()
        .copied()
        .find(|option| category(option) == "model_option")
        .or_else(|| {
            candidates.iter().copied().find(|option| {
                string_field(option, "id").is_some_and(|id| id.eq_ignore_ascii_case("effort"))
            })
        })
        .or_else(|| {
            candidates
                .iter()
                .copied()
                .find(|option| category(option) == "thought_level")
        })
        .or_else(|| candidates.first().copied())
}

fn find_thinking_option(options: &[Value]) -> Option<&Value> {
    options
        .iter()
        .find(|option| option_matches(option, &["thinking"]) && boolean_like(option))
}

fn select_entries(option: &Value) -> Vec<(String, String)> {
    let Some(entries) = option.get("options").and_then(Value::as_array) else {
        return vec![];
    };
    let mut flattened = Vec::new();
    for entry in entries {
        if let Some(value) = string_field(entry, "value") {
            flattened.push((
                value.to_string(),
                string_field(entry, "name").unwrap_or(value).to_string(),
            ));
        } else if let Some(group) = entry.get("options").and_then(Value::as_array) {
            for nested in group {
                if let Some(value) = string_field(nested, "value") {
                    flattened.push((
                        value.to_string(),
                        string_field(nested, "name").unwrap_or(value).to_string(),
                    ));
                }
            }
        }
    }
    flattened
}

fn select_values(option: &Value) -> Vec<String> {
    let mut seen = HashSet::new();
    select_entries(option)
        .into_iter()
        .filter_map(|(value, _)| normalize_effort(value))
        .filter(|value| seen.insert(value.clone()))
        .collect()
}

fn normalize_effort(value: impl AsRef<str>) -> Option<String> {
    let normalized = value
        .as_ref()
        .trim()
        .to_ascii_lowercase()
        .replace([' ', '-'], "_");
    if normalized.is_empty() {
        return None;
    }
    match normalized.as_str() {
        "low" => Some("low".into()),
        "medium" => Some("medium".into()),
        "high" => Some("high".into()),
        "max" => Some("max".into()),
        "xhigh" | "extra_high" => Some("xhigh".into()),
        // Preserve new provider-defined levels instead of filtering them out.
        _ => Some(normalized),
    }
}

fn current_string(option: &Value) -> Option<String> {
    string_field(option, "currentValue").map(str::to_string)
}

fn current_boolean_like(option: &Value) -> Option<bool> {
    option
        .get("currentValue")
        .and_then(|value| value.as_bool().or_else(|| value.as_str()?.parse().ok()))
}

fn boolean_like(option: &Value) -> bool {
    match string_field(option, "type") {
        Some("boolean") => true,
        Some("select") => {
            let values: HashSet<String> = select_entries(option)
                .into_iter()
                .map(|(value, _)| value.to_ascii_lowercase())
                .collect();
            values.contains("true") && values.contains("false")
        }
        _ => false,
    }
}

fn parse_context_tokens(raw: &str) -> Option<u64> {
    let normalized = raw.trim().to_ascii_lowercase().replace([',', ' '], "");
    if let Some(number) = normalized.strip_suffix('k') {
        return number.parse::<u64>().ok()?.checked_mul(1_000);
    }
    if let Some(number) = normalized.strip_suffix('m') {
        return number.parse::<u64>().ok()?.checked_mul(1_000_000);
    }
    normalized
        .parse::<u64>()
        .ok()
        .filter(|value| *value >= 1_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_each_models_own_dynamic_options() {
        let entries: AvailableModelsResponse = serde_json::from_value(json!({
            "models": [
                {
                    "value": "gpt-next",
                    "name": "GPT Next",
                    "configOptions": [
                        {"id":"reasoning","name":"Reasoning","category":"thought_level","type":"select","currentValue":"high","options":[{"value":"low","name":"Low"},{"value":"high","name":"High"}]},
                        {"id":"context","name":"Context","category":"model_config","type":"select","currentValue":"200k","options":[{"value":"200k","name":"200K"},{"value":"1m","name":"1M"}]},
                        {"id":"fast","name":"Fast Mode","category":"model_config","type":"boolean","currentValue":false}
                    ]
                },
                {"value":"small-model","name":"Small Model"}
            ]
        })).unwrap();
        let caps = build_capabilities(entries.models);
        let first = &caps.models[0];
        assert_eq!(first.effort_levels, ["low", "high"]);
        assert_eq!(first.default_effort.as_deref(), Some("high"));
        assert_eq!(
            first.context_window_options[1].context_window_tokens,
            Some(1_000_000)
        );
        assert!(first.supports_fast_mode);
        assert!(caps.models[1].effort_levels.is_empty());
        assert!(!caps.models[1].supports_fast_mode);
    }

    #[test]
    fn handles_grouped_select_options_and_deduplicates_models() {
        let entries: AvailableModelsResponse = serde_json::from_value(json!({
            "models": [
                {"value":"m","name":"M","configOptions":[{"id":"reasoning","name":"Effort","category":"model_option","type":"select","options":[{"name":"Normal","options":[{"value":"medium","name":"Medium"}]}]}]},
                {"value":"m","name":"Duplicate"}
            ]
        })).unwrap();
        let caps = build_capabilities(entries.models);
        assert_eq!(caps.models.len(), 1);
        assert_eq!(caps.models[0].effort_levels, ["medium"]);
    }

    #[test]
    fn preserves_new_effort_values_and_maps_thinking_off_to_none() {
        let entries: AvailableModelsResponse = serde_json::from_value(json!({
            "models": [{
                "value": "future-model",
                "name": "Future Model",
                "configOptions": [
                    {
                        "id": "effort",
                        "name": "Effort",
                        "category": "thought_level",
                        "type": "select",
                        "currentValue": "adaptive",
                        "options": [
                            {"value": "minimal", "name": "Minimal"},
                            {"value": "adaptive", "name": "Adaptive"}
                        ]
                    },
                    {
                        "id": "thinking",
                        "name": "Thinking",
                        "category": "thought_level",
                        "type": "select",
                        "currentValue": "false",
                        "options": [
                            {"value": "false", "name": "Off"},
                            {"value": "true", "name": "On"}
                        ]
                    }
                ]
            }]
        }))
        .unwrap();
        let caps = build_capabilities(entries.models);
        let model = &caps.models[0];
        assert_eq!(model.effort_levels, ["none", "minimal", "adaptive"]);
        assert_eq!(model.default_effort.as_deref(), Some("none"));
        assert!(model.supports_thinking_toggle);
        assert_eq!(caps.effort_label_map["none"], "Off");
        assert_eq!(caps.effort_label_map["adaptive"], "Adaptive");
    }

    #[test]
    fn direct_reasoning_label_wins_over_synthesized_thinking_label() {
        let entries: AvailableModelsResponse = serde_json::from_value(json!({
            "models": [
                {
                    "value": "thinking-model",
                    "name": "Thinking Model",
                    "configOptions": [{
                        "id": "thinking",
                        "name": "Thinking",
                        "category": "thought_level",
                        "type": "select",
                        "options": [
                            {"value": "false", "name": "Off"},
                            {"value": "true", "name": "On"}
                        ]
                    }]
                },
                {
                    "value": "reasoning-model",
                    "name": "Reasoning Model",
                    "configOptions": [{
                        "id": "reasoning",
                        "name": "Reasoning",
                        "category": "model_option",
                        "type": "select",
                        "options": [
                            {"value": "none", "name": "None"},
                            {"value": "high", "name": "High"}
                        ]
                    }]
                }
            ]
        }))
        .unwrap();
        let caps = build_capabilities(entries.models);
        assert_eq!(caps.effort_label_map["none"], "None");
    }
}
