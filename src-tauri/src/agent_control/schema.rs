//! The JSON Schema subset the control plane advertises, and the one validator
//! every route runs before a handler.
//!
//! Only the keywords in [`KEYWORDS`] carry meaning; a schema that uses any other
//! keyword fails a test (`vocabulary_errors`) rather than being ignored at
//! runtime. The validator never rewrites arguments, so receipt fingerprints are
//! unaffected; the handlers' typed deserialization stays a second gate.
use serde_json::Value;
use std::fmt;

/// Keywords the validator understands.
#[cfg(test)]
pub const KEYWORDS: &[&str] = &[
    "type", "properties", "required", "additionalProperties", "enum", "minLength", "maxLength",
    "minimum", "maximum", "items", "minItems", "maxItems", "description", "default",
];

#[cfg(test)]
const TYPES: &[&str] = &["object", "array", "string", "integer", "number", "boolean", "null"];

/// The first problem found, at a JSON-pointer-like path ("" is the arguments object).
#[derive(Debug, PartialEq)]
pub struct Violation {
    pub path: String,
    pub problem: Problem,
}

#[derive(Debug, PartialEq)]
pub enum Problem {
    UnknownField(String),
    MissingField(String),
    /// The accepted types, as "string or null".
    WrongType(String),
    /// The allowed values, as "auto, queue, steer".
    NotInEnum(String),
    TooShort(u64),
    TooLong(u64),
    BelowMinimum(f64),
    AboveMaximum(f64),
    TooFewItems(u64),
    TooManyItems(u64),
}

/// Messages read like serde's, so a model can correct the named field and
/// existing "unknown field" expectations keep holding.
impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let at = if self.path.is_empty() { String::new() } else { format!(" at {}", self.path) };
        let subject = if self.path.is_empty() { "the arguments" } else { self.path.as_str() };
        let plural = |n: u64, noun: &str| if n == 1 { noun.to_string() } else { format!("{noun}s") };
        match &self.problem {
            Problem::UnknownField(name) => write!(f, "unknown field `{name}`{at}"),
            Problem::MissingField(name) => write!(f, "missing field `{name}`{at}"),
            Problem::WrongType(expected) => write!(f, "invalid type{at}: expected {expected}"),
            Problem::NotInEnum(allowed) => write!(f, "{subject} must be one of {allowed}"),
            Problem::TooShort(n) => write!(f, "{subject} needs at least {n} {}", plural(*n, "character")),
            Problem::TooLong(n) => write!(f, "{subject} is longer than {n} {}", plural(*n, "character")),
            Problem::BelowMinimum(minimum) => write!(f, "{subject} is below the minimum {minimum}"),
            Problem::AboveMaximum(maximum) => write!(f, "{subject} is above the maximum {maximum}"),
            Problem::TooFewItems(n) => write!(f, "{subject} needs at least {n} {}", plural(*n, "item")),
            Problem::TooManyItems(n) => write!(f, "{subject} allows at most {n} {}", plural(*n, "item")),
        }
    }
}

/// Validates `value` against `schema`.
///
/// - No `type` accepts any value. `integer` matches what serde accepts for
///   integer fields (`1.5` and `20.0` are not integers); `number` is any number.
/// - An optional property whose value is `null` counts as absent, like serde's
///   `Option<T>`. A required property must match its own schema, so `null`
///   passes only where the type allows it.
/// - String lengths count characters.
pub fn check(schema: &Value, value: &Value) -> Result<(), Violation> {
    check_at(schema, value, String::new())
}

