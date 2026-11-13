//! Live capability discovery for Google Antigravity's ACP server (`agy acp`).
//!
//! Antigravity publishes its model and thinking-level choices only as session
//! config options, so discovery signs in, opens a throwaway session, selects
//! each model in turn to read the thinking levels that model offers, and
//! closes it again. Values stay opaque so a newer CLI can offer new models
//! without a Codemux release.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::sync::Mutex;

use crate::agent_provider::acp::protocol::{config_options, initialize_params, looks_unauthenticated};
use crate::agent_provider::acp::session::{
    advertises_auth_method, AcpDialect, ANTIGRAVITY_AUTH_METHOD, ANTIGRAVITY_INSTALL_HINT,
    ANTIGRAVITY_SIGN_IN_HINT,
};
use crate::agent_provider::{
    ChatModelInfo, EffortGranularity, PermissionModeOption, ProviderChatCapabilities,
};
use crate::json_rpc_child::{JsonRpcChild, RpcChildError, SpawnConfig};

/// `agy acp` unpacks its runtime on first launch, which can take a while.
const HARVEST_TIMEOUT: Duration = Duration::from_secs(45);
const CAPABILITY_CACHE_TTL: Duration = Duration::from_secs(5 * 60);
/// Budget for reading every model's thinking levels in one discovery.
const MODEL_PROBE_BUDGET: Duration = Duration::from_secs(20);

#[derive(Debug, Clone)]
pub enum HarvestError {
    NotInstalled { hint: String },
    NotAuthenticated { hint: String },
    HarvestFailed { message: String },
}

impl HarvestError {
    pub fn to_command_string(&self) -> String {
        match self {
            Self::NotInstalled { hint } => format!("antigravity_not_installed: {hint}"),
            Self::NotAuthenticated { hint } => format!("antigravity_not_authenticated: {hint}"),
            Self::HarvestFailed { message } => format!("antigravity_harvest_failed: {message}"),
        }
    }
}

struct CachedCapabilities {
    harvested_at: Instant,
    capabilities: ProviderChatCapabilities,
}

#[derive(Default)]
pub struct AntigravityCapabilityCache {
    inner: Mutex<Option<CachedCapabilities>>,
}

impl AntigravityCapabilityCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn get_or_harvest(
        &self,
        binary_path: &Path,
    ) -> Result<ProviderChatCapabilities, HarvestError> {
        // Holding the lock through the refresh keeps simultaneous picker
        // mounts down to one short-lived ACP process.
        let mut cached = self.inner.lock().await;
        if let Some(entry) = cached.as_ref() {
            if entry.harvested_at.elapsed() < CAPABILITY_CACHE_TTL {
                return Ok(entry.capabilities.clone());
            }
        }
        let fresh = harvest_antigravity_capabilities(binary_path).await?;
        *cached = Some(CachedCapabilities {
            harvested_at: Instant::now(),
            capabilities: fresh.clone(),
        });
        Ok(fresh)
    }

    /// The last harvested catalog, whatever its age. Never harvests, and
    /// reads nothing while a harvest holds the cache.
    pub fn cached(&self) -> Option<ProviderChatCapabilities> {
        let cached = self.inner.try_lock().ok()?;
        cached.as_ref().map(|entry| entry.capabilities.clone())
    }

    pub async fn invalidate(&self) {
        *self.inner.lock().await = None;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AntigravityHealthProbe {
    pub authenticated: bool,
    pub version: Option<String>,
}

pub async fn harvest_antigravity_capabilities(
    binary_path: &Path,
) -> Result<ProviderChatCapabilities, HarvestError> {
    let child = Arc::new(spawn_probe(binary_path).await?);
    let result = async {
        let initialize = sign_in(&child).await?;
        let cwd = probe_directory()?;
        let session = child
            .request("session/new", json!({ "cwd": cwd, "mcpServers": [] }))
            .await
            .map_err(map_rpc_error)?;
        let per_model = per_model_options(&child, &session).await;
        if initialize
            .pointer("/agentCapabilities/sessionCapabilities/close")
            .is_some_and(Value::is_object)
        {
            if let Some(session_id) = session.get("sessionId").and_then(Value::as_str) {
                let _ = child
                    .request("session/close", json!({ "sessionId": session_id }))
                    .await;
            }
        }
        build_capabilities(&initialize, &session, &per_model)
    }
    .await;
    let _ = child.shutdown().await;
    result
}

/// The config options Antigravity reports after selecting each model, keyed
/// by model id. Thinking levels belong to a model family, so one snapshot
/// cannot describe them all. A model that cannot be switched to within the
/// budget is left out and advertises no thinking levels rather than another
/// family's.
async fn per_model_options(child: &JsonRpcChild, session: &Value) -> HashMap<String, Vec<Value>> {
    let options = config_options(session);
    let mut per_model = HashMap::new();
    let (Some(session_id), Some(option)) = (
        session.get("sessionId").and_then(Value::as_str),
        model_option(&options),
    ) else {
        return per_model;
    };
    // The session snapshot already describes the current model.
    let current = option.get("currentValue").and_then(Value::as_str);
    let Some(config_id) = option.get("id").and_then(Value::as_str) else {
        return per_model;
    };
    let deadline = Instant::now() + MODEL_PROBE_BUDGET;
    for (model, _) in select_entries(option) {
        if Some(model.as_str()) == current {
            continue;
        }
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            break;
        };
        let params = json!({ "sessionId": session_id, "configId": config_id, "value": model });
        match child.request_with_timeout("session/set_config_option", params, remaining).await {
            Ok(response) => {
                let updated = config_options(&response);
                if !updated.is_empty() {
                    per_model.insert(model, updated);
                }
            }
            // The agent refused this one model; the rest may still switch.
            Err(RpcChildError::RpcError(_)) => {}
            // A timeout or dead process will not get better for the next.
            Err(_) => break,
        }
    }
    per_model
}

