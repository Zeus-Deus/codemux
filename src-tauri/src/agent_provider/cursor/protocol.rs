//! Small, forward-compatible helpers around ACP and Cursor extension values.
//!
//! ACP config options are intentionally open-ended.  Keeping them as JSON at
//! the protocol boundary means a newer Cursor CLI can add an option without
//! making this adapter fail to decode the entire session response.

use serde_json::{json, Value};

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
}