fn check_at(schema: &Value, value: &Value, path: String) -> Result<(), Violation> {
    let fail = |problem| Err(Violation { path: path.clone(), problem });
    let types: Vec<&str> = match &schema["type"] {
        Value::String(name) => vec![name.as_str()],
        Value::Array(names) => names.iter().filter_map(Value::as_str).collect(),
        _ => vec![],
    };
    if schema.get("type").is_some() && !types.iter().any(|name| type_matches(name, value)) {
        return fail(Problem::WrongType(types.join(" or ")));
    }
    if let Some(allowed) = schema.get("enum").and_then(Value::as_array) {
        if !allowed.contains(value) {
            let listed: Vec<String> = allowed.iter().map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_string)).collect();
            return fail(Problem::NotInEnum(listed.join(", ")));
        }
    }
    match value {
        Value::Object(object) => {
            let properties = schema.get("properties").and_then(Value::as_object);
            let required: Vec<&str> = schema.get("required").and_then(Value::as_array)
                .map(|names| names.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
            if schema["additionalProperties"] == false {
                if let Some(unknown) = object.keys().find(|key| properties.is_none_or(|p| !p.contains_key(*key))) {
                    return fail(Problem::UnknownField(unknown.clone()));
                }
            }
            if let Some(missing) = required.iter().find(|name| !object.contains_key(**name)) {
                return fail(Problem::MissingField(missing.to_string()));
            }
            for (key, field) in object {
                let Some(field_schema) = properties.and_then(|p| p.get(key)) else { continue };
                if field.is_null() && !required.contains(&key.as_str()) {
                    continue;
                }
                check_at(field_schema, field, format!("{path}/{key}"))?;
            }
        }
        Value::Array(items) => {
            let count = items.len() as u64;
            if let Some(minimum) = schema["minItems"].as_u64().filter(|minimum| count < *minimum) {
                return fail(Problem::TooFewItems(minimum));
            }
            if let Some(maximum) = schema["maxItems"].as_u64().filter(|maximum| count > *maximum) {
                return fail(Problem::TooManyItems(maximum));
            }
            if let Some(item_schema) = schema.get("items") {
                for (index, item) in items.iter().enumerate() {
                    check_at(item_schema, item, format!("{path}/{index}"))?;
                }
            }
        }
        Value::String(text) => {
            let length = text.chars().count() as u64;
            if let Some(minimum) = schema["minLength"].as_u64().filter(|minimum| length < *minimum) {
                return fail(Problem::TooShort(minimum));
            }
            if let Some(maximum) = schema["maxLength"].as_u64().filter(|maximum| length > *maximum) {
                return fail(Problem::TooLong(maximum));
            }
        }
        Value::Number(number) => {
            let number = number.as_f64().unwrap_or(f64::NAN);
            if let Some(minimum) = schema["minimum"].as_f64().filter(|minimum| number < *minimum) {
                return fail(Problem::BelowMinimum(minimum));
            }
            if let Some(maximum) = schema["maximum"].as_f64().filter(|maximum| number > *maximum) {
                return fail(Problem::AboveMaximum(maximum));
            }
        }
        Value::Bool(_) | Value::Null => {}
    }
    Ok(())
}

fn type_matches(name: &str, value: &Value) -> bool {
    match name {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        _ => false,
    }
}

/// Test helper: what in `schema` the validator would not understand. Unknown
/// keywords or type names, `required` names missing from `properties`, and a
/// non-boolean `additionalProperties`.
#[cfg(test)]
pub fn vocabulary_errors(schema: &Value) -> Vec<String> {
    let mut errors = vec![];
    vocabulary_at(schema, "", &mut errors);
    errors
}

