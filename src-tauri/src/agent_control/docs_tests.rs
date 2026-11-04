//! Tests that keep the outside-agent guide in step with the registry: its tool
//! reference, the example tool calls in its flow, its wait states and its
//! error codes.
use super::outcome::RunState;
use super::{registry, schema, Audience, CallerView, ControlAccess, ErrorCode};
use serde_json::Value;
use std::collections::BTreeSet;

const GUIDE: &str = include_str!("../../../docs/outside-agents.md");

/// The words the guide and Settings use for each access level.
fn label(access: ControlAccess) -> &'static str {
    match access {
        ControlAccess::ReadOnly => "Read-only",
        ControlAccess::Supervised => "Supervised",
        ControlAccess::FullAccess => "Full access",
    }
}

/// The lines between `<!-- {name}:start -->` and `<!-- {name}:end -->`.
fn block(name: &str) -> Vec<&'static str> {
    let (start, end) = (format!("<!-- {name}:start -->"), format!("<!-- {name}:end -->"));
    let lines: Vec<&str> = GUIDE.lines().collect();
    let from = lines.iter().position(|line| line.trim() == start).unwrap_or_else(|| panic!("the guide has {start}"));
    let to = lines.iter().position(|line| line.trim() == end).unwrap_or_else(|| panic!("the guide has {end}"));
    assert!(from < to, "{start} comes before {end}");
    lines[from + 1..to].to_vec()
}

/// A Markdown table's body rows, as trimmed cells.
fn rows(lines: &[&str]) -> Vec<Vec<String>> {
    lines
        .iter()
        .map(|line| line.trim())
        .filter(|line| line.starts_with('|'))
        .map(|line| line.trim_matches('|').split('|').map(|cell| cell.trim().to_owned()).collect::<Vec<_>>())
        // The header row and the `|---|` divider.
        .skip(2)
        .collect()
}

/// A cell that is exactly one backticked name.
fn code_cell(cell: &str) -> &str {
    cell.strip_prefix('`')
        .and_then(|rest| rest.strip_suffix('`'))
        .filter(|name| !name.is_empty() && !name.contains('`'))
        .unwrap_or_else(|| panic!("`{cell}` should be one backticked name"))
}

#[test]
fn outside_agents_guide_lists_only_registered_tools() {
    assert!(!GUIDE.contains("⟨verify"), "the guide still has a ⟨verify⟩ marker");
    let registered = registry::outside_tool_index();
    let mut documented = BTreeSet::new();
    for row in rows(&block("tool-reference")) {
        let [name, what, needs] = row.as_slice() else { panic!("a tool reference row has three cells: {row:?}") };
        let name = code_cell(name);
        let (tool, minimum, _) = registered
            .iter()
            .find(|(tool, _, _)| *tool == name)
            .unwrap_or_else(|| panic!("the guide documents `{name}`, which is not an outside tool"));
        assert!(!what.is_empty(), "`{name}` says what it does");
        assert!(needs.starts_with(label(*minimum)), "`{name}` needs {:?} at least, but the guide says {needs:?}", label(*minimum));
        assert!(documented.insert(*tool), "`{name}` is documented once");
    }
    let missing: Vec<&str> = registered.iter().map(|(tool, _, _)| *tool).filter(|tool| !documented.contains(tool)).collect();
    assert!(missing.is_empty(), "document every outside tool in the guide's tool reference: {missing:?}");
}

#[test]
fn outside_agents_guide_counts_the_stdio_tools() {
    // What outside `codemux mcp` lists at full access: the legacy tools no
    // registry tool replaces, then the registry's outside catalog.
    // `mcp_server`'s `tool_registry_has_all_tools` checks that `tools/list`
    // serves exactly this many.
    let kept = crate::mcp_server::LEGACY_TOOL_NAMES
        .iter()
        .filter(|name| registry::served(name, Audience::Outside).is_none())
        .count();
    let count = kept + registry::list(&CallerView::outside(ControlAccess::FullAccess)).len();
    let claim = format!("`codemux mcp` lists {count} tools at full access");
    assert!(GUIDE.contains(&claim), "the guide's release notes say {claim:?}");
}

