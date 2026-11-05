//! `codemux tools` lists the registry tools this caller may run, live from
//! the app; `codemux call` runs one of them with JSON arguments.

use super::args;
use super::door::{code, Door};
use super::output::{self, CallEnvelope, CliError, ErrorBody, Exit};
use serde::Serialize;
use serde_json::{json, Value};

/// What `codemux tools --json` prints. `door` is always `local` (the
/// control socket); `caller` is who the app says is calling.
#[derive(Serialize)]
struct ToolsEnvelope {
    ok: bool,
    door: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    caller: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ErrorBody>,
}

pub(crate) async fn run_tools(door: &Door, json: bool, markdown: bool) -> Result<Exit, String> {
    let listed = match door.list().await {
        Ok(listed) => listed,
        Err(mut failure) => {
            if failure.error.code == code::UNAVAILABLE {
                failure.error.message.push_str(" `codemux capabilities` lists the tools this build ships.");
            }
            if json {
                output::emit_json(&ToolsEnvelope { ok: false, door: "local", caller: None, tools: None, error: Some(failure.error.clone()) })?;
            } else {
                output::note(&failure.to_string());
            }
            return Ok(failure.exit);
        }
    };
    if json {
        output::emit_json(&ToolsEnvelope { ok: true, door: "local", caller: Some(listed.caller), tools: Some(listed.tools), error: None })?;
    } else if markdown {
        output::out(&render_tools_markdown(&listed.tools, "Codemux control tools"))?;
    } else {
        let mut rows = vec![vec!["NAME".to_owned(), "ACCESS".to_owned(), "SUMMARY".to_owned()]];
        for tool in &listed.tools {
            rows.push(vec![name(tool).to_owned(), access(tool), summary(tool["description"].as_str().unwrap_or_default())]);
        }
        output::table(&rows)?;
    }
    Ok(Exit::Done)
}

fn name(tool: &Value) -> &str {
    tool["name"].as_str().unwrap_or("?")
}

/// The least access that runs the tool: what the app declares, else what
/// this build's registry says, else the read-only hint.
fn access(tool: &Value) -> String {
    if let Some(declared) = tool["_meta"]["codemux/access"].as_str() {
        return declared.to_owned();
    }
    if let Some((_, spec)) = crate::agent_control::registry::lookup(name(tool)) {
        return args::access_label(spec.access().minimum()).to_owned();
    }
    match tool["annotations"]["readOnlyHint"].as_bool() {
        Some(true) => "read-only".to_owned(),
        _ => "?".to_owned(),
    }
}

/// The first sentence, at most 80 characters.
fn summary(description: &str) -> String {
    let sentence = description.find(". ").map_or(description, |end| &description[..=end]).trim();
    if sentence.chars().count() <= 80 {
        return sentence.to_owned();
    }
    format!("{}…", sentence.chars().take(79).collect::<String>().trim_end())
}

/// A Markdown reference for MCP tool definitions: one section per tool with
/// its description and an argument table from its input schema.
pub(crate) fn render_tools_markdown(tools: &[Value], heading: &str) -> String {
    let mut text = format!("# {heading}\n");
    for tool in tools {
        text.push_str(&format!("\n## {}\n\n{}\n", name(tool), tool["description"].as_str().unwrap_or_default().trim()));
        let schema = &tool["inputSchema"];
        let Some(properties) = schema["properties"].as_object().filter(|properties| !properties.is_empty()) else {
            text.push_str("\nNo arguments.\n");
            continue;
        };
        let required: Vec<&str> = schema["required"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        text.push_str("\n| Argument | Type | Required | Notes |\n|---|---|---|---|\n");
        for (argument, property) in properties {
            let required = if required.contains(&argument.as_str()) { "yes" } else { "no" };
            text.push_str(&format!("| `{argument}` | {} | {required} | {} |\n", type_name(property), notes(property)));
        }
    }
    text
}

fn type_name(property: &Value) -> String {
    match &property["type"] {
        Value::String(name) => name.clone(),
        Value::Array(names) => names.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" or "),
        _ => "any".to_owned(),
    }
}

fn notes(property: &Value) -> String {
    let mut notes = Vec::new();
    if let Some(values) = property["enum"].as_array() {
        let values: Vec<String> = values.iter().map(|value| format!("`{}`", value.as_str().map_or_else(|| value.to_string(), str::to_owned))).collect();
        notes.push(format!("one of {}", values.join(", ")));
    }
    if let Some(default) = property.get("default") {
        notes.push(format!("default `{}`", default.as_str().map_or_else(|| default.to_string(), str::to_owned)));
    }
    for (key, label) in [("minimum", "min"), ("maximum", "max"), ("minLength", "min length"), ("maxLength", "max length")] {
        if let Some(limit) = property.get(key) {
            notes.push(format!("{label} {limit}"));
        }
    }
    if let Some(description) = property["description"].as_str() {
        notes.push(description.to_owned());
    }
    notes.join("; ").replace('|', "\\|")
}