#[cfg(test)]
fn vocabulary_at(schema: &Value, path: &str, errors: &mut Vec<String>) {
    let Some(object) = schema.as_object() else {
        errors.push(format!("{path}: a schema must be an object"));
        return;
    };
    for key in object.keys().filter(|key| !KEYWORDS.contains(&key.as_str())) {
        errors.push(format!("{path}: unsupported keyword `{key}`"));
    }
    let types: Vec<&Value> = match &schema["type"] {
        Value::Array(names) => names.iter().collect(),
        Value::Null => vec![],
        name => vec![name],
    };
    for name in types.into_iter().filter(|name| !name.as_str().is_some_and(|name| TYPES.contains(&name))) {
        errors.push(format!("{path}: unsupported type {name}"));
    }
    if schema.get("additionalProperties").is_some_and(|value| !value.is_boolean()) {
        errors.push(format!("{path}: additionalProperties must be a boolean"));
    }
    let properties = schema.get("properties").and_then(Value::as_object);
    for name in schema.get("required").and_then(Value::as_array).into_iter().flatten() {
        if !name.as_str().is_some_and(|name| properties.is_some_and(|p| p.contains_key(name))) {
            errors.push(format!("{path}: required {name} is not a property"));
        }
    }
    for (name, property) in properties.into_iter().flatten() {
        vocabulary_at(property, &format!("{path}/{name}"), errors);
    }
    if let Some(items) = schema.get("items") {
        vocabulary_at(items, &format!("{path}/items"), errors);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn message(schema: &Value, value: Value) -> String {
        check(schema, &value).unwrap_err().to_string()
    }

    #[test]
    fn schema_rejects_unknown_args_wrong_types_and_nested_authority() {
        let s = json!({"type":"object","additionalProperties":false,"required":["target"],"properties":{"target":{"type":"string","minLength":1},"timeout":{"type":"integer","minimum":1,"maximum":20},"nested":{"type":"object","additionalProperties":false,"properties":{"branch":{"type":"string"}}}}});
        assert!(check(&s, &json!({"target":"known","timeout":10})).is_ok());
        for v in [
            json!({}),
            json!({"target":""}),
            json!({"target":"known","access":"full_access"}),
            json!({"target":"known","timeout":1.5}),
            json!({"target":"known","timeout":21}),
            json!({"target":"known","nested":{"env":{"API_KEY":"no"}}}),
        ] {
            assert!(check(&s, &v).is_err());
        }
    }

    #[test]
    fn schema_accepts_numbers_untyped_values_and_null_optionals() {
        assert!(check(&json!({"type":"number"}), &json!(1.5)).is_ok());
        for fraction in [json!(1.5), json!(20.0)] {
            assert!(check(&json!({"type":"integer"}), &fraction).is_err(), "{fraction}");
        }
        for anything in [json!({"a":1}), json!("text"), json!(null), json!([1])] {
            assert!(check(&json!({}), &anything).is_ok(), "{anything}");
        }
        let s = json!({"type":"object","additionalProperties":false,"required":["mode"],
            "properties":{"cursor":{"type":"integer"},"mode":{"type":["string","null"]},"id":{"type":"string"}}});
        assert!(check(&s, &json!({"mode":"plan","cursor":null})).is_ok(), "optional null is absent");
        assert!(check(&s, &json!({"mode":null})).is_ok(), "required null where the type allows it");
        let required = json!({"type":"object","required":["id"],"properties":{"id":{"type":"string"}}});
        assert_eq!(message(&required, json!({"id":null})), "invalid type at /id: expected string");
    }

    #[test]
    fn schema_checks_array_items_and_bounds() {
        let s = json!({"type":"array","minItems":1,"maxItems":2,"items":{"type":"object"}});
        assert!(check(&s, &json!([{}])).is_ok());
        assert_eq!(check(&s, &json!([])).unwrap_err().problem, Problem::TooFewItems(1));
        assert_eq!(check(&s, &json!([{}, {}, {}])).unwrap_err().problem, Problem::TooManyItems(2));
        assert_eq!(check(&s, &json!([{}, "x"])).unwrap_err(), Violation { path: "/1".into(), problem: Problem::WrongType("object".into()) });
    }

    #[test]
    fn schema_errors_read_like_serde() {
        let s = json!({"type":"object","additionalProperties":false,"required":["thread_id"],"properties":{
            "thread_id":{"type":"string"},"timeout_ms":{"type":"integer","maximum":600000},
            "delivery":{"type":"string","enum":["auto","queue","steer"]},"message":{"type":"string","maxLength":65536},
            "threads":{"type":"array","minItems":1}}});
        assert_eq!(message(&s, json!({"thread_id":"t","caller":{}})), "unknown field `caller`");
        assert_eq!(message(&s, json!({})), "missing field `thread_id`");
        assert_eq!(message(&s, json!({"thread_id":"t","timeout_ms":"1"})), "invalid type at /timeout_ms: expected integer");
        assert_eq!(message(&s, json!({"thread_id":"t","timeout_ms":600001})), "/timeout_ms is above the maximum 600000");
        assert_eq!(message(&s, json!({"thread_id":"t","delivery":"later"})), "/delivery must be one of auto, queue, steer");
        assert_eq!(message(&s, json!({"thread_id":"t","message":"x".repeat(65537)})), "/message is longer than 65536 characters");
        assert_eq!(message(&s, json!({"thread_id":"t","threads":[]})), "/threads needs at least 1 item");
        assert_eq!(message(&s, json!([])), "invalid type: expected object");
    }

    #[test]
    fn vocabulary_errors_name_what_the_validator_ignores() {
        assert!(vocabulary_errors(&json!({"type":"object","properties":{"a":{"type":["string","null"],"description":"x"}},"required":["a"],"additionalProperties":false})).is_empty());
        let errors = vocabulary_errors(&json!({"type":"object","properties":{"a":{"type":"string","format":"uri"},"b":{"type":"float"}},"required":["c"],"additionalProperties":{}}));
        assert_eq!(errors.len(), 4, "{errors:?}");
    }
}
