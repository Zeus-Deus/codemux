use crate::agent_provider::{
    ApprovalDecision, ChatModelInfo, EffortGranularity, PermissionModeOption,
    ProviderChatCapabilities, ProviderSessionId,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpCatalog {
    pub agent_id: String,
    pub agent_name: String,
    pub capabilities: ProviderChatCapabilities,
    pub config_options: Vec<AcpConfigOption>,
    pub current_model: Option<String>,
    pub supports_resume: bool,
    pub supports_images: bool,
    pub auth_methods: Vec<AcpAuthMethod>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpBinding {
    pub thread_id: String,
    pub agent_id: String,
    pub revision: String,
    pub cwd: String,
    pub session_id: Option<String>,
    pub catalog: AcpCatalog,
    pub config_values: HashMap<String, Value>,
}
impl AcpBinding {
    pub(crate) fn reconcile_config_values(&mut self) {
        // The catalog is acknowledged state, not a list of requests to replay.
        self.config_values.retain(|id, value| {
            let Some(option) = self.catalog.config_options.iter().find(|o| &o.id == id) else {
                return false;
            };
            if validate_value(option, &option.current_value).is_err() { return false; }
            *value = option.current_value.clone();
            true
        });
        // A model transition can replace the reasoning control entirely.
        for category in ["model", "thought_level"] {
            if let Some(option) = semantic_option(&self.catalog.config_options, category) {
                if validate_value(option, &option.current_value).is_ok() {
                    self.config_values.insert(option.id.clone(), option.current_value.clone());
                }
            }
        }
    }
}
pub(crate) fn semantic_option<'a>(
    options: &'a [AcpConfigOption],
    category: &str,
) -> Option<&'a AcpConfigOption> {
    // Only inspect metadata; never transform protocol values.
    options.iter().find(|o| {
        o.kind == "select"
            && (o.category.as_deref() == Some(category)
                || match category {
                    "model" => o.id.eq_ignore_ascii_case("model"),
                    "thought_level" => ["effort", "reasoning_effort", "thought_level"]
                        .iter()
                        .any(|id| o.id.eq_ignore_ascii_case(id)),
                    _ => false,
                })
    })
}
pub(crate) fn catalog_from(
    agent_id: &str,
    agent_name: &str,
    caps: &Negotiation,
    response: &Value,
) -> Result<AcpCatalog, String> {
    if !response.is_object() {
        return Err("ACP: session response must be an object".into());
    }
    let config_options = match response.get("configOptions") {
        None | Some(Value::Null) => vec![],
        Some(v) => parse_options(v)?,
    };
    let model = semantic_option(&config_options, "model");
    let effort = semantic_option(&config_options, "thought_level");
    let current_model = model
        .and_then(|o| o.current_value.as_str())
        .map(str::to_owned)
        .or_else(|| {
            response
                .pointer("/models/currentModelId")
                .and_then(Value::as_str)
                .map(str::to_owned)
        });
    let mut available = Vec::new();
    if let Some(model) = model {
        available = model.options.clone();
    } else if let Some(legacy) = response.pointer("/models/availableModels") {
        for value in legacy
            .as_array()
            .ok_or("ACP: availableModels must be an array")?
        {
            available.push(AcpConfigValue {
                value: required(value, "modelId")?,
                name: required(value, "name")?,
                description: text(value, "description").map(str::to_owned),
            });
        }
    }
    if current_model.as_deref().is_some_and(|current| !available.iter().any(|model| model.value == current)) {
        return Err("ACP: current model is not advertised".into());
    }
    let models = available
        .into_iter()
        .map(|model| {
            let selected_effort =
                effort.filter(|_| current_model.as_deref() == Some(model.value.as_str()));
            ChatModelInfo {
                id: model.value,
                label: model.name,
                description: model.description,
                effort_levels: selected_effort
                    .map(|e| e.options.iter().map(|o| o.value.clone()).collect())
                    .unwrap_or_default(),
                default_effort: selected_effort
                    .and_then(|e| e.current_value.as_str())
                    .map(str::to_owned),
                effort_descriptions: selected_effort
                    .map(|e| {
                        e.options
                            .iter()
                            .filter_map(|o| o.description.clone().map(|d| (o.value.clone(), d)))
                            .collect()
                    })
                    .unwrap_or_default(),
                prompt_injected_effort_levels: vec![],
                context_window_options: vec![],
                supports_adaptive_thinking: false,
                supports_thinking_toggle: false,
                supports_fast_mode: false,
                supports_images: caps.images,
                sub_provider: None,
                is_free: false,
                max_context_tokens: None,
            }
        })
        .collect();
    let effort_label_map = effort
        .map(|e| {
            e.options
                .iter()
                .map(|o| (o.value.clone(), o.name.clone()))
                .collect()
        })
        .unwrap_or_default();
    Ok(AcpCatalog { agent_id:agent_id.into(), agent_name:agent_name.into(), config_options, current_model,
        supports_resume:caps.resume || caps.load, supports_images:caps.images, auth_methods:caps.auth_methods.clone(),
        capabilities:ProviderChatCapabilities { supports_steering:false, models, effort_granularity:EffortGranularity::PerTurn, effort_label_map,
            permission_modes:vec![PermissionModeOption { value:"supervised".into(), label:"Supervised".into(), description:"Every ACP permission callback requires your approval; agent modes never bypass host approval.".into(), is_default:true }],
            default_permission_mode:Some("supervised".into()), permission_granularity:EffortGranularity::PerSession },
    })
}
pub(crate) fn validate_value(option: &AcpConfigOption, value: &Value) -> Result<(), String> {
    match option.kind.as_str() {
        "boolean" if value.is_boolean() => Ok(()),
        "select"
            if value
                .as_str()
                .is_some_and(|value| option.options.iter().any(|o| o.value == value)) =>
        {
            Ok(())
        }
        _ => Err(format!(
            "ACP: value is not advertised for config option {}",
            option.id
        )),
    }
}
pub(crate) fn permission_outcome(
    options: &[Value],
    decision: &ApprovalDecision,
) -> Result<Value, String> {
    let select = |kind: &str| {
        options
            .iter()
            .find(|option| text(option, "kind") == Some(kind))
            .and_then(|option| text(option, "optionId"))
    };
    let selected = match decision {
        ApprovalDecision::Cancel => None,
        ApprovalDecision::ProviderOption { option_id } => {
            if !options
                .iter()
                .any(|option| text(option, "optionId") == Some(option_id))
            {
                return Err("ACP: permission option is not advertised by this request".into());
            }
            Some(option_id.as_str())
        }
        ApprovalDecision::Allow {
            updated_input,
            updated_permissions,
        } => {
            if updated_input.is_some() || updated_permissions.is_some() {
                return Err("ACP: permission input/permission rewrites are unsupported".into());
            }
            Some(select("allow_once").ok_or("ACP: request has no one-time approval option")?)
        }
        ApprovalDecision::AllowForSession => {
            return Err("ACP: session-scoped host approval is unsupported; select an exact advertised provider option for its own persistence policy".into());
        }
        ApprovalDecision::Deny { .. } => select("reject_once"),
    };
    Ok(match selected {
        Some(id) => json!({"outcome":{"outcome":"selected","optionId":id}}),
        None => json!({"outcome":{"outcome":"cancelled"}}),
    })
}
pub(crate) fn namespaced_session(agent: &str, revision: &str, native: &str) -> ProviderSessionId {
    ProviderSessionId(json!(["acp", agent, revision, native]).to_string())
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AcpAuthMethod {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    #[serde(rename = "type")]
    pub kind: String,
}
#[derive(Debug, Clone, Default)]
pub(crate) struct Negotiation {
    pub resume: bool,
    pub load: bool,
    pub close: bool,
    pub images: bool,
    pub additional_directories: bool,
    pub auth_methods: Vec<AcpAuthMethod>,
}
impl Negotiation {
    pub fn parse(value: &Value) -> Result<Self, String> {
        if value.get("protocolVersion").and_then(Value::as_u64) != Some(1) {
            return Err("unsupported: agent must negotiate ACP protocolVersion 1".into());
        }
        let capabilities = value
            .get("agentCapabilities")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        if !capabilities.is_object() {
            return Err("ACP: agentCapabilities must be an object".into());
        }
        let boolean = |path: &str| -> Result<bool, String> {
            match capabilities.pointer(path) {
                None | Some(Value::Null) => Ok(false),
                Some(Value::Bool(value)) => Ok(*value),
                _ => Err(format!("ACP: capability {path} must be boolean")),
            }
        };
        let object = |path: &str| -> Result<bool, String> {
            match capabilities.pointer(path) {
                None | Some(Value::Null) => Ok(false),
                Some(Value::Object(_)) => Ok(true),
                _ => Err(format!("ACP: capability {path} must be an object")),
            }
        };
        let mut auth_methods = Vec::new();
        if let Some(auth) = value.get("authMethods") {
            for method in auth.as_array().ok_or("ACP: authMethods must be an array")? {
                let id = required(method, "id")?;
                if auth_methods.iter().any(|m: &AcpAuthMethod| m.id == id) {
                    return Err("ACP: duplicate auth method ID".into());
                }
                auth_methods.push(AcpAuthMethod {
                    id,
                    name: required(method, "name")?,
                    description: text(method, "description").map(str::to_owned),
                    kind: text(method, "type").unwrap_or("agent").into(),
                });
            }
        }
        Ok(Self {
            resume: object("/sessionCapabilities/resume")?,
            load: boolean("/loadSession")?,
            close: object("/sessionCapabilities/close")?,
            images: boolean("/promptCapabilities/image")?,
            additional_directories: object("/sessionCapabilities/additionalDirectories")?,
            auth_methods,
        })
    }
    pub fn resume_method(&self) -> Result<&'static str, String> {
        if self.resume {
            Ok("session/resume")
        } else if self.load {
            Ok("session/load")
        } else {
            Err("unavailable: agent does not advertise session resume or load".into())
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpConfigValue {
    pub value: String,
    pub name: String,
    pub description: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpConfigOption {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub category: Option<String>,
    #[serde(rename = "type")]
    pub kind: String,
    pub current_value: Value,
    pub options: Vec<AcpConfigValue>,
}
pub(crate) fn text<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}
fn required(v: &Value, key: &str) -> Result<String, String> {
    text(v, key)
        .map(str::to_owned)
        .ok_or_else(|| format!("ACP: missing string {key}"))
}
pub(crate) fn parse_options(value: &Value) -> Result<Vec<AcpConfigOption>, String> {
    let entries = value
        .as_array()
        .ok_or("ACP: configOptions must be an array")?;
    let mut result = Vec::new();
    for entry in entries {
        let kind = required(entry, "type")?;
        if !matches!(kind.as_str(), "select" | "boolean") {
            continue;
        }
        let current_value = entry
            .get("currentValue")
            .cloned()
            .ok_or("ACP: missing currentValue")?;
        let mut options = Vec::new();
        if kind == "select" {
            if !current_value.is_string() {
                return Err("ACP: select currentValue must be a string".into());
            }
            for item in entry
                .get("options")
                .and_then(Value::as_array)
                .ok_or("ACP: select options must be an array")?
            {
                let group;
                let values = if let Some(nested) = item.get("options") {
                    nested
                        .as_array()
                        .ok_or("ACP: grouped options must be an array")?
                } else {
                    group = vec![item.clone()];
                    &group
                };
                for item in values {
                    options.push(AcpConfigValue {
                        value: required(item, "value")?,
                        name: required(item, "name")?,
                        description: text(item, "description").map(str::to_owned),
                    });
                }
            }
        } else if !current_value.is_boolean() {
            return Err("ACP: boolean currentValue must be a boolean".into());
        }
        let id = required(entry, "id")?;
        if result.iter().any(|other: &AcpConfigOption| other.id == id) {
            return Err("ACP: duplicate config option ID".into());
        }
        if options.iter().enumerate().any(|(index, item)| {
            options[..index]
                .iter()
                .any(|other| other.value == item.value)
        }) {
            return Err("ACP: duplicate config value ID".into());
        }
        if kind == "select" && !options.iter().any(|option| Some(option.value.as_str()) == current_value.as_str()) {
            return Err("ACP: select currentValue is not advertised".into());
        }
        result.push(AcpConfigOption {
            id,
            name: required(entry, "name")?,
            description: text(entry, "description").map(str::to_owned),
            category: text(entry, "category").map(str::to_owned),
            kind,
            current_value,
            options,
        });
    }
    Ok(result)
}