/// Whether `agy acp` starts and holds a Google sign-in. Never opens a
/// browser (see [`spawn_probe`]).
pub async fn probe_antigravity_health(
    binary_path: &Path,
) -> Result<AntigravityHealthProbe, HarvestError> {
    let child = Arc::new(spawn_probe(binary_path).await?);
    let result = async {
        let initialize = child
            .request("initialize", initialize_params("codemux-health-probe"))
            .await
            .map_err(map_rpc_error)?;
        let version = initialize
            .pointer("/agentInfo/version")
            .and_then(Value::as_str)
            .map(str::to_string);
        let authenticated = match authenticate(&child, &initialize).await {
            Ok(()) => true,
            Err(HarvestError::NotAuthenticated { .. }) => false,
            Err(error) => return Err(error),
        };
        Ok(AntigravityHealthProbe {
            authenticated,
            version,
        })
    }
    .await;
    let _ = child.shutdown().await;
    result
}

/// Start `agy acp` for a background probe.
///
/// A signed-out `agy acp` answers `authenticate` by opening Google sign-in in
/// the browser. A probe runs without the user asking, so it presents itself
/// as an SSH session: Antigravity then refuses browser sign-in at once and
/// reports "not signed in", while a cached sign-in still works.
async fn spawn_probe(binary_path: &Path) -> Result<JsonRpcChild, HarvestError> {
    JsonRpcChild::spawn(SpawnConfig {
        program: PathBuf::from(binary_path),
        args: AcpDialect::Antigravity.spawn_args(),
        env: HashMap::from([("SSH_CONNECTION".to_string(), "codemux-probe".to_string())]),
        cwd: Some(probe_directory()?),
        default_timeout: HARVEST_TIMEOUT,
    })
    .await
    .map_err(|error| match error {
        RpcChildError::SpawnFailed(source) if source.kind() == std::io::ErrorKind::NotFound => {
            HarvestError::NotInstalled {
                hint: ANTIGRAVITY_INSTALL_HINT.into(),
            }
        }
        other => HarvestError::HarvestFailed {
            message: format!("Could not start Antigravity ACP: {other}"),
        },
    })
}

/// A Codemux-owned empty folder, so the throwaway discovery session never
/// reads or indexes a real project.
fn probe_directory() -> Result<PathBuf, HarvestError> {
    let dir = std::env::temp_dir().join("codemux-antigravity-probe");
    std::fs::create_dir_all(&dir).map_err(|error| HarvestError::HarvestFailed {
        message: format!("Could not create the Antigravity probe folder: {error}"),
    })?;
    Ok(dir)
}

async fn sign_in(child: &JsonRpcChild) -> Result<Value, HarvestError> {
    let initialize = child
        .request("initialize", initialize_params("codemux-capability-harvest"))
        .await
        .map_err(map_rpc_error)?;
    authenticate(child, &initialize).await?;
    Ok(initialize)
}

async fn authenticate(child: &JsonRpcChild, initialize: &Value) -> Result<(), HarvestError> {
    let not_signed_in = || HarvestError::NotAuthenticated {
        hint: ANTIGRAVITY_SIGN_IN_HINT.into(),
    };
    if !advertises_auth_method(initialize, ANTIGRAVITY_AUTH_METHOD) {
        return Err(not_signed_in());
    }
    child
        .request("authenticate", json!({ "methodId": ANTIGRAVITY_AUTH_METHOD }))
        .await
        .map(|_| ())
        .map_err(|error| match error {
            // The probe can never complete a browser sign-in, so any refusal
            // means the machine has no usable Google sign-in.
            RpcChildError::RpcError(_) => not_signed_in(),
            other => map_rpc_error(other),
        })
}