#[test]
fn outside_agents_guide_names_the_mcp_versions_codemux_speaks() {
    use crate::mcp::protocol::{MODERN_ERA_ENABLED, MODERN_PROTOCOL_VERSIONS, SUPPORTED_PROTOCOL_VERSIONS};
    for version in SUPPORTED_PROTOCOL_VERSIONS {
        assert!(GUIDE.contains(version), "the guide names MCP {version}");
    }
    // Switched off, the stateless era is gone and server/discover is unknown
    // again, so the guide must stop offering it.
    assert_eq!(GUIDE.contains("server/discover"), MODERN_ERA_ENABLED, "the guide offers server/discover only while the stateless era is on");
    for version in MODERN_PROTOCOL_VERSIONS {
        assert_eq!(GUIDE.contains(version), MODERN_ERA_ENABLED, "the guide names MCP {version} only while the stateless era is on");
    }
}

/// One example call from a `text` fence: `tool {json arguments}`, the JSON on
/// that line or spread over the following ones.
struct ExampleCall {
    tool: String,
    arguments: Value,
}

fn example_calls() -> Vec<ExampleCall> {
    let mut calls = Vec::new();
    let mut in_text = false;
    let mut open: Option<(String, String)> = None;
    for line in GUIDE.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            assert!(open.is_none(), "an example call ends before its JSON closes: {open:?}");
            in_text = !in_text && trimmed == "```text";
            continue;
        }
        if !in_text {
            continue;
        }
        if let Some((tool, json)) = open.as_mut() {
            json.push('\n');
            json.push_str(trimmed);
            if let Ok(arguments) = serde_json::from_str(json) {
                calls.push(ExampleCall { tool: std::mem::take(tool), arguments });
                open = None;
            }
            continue;
        }
        // A call starts with a tool-like name and its JSON object.
        let Some((name, rest)) = trimmed.split_once(' ') else { continue };
        if !rest.starts_with('{') || !name.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
            continue;
        }
        match serde_json::from_str(rest) {
            Ok(arguments) => calls.push(ExampleCall { tool: name.to_owned(), arguments }),
            Err(_) => open = Some((name.to_owned(), rest.to_owned())),
        }
    }
    assert!(open.is_none(), "an example call never closes its JSON: {open:?}");
    calls
}

#[test]
fn outside_agents_guide_tool_calls_match_the_schemas() {
    let outside = registry::list(&CallerView::outside(ControlAccess::FullAccess));
    let calls = example_calls();
    for call in &calls {
        let tool = outside
            .iter()
            .find(|tool| tool.spec.name() == call.tool)
            .unwrap_or_else(|| panic!("the guide calls `{}`, which is not an outside tool", call.tool));
        schema::check(&tool.definition.input_schema, &call.arguments)
            .unwrap_or_else(|violation| panic!("the guide's `{}` example {} is refused: {violation}", call.tool, call.arguments));
    }
    // The flow walks through the whole loop, so these must stay in it.
    let shown: BTreeSet<&str> = calls.iter().map(|call| call.tool.as_str()).collect();
    for tool in [
        "workspace_list", "workspace_create", "agent_capabilities", "thread_launch", "operation_status", "thread_wait",
        "thread_respond", "thread_answer", "thread_read", "thread_send", "workspace_close",
    ] {
        assert!(shown.contains(tool), "the guide's flow shows a `{tool}` call");
    }
}

#[test]
fn outside_agents_guide_names_real_wait_states_and_error_codes() {
    // A new state stops this match compiling; add it to `RunState::ALL`, and
    // the guide's table must then explain it.
    for state in RunState::ALL {
        match state {
            RunState::Running | RunState::NeedsInput | RunState::Done | RunState::Failed => {}
        }
    }
    let expected: BTreeSet<String> = RunState::ALL
        .iter()
        .map(|state| serde_json::to_value(state).unwrap().as_str().unwrap().to_owned())
        .collect();
    let documented: BTreeSet<String> = rows(&block("wait-states")).iter().map(|row| code_cell(&row[0]).to_owned()).collect();
    assert_eq!(documented, expected, "the guide's wait states are thread_wait's states");

    let codes = rows(&block("error-codes"));
    assert!(!codes.is_empty(), "the guide lists error codes");
    for row in codes {
        let [code, means, retry] = row.as_slice() else { panic!("an error code row has three cells: {row:?}") };
        let parsed = ErrorCode::parse(code_cell(code)).unwrap_or_else(|| panic!("the guide lists {code}, which is not an error code"));
        assert!(!means.is_empty(), "{code} says what it means");
        assert!(matches!(retry.as_str(), "Yes" | "No"), "{code}: Retry is Yes or No");
        assert_eq!(retry == "Yes", parsed.retryable(), "{code}: the Retry column says {retry:?}");
    }
}
