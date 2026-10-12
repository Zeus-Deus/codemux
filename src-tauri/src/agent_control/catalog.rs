//! The native control contract shared by stdio and HTTP MCP.
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
    pub annotations: Value,
}

pub const INSTRUCTIONS: &str = "Control CodeMux-owned native threads, not the outside client's conversation. Discover workspaces, then providers/models. Use explicit workspace and thread IDs. Select a provider-advertised permission mode; omission is an error. Mutations require a stable client_request_id: retry the same request unchanged. An operation accepted or a message queued is NOT completed work. Read operation_status, then thread_status/thread_read or a bounded thread_wait. Never blindly resend an operation marked needs_attention. Supervised clients leave worker approvals to the human in CodeMux.";

fn tool(
    name: &'static str,
    description: &'static str,
    properties: Value,
    required: &[&str],
    read_only: bool,
) -> ToolDefinition {
    ToolDefinition {
        name,
        description,
        input_schema: json!({
            "type": "object", "properties": properties, "required": required,
            "additionalProperties": false,
        }),
        annotations: json!({"readOnlyHint": read_only, "openWorldHint": false}),
    }
}

fn target() -> Value {
    json!({
        "workspace_id": {"type": "string", "minLength": 1},
        "thread_id": {"type": "string", "minLength": 1},
    })
}

fn mutation() -> Value {
    let mut properties = target();
    properties["client_request_id"] = json!({"type": "string", "minLength": 1, "maxLength": 128});
    properties
}

pub fn native_tools() -> Vec<ToolDefinition> {
    let mut send = mutation();
    send["message"] = json!({"type": "string", "minLength": 1, "maxLength": 65536});
    send["delivery"] = json!({"type": "string", "enum": ["queue", "steer", "interrupt"], "default": "queue"});
    let mut read = target();
    read["cursor"] = json!({"type": "integer", "minimum": 0});
    read["limit"] = json!({"type": "integer", "minimum": 1, "maximum": 100, "default": 20});
    let mut wait = target();
    wait["turn_id"] = json!({"type": "string", "minLength": 1});
    wait["timeout_ms"] = json!({"type": "integer", "minimum": 1, "maximum": 20000, "default": 1000});
    let mut interrupt = mutation();
    interrupt["turn_id"] = json!({"type": "string", "minLength": 1});
    let mut respond = mutation();
    respond["request_id"] = json!({"type": "string", "minLength": 1});
    respond["decision"] = json!({"type": "string", "enum": ["allow_once", "allow_always", "deny"]});
    vec![
        tool("agent_capabilities", "Discover provider-owned model, effort, permission and operation capabilities for a workspace. Registered/configured is not launch readiness: inspect discovery_error. Hermes launch requires an existing native profile binding and is unavailable through this launch API. Values are opaque; do not invent model IDs or permission defaults.", json!({
            "workspace_id": {"type": "string", "minLength": 1},
            "provider": {"type": "string"},
        }), &["workspace_id"], true),
        tool("thread_list", "List native threads in the explicitly selected workspace, including threads whose provider has not assigned a resume ID. Does not start or resume providers.", json!({
            "workspace_id": {"type": "string", "minLength": 1},
            "cursor": {"type": "string"},
            "limit": {"type": "integer", "minimum": 1, "maximum": 100, "default": 20},
        }), &["workspace_id"], true),
        tool("thread_launch", "Launch a CodeMux-owned native thread in an existing workspace. Creating worktrees and selecting Hermes profiles are unavailable through this API. Returns an operation receipt; use operation_status. Explicit permission_mode is mandatory (null only for providers without a mode). No raw environment or filesystem attachment inputs.", json!({
            "workspace_id": {"type": "string", "minLength": 1},
            "client_request_id": {"type": "string", "minLength": 1, "maxLength": 128},
            "provider": {"type": "string"},
            "model": {"type": ["string", "null"]},
            "permission_mode": {"type": ["string", "null"]},
            "effort": {"type": ["string", "null"]},
            "context_window": {"type": ["string", "null"]},
            "fast_mode": {"type": "boolean", "default": false},
            "title": {"type": "string", "minLength": 1, "maxLength": 100},
            "message": {"type": "string", "minLength": 1, "maxLength": 65536},
            "select": {"type": "boolean", "default": false},
        }), &["workspace_id", "client_request_id", "provider", "permission_mode"], false),
        tool("thread_send", "Send a follow-up to a native thread through CodeMux's normal intake. Delivery is queue, safe steer, or explicit interrupt-and-send. A queued receipt is not completion. Reuse client_request_id unchanged on retry.", send, &["workspace_id", "thread_id", "client_request_id", "message"], false),
        tool("thread_read", "Read cursor-addressable safe-visible user and top-level assistant prose. Hidden reasoning, raw tool payloads and subagent internals are excluded. Does not resume a provider.", read, &["workspace_id", "thread_id"], true),
        tool("thread_status", "Read live/terminal/unknown status and current pending approval IDs. A missing runtime after restart is not proof of completion. Does not resume providers.", target(), &["workspace_id", "thread_id"], true),
        tool("thread_wait", "Wait at most 20 seconds for the selected native run to settle. A timeout does not cancel work; read status or wait again. Specify turn_id to avoid following a newer run.", wait, &["workspace_id", "thread_id"], true),
        tool("thread_interrupt", "Interrupt the selected native turn without deleting its thread, pane, history or worktree. Use turn_id when available to avoid interrupting a newer run.", interrupt, &["workspace_id", "thread_id", "client_request_id"], false),
        tool("thread_stop", "Stop the native provider session and queued work, preserving the pane, history and files. Stopping is not closing or deleting a workspace.", mutation(), &["workspace_id", "thread_id", "client_request_id"], false),
        tool("thread_respond", "Deliver an exact decision to a currently live provider approval. Stale callbacks are errors, not successful approvals. Supervised outside clients may not approve their own worker requests.", respond, &["workspace_id", "thread_id", "client_request_id", "request_id", "decision"], false),
        tool("operation_status", "Read an admitted mutation's durable receipt, scoped to its calling client. Accepted/running is not completion. needs_attention is an uncertain prior dispatch: inspect its thread instead of resending.", json!({
            "operation_id": {"type": "string", "minLength": 1},
        }), &["operation_id"], true),
    ]
}

#[cfg(test)]
mod review_tests {
    use super::*;
    #[test]
    fn review_n1_launch_catalog_does_not_promise_unsupported_worktrees() {
        let tools = native_tools();
        let launch = tools.iter().find(|tool| tool.name == "thread_launch").unwrap();
        assert!(launch.input_schema["properties"].get("worktree").is_none(),
            "a schema field promises support; worktree launch is deliberately unavailable");
        assert!(!launch.description.contains("new worktree"));
    }
}

pub fn remote_tools() -> Vec<ToolDefinition> {
    let mut tools = vec![tool("workspace_list", "List workspaces on this CodeMux instance. Use the returned workspace IDs explicitly; outside clients have no current pane or calling thread.", json!({}), &[], true)];
    tools.extend(native_tools());
    tools
}

pub fn is_native_tool(name: &str) -> bool {
    native_tools().iter().any(|tool| tool.name == name)
}

pub fn is_read_only(name: &str) -> bool {
    matches!(name, "workspace_list" | "agent_capabilities" | "thread_list" | "thread_read" | "thread_status" | "thread_wait" | "operation_status")
}