fn map_rpc_error(error: RpcChildError) -> HarvestError {
    let message = error.to_string();
    if looks_unauthenticated(&message) {
        HarvestError::NotAuthenticated {
            hint: ANTIGRAVITY_SIGN_IN_HINT.into(),
        }
    } else {
        HarvestError::HarvestFailed { message }
    }
}

/// The session config option that carries Antigravity's thinking level.
pub(crate) fn effort_option(options: &[Value]) -> Option<&Value> {
    options.iter().find(|option| {
        let field = |key: &str| {
            option
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_ascii_lowercase()
        };
        option.get("type").and_then(Value::as_str) == Some("select")
            && (field("category") == "thought_level"
                || ["effort", "thinking", "reasoning"]
                    .iter()
                    .any(|needle| field("id").contains(needle) || field("name").contains(needle)))
    })
}

fn model_option(options: &[Value]) -> Option<&Value> {
    options.iter().find(|option| {
        option.get("id").and_then(Value::as_str) == Some("model")
            || option.get("category").and_then(Value::as_str) == Some("model")
    })
}

/// `(value, name)` pairs of a select option, including grouped entries.
fn select_entries(option: &Value) -> Vec<(String, String)> {
    let mut entries = Vec::new();
    for entry in option
        .get("options")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let group = entry.get("options").and_then(Value::as_array);
        for item in group.map_or_else(|| vec![entry], |items| items.iter().collect()) {
            let Some(value) = item
                .get("value")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .unwrap_or(value);
            entries.push((value.to_string(), name.to_string()));
        }
    }
    entries
}

pub(crate) fn build_capabilities(
    initialize: &Value,
    session: &Value,
    per_model: &HashMap<String, Vec<Value>>,
) -> Result<ProviderChatCapabilities, HarvestError> {
    let options = config_options(session);
    let supports_images = initialize
        .pointer("/agentCapabilities/promptCapabilities/image")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let (mut entries, current) = match model_option(&options) {
        Some(option) => (
            select_entries(option),
            option
                .get("currentValue")
                .and_then(Value::as_str)
                .map(str::to_string),
        ),
        None => (
            session
                .pointer("/models/availableModels")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|model| {
                    let id = model.get("modelId").and_then(Value::as_str)?.trim();
                    let name = model.get("name").and_then(Value::as_str).unwrap_or(id);
                    (!id.is_empty()).then(|| (id.to_string(), name.to_string()))
                })
                .collect(),
            session
                .pointer("/models/currentModelId")
                .and_then(Value::as_str)
                .map(str::to_string),
        ),
    };
    let mut seen = HashSet::new();
    entries.retain(|(value, _)| seen.insert(value.clone()));
    // The composer treats the first row as the provider default.
    if let Some(index) = current
        .as_deref()
        .and_then(|current| entries.iter().position(|(value, _)| value == current))
    {
        let entry = entries.remove(index);
        entries.insert(0, entry);
    }
    if entries.is_empty() {
        return Err(HarvestError::HarvestFailed {
            message: "Antigravity did not offer any models for this account.".into(),
        });
    }

    let mut effort_label_map = HashMap::new();
    let mut models = Vec::with_capacity(entries.len());
    for (id, label) in entries {
        // Only a snapshot taken with this model selected describes it.
        let verified = per_model
            .get(&id)
            .or_else(|| (current.as_deref() == Some(id.as_str())).then_some(&options));
        let effort = verified.and_then(|options| effort_option(options));
        let effort_entries = effort.map(select_entries).unwrap_or_default();
        let effort_levels = effort_entries
            .iter()
            .map(|(value, _)| value.clone())
            .collect::<Vec<_>>();
        let default_effort = effort
            .and_then(|option| option.get("currentValue"))
            .and_then(Value::as_str)
            .filter(|value| effort_levels.iter().any(|level| level == value))
            .map(str::to_string)
            .or_else(|| effort_levels.first().cloned());
        for (value, name) in effort_entries {
            effort_label_map.entry(value).or_insert(name);
        }
        models.push(ChatModelInfo {
            id,
            label,
            description: None,
            effort_levels,
            default_effort,
            effort_descriptions: HashMap::new(),
            prompt_injected_effort_levels: Vec::new(),
            context_window_options: Vec::new(),
            supports_adaptive_thinking: false,
            supports_thinking_toggle: false,
            supports_fast_mode: false,
            service_tiers: vec![],
            supports_images,
            sub_provider: None,
            is_free: false,
            max_context_tokens: None,
        });
    }

    Ok(ProviderChatCapabilities {
        // `agy acp` folds a prompt sent mid-turn into the running turn, but
        // Codemux has not verified that it keeps running tools; queue instead.
        supports_steering: false,
        models,
        effort_granularity: EffortGranularity::PerTurn,
        effort_label_map,
        permission_modes: antigravity_permission_modes(),
        default_permission_mode: Some("agent".into()),
        permission_granularity: EffortGranularity::PerTurn,
    })
}