/// `codemux call <tool> [JSON|-]`: exactly what the tool returned, as the
/// call envelope. Receipts are not settled here.
pub(crate) async fn run_call(door: &Door, tool: &str, arguments: Option<&str>) -> Result<Exit, String> {
    let outcome = match call_arguments(arguments) {
        Ok(arguments) => door.call(tool, arguments).await.map_err(|failure| failure.error),
        Err(failure) => Err(failure.error),
    };
    let envelope = CallEnvelope::of(tool, outcome);
    output::emit_json(&envelope)?;
    Ok(envelope.exit())
}

/// A JSON object inline, `-` for stdin, or nothing for `{}`.
fn call_arguments(raw: Option<&str>) -> Result<Value, CliError> {
    let Some(raw) = raw else { return Ok(json!({})) };
    let text = args::inline_or_stdin(raw)?;
    match serde_json::from_str::<Value>(&text) {
        Ok(value) if value.is_object() => Ok(value),
        Ok(_) => Err(CliError::failed(code::INVALID_ARGUMENTS, "Arguments must be a JSON object")),
        Err(error) => Err(CliError::failed(code::INVALID_ARGUMENTS, format!("Arguments are not valid JSON: {error}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_control::{registry, CallerView, ControlAccess};

    fn outside_definitions() -> Vec<Value> {
        registry::list(&CallerView::outside(ControlAccess::FullAccess))
            .into_iter()
            .map(|tool| serde_json::to_value(tool.definition).unwrap())
            .collect()
    }

    #[test]
    fn markdown_reference_lists_schema_arguments() {
        let send: Vec<Value> = outside_definitions().into_iter().filter(|tool| tool["name"] == "thread_send").collect();
        let text = render_tools_markdown(&send, "Tools");
        assert!(text.starts_with("# Tools\n\n## thread_send\n\n"), "{text}");
        assert!(text.contains("| `message` | string | yes | min length 1; max length 65536 |"), "{text}");
        assert!(text.contains("| `delivery` | string | no | one of `auto`, `queue`, `steer`, `interrupt`; default `auto` |"), "{text}");

        let list = render_tools_markdown(&[json!({"name": "workspace_list", "description": "List.", "inputSchema": {"type": "object", "properties": {}}})], "T");
        assert!(list.ends_with("## workspace_list\n\nList.\n\nNo arguments.\n"), "{list}");
    }

    #[test]
    fn the_text_table_names_access_and_a_short_summary() {
        let tools = outside_definitions();
        let launch = tools.iter().find(|tool| tool["name"] == "thread_launch").unwrap();
        assert_eq!(access(launch), "supervised");
        let list = tools.iter().find(|tool| tool["name"] == "thread_list").unwrap();
        assert_eq!(access(list), "read-only");
        // A newer app's tool this build does not know falls back to its hints.
        assert_eq!(access(&json!({"name": "later_tool", "annotations": {"readOnlyHint": true}})), "read-only");
        assert_eq!(access(&json!({"name": "later_tool", "_meta": {"codemux/access": "full-access"}})), "full-access");

        assert_eq!(summary("Wait for a run. Then more."), "Wait for a run.");
        let long = summary(&"word ".repeat(40));
        assert!(long.chars().count() <= 80 && long.ends_with('…'), "{long}");
    }

    #[test]
    fn call_arguments_must_be_a_json_object() {
        assert_eq!(call_arguments(None).unwrap(), json!({}));
        assert_eq!(call_arguments(Some(r#"{"workspace_id":"ws"}"#)).unwrap(), json!({"workspace_id": "ws"}));
        for bad in ["[1]", "\"x\"", "{nope"] {
            assert_eq!(call_arguments(Some(bad)).unwrap_err().error.code, code::INVALID_ARGUMENTS, "{bad}");
        }
    }

    #[tokio::test]
    async fn call_prints_exactly_what_the_tool_returned() {
        let door = Door::fake(vec![("thread_read", Err(CliError::failed("workspace_not_found", "No such workspace")))]);
        assert_eq!(run_call(&door, "thread_read", Some(r#"{"workspace_id":"nope","thread_id":"x"}"#)).await.unwrap(), Exit::Failed);
        // A receipt comes back as returned: `call` never settles it.
        let door = Door::fake(vec![("thread_launch", Ok(json!({"operation_id": "op", "state": "accepted"})))]);
        assert_eq!(run_call(&door, "thread_launch", Some("{}")).await.unwrap(), Exit::Done);
        assert_eq!(door.script().names(), ["thread_launch"]);
    }
}
