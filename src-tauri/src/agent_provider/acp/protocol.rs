//! Small, forward-compatible helpers around ACP and provider extensions.
//!
//! ACP config options are intentionally open-ended.  Keeping them as JSON at
//! the protocol boundary means a newer Cursor CLI can add an option without
//! making this adapter fail to decode the entire session response.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Value};

/// ACP initialize payload shared by every Codemux ACP driver.
///
/// `configOptions.boolean` lets newer agents advertise boolean selectors,
/// while the metadata flag opts into parameterized model catalogs used by
/// Cursor. Unknown capabilities are deliberately omitted rather than guessed.
pub fn initialize_params(client_name: &str) -> Value {
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

/// Pick the non-interactive authentication method for Grok Build.
///
/// Current CLIs advertise their policy-selected `defaultAuthMethodId`; that
/// takes precedence over ambient credentials as long as it is also present in
/// `authMethods` and is not the browser flow. Older CLIs without that metadata
/// fall back to an advertised API key or cached login. We never select (or
/// silently bypass) the browser default from an invisible child process.
pub fn grok_auth_method(initialize: &Value, env: &HashMap<String, String>) -> Option<String> {
    let has_api_key = env
        .get("XAI_API_KEY")
        .map(|value| !value.trim().is_empty())
        .unwrap_or_else(|| {
            std::env::var("XAI_API_KEY").is_ok_and(|value| !value.trim().is_empty())
        });
    let methods = initialize
        .get("authMethods")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|method| method.get("id").and_then(Value::as_str))
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .collect::<HashSet<_>>();

    let advertised_default = initialize
        .pointer("/_meta/defaultAuthMethodId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty());
    if let Some(default_id) = advertised_default {
        return (methods.contains(default_id) && !is_grok_browser_auth(default_id))
            .then(|| default_id.to_string());
    }

    // Compatibility for older CLIs. Follow the advertised surface so
    // enterprise policy can hide API-key auth even when a stale environment
    // variable remains in the parent process.
    if has_api_key && methods.contains("xai.api_key") {
        return Some("xai.api_key".into());
    }
    if methods.contains("cached_token") {
        return Some("cached_token".into());
    }
    None
}

fn is_grok_browser_auth(method_id: &str) -> bool {
    method_id.eq_ignore_ascii_case("grok.com")
}

pub fn looks_unauthenticated(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("authentication required")
        || lower.contains("not authenticated")
        || lower.contains("not logged in")
        || lower.contains("login required")
        || lower.contains("no auth method")
}

/// Current model from either the standard session model state or Grok's
/// initialize-time metadata mirror.
pub fn current_model_id(response: &Value) -> Option<String> {
    response
        .pointer("/models/currentModelId")
        .or_else(|| response.pointer("/_meta/modelState/currentModelId"))
        .or_else(|| response.get("currentModelId"))
        .or_else(|| response.pointer("/modelState/currentModelId"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Grok's ACP dialect applies both model and reasoning effort through the
/// unstable `session/set_model` request. Effort values stay opaque: the
/// provider's live catalog is the authority, including future values.
pub fn set_model_params(session_id: &str, model_id: &str, effort: Option<&str>) -> Value {
    let mut params = json!({
        "sessionId": session_id,
        "modelId": model_id,
    });
    if let Some(effort) = effort.map(str::trim).filter(|value| !value.is_empty()) {
        params["_meta"] = json!({ "reasoningEffort": effort });
    }
    params
}

/// Reasoning-effort values advertised per Grok model. `Some(set)` means the
/// CLI supplied an authoritative list (including an explicitly empty one);
/// `None` means the model is known but the extension did not describe its
/// efforts, so callers must preserve opaque future values.
pub type GrokModelEffortCatalog = HashMap<String, Option<HashSet<String>>>;

pub fn grok_model_effort_catalog(response: &Value) -> GrokModelEffortCatalog {
    let mut catalog = HashMap::new();
    for model in grok_available_models(response) {
        let Some(model_id) = ["modelId", "id", "value"]
            .iter()
            .find_map(|key| model.get(*key).and_then(Value::as_str))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
        else {
            continue;
        };
        let supports_reasoning = model
            .pointer("/_meta/supportsReasoningEffort")
            .or_else(|| model.get("supportsReasoningEffort"))
            .and_then(Value::as_bool);
        let efforts = grok_reasoning_efforts(&model).map(|entries| {
            entries
                .iter()
                .filter_map(|effort| match effort {
                    Value::String(value) => Some(value.as_str()),
                    Value::Object(_) => effort
                        .get("value")
                        .or_else(|| effort.get("id"))
                        .and_then(Value::as_str),
                    _ => None,
                })
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect::<HashSet<_>>()
        });
        let authoritative = match supports_reasoning {
            Some(false) => Some(HashSet::new()),
            _ => efforts,
        };
        catalog.insert(model_id, authoritative);
    }
    catalog
}

fn grok_available_models(response: &Value) -> Vec<Value> {
    for pointer in [
        "/_meta/modelState/availableModels",
        "/modelState/availableModels",
        "/models/availableModels",
        "/_meta/models/availableModels",
        "/availableModels",
        "/models",
        "/_meta/models",
    ] {
        if let Some(models) = response.pointer(pointer).and_then(Value::as_array) {
            return models.clone();
        }
    }
    Vec::new()
}

fn grok_reasoning_efforts(model: &Value) -> Option<&Vec<Value>> {
    [
        "/_meta/reasoningEfforts",
        "/reasoningEfforts",
        "/_meta/supportedReasoningEfforts",
        "/supportedReasoningEfforts",
    ]
    .iter()
    .find_map(|pointer| model.pointer(pointer).and_then(Value::as_array))
}

pub fn config_options(response: &Value) -> Vec<Value> {
    response
        .get("configOptions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

pub fn session_id(response: &Value) -> Option<String> {
    response
        .get("sessionId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub fn config_id_for(options: &[Value], kind: ConfigKind) -> Option<String> {
    let exact = match kind {
        ConfigKind::Model => &["model"][..],
        ConfigKind::Effort => &["effort", "reasoning"][..],
        ConfigKind::Context => &["context", "context_size"][..],
        ConfigKind::Fast => &["fast"][..],
        ConfigKind::Thinking => &["thinking"][..],
        ConfigKind::Mode => &["mode"][..],
    };
    options.iter().find_map(|option| {
        let id = option.get("id")?.as_str()?.trim();
        let name = option
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        let normalized_id = token(id);
        let normalized_name = token(name);
        let category = option
            .get("category")
            .and_then(Value::as_str)
            .map(token)
            .unwrap_or_default();
        let semantic_match = exact.iter().any(|needle| {
            normalized_id == *needle
                || normalized_name == *needle
                || normalized_name.contains(needle)
        });
        let category_match = match kind {
            ConfigKind::Model => category == "model",
            ConfigKind::Mode => category == "mode",
            ConfigKind::Effort => {
                matches!(category.as_str(), "thought_level" | "model_option") && semantic_match
            }
            ConfigKind::Thinking => category == "thought_level" && semantic_match,
            ConfigKind::Context | ConfigKind::Fast => category == "model_config" && semantic_match,
        };
        (semantic_match || category_match).then(|| id.to_string())
    })
}

#[derive(Debug, Clone, Copy)]
pub enum ConfigKind {
    Model,
    Effort,
    Context,
    Fast,
    Thinking,
    Mode,
}

pub fn set_config_params(session_id: &str, config_id: &str, value: Value) -> Value {
    match value {
        Value::Bool(value) => json!({
            "sessionId": session_id,
            "configId": config_id,
            "type": "boolean",
            "value": value
        }),
        other => json!({
            "sessionId": session_id,
            "configId": config_id,
            "value": other.as_str().map(str::to_string).unwrap_or_else(|| other.to_string())
        }),
    }
}

pub fn resolve_select_value(option: &Value, requested: &str) -> Option<String> {
    let requested = token(requested);
    select_entries(option)
        .into_iter()
        .find_map(|(value, name)| {
            (token(&value) == requested || token(&name) == requested).then_some(value)
        })
}

pub fn resolve_effort_value(option: &Value, requested: &str) -> Option<String> {
    let requested = normalize_effort(requested)?;
    select_entries(option)
        .into_iter()
        .find_map(|(value, name)| {
            let candidate = normalize_effort(&value).or_else(|| normalize_effort(&name));
            (candidate.as_deref() == Some(requested.as_str())).then_some(value)
        })
}

pub fn resolve_boolean_value(option: &Value, requested: bool) -> Option<Value> {
    if option.get("type").and_then(Value::as_str) == Some("boolean") {
        return Some(Value::Bool(requested));
    }
    resolve_select_value(option, if requested { "true" } else { "false" }).map(Value::String)
}

pub fn option_by_id<'a>(options: &'a [Value], id: &str) -> Option<&'a Value> {
    options.iter().find(|option| {
        option
            .get("id")
            .and_then(Value::as_str)
            .is_some_and(|candidate| candidate == id)
    })
}

fn select_entries(option: &Value) -> Vec<(String, String)> {
    let Some(entries) = option.get("options").and_then(Value::as_array) else {
        return vec![];
    };
    entries
        .iter()
        .flat_map(|entry| {
            if let Some(value) = entry.get("value").and_then(Value::as_str) {
                return vec![(
                    value.to_string(),
                    entry
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or(value)
                        .to_string(),
                )];
            }
            entry
                .get("options")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|nested| {
                    let value = nested.get("value")?.as_str()?;
                    Some((
                        value.to_string(),
                        nested
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or(value)
                            .to_string(),
                    ))
                })
                .collect()
        })
        .collect()
}

fn normalize_effort(value: &str) -> Option<String> {
    let normalized = token(value);
    if normalized.is_empty() {
        return None;
    }
    match normalized.as_str() {
        "low" => Some("low".into()),
        "medium" => Some("medium".into()),
        "high" => Some("high".into()),
        "max" => Some("max".into()),
        "xhigh" | "extra_high" => Some("xhigh".into()),
        // ACP config options are open-ended. Preserve any future Cursor
        // effort identifier so a newly-added level remains selectable
        // without waiting for a Codemux release.
        _ => Some(normalized),
    }
}

fn token(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace([' ', '-'], "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_current_and_future_effort_values() {
        let option = json!({
            "id": "reasoning",
            "type": "select",
            "options": [
                {"value": "none", "name": "None"},
                {"value": "extra-high", "name": "Extra High"},
                {"value": "adaptive", "name": "Adaptive"}
            ]
        });
        assert_eq!(
            resolve_effort_value(&option, "none").as_deref(),
            Some("none")
        );
        assert_eq!(
            resolve_effort_value(&option, "xhigh").as_deref(),
            Some("extra-high")
        );
        assert_eq!(
            resolve_effort_value(&option, "adaptive").as_deref(),
            Some("adaptive")
        );
    }

    #[test]
    fn serializes_boolean_config_options_with_the_acp_type_tag() {
        assert_eq!(
            set_config_params("session-1", "fast", Value::Bool(false)),
            json!({
                "sessionId": "session-1",
                "configId": "fast",
                "type": "boolean",
                "value": false
            })
        );
    }

    #[test]
    fn grok_model_selection_preserves_provider_owned_effort_values() {
        assert_eq!(
            set_model_params("session-1", "grok-future", Some("adaptive-deep")),
            json!({
                "sessionId": "session-1",
                "modelId": "grok-future",
                "_meta": { "reasoningEffort": "adaptive-deep" }
            })
        );
    }

    #[test]
    fn grok_effort_catalog_distinguishes_authoritative_and_unknown_values() {
        let catalog = grok_model_effort_catalog(&json!({
            "_meta": { "modelState": { "availableModels": [{
                "modelId": "grok-live",
                "_meta": {
                    "supportsReasoningEffort": true,
                    "reasoningEfforts": [
                        { "value": "future/deep" },
                        { "id": "fallback-id" }
                    ]
                }
            }, {
                "modelId": "grok-no-reasoning",
                "_meta": { "supportsReasoningEffort": false }
            }, {
                "modelId": "grok-opaque"
            }] } }
        }));
        assert_eq!(
            catalog.get("grok-live").and_then(Option::as_ref),
            Some(&HashSet::from([
                "future/deep".to_string(),
                "fallback-id".to_string()
            ]))
        );
        assert!(catalog
            .get("grok-no-reasoning")
            .and_then(Option::as_ref)
            .is_some_and(HashSet::is_empty));
        assert_eq!(catalog.get("grok-opaque"), Some(&None));
    }

    #[test]
    fn reads_standard_and_xai_model_state_shapes() {
        assert_eq!(
            current_model_id(&json!({ "models": { "currentModelId": "grok-4" } })).as_deref(),
            Some("grok-4")
        );
        assert_eq!(
            current_model_id(&json!({
                "_meta": { "modelState": { "currentModelId": "grok-next" } }
            }))
            .as_deref(),
            Some("grok-next")
        );
    }

    #[test]
    fn grok_advertised_default_wins_over_mixed_credentials() {
        let initialized = json!({
            "authMethods": [
                {"id": "grok.com"},
                {"id": "cached_token"},
                {"id": "xai.api_key"}
            ],
            "_meta": { "defaultAuthMethodId": "cached_token" }
        });
        let env = HashMap::from([("XAI_API_KEY".into(), "secret".into())]);
        assert_eq!(
            grok_auth_method(&initialized, &env).as_deref(),
            Some("cached_token")
        );
    }

    #[test]
    fn grok_legacy_auth_prefers_present_api_key() {
        let initialized = json!({
            "authMethods": [
                {"id": "grok.com"},
                {"id": "cached_token"},
                {"id": "xai.api_key"}
            ]
        });
        let env = HashMap::from([("XAI_API_KEY".into(), "secret".into())]);
        assert_eq!(
            grok_auth_method(&initialized, &env).as_deref(),
            Some("xai.api_key")
        );
    }

    #[test]
    fn grok_legacy_policy_does_not_force_hidden_api_key_auth() {
        let initialized = json!({
            "authMethods": [{"id": "grok.com"}, {"id": "cached_token"}]
        });
        let env = HashMap::from([("XAI_API_KEY".into(), "stale-secret".into())]);
        assert_eq!(
            grok_auth_method(&initialized, &env).as_deref(),
            Some("cached_token")
        );
    }

    #[test]
    fn grok_accepts_future_advertised_non_browser_default() {
        let initialized = json!({
            "authMethods": [
                {"id": "grok.com"},
                {"id": "enterprise_sso_token"}
            ],
            "_meta": { "defaultAuthMethodId": "enterprise_sso_token" }
        });
        assert_eq!(
            grok_auth_method(&initialized, &HashMap::new()).as_deref(),
            Some("enterprise_sso_token")
        );
    }

    #[test]
    fn grok_never_auto_picks_or_bypasses_browser_default() {
        let initialized = json!({
            "authMethods": [
                {"id": "grok.com"},
                {"id": "cached_token"},
                {"id": "xai.api_key"}
            ],
            "_meta": { "defaultAuthMethodId": "grok.com" }
        });
        let env = HashMap::from([("XAI_API_KEY".into(), "secret".into())]);
        assert_eq!(grok_auth_method(&initialized, &env), None);
    }

    #[test]
    fn grok_rejects_unadvertised_policy_default() {
        let initialized = json!({
            "authMethods": [{"id": "cached_token"}],
            "_meta": { "defaultAuthMethodId": "removed_by_policy" }
        });
        assert_eq!(grok_auth_method(&initialized, &HashMap::new()), None);
    }
}