pub(crate) fn antigravity_permission_modes() -> Vec<PermissionModeOption> {
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
            description: "Allow Antigravity to work without approval prompts.".into(),
            is_default: true,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn initialize() -> Value {
        json!({
            "agentCapabilities": { "promptCapabilities": { "image": true } },
            "authMethods": [{ "id": "oauth-personal", "name": "Log in with Google" }]
        })
    }

    #[test]
    fn models_come_from_the_session_model_option_with_the_current_one_first() {
        let session = json!({ "configOptions": [
            { "id": "mode", "category": "mode", "type": "select", "currentValue": "default",
              "options": [{ "value": "default", "name": "Default" }] },
            { "id": "model", "category": "model", "type": "select", "currentValue": "gemini-b",
              "options": [
                { "value": "gemini-a", "name": "Gemini A" },
                { "group": "More", "options": [{ "value": "gemini-b", "name": "Gemini B" }] },
                { "value": "gemini-a", "name": "Duplicate" }
              ] },
            { "id": "thinking_level", "name": "Thinking level", "category": "thought_level",
              "type": "select", "currentValue": "high",
              "options": [{ "value": "low", "name": "Low" }, { "value": "high", "name": "High" }] }
        ]});
        // Gemini A belongs to a family with its own thinking levels.
        let gemini_a = vec![json!({ "id": "thinking_level", "category": "thought_level",
            "type": "select", "currentValue": "medium",
            "options": [{ "value": "minimal", "name": "Minimal" }, { "value": "medium", "name": "Medium" }] })];
        let per_model = HashMap::from([("gemini-a".to_string(), gemini_a)]);
        let caps = build_capabilities(&initialize(), &session, &per_model).unwrap();
        let ids = caps.models.iter().map(|model| model.id.as_str()).collect::<Vec<_>>();
        assert_eq!(ids, ["gemini-b", "gemini-a"]);
        assert_eq!(caps.models[0].label, "Gemini B");
        assert!(caps.models[0].supports_images);
        assert_eq!(caps.models[0].effort_levels, ["low", "high"]);
        assert_eq!(caps.models[0].default_effort.as_deref(), Some("high"));
        assert_eq!(caps.models[1].effort_levels, ["minimal", "medium"]);
        assert_eq!(caps.models[1].default_effort.as_deref(), Some("medium"));
        assert_eq!(caps.effort_label_map["high"], "High");
        assert_eq!(caps.effort_label_map["minimal"], "Minimal");
        assert_eq!(caps.default_permission_mode.as_deref(), Some("agent"));
    }

    #[test]
    fn a_model_that_could_not_be_probed_borrows_no_thinking_levels() {
        let session = json!({ "configOptions": [
            { "id": "model", "category": "model", "type": "select", "currentValue": "gemini-b",
              "options": [{ "value": "gemini-a" }, { "value": "gemini-b" }] },
            { "id": "thinking_level", "category": "thought_level", "type": "select",
              "currentValue": "high", "options": [{ "value": "low" }, { "value": "high" }] }
        ]});
        let caps = build_capabilities(&initialize(), &session, &HashMap::new()).unwrap();
        assert_eq!(caps.models[0].id, "gemini-b");
        assert_eq!(caps.models[0].effort_levels, ["low", "high"]);
        assert!(caps.models[1].effort_levels.is_empty());
        assert_eq!(caps.models[1].default_effort, None);
    }

    #[test]
    fn falls_back_to_the_standard_model_state() {
        let session = json!({ "models": {
            "currentModelId": "gemini-y",
            "availableModels": [{ "modelId": "gemini-x", "name": "X" }, { "modelId": "gemini-y" }]
        }});
        let caps = build_capabilities(&initialize(), &session, &HashMap::new()).unwrap();
        assert_eq!(caps.models[0].id, "gemini-y");
        assert_eq!(caps.models[0].label, "gemini-y");
        assert!(caps.models[0].effort_levels.is_empty());
    }

    #[test]
    fn an_empty_catalogue_is_a_typed_failure() {
        let error = build_capabilities(&initialize(), &json!({}), &HashMap::new()).unwrap_err();
        assert!(error.to_command_string().starts_with("antigravity_harvest_failed:"));
    }
}
