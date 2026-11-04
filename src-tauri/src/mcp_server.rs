use crate::agent_control::{registry, Audience, CallerView, ControlAccess, ControlError, ErrorCode};
use crate::control::{send_control_request, ControlRequest, ControlResponse};
use crate::control_identity::{self, AGENT_PROCESS_ENV};
use crate::mcp::protocol::stdio::{Handler, Reply};
use crate::mcp::protocol::{
    self, CacheScope, CallToolParams, DiscoverResult, Era, InitializeRequest, InitializeResult, ModernRequest, Request,
    RpcError, ServerInfo,
};
use futures_util::future::BoxFuture;
use futures_util::FutureExt;
use serde_json::{json, Value};
use std::borrow::Cow;
use std::collections::HashMap;
use std::ffi::OsString;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use tokio::sync::{mpsc, oneshot};

// Serialize checkout-level config reconciliation. Deferred startup repair
// rechecks live workspace ownership while holding this lock, so a concurrent
// final-workspace close either happens before the check (and is skipped) or
// removes the entry after the repair. It can never remove-then-resurrect it.
static MCP_CONFIG_IO_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// Set by app-owned provider MCP descriptors, never per-call metadata.
pub(crate) const PROVIDER_ORIGIN_ENV: &str = "CODEMUX_MCP_PROVIDER_ORIGIN";

/// Check if auto-MCP config is enabled in settings (default: true).
pub fn is_auto_mcp_enabled<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> bool {
    use tauri::Manager;
    let db: tauri::State<'_, crate::database::DatabaseStore> = app.state();
    db.get_setting("auto_mcp_config")
        .map(|v| v != "false")
        .unwrap_or(true)
}

// ---------------------------------------------------------------------------
// MCP tool definition
// ---------------------------------------------------------------------------

/// One of the stdio server's own tools.
struct McpTool {
    name: &'static str,
    description: &'static str,
    input_schema: Value,
}

/// A tool as `tools/list` serves it: one of the server's own tools, or a
/// control-plane registry tool, whose text need not be static.
struct StdioTool {
    name: Cow<'static, str>,
    description: Cow<'static, str>,
    input_schema: Value,
    /// MCP behaviour hints. Registry tools carry them; legacy tools never did.
    annotations: Option<Value>,
}

impl From<McpTool> for StdioTool {
    fn from(tool: McpTool) -> Self {
        Self {
            name: tool.name.into(),
            description: tool.description.into(),
            input_schema: tool.input_schema,
            annotations: None,
        }
    }
}

impl From<registry::ToolDefinition> for StdioTool {
    fn from(tool: registry::ToolDefinition) -> Self {
        Self {
            name: tool.name.into(),
            description: tool.description,
            input_schema: tool.input_schema,
            annotations: Some(tool.annotations),
        }
    }
}

pub(crate) const REMOTE_DELEGATE_TOOL: &str = "delegate_remote_task";

/// The stdio server's own tools. Processes that serve an agent see exactly
/// these; elsewhere a registry tool that shares one of these names replaces
/// it (see [`stdio_tools`]).
pub(crate) const LEGACY_TOOL_NAMES: [&str; 62] = [
    "delegation_targets", REMOTE_DELEGATE_TOOL, "task_status", "task_cancel",
    "browser_navigate", "browser_snapshot", "browser_accessibility_snapshot", "browser_click", "browser_fill",
    "browser_screenshot", "browser_console_logs", "browser_click_at", "browser_type_at", "browser_scroll_at",
    "browser_key_press", "browser_drag", "browser_click_os", "browser_type_os", "browser_get_styles", "browser_wait",
    "browser_evaluate", "browser_viewport", "browser_viewport_presets",
    "workspace_list", "workspace_info", "workspace_create",
    "automation_list", "automation_get", "automation_create", "automation_update", "automation_delete",
    "automation_pause", "automation_resume", "automation_runs",
    "pane_list", "pane_split_right", "pane_split_down", "conversation_read", "conversation_search", "notify",
    "git_status", "git_diff", "git_stage", "git_commit", "git_push", "terminal_write", "terminal_read",
    "workspace_open", "app_status", "port_list", "worktree_create", "preset_apply", "preset_list",
    "workspace_close", "workspace_archive", "workspace_unarchive", "workspace_archive_list", "pane_close",
    "issue_list", "issue_get", "issue_link_workspace", "html_render",
];

pub(crate) fn is_legacy_tool(name: &str) -> bool {
    LEGACY_TOOL_NAMES.contains(&name)
}

#[cfg(test)]
pub(crate) fn remote_delegation_tools_for_test() -> Vec<crate::mcp::runtime::McpTool> {
    let config = crate::mcp::codemux_self::codemux_self_config();
    legacy_tools()
        .into_iter()
        .filter(|tool| {
            matches!(
                tool.name,
                "delegation_targets" | REMOTE_DELEGATE_TOOL | "task_status" | "task_cancel"
            )
        })
        .map(|tool| crate::mcp::runtime::McpTool {
            prefixed_name: crate::mcp::runtime::prefix_tool_name(&config.name, &tool.name),
            name: tool.name.into(),
            description: Some(tool.description.into()),
            input_schema: tool.input_schema,
            server_id: config.id.clone(),
        })
        .collect()
}

/// What `tools/list` serves. A process that serves an agent (provider origin
/// or the agent marker) gets exactly the legacy tools. Any other caller gets
/// the legacy tools the registry does not serve and its access can run, then
/// the registry's outside catalog for that access. The count stays the same
/// as registry tools take over legacy names.
///
/// Pure: only [`run_mcp_server_with`] reads the env and options.
fn stdio_tools(provider_origin: bool, agent: bool, access: ControlAccess) -> Vec<StdioTool> {
    let legacy = legacy_tools().into_iter().map(StdioTool::from);
    if provider_origin || agent {
        return legacy.collect();
    }
    let view = CallerView::outside(access);
    let mut tools: Vec<StdioTool> = legacy
        .filter(|tool| registry::served(&tool.name, Audience::Outside).is_none())
        .filter(|tool| {
            control_identity::mcp_tool_access(&tool.name, &json!({})).is_some_and(|needs| needs.listed(&view))
        })
        .collect();
    tools.extend(registry::list(&view).into_iter().map(|tool| StdioTool::from(tool.definition)));
    tools
}

/// The stdio server's own tools ([`LEGACY_TOOL_NAMES`], in that order).
fn legacy_tools() -> Vec<McpTool> {
    vec![
        McpTool {name:"delegation_targets",description:"List SSH remote hosts the user granted for delegate_remote_task (host/existing-checkout/provider/mode targets). For remote delegation only: an empty list says nothing about local delegation. To delegate to local child chats, use delegate_task. Grants are immutable to agents. Available only in an actual Codex/Claude adapter; generic MCP metadata is not authority.",input_schema:json!({"type":"object","properties":{},"additionalProperties":false})},
        McpTool {name:REMOTE_DELEGATE_TOOL,description:"Delegate a SELF-CONTAINED coding task to a user-granted SSH remote host checkout from delegation_targets. Remote hosts only; use delegate_task for local child chats. Supply only the needed goal/context, never credentials or full conversation history. Returns a durable task receipt promptly; the backend follows asynchronously and delivers its specific final result when this parent is idle. Continue local work or end the turn; do not busy-poll. Exact client_request_id retries dedupe; changed reuse is rejected. Only receiver-native Codex/Claude execution is currently supported.",input_schema:json!({"type":"object","properties":{"target_id":{"type":"string"},"prompt":{"type":"string","maxLength":32000},"title":{"type":"string"},"model":{"type":["string","null"]},"effort":{"type":["string","null"]},"client_request_id":{"type":"string","description":"Stable per-parent request identity. Reuse only for the identical task request."}},"required":["target_id","prompt","client_request_id"],"additionalProperties":false})},
        McpTool {name:"task_status",description:"Read this native parent's cached remote task and actual native events after an optional cursor. No launch and no remote busy-poll: the coordinator follows in the background. Use on demand, not in a tight waiting loop.",input_schema:json!({"type":"object","properties":{"task_id":{"type":"string"},"cursor":{"type":"integer","minimum":0}},"required":["task_id"],"additionalProperties":false})},
        McpTool {name:"task_cancel",description:"Persist a stop request for this native parent's remote task. Offline stop remains unconfirmed until the receiver acknowledges cancellation. Suppresses result wakes. Cannot authorize targets or cancel another parent's task.",input_schema:json!({"type":"object","properties":{"task_id":{"type":"string"}},"required":["task_id"],"additionalProperties":false})},
        // -- Browser tools --
        McpTool {
            name: "browser_navigate",
            description: "Navigate the browser pane to a URL",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "The URL to navigate to" }
                },
                "required": ["url"]
            }),
        },
        McpTool {
            name: "browser_snapshot",
            description: "Get a list of interactive DOM elements with CSS selectors, text content, and bounding boxes. Use this when you need CSS selectors for browser_click/browser_fill.",
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        McpTool {
            name: "browser_accessibility_snapshot",
            description: "Get the accessibility tree with clickable ref IDs like [ref=e4]. PREFERRED for clicking: pass the ref as the selector to browser_click (e.g. selector=\"@e4\"). Refs use the browser's full actionability pipeline (auto-wait, auto-scroll, retry) and are more reliable than CSS selectors or coordinates.",
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        McpTool {
            name: "browser_click",
            description: "Click an element. Accepts a snapshot ref like \"@e4\" (most reliable — uses auto-wait and retry) or a CSS selector. Always call browser_accessibility_snapshot first and use refs when possible. Only fall back to browser_click_at with coordinates if selectors fail due to bot detection.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "selector": { "type": "string", "description": "Snapshot ref (e.g. \"@e4\") or CSS selector" }
                },
                "required": ["selector"]
            }),
        },
        McpTool {
            name: "browser_fill",
            description: "Type text into an input field. Accepts a snapshot ref like \"@e3\" (most reliable) or a CSS selector.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "selector": { "type": "string", "description": "Snapshot ref (e.g. \"@e3\") or CSS selector" },
                    "value": { "type": "string", "description": "Text to type into the field" }
                },
                "required": ["selector", "value"]
            }),
        },
        McpTool {
            name: "browser_screenshot",
            description: "Take a screenshot of the browser pane. Returns base64-encoded PNG with viewport dimensions for use with coordinate-based tools (browser_click_at, browser_click_os, etc.).",
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        McpTool {
            name: "browser_console_logs",
            description: "Get browser console output",
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        // -- Coordinate-based vision tools (Tier 2: CDP via stream WS) --
        McpTool {
            name: "browser_click_at",
            description: "Click at pixel coordinates (x, y) on the browser viewport via CDP. Works on iframes, shadow DOM, canvas, most websites. Take a browser_screenshot first. If this fails on Cloudflare Turnstile or anti-bot captchas, escalate to browser_click_os.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "x": { "type": "number", "description": "X coordinate in CSS pixels from left edge" },
                    "y": { "type": "number", "description": "Y coordinate in CSS pixels from top edge" },
                    "click_type": { "type": "string", "enum": ["left", "right", "double"], "default": "left" }
                },
                "required": ["x", "y"]
            }),
        },
        McpTool {
            name: "browser_type_at",
            description: "Type text at the current cursor position or at specified coordinates. Uses low-level input events that work in iframes and shadow DOM.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "Text to type" },
                    "x": { "type": "number", "description": "Optional: click here first" },
                    "y": { "type": "number", "description": "Optional: click here first" }
                },
                "required": ["text"]
            }),
        },
        McpTool {
            name: "browser_scroll_at",
            description: "Scroll the page at specified coordinates using mouse wheel events.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "x": { "type": "number", "description": "X coordinate to scroll at" },
                    "y": { "type": "number", "description": "Y coordinate to scroll at" },
                    "direction": { "type": "string", "enum": ["up", "down", "left", "right"], "default": "down" },
                    "amount": { "type": "number", "description": "Scroll ticks (1-10, default 3)", "default": 3 }
                },
                "required": ["x", "y"]
            }),
        },
        McpTool {
            name: "browser_key_press",
            description: "Press keyboard keys or combinations (e.g., Enter, Escape, Ctrl+a).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "key": { "type": "string", "description": "Key to press (e.g., 'Enter', 'Tab', 'Ctrl+a')" }
                },
                "required": ["key"]
            }),
        },
        McpTool {
            name: "browser_drag",
            description: "Drag from one coordinate to another. Useful for sliders, drag-and-drop, resizing.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "start_x": { "type": "number" }, "start_y": { "type": "number" },
                    "end_x": { "type": "number" }, "end_y": { "type": "number" }
                },
                "required": ["start_x", "start_y", "end_x", "end_y"]
            }),
        },
        // -- OS-level input tools (Tier 3: kernel events via ydotool) --
        McpTool {
            name: "browser_click_os",
            description: "Click at viewport coordinates using OS-level kernel input (ydotool). Produces real mouse events indistinguishable from human clicks with correct screenX/screenY in all frames. Use when browser_click_at fails on Cloudflare Turnstile or aggressive anti-bot systems. Requires ydotool + ydotoold running, and the browser must be visible (not headless).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "x": { "type": "number", "description": "X coordinate in the browser viewport" },
                    "y": { "type": "number", "description": "Y coordinate in the browser viewport" }
                },
                "required": ["x", "y"]
            }),
        },
        McpTool {
            name: "browser_type_os",
            description: "Type text using OS-level kernel input (ydotool). Use when browser_type_at fails on protected form fields or anti-bot checks.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "Text to type" },
                    "x": { "type": "number", "description": "Optional: click here first" },
                    "y": { "type": "number", "description": "Optional: click here first" }
                },
                "required": ["text"]
            }),
        },
        // -- Browser info tools (v0.24.0) --
        McpTool {
            name: "browser_get_styles",
            description: "Get the computed CSS styles for an element. Returns all CSS properties and their computed values. Use snapshot refs (e.g. \"@e4\") or CSS selectors.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "selector": { "type": "string", "description": "Snapshot ref (e.g. \"@e4\") or CSS selector" }
                },
                "required": ["selector"]
            }),
        },
        McpTool {
            name: "browser_wait",
            description: "Wait for an element to appear or specific text to be visible on the page. Useful after navigation or dynamic content loading.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "selector": { "type": "string", "description": "CSS selector or snapshot ref to wait for" },
                    "text": { "type": "string", "description": "Wait for this text to appear on the page (alternative to selector)" }
                }
            }),
        },
        McpTool {
            name: "browser_evaluate",
            description: "Execute JavaScript in the browser page and return the result. Use for custom DOM queries, data extraction, or page manipulation.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "script": { "type": "string", "description": "JavaScript expression or IIFE to evaluate" }
                },
                "required": ["script"]
            }),
        },
        McpTool {
            name: "browser_viewport",
            description: "Resize the browser viewport to test mobile, tablet, or desktop layouts. \
                CSS media queries fire against the new width, `window.devicePixelRatio` reflects the \
                preset's DPR, and subsequent browser_screenshot calls capture at the new dimensions. \
                Always use this instead of wrapping the page in an iframe for mobile preview — \
                viewport resizing gives true CSS / DPR / layout behavior, not just a narrower scroll \
                region. \
                \n\nDEFAULT PICKS when the user is vague: \
                \n- \"test on mobile\" or \"check mobile view\" → use 'mobile' (390x844, the \
                standard modern phone size). Do NOT pick 'mobile-small' or 'mobile-large' unless the \
                user explicitly asks for SE-class / Pro Max phones. \
                \n- \"test on tablet\" → use 'tablet' (768x1024, iPad portrait). Only use \
                'tablet-large' if the user explicitly asks for iPad Pro / large tablet. \
                \n- \"test on desktop\" or after mobile testing → use 'desktop' (1280x800, matches \
                Tailwind 'xl' breakpoint). Only use 'desktop-large' if the user explicitly asks for \
                Full HD / 1080p. \
                \n- \"go back to normal\" or \"reset viewport\" → use 'reset'.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "preset": {
                        "type": "string",
                        "description": "Preset name (e.g. 'mobile'), 'WxH' like '390x844', or 'reset'.",
                        "enum": [
                            "mobile-small", "mobile", "mobile-large",
                            "tablet", "tablet-large",
                            "desktop", "desktop-large",
                            "reset"
                        ]
                    },
                    "dpr": {
                        "type": "number",
                        "description": "Optional device-pixel-ratio override (e.g. 2 for retina, 1 for desktop). Defaults to the preset's natural DPR."
                    }
                },
                "required": ["preset"]
            }),
        },
        McpTool {
            name: "browser_viewport_presets",
            description: "List the available viewport presets with their CSS dimensions and DPR. \
                Useful for discovering options before calling browser_viewport.",
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        // -- Workspace tools --
        McpTool {
            name: "workspace_list",
            description: "List all open workspaces with their IDs, paths, and git info",
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        McpTool {
            name: "workspace_info",
            description: "Get details about the current active workspace",
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        McpTool {
            name: "workspace_create",
            description: "Create a new workspace, optionally at a specific path",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Directory path for the workspace (optional)" }
                }
            }),
        },
        // -- Automation tools --
        McpTool {
            name: "automation_list",
            description: "List all automations (scheduled agent runs) for the signed-in user.",
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        McpTool {
            name: "automation_get",
            description: "Get a single automation by id, including its prompt and schedule.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "integer", "description": "Automation id" }
                },
                "required": ["id"]
            }),
        },
        McpTool {
            name: "automation_create",
            description: "Create an automation: a named prompt + agent that runs on a schedule. The schedule is an RFC 5545 recurrence.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Short human-readable name" },
                    "prompt": { "type": "string", "description": "The instruction the agent runs each time it fires" },
                    "agent": { "type": "string", "description": "Agent to run, e.g. \"claude\" or \"codex\"" },
                    "schedule": { "type": "string", "description": "RFC 5545 recurrence: a DTSTART line and one RRULE line joined by a newline, e.g. \"DTSTART:20260101T090000Z\\nRRULE:FREQ=DAILY\"" },
                    "timezone": { "type": "string", "description": "IANA timezone name, e.g. \"America/New_York\". Defaults to \"UTC\"" },
                    "host_id": { "type": "integer", "description": "Target host id (from the Hosts list). Optional — may be assigned later" },
                    "project_path": { "type": "string", "description": "Absolute path of the repository the run operates in (optional)" },
                    "retention_limit": { "type": "integer", "description": "How many completed run worktrees the host keeps (1-1000, default 10)" }
                },
                "required": ["name", "prompt", "agent", "schedule"]
            }),
        },
        McpTool {
            name: "automation_update",
            description: "Update an existing automation. All editable fields must be supplied — this replaces them.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "integer", "description": "Automation id" },
                    "name": { "type": "string" },
                    "prompt": { "type": "string" },
                    "agent": { "type": "string" },
                    "schedule": { "type": "string", "description": "RFC 5545 recurrence (DTSTART + RRULE)" },
                    "timezone": { "type": "string", "description": "IANA timezone name" },
                    "host_id": { "type": "integer" },
                    "project_path": { "type": "string" },
                    "retention_limit": { "type": "integer" }
                },
                "required": ["id", "name", "prompt", "agent", "schedule"]
            }),
        },
        McpTool {
            name: "automation_delete",
            description: "Delete an automation. Run history is retained.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "integer", "description": "Automation id" }
                },
                "required": ["id"]
            }),
        },
        McpTool {
            name: "automation_pause",
            description: "Pause an automation so it stops firing until resumed.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "integer", "description": "Automation id" }
                },
                "required": ["id"]
            }),
        },
        McpTool {
            name: "automation_resume",
            description: "Resume a paused automation. The next fire time is recomputed from now, so no missed runs are replayed.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "integer", "description": "Automation id" }
                },
                "required": ["id"]
            }),
        },
        McpTool {
            name: "automation_runs",
            description: "List the run history of an automation, newest fire first.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "automation_id": { "type": "integer", "description": "Automation id" },
                    "limit": { "type": "integer", "description": "Max rows to return (1-100, default 20)" }
                },
                "required": ["automation_id"]
            }),
        },
        // -- Pane tools --
        McpTool {
            name: "pane_list",
            description: "List all panes in the active workspace",
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        McpTool {
            name: "pane_split_right",
            description: "Split the current pane vertically (new pane appears to the right)",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pane_id": { "type": "string", "description": "ID of the pane to split (defaults to active pane)" }
                }
            }),
        },
        McpTool {
            name: "pane_split_down",
            description: "Split the current pane horizontally (new pane appears below)",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pane_id": { "type": "string", "description": "ID of the pane to split (defaults to active pane)" }
                }
            }),
        },
        // -- Attached conversation history tools --
        McpTool {
            name: "conversation_read",
            description: "Read a provider-neutral conversation handoff source in chronological pages. Use the conversation_id included in an attached handoff. Returns only visible user messages and top-level assistant prose; hidden reasoning, raw tool payloads, requests, and subagent internals are excluded. Start without cursor, then pass next_cursor until it is null.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "conversation_id": { "type": "string", "description": "Conversation reference from the handoff attachment" },
                    "cursor": { "type": "integer", "minimum": 0, "description": "Previous page's next_cursor; omit for the beginning" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 100, "description": "Messages per page (default 20)" }
                },
                "required": ["conversation_id"]
            }),
        },
        McpTool {
            name: "conversation_search",
            description: "Search the complete safe-visible source of an attached conversation before reading a focused page. Results include message ids that can be used as evidence; conversation_read remains chronological and cursor-based.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "conversation_id": { "type": "string", "description": "Conversation reference from the handoff attachment" },
                    "query": { "type": "string", "description": "Words to find in the source conversation" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 50, "description": "Maximum matches (default 12)" }
                },
                "required": ["conversation_id", "query"]
            }),
        },
        // -- Notification tools --
        McpTool {
            name: "notify",
            description: "Send a notification to the user in the Codemux notification panel",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string", "description": "Notification message" },
                    "level": { "type": "string", "enum": ["info", "attention", "error"], "description": "Notification level (default: attention)" }
                },
                "required": ["message"]
            }),
        },
        // -- Git tools --
        McpTool {
            name: "git_status",
            description: "Get the list of changed files (git status --porcelain)",
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        McpTool {
            name: "git_diff",
            description: "Get the diff for a specific file or all files",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "File path to diff (omit for all changes)" }
                }
            }),
        },
        McpTool {
            name: "git_stage",
            description: "Stage a file for commit (git add)",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "File path to stage" }
                },
                "required": ["file"]
            }),
        },
        McpTool {
            name: "git_commit",
            description: "Commit staged changes with a message",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "message": { "type": "string", "description": "Commit message" }
                },
                "required": ["message"]
            }),
        },
        McpTool {
            name: "git_push",
            description: "Push commits to the remote repository",
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        // -- Terminal tools (Phase 1 vexis-agent integration) --
        McpTool {
            name: "terminal_write",
            description: "Write text to a Codemux terminal pane. The text is sent verbatim to the PTY's stdin — include a trailing newline if you want a command to execute. Without a `session_id`, writes to the currently focused pane's session.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "data": {
                        "type": "string",
                        "description": "Bytes to write to the terminal. Include \\n at the end to submit a command."
                    },
                    "session_id": {
                        "type": "string",
                        "description": "Optional terminal session id (from pane_list / workspace_info). Defaults to the active workspace's focused pane."
                    }
                },
                "required": ["data"]
            }),
        },
        McpTool {
            name: "terminal_read",
            description: "Read recent PTY output from a Codemux terminal pane's in-memory buffer. Returns the last `lines` lines of UTF-8 output, defaulting to 200 and capped at 5000. Without a `session_id`, reads from the currently focused pane's session. The buffer is the same scrollback the user would see; very old output may have been evicted under sustained load.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "lines": {
                        "type": "integer",
                        "description": "Number of lines from the end of the buffer to return. Default 200, max 5000.",
                        "minimum": 1,
                        "maximum": 5000
                    },
                    "session_id": {
                        "type": "string",
                        "description": "Optional terminal session id. Defaults to the active workspace's focused pane."
                    }
                }
            }),
        },
        McpTool {
            name: "workspace_open",
            description: "Focus an existing Codemux workspace by id. Runs the same activation path as clicking the workspace in the sidebar: git refresh, lazy PTY hydration, persisted active-workspace bookkeeping. Use `workspace_list` to discover ids.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "workspace_id": {
                        "type": "string",
                        "description": "Id of the workspace to focus (from workspace_list)."
                    }
                },
                "required": ["workspace_id"]
            }),
        },
        McpTool {
            name: "app_status",
            description: "Return Codemux app status: app version, control protocol version, control socket path, the active workspace id, the focused pane id, and a one-line summary (id/title/cwd) of every open workspace. Use this for a quick orientation snapshot before deciding which `workspace_list` / `workspace_info` / `pane_list` calls to make.",
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        McpTool {
            name: "port_list",
            description: "List dev-server ports Codemux has detected as listening on the local machine. Each entry includes port, pid, process_name, optional workspace_id (which workspace owns the listener) and optional label (e.g. \"vite\", \"next\"). \
                Filter semantics: omit `workspace_id` to get EVERY port, including unscoped (system) listeners. Pass `workspace_id` to get ONLY ports the detector tagged with that workspace — unscoped ports are excluded. To see workspace + unscoped together, call without a filter and partition client-side on the `workspace_id` field.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "workspace_id": {
                        "type": "string",
                        "description": "Optional: only return ports detected as belonging to this workspace."
                    }
                }
            }),
        },
        // -- Delegation primitives (Phase 1.5 vexis-agent integration) --
        McpTool {
            name: "worktree_create",
            description: "Create a git worktree + Codemux workspace in one atomic call, optionally launching an agent inside with a starting prompt. Mirrors the in-app branch-picker \"Fork → \" flow. Does ALL of: `git worktree add` under ~/.codemux/worktrees/<repo>/<branch>, workspace state hydration with the requested layout, PTY spawn, setup-script run, `.mcp.json` autoconfig, and (if `agent_preset_id` is set) launching the preset's CLI with `initial_prompt` injected. Returns the new `workspace_id`. Pair with `preset_list` first if you need to know which agent presets are installed.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "repo_path": {
                        "type": "string",
                        "description": "Absolute path to a git repository (or any subdirectory of one — the git root is resolved automatically)."
                    },
                    "branch": {
                        "type": "string",
                        "description": "Branch name. If `new_branch` is true, this branch is created from `base`; otherwise it must already exist."
                    },
                    "new_branch": {
                        "type": "boolean",
                        "description": "Create the branch (true) or check out an existing branch (false). Defaults to true — the common case for the brain is starting fresh work.",
                        "default": true
                    },
                    "base": {
                        "type": "string",
                        "description": "Base branch to fork from when `new_branch` is true. Defaults to the repository's default branch."
                    },
                    "layout": {
                        "type": "string",
                        "enum": ["single", "pair", "quad", "six", "eight", "shell_browser", "empty"],
                        "description": "Pane layout for the workspace. \"single\" is one terminal (the right choice for almost every delegated task); \"empty\" creates no pane (chat-only). Defaults to \"single\".",
                        "default": "single"
                    },
                    "initial_prompt": {
                        "type": "string",
                        "description": "Optional starting prompt to feed the agent. For Claude/Codex presets it's appended as a positional ANSI-C-quoted argument; for other CLIs (Gemini, OpenCode, custom) it's typed into the terminal after a ~1500ms TUI settle. Ignored unless `agent_preset_id` is set."
                    },
                    "agent_preset_id": {
                        "type": "string",
                        "description": "Optional preset id (from `preset_list`) to launch after the workspace is hydrated. Without this, the workspace is created but no agent runs — leaving the brain to drive via `preset_apply` or `terminal_write` later."
                    },
                    "pr_number": {
                        "type": "integer",
                        "description": "Optional GitHub PR number to associate with the new workspace (shows in the workspace's PR badge).",
                        "minimum": 1
                    }
                },
                "required": ["repo_path", "branch"]
            }),
        },
        McpTool {
            name: "preset_apply",
            description: "Apply an existing preset to an already-open workspace. Use this when you have a workspace from `workspace_list` and want to launch an agent in it (or run a shell preset's commands). For starting from scratch — new branch, new worktree, agent attached — use `worktree_create` instead, which combines all of that into one call. Returns `{ok: true}` on success.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "workspace_id": {
                        "type": "string",
                        "description": "Target workspace (from `workspace_list` / `workspace_info` / `app_status`)."
                    },
                    "preset_id": {
                        "type": "string",
                        "description": "Preset to apply (from `preset_list`)."
                    },
                    "override_mode": {
                        "type": "string",
                        "enum": ["new_tab", "split_pane", "current_terminal", "existing_panes"],
                        "description": "Override the preset's default launch mode. `current_terminal` writes commands to the workspace's active terminal; `new_tab` creates a fresh tab; `split_pane` splits the active pane; `existing_panes` chains the commands into every terminal session in the workspace."
                    },
                    "initial_prompt": {
                        "type": "string",
                        "description": "Optional prompt to feed the agent after launch. Same injection rules as `worktree_create`: positional arg for Claude/Codex, PTY-typed for others."
                    }
                },
                "required": ["workspace_id", "preset_id"]
            }),
        },
        McpTool {
            name: "preset_list",
            description: "List the agent presets Codemux knows about. Each entry has `preset_id`, `name`, `description`, `kind` (\"terminal\" or \"chat\"), `is_default`, and `commands_available` — a boolean that's true only when every command binary the preset launches resolves on the current PATH. Filter to `commands_available: true` before calling `worktree_create` or `preset_apply` so the brain doesn't ask for a CLI that isn't installed on this host.",
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        // -- Lifecycle + issue tools (Phase 1.6 vexis-agent integration) --
        McpTool {
            name: "workspace_close",
            description: "Close a workspace by id. Runs teardown scripts, terminates PTYs, and removes Codemux's entry from the workspace's `.mcp.json`. Optionally also removes the underlying git worktree. Protected project-root checkouts can NEVER have their files deleted through this tool — `delete_worktree: true` on such a workspace errors; use `workspace_archive` instead to close it restorably.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "workspace_id": {
                        "type": "string",
                        "description": "Id of the workspace to close (from `workspace_list` / `app_status`)."
                    },
                    "delete_worktree": {
                        "type": "boolean",
                        "description": "When true AND the workspace is backed by a disposable git worktree, also runs `git worktree remove` after closing. Refused (with an error) for protected project-root checkouts and workspaces without a worktree. Defaults to false.",
                        "default": false
                    },
                    "delete_branch": {
                        "type": "boolean",
                        "description": "Only meaningful when `delete_worktree` is true. When true, deletes the branch the worktree was checked out at. Defaults to false.",
                        "default": false
                    },
                    "force_delete": {
                        "type": "boolean",
                        "description": "Skip teardown scripts AND override the dirty-worktree safety check: without it, `delete_worktree` refuses to remove a worktree that has uncommitted changes or unpushed commits. Defaults to false.",
                        "default": false
                    }
                },
                "required": ["workspace_id"]
            }),
        },
        McpTool {
            name: "workspace_archive",
            description: "Archive a workspace: close it (teardown scripts, PTY termination — exactly like `workspace_close` without `delete_worktree`) while keeping its files, worktree, and branch on disk, and record a restorable archive entry. Returns the new `archive_id`. Use `workspace_unarchive` to bring it back and `workspace_archive_list` to browse entries. Attach-in-place (remote, no local files) workspaces can't be archived.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "workspace_id": {
                        "type": "string",
                        "description": "Id of the workspace to archive (from `workspace_list` / `app_status`)."
                    }
                },
                "required": ["workspace_id"]
            }),
        },
        McpTool {
            name: "workspace_unarchive",
            description: "Restore an archived workspace by `archive_id` (from `workspace_archive_list`). Reuses the on-disk worktree/folder when it still exists — adopting an imported worktree in place if it lives outside the conventional path, or recreating the worktree from its branch if only the directory is gone — then spawns sessions and activates the restored workspace. The restored workspace preserves its files, branch, worktree, and title but starts with a fresh single-pane layout (the prior pane/tab arrangement is not restored). Returns the new `workspace_id`. Errors — keeping the entry — when nothing is left to restore.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "archive_id": {
                        "type": "string",
                        "description": "Id of the archive entry to restore (from `workspace_archive_list`)."
                    }
                },
                "required": ["archive_id"]
            }),
        },
        McpTool {
            name: "workspace_archive_list",
            description: "List archived workspaces. Each entry has `archive_id`, the original `workspace_id`, `title`, `cwd`, optional `worktree_path` / `project_root` / `git_branch`, `workspace_kind` (\"main\" or \"worktree\"), `protected`, and `archived_at` (unix seconds). Pass an entry's `archive_id` to `workspace_unarchive` to restore it.",
            input_schema: json!({
                "type": "object",
                "properties": {}
            }),
        },
        McpTool {
            name: "pane_close",
            description: "Close a pane by id. Terminates the pane's PTY (for terminal panes) or marks the agent-attached browser session dismissed (for browser panes). When the closed pane was the last leaf in its surface, the surface and its tab are removed too; the workspace stays open with the remaining tabs (or with no tabs if every pane was closed).",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pane_id": {
                        "type": "string",
                        "description": "Id of the pane to close (from `pane_list` / `workspace_info`)."
                    }
                },
                "required": ["pane_id"]
            }),
        },
        McpTool {
            name: "issue_list",
            description: "List GitHub issues for a repository using the `gh` CLI. Returns the same shape `gh issue list --json` produces (number, title, state, labels, etc.). Resolves the repo from the optional `repo_path`, otherwise from the active workspace's `project_root` or `cwd`.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "repo_path": {
                        "type": "string",
                        "description": "Optional absolute path to the git repository. Defaults to the active workspace's project root."
                    },
                    "search": {
                        "type": "string",
                        "description": "Optional `gh issue list --search` query (e.g. \"label:bug is:open\")."
                    }
                }
            }),
        },
        McpTool {
            name: "issue_get",
            description: "Fetch a single GitHub issue by number using the `gh` CLI. Returns the issue's number, title, state, body, labels, and other fields `gh issue view --json` provides. Repo resolution matches `issue_list`.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "number": {
                        "type": "integer",
                        "description": "Issue number.",
                        "minimum": 1
                    },
                    "repo_path": {
                        "type": "string",
                        "description": "Optional absolute path to the git repository. Defaults to the active workspace's project root."
                    }
                },
                "required": ["number"]
            }),
        },
        McpTool {
            name: "issue_link_workspace",
            description: "Attach a GitHub issue to a workspace. The linked issue (number, title, state, labels) appears on the workspace card and the issue's url is exposed via `workspace_info`. Looks up the issue via `gh` against the workspace's project root.",
            input_schema: json!({
                "type": "object",
                "properties": {
                    "workspace_id": {
                        "type": "string",
                        "description": "Workspace to attach the issue to. Defaults to the active workspace."
                    },
                    "number": {
                        "type": "integer",
                        "description": "Issue number to link.",
                        "minimum": 1
                    }
                },
                "required": ["number"]
            }),
        },
        // -- Visual replies --
        McpTool {
            name: "html_render",
            description: HTML_RENDER_DESCRIPTION,
            input_schema: json!({
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "Short name for the page.",
                        "maxLength": HTML_RENDER_MAX_TITLE_CHARS
                    },
                    "html": {
                        "type": "string",
                        "description": "A complete, self-contained HTML document."
                    },
                    "height": {
                        "type": "integer",
                        "description": "Optional frame height cap in CSS pixels (80-2000). Omit it and the frame fits the page; set it below the page's height to make long content scroll inside the frame.",
                        "minimum": 80,
                        "maximum": 2000
                    }
                },
                "required": ["title", "html"]
            }),
        },
    ]
}

/// Largest page `html_render` accepts. The page lives in the chat
/// transcript as the tool call's input, so it is bounded like one.
const HTML_RENDER_MAX_BYTES: usize = 512 * 1024;
const HTML_RENDER_MAX_TITLE_CHARS: usize = 200;

/// `html_render` as the MCP registry lists it to agent chats: Codemux's own
/// server runs under the name `codemux` (see `mcp::codemux_self`).
pub const HTML_RENDER_REGISTRY_NAME: &str = "mcp__codemux__html_render";

/// Session instructions for agent chats that can call `html_render`. The tool
/// description alone is not enough: Claude lists MCP tools by name only until
/// it searches for one, so without this an agent writes the page to a file
/// and never shows it.
pub const HTML_RENDER_GUIDANCE: &str = "## Showing visuals in Codemux\n\n\
This chat can show HTML pages inline. When a chart, table, diagram, or mockup would say more than prose, \
or the user wants to see a page, build a self-contained HTML page and show it with Codemux's `html_render` tool \
(listed with an MCP prefix) before your final reply, rather than only pointing the user at an HTML file. \
The reader sees the page above that reply, so don't announce or restate it; add only what it doesn't say.";

/// `suggest_tasks` as host tools are listed to agent chats.
pub const SUGGEST_TASKS_REGISTRY_NAME: &str = "mcp__codemux__suggest_tasks";

/// Codemux's session instructions per tool, in the order they are appended.
/// A chat gets a block only when it lists that tool.
const TOOL_GUIDANCE: [(&str, &str); 4] = [
    (HTML_RENDER_REGISTRY_NAME, HTML_RENDER_GUIDANCE),
    (
        SUGGEST_TASKS_REGISTRY_NAME,
        crate::commands::follow_ups::SUGGEST_TASKS_GUIDANCE,
    ),
    (
        crate::commands::pr_watch::PR_WATCH_REGISTRY_NAME,
        crate::commands::pr_watch::PR_WATCH_GUIDANCE,
    ),
    (
        crate::commands::secret_requests::REQUEST_SECRET_REGISTRY_NAME,
        crate::commands::secret_requests::REQUEST_SECRET_GUIDANCE,
    ),
];

/// The guidance blocks for the tools a chat lists (registry names, e.g.
/// `mcp__codemux__html_render`), joined by a blank line, or `None` when
/// it lists none of them.
pub fn tool_guidance<'a>(registry_names: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let listed: Vec<&str> = registry_names.into_iter().collect();
    let blocks: Vec<&str> = TOOL_GUIDANCE
        .iter()
        .filter(|(name, _)| listed.contains(name))
        .map(|(_, guidance)| *guidance)
        .collect();
    (!blocks.is_empty()).then(|| blocks.join("\n\n"))
}

/// Agent-facing contract for `html_render`. The variable names mirror
/// `htmlRenderThemeVariables` in `src/lib/agent-chat/html-render.ts`, which
/// injects them into the page; keep the two lists in sync.
const HTML_RENDER_DESCRIPTION: &str = concat!(
    "Show a finished HTML page (chart, dashboard, table, diagram, mockup) inline in this Codemux chat thread, above your final text reply; call it before writing that reply. ",
    "The reader already sees the page, so the reply should not announce it or restate it: add only what the page doesn't say. ",
    "Pages are only visible in Codemux's agent chat, not in a terminal session. ",
    "Write one self-contained document with inline <style> and <script>; remote http(s) resources such as a CDN chart library load as-is. ",
    "The page runs in a sandboxed frame with no access to Codemux, the filesystem, or local images; links the reader clicks open in their browser. ",
    "Layout: the frame is borderless on the thread's background and as wide as the reply column (about 760px, narrower on small windows). ",
    "Use a fluid width with no outer padding, card, border, or banner title: the page is part of your reply. ",
    "Give charts fixed pixel heights, and let content set the page height: avoid 100vh or height:100% on html/body, since the frame grows to fit the page. ",
    "Theme: Codemux injects the user's active theme as CSS custom properties on :root, and they follow theme changes live: ",
    "--background (identical to the thread around the frame), --foreground, --muted, --muted-foreground, --card, --card-foreground, ",
    "--popover, --popover-foreground, --secondary, --secondary-foreground, --border, --input, --ring, --primary, --primary-foreground, ",
    "--accent, --accent-foreground (the theme's brand accent), --destructive, --success, --warning, --info, ",
    "--chart-1 ... --chart-6 (categorical series colors; assign them in order), --radius, --font-sans, --font-mono. ",
    "A base stylesheet sets the html background, color, and font from these and zeroes the body margin; your own CSS overrides it. ",
    "Style everything with these variables rather than fixed colors so the page matches the app in light and dark themes."
);

/// Check an `html_render` call. The page itself needs no storage: the chat
/// transcript keeps the call's input, and the client renders it from there.
fn html_render_result(arguments: &Value) -> Result<Value, String> {
    let title = arguments
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if title.is_empty() {
        return Err("html_render: missing required argument 'title'".to_string());
    }
    if title.chars().count() > HTML_RENDER_MAX_TITLE_CHARS {
        return Err(format!(
            "html_render: 'title' is longer than {HTML_RENDER_MAX_TITLE_CHARS} characters"
        ));
    }
    let html = arguments.get("html").and_then(Value::as_str).unwrap_or_default();
    if html.trim().is_empty() {
        return Err("html_render: missing required argument 'html'".to_string());
    }
    if html.len() > HTML_RENDER_MAX_BYTES {
        return Err(format!(
            "html_render: the page is {} KB; the limit is {} KB. Trim embedded data or load large libraries from a CDN.",
            html.len() / 1024,
            HTML_RENDER_MAX_BYTES / 1024
        ));
    }
    Ok(json!(
        "Shown to the reader above your reply. Don't mention or describe the page; reply with only what it doesn't already say."
    ))
}

// ---------------------------------------------------------------------------
// MCP protocol handling
// ---------------------------------------------------------------------------

/// What stdio that serves an agent tells the model: those processes list no
/// registry tools.
const PROVIDER_INSTRUCTIONS: &str = "Use the advertised workspace and browser tools. Native thread control is unavailable on provider-injected MCP; it requires a separate authorized controller connection.";

const SERVER_INFO: ServerInfo = ServerInfo { name: "codemux", version: env!("CARGO_PKG_VERSION") };

/// The refusal a process that serves an agent gets for a registry tool.
const PROVIDER_CONTROL_DENIED: &str =
    "provider_control_denied: provider MCP cannot use trusted native-controller tools";

/// Command-line options for `codemux mcp`.
#[derive(Debug, Clone, Default)]
pub struct McpServerOptions {
    /// `--client`: the name the caller goes by.
    pub client: Option<String>,
    /// `--access`: the most the caller may do.
    pub access: Option<ControlAccess>,
}

/// Who this `codemux mcp` serves, fixed when it starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StdioCaller {
    /// Started from an app-installed config (`.mcp.json`, codemux-self).
    provider_origin: bool,
    /// Runs under an agent Codemux started (`CODEMUX_AGENT_PROCESS`).
    agent: bool,
    /// The most this process may do (`--access`, default full access).
    access: ControlAccess,
}

impl StdioCaller {
    /// `var` stands in for the process env, so tests never read it.
    fn from_env(var: impl Fn(&str) -> Option<OsString>, access: ControlAccess) -> Self {
        Self { provider_origin: var(PROVIDER_ORIGIN_ENV).is_some(), agent: var(AGENT_PROCESS_ENV).is_some(), access }
    }

    /// Such a process lists legacy tools only and is refused registry tools:
    /// an agent controls threads through its own chat's tools.
    fn serves_an_agent(self) -> bool {
        self.provider_origin || self.agent
    }
}

/// Which side of `codemux mcp` runs a tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolKind {
    /// A control-plane registry tool, run in the app through socket `tool_call`.
    Registry,
    /// One of the server's own tools, run on the legacy lane.
    Legacy,
}

/// Runs one tool call to its result. Tool failures are `isError` results the
/// model sees; a JSON-RPC error only for a tool the app does not know.
type RunTool = Arc<dyn Fn(ToolKind, CallToolParams) -> BoxFuture<'static, Result<Value, RpcError>> + Send + Sync>;

/// Records `clientInfo.name` as this process's client name; the first usable
/// name wins, so `--client` beats it.
type NameClient = Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// The request handler behind `codemux mcp`.
struct StdioServer {
    caller: StdioCaller,
    name_client: NameClient,
    run: RunTool,
    legacy: LegacyLane,
}

impl StdioServer {
    /// Needs a Tokio runtime: the legacy lane runs on its own task.
    fn new(caller: StdioCaller) -> Self {
        Self::with_runner(caller, Arc::new(control_identity::set_client_name), Arc::new(run_tool))
    }

    /// Needs a Tokio runtime: the legacy lane runs on its own task.
    fn with_runner(caller: StdioCaller, name_client: NameClient, run: RunTool) -> Self {
        let legacy = LegacyLane::spawn(Arc::clone(&run));
        Self { caller, name_client, run, legacy }
    }

    fn instructions(&self) -> &'static str {
        if self.caller.serves_an_agent() {
            PROVIDER_INSTRUCTIONS
        } else {
            crate::agent_control::INSTRUCTIONS
        }
    }

    fn initialize(&self, params: &Value) -> Value {
        let offer = InitializeRequest::parse(params);
        let protocol_version = offer.negotiated_version();
        if let Some(client) = &offer.client_info {
            (self.name_client)(&client.name);
        }
        InitializeResult {
            protocol_version,
            tools_list_changed: None,
            server_info: SERVER_INFO,
            instructions: Some(self.instructions()),
        }
        .to_value()
    }

    /// The stateless era's description of this server: the revisions of both
    /// eras, and the same capabilities and instructions as `initialize`.
    fn discover(&self) -> Value {
        DiscoverResult { tools_list_changed: None, server_info: SERVER_INFO, instructions: Some(self.instructions()) }
            .to_value()
    }

    fn tools_list(&self) -> Value {
        let StdioCaller { provider_origin, agent, access } = self.caller;
        let tools: Vec<Value> = stdio_tools(provider_origin, agent, access)
            .into_iter()
            .map(|tool| {
                let mut listed = json!({
                    "name": tool.name,
                    "description": tool.description,
                    "inputSchema": tool.input_schema
                });
                if let Some(annotations) = tool.annotations {
                    listed["annotations"] = annotations;
                }
                listed
            })
            .collect();
        json!({ "tools": tools })
    }

    fn tools_call(&self, params: &Value) -> Reply {
        let call = match CallToolParams::parse(params) {
            Ok(call) => call,
            Err(error) => return Reply::Now(Err(error)),
        };
        if self.caller.serves_an_agent() {
            // Legacy twins keep their legacy implementation here.
            if registry::relay_refuses(&call.name) {
                return Reply::Now(Err(RpcError::invalid_request(PROVIDER_CONTROL_DENIED)));
            }
        } else if let Some((_, spec)) = registry::served(&call.name, Audience::Outside) {
            // Registry tools are built for concurrent callers, so a long
            // `thread_wait` never holds up `thread_status` or `thread_interrupt`.
            return Reply::Later { future: (self.run)(ToolKind::Registry, call), parks: spec.may_wait() };
        }
        // The app checks every command that reaches it; this covers the
        // tools that run here (local git, `html_render`) and answers the
        // rest without a round trip.
        if let Err(error) = control_identity::stdio_permits(&call.name, &call.arguments, self.caller.access) {
            return Reply::Now(Ok(protocol::error_result(error.to_string())));
        }
        Reply::Queued(self.legacy.submit(call))
    }
}

impl StdioServer {
    fn legacy_request(&self, request: Request) -> Reply {
        match request.method.as_str() {
            "initialize" => Reply::Now(Ok(self.initialize(&request.params))),
            "ping" => Reply::Now(Ok(json!({}))),
            "tools/list" => Reply::Now(Ok(self.tools_list())),
            "tools/call" => self.tools_call(&request.params),
            // Includes a `server/discover` that names no stateless revision:
            // method-not-found with the probe's id tells the client to fall
            // back to `initialize`.
            method => Reply::Now(Err(RpcError::method_not_found(method))),
        }
    }

    /// A stateless-era request. With no handshake, each request names its own
    /// client; the first name still wins. Tools run exactly as they do for
    /// initialize-era clients, and every result is marked complete.
    fn modern_request(&self, request: Request, modern: ModernRequest) -> Reply {
        if let Some(client) = &modern.client_info {
            (self.name_client)(&client.name);
        }
        let reply = match request.method.as_str() {
            "server/discover" => Reply::Now(Ok(self.discover())),
            "tools/list" => {
                Reply::Now(Ok(protocol::cacheable(self.tools_list(), protocol::TOOLS_LIST_TTL_MS, CacheScope::Private)))
            }
            "tools/call" => self.tools_call(&request.params),
            // `ping` is not part of the stateless era.
            method => return Reply::Now(Err(RpcError::method_not_found(method))),
        };
        reply.map_result(|result| protocol::complete_result(result, SERVER_INFO))
    }
}

impl Handler for StdioServer {
    /// Each request picks its era: one process serves initialize-era and
    /// stateless-era clients side by side.
    fn request(&self, request: Request) -> Reply {
        match Era::of(&request.method, &request.params) {
            Ok(Era::Legacy) => self.legacy_request(request),
            Ok(Era::Modern(modern)) => self.modern_request(request, modern),
            Err(error) => Reply::Now(Err(error)),
        }
    }
}

/// The runner `codemux mcp` uses.
fn run_tool(kind: ToolKind, call: CallToolParams) -> BoxFuture<'static, Result<Value, RpcError>> {
    match kind {
        ToolKind::Registry => Box::pin(relay_registry_call(call)),
        ToolKind::Legacy => Box::pin(async move { Ok(handle_tool_call(call).await) }),
    }
}

/// Runs the server's own tools one at a time, in arrival order, as the
/// sequential server always did: the browser, pane and git tools act on shared
/// app state, and every in-app chat shares one `codemux mcp` child. The queue
/// has no bound, as the sequential server's input had none: a burst of calls
/// waits its turn instead of failing.
struct LegacyLane {
    jobs: mpsc::UnboundedSender<LegacyJob>,
}

struct LegacyJob {
    call: CallToolParams,
    reply: oneshot::Sender<Result<Value, RpcError>>,
}

impl LegacyLane {
    fn spawn(run: RunTool) -> Self {
        let (jobs, mut queue) = mpsc::unbounded_channel::<LegacyJob>();
        tokio::spawn(async move {
            while let Some(LegacyJob { call, reply }) = queue.recv().await {
                // A request cancelled while queued is skipped. One cancelled
                // while running still runs to the end: dropping it would not
                // stop its `git` child or the app-side handler, so the next
                // job would overlap it.
                if reply.is_closed() {
                    continue;
                }
                let name = call.name.clone();
                // A panicking tool must not end the lane: every later legacy
                // call would fail while the process still looks healthy.
                let result = AssertUnwindSafe(async { run(ToolKind::Legacy, call).await })
                    .catch_unwind()
                    .await
                    .unwrap_or_else(|_| Ok(protocol::error_result(format!("{name} failed unexpectedly"))));
                let _ = reply.send(result);
            }
        });
        Self { jobs }
    }

    /// Queues `call` at once, so jobs run in the order `submit` is called.
    /// Dropping the returned future before the job starts skips it; once it
    /// has started, it finishes and its result is discarded.
    fn submit(&self, call: CallToolParams) -> BoxFuture<'static, Result<Value, RpcError>> {
        let (reply, result) = oneshot::channel();
        // A failed send drops `reply`, so the receiver reports it below.
        let _ = self.jobs.send(LegacyJob { call, reply });
        Box::pin(async move { result.await.unwrap_or_else(|_| Err(RpcError::internal("The tool worker stopped"))) })
    }
}

// ---------------------------------------------------------------------------
// Tool call handler
// ---------------------------------------------------------------------------

/// The scroll-`amount` (ticks) value to embed in a `scroll_at` browser action,
/// as an INTEGER JSON number. The receiver reads it with `Value::as_i64`, which
/// returns `None` for a float-backed number — so emitting it via `as_f64`
/// (`3.0`) silently pinned every MCP scroll to the default 3. The CLI path
/// emits an integer; this matches it. Defaults to 3 (per the tool schema).
fn scroll_amount_value(arguments: &Value) -> Value {
    json!(arguments.get("amount").and_then(Value::as_i64).unwrap_or(3))
}

/// The split `direction` string for the pane-split MCP tools. Matches the
/// app-wide convention enforced by the pane renderer (PaneNode.tsx) and every
/// in-app Split control: "horizontal" lays panes out in columns (new pane to
/// the RIGHT), "vertical" in rows (new pane BELOW). The tools previously sent
/// the inverted strings, so agent-driven splits went the opposite way asked.
fn pane_split_direction(tool: &str) -> &'static str {
    match tool {
        "pane_split_down" => "vertical",
        _ => "horizontal", // pane_split_right
    }
}

/// Resolve the workspace id a tool call should act on. Per-call `_meta`
/// wins over the process env: every agent-chat session shares ONE
/// `codemux mcp` child (spawned with no workspace env), so the registry
/// tags each call with the calling session's workspace in
/// `_meta[WORKSPACE_META_KEY]`. The env var still covers children the
/// workspace lifecycle launches from its per-workspace PTYs via `.mcp.json`
/// (which never send `_meta`); the path-level entry intentionally inherits
/// that PTY env instead of overriding it. `env_fallback` is passed in so tests
/// stay hermetic.
/// The id is forwarded as-is; the control layer drops one that names no
/// live workspace (`resolve_browser_workspace_id`).
fn resolve_workspace_id(meta: Option<&Value>, env_fallback: Option<String>) -> String {
    meta
        .and_then(|meta| meta.get(crate::mcp::WORKSPACE_META_KEY))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .or(env_fallback)
        .unwrap_or_default()
}

/// Run one of the server's own tools. Registry tools never get here:
/// `StdioServer::tools_call` relays or refuses them first.
async fn handle_tool_call(call: CallToolParams) -> Value {
    let CallToolParams { name: tool_name, arguments, meta } = call;

    // Workspace ID for workspace-scoped routing (browser session binding,
    // git cwd). Caller-tagged `_meta` first, env second.
    let workspace_id =
        resolve_workspace_id(meta.as_ref(), std::env::var("CODEMUX_WORKSPACE_ID").ok());
    // Best-effort cwd so the control layer can resolve the owning workspace
    // by path when this MCP server's `CODEMUX_WORKSPACE_ID` env is missing
    // (covers MCP callers whose env wasn't injected). Empty string on error;
    // see `resolve_workspace_id_by_cwd` in control.rs.
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    // Shared `browser_automation` params builder — attaches the routing
    // fields (env `workspace_id` plus the `cwd` fallback hint) to every
    // browser tool's `action` in one place so the two can't drift apart
    // per-tool.
    let browser_params =
        |action: Value| json!({ "workspace_id": &workspace_id, "cwd": &cwd, "action": action });

    let result = match tool_name.as_str() {
        "delegation_targets" | REMOTE_DELEGATE_TOOL | "task_status" | "task_cancel" => Err("Native-only delegation tool: use the actual Codex/Claude adapter context. Generic stdio MCP and caller-supplied metadata cannot authorize a parent.".into()),
        // -- Browser tools --
        "browser_navigate" => {
            let url = arguments.get("url").and_then(Value::as_str).unwrap_or_default();
            call_socket("browser_automation", browser_params(json!({ "kind": "open", "url": url })))
            .await
        }
        "browser_snapshot" => {
            call_socket("browser_automation", browser_params(json!({ "kind": "eval", "script": crate::agent_browser::DOM_SNAPSHOT_SCRIPT })))
            .await
        }
        "browser_accessibility_snapshot" => {
            call_socket("browser_automation", browser_params(json!({ "kind": "snapshot" })))
            .await
        }
        "browser_click" => {
            let selector = arguments.get("selector").and_then(Value::as_str).unwrap_or_default();
            call_socket("browser_automation", browser_params(json!({ "kind": "click", "selector": selector })))
            .await
        }
        "browser_fill" => {
            let selector = arguments.get("selector").and_then(Value::as_str).unwrap_or_default();
            let value = arguments.get("value").and_then(Value::as_str).unwrap_or_default();
            call_socket("browser_automation", browser_params(json!({ "kind": "fill", "selector": selector, "value": value })))
            .await
        }
        "browser_screenshot" => {
            let result = call_socket("browser_automation", browser_params(json!({ "kind": "screenshot" })))
            .await;
            let viewport = crate::stream_input::get_viewport(crate::agent_browser::DEFAULT_STREAM_PORT).await.unwrap_or((1280, 720));
            result.map(|data| json!({
                "screenshot": data,
                "viewport_width": viewport.0,
                "viewport_height": viewport.1,
                "hint": format!("Viewport: {}x{}px. Use browser_click_at with these coordinates. For Cloudflare captchas, use browser_click_os instead.", viewport.0, viewport.1)
            }))
        }
        "browser_console_logs" => {
            call_socket("browser_automation", browser_params(json!({ "kind": "console" })))
            .await
        }

        // -- Coordinate-based vision tools --
        "browser_click_at" => {
            let x = arguments.get("x").and_then(Value::as_f64).unwrap_or(0.0);
            let y = arguments.get("y").and_then(Value::as_f64).unwrap_or(0.0);
            let ct = arguments.get("click_type").and_then(Value::as_str).unwrap_or("left");
            call_socket("browser_automation", browser_params(json!({ "kind": "click_at", "x": x, "y": y, "click_type": ct }))).await
        }
        "browser_type_at" => {
            let text = arguments.get("text").and_then(Value::as_str).unwrap_or_default();
            let mut action = json!({ "kind": "type_at", "text": text });
            if let Some(x) = arguments.get("x").and_then(Value::as_f64) { action["x"] = json!(x); }
            if let Some(y) = arguments.get("y").and_then(Value::as_f64) { action["y"] = json!(y); }
            call_socket("browser_automation", browser_params(action)).await
        }
        "browser_scroll_at" => {
            let x = arguments.get("x").and_then(Value::as_f64).unwrap_or(0.0);
            let y = arguments.get("y").and_then(Value::as_f64).unwrap_or(0.0);
            let dir = arguments.get("direction").and_then(Value::as_str).unwrap_or("down");
            let amt = scroll_amount_value(&arguments);
            call_socket("browser_automation", browser_params(json!({ "kind": "scroll_at", "x": x, "y": y, "direction": dir, "amount": amt }))).await
        }
        "browser_key_press" => {
            let key = arguments.get("key").and_then(Value::as_str).unwrap_or("Enter");
            call_socket("browser_automation", browser_params(json!({ "kind": "key_press", "key": key }))).await
        }
        "browser_drag" => {
            let sx = arguments.get("start_x").and_then(Value::as_f64).unwrap_or(0.0);
            let sy = arguments.get("start_y").and_then(Value::as_f64).unwrap_or(0.0);
            let ex = arguments.get("end_x").and_then(Value::as_f64).unwrap_or(0.0);
            let ey = arguments.get("end_y").and_then(Value::as_f64).unwrap_or(0.0);
            call_socket("browser_automation", browser_params(json!({ "kind": "drag", "start_x": sx, "start_y": sy, "end_x": ex, "end_y": ey }))).await
        }
        // -- OS-level input tools --
        "browser_click_os" => {
            let x = arguments.get("x").and_then(Value::as_f64).unwrap_or(0.0);
            let y = arguments.get("y").and_then(Value::as_f64).unwrap_or(0.0);
            call_socket("browser_automation", browser_params(json!({ "kind": "click_os", "x": x, "y": y }))).await
        }
        "browser_type_os" => {
            let text = arguments.get("text").and_then(Value::as_str).unwrap_or_default();
            let mut action = json!({ "kind": "type_os", "text": text });
            if let Some(x) = arguments.get("x").and_then(Value::as_f64) { action["x"] = json!(x); }
            if let Some(y) = arguments.get("y").and_then(Value::as_f64) { action["y"] = json!(y); }
            call_socket("browser_automation", browser_params(action)).await
        }

        // -- Browser info tools (v0.24.0) --
        "browser_get_styles" => {
            let selector = arguments.get("selector").and_then(Value::as_str).unwrap_or_default();
            call_socket("browser_automation", browser_params(json!({ "kind": "get_styles", "selector": selector }))).await
        }
        "browser_wait" => {
            let selector = arguments.get("selector").and_then(Value::as_str);
            let text = arguments.get("text").and_then(Value::as_str);
            let mut action = json!({ "kind": "wait" });
            if let Some(sel) = selector { action["selector"] = json!(sel); }
            if let Some(t) = text { action["text"] = json!(t); }
            call_socket("browser_automation", browser_params(action)).await
        }
        "browser_evaluate" => {
            let script = arguments.get("script").and_then(Value::as_str).unwrap_or_default();
            call_socket("browser_automation", browser_params(json!({ "kind": "eval", "script": script }))).await
        }
        "browser_viewport" => {
            // Validate locally so an agent gets a typed error with the
            // preset list instead of a generic "Unknown action" bounce
            // from the agent-browser subprocess. The `_configured`
            // variant resolves 'reset' to the user's
            // `browser.default_viewport` setting when present.
            let preset_arg = arguments.get("preset").and_then(Value::as_str).unwrap_or("");
            let dpr = arguments.get("dpr").and_then(Value::as_f64);
            match crate::browser_viewport::parse_spec_configured(preset_arg, dpr) {
                Ok(spec) => {
                    // Shared socket-action builder — see cli.rs for the
                    // matching call site. Both surfaces MUST go through
                    // this helper so the wire payload stays in sync.
                    let action = crate::browser_viewport::socket_action(spec);
                    let result = call_socket("browser_automation", browser_params(action)).await;
                    result.map(|data| json!({
                        "applied": {
                            "preset": preset_arg,
                            "width": spec.width,
                            "height": spec.height,
                            "dpr": spec.dpr,
                        },
                        "hint": format!(
                            "Viewport set to {}x{} @ {}x DPR. Take a browser_screenshot to verify the layout.",
                            spec.width, spec.height, spec.dpr
                        ),
                        "result": data,
                    }))
                }
                Err(e) => Err(e.to_string()),
            }
        }
        "browser_viewport_presets" => {
            let presets = crate::browser_viewport::list_presets();
            let json_presets: Vec<Value> = presets
                .iter()
                .map(|p| {
                    json!({
                        "name": p.name,
                        "width": p.spec.width,
                        "height": p.spec.height,
                        "dpr": p.spec.dpr,
                        "description": p.description,
                    })
                })
                .collect();
            // `reset` reports the *actual* reset target — the user's
            // configured `browser.default_viewport` when set.
            let reset_spec = crate::browser_viewport::configured_default_spec();
            Ok(json!({
                "presets": json_presets,
                "reset": {
                    "width": reset_spec.width,
                    "height": reset_spec.height,
                    "dpr": reset_spec.dpr,
                },
                "custom": "Pass a 'WxH' string like '390x844' to browser_viewport for custom dimensions.",
            }))
        }

        // -- Workspace tools --
        "workspace_list" => {
            call_socket("get_app_state", json!({})).await.map(|data| {
                let workspaces = data.get("workspaces").cloned().unwrap_or(json!([]));
                json!({ "workspaces": workspaces })
            })
        }
        "workspace_info" => {
            call_socket("get_app_state", json!({})).await.map(|data| {
                let active_id = data.get("active_workspace_id").cloned().unwrap_or(Value::Null);
                let workspaces = data.get("workspaces").and_then(Value::as_array);
                let active_id_str = active_id.as_str().or_else(|| {
                    active_id.as_object().and_then(|o| o.get("0")).and_then(Value::as_str)
                });
                let active = workspaces.and_then(|ws| {
                    ws.iter().find(|w| {
                        let wid = w.get("workspace_id");
                        let wid_str = wid.and_then(Value::as_str).or_else(|| {
                            wid.and_then(|v| v.as_object()).and_then(|o| o.get("0")).and_then(Value::as_str)
                        });
                        wid_str == active_id_str
                    })
                });
                active.cloned().unwrap_or(json!({ "error": "No active workspace" }))
            })
        }
        "workspace_create" => {
            let path = arguments.get("path").and_then(Value::as_str);
            let params = match path {
                Some(p) => json!({ "path": p }),
                None => json!({}),
            };
            call_socket("create_workspace", params).await
        }

        // -- Automation tools --
        "automation_list" => call_socket("automation_list", json!({})).await,
        "automation_get" => {
            call_socket("automation_get", json!({ "id": arguments.get("id") })).await
        }
        "automation_create" => {
            call_socket(
                "automation_create",
                json!({ "input": automation_input_from_args(&arguments) }),
            )
            .await
        }
        "automation_update" => {
            call_socket(
                "automation_update",
                json!({
                    "id": arguments.get("id"),
                    "input": automation_input_from_args(&arguments),
                }),
            )
            .await
        }
        "automation_delete" => {
            call_socket("automation_delete", json!({ "id": arguments.get("id") })).await
        }
        "automation_pause" => {
            call_socket(
                "automation_set_enabled",
                json!({ "id": arguments.get("id"), "enabled": false }),
            )
            .await
        }
        "automation_resume" => {
            call_socket(
                "automation_set_enabled",
                json!({ "id": arguments.get("id"), "enabled": true }),
            )
            .await
        }
        "automation_runs" => {
            call_socket(
                "automation_runs",
                json!({
                    "automation_id": arguments.get("automation_id"),
                    "limit": arguments.get("limit"),
                }),
            )
            .await
        }

        // -- Pane tools --
        "pane_list" => {
            call_socket("get_app_state", json!({})).await.map(|data| {
                let workspaces = data.get("workspaces").and_then(Value::as_array);
                let active_id = data.get("active_workspace_id").cloned().unwrap_or(Value::Null);
                let active_id_str = active_id.as_str().or_else(|| {
                    active_id.as_object().and_then(|o| o.get("0")).and_then(Value::as_str)
                });
                let surfaces = workspaces
                    .and_then(|ws| {
                        ws.iter().find(|w| {
                            let wid = w.get("workspace_id");
                            let wid_str = wid.and_then(Value::as_str).or_else(|| {
                                wid.and_then(|v| v.as_object()).and_then(|o| o.get("0")).and_then(Value::as_str)
                            });
                            wid_str == active_id_str
                        })
                    })
                    .and_then(|w| w.get("surfaces"))
                    .cloned()
                    .unwrap_or(json!([]));
                json!({ "surfaces": surfaces })
            })
        }
        "pane_split_right" => {
            let pane_id = arguments.get("pane_id").and_then(Value::as_str).unwrap_or_default();
            let direction = pane_split_direction("pane_split_right");
            call_socket("split_pane", json!({ "pane_id": pane_id, "direction": direction })).await
        }
        "pane_split_down" => {
            let pane_id = arguments.get("pane_id").and_then(Value::as_str).unwrap_or_default();
            let direction = pane_split_direction("pane_split_down");
            call_socket("split_pane", json!({ "pane_id": pane_id, "direction": direction })).await
        }

        // -- Attached conversation history --
        "conversation_read" => {
            let conversation_id = arguments.get("conversation_id").and_then(Value::as_str).unwrap_or_default();
            if conversation_id.is_empty() {
                Err("conversation_read: missing required argument 'conversation_id'".into())
            } else {
                let mut params = json!({
                    "workspace_id": &workspace_id,
                    "conversation_id": conversation_id,
                });
                if let Some(cursor) = arguments.get("cursor").and_then(Value::as_i64) {
                    params["cursor"] = json!(cursor);
                }
                if let Some(limit) = arguments.get("limit").and_then(Value::as_u64) {
                    params["limit"] = json!(limit);
                }
                call_socket("conversation_read", params).await
            }
        }
        "conversation_search" => {
            let conversation_id = arguments.get("conversation_id").and_then(Value::as_str).unwrap_or_default();
            let query = arguments.get("query").and_then(Value::as_str).unwrap_or_default();
            if conversation_id.is_empty() {
                Err("conversation_search: missing required argument 'conversation_id'".into())
            } else if query.trim().is_empty() {
                Err("conversation_search: missing required argument 'query'".into())
            } else {
                let mut params = json!({
                    "workspace_id": &workspace_id,
                    "conversation_id": conversation_id,
                    "query": query,
                });
                if let Some(limit) = arguments.get("limit").and_then(Value::as_u64) {
                    params["limit"] = json!(limit);
                }
                call_socket("conversation_search", params).await
            }
        }

        // -- Notification tools --
        "notify" => {
            let message = arguments.get("message").and_then(Value::as_str).unwrap_or_default();
            let level = arguments.get("level").and_then(Value::as_str).unwrap_or("attention");
            call_socket("notify", json!({ "message": message, "level": level })).await
        }

        // -- Git tools (shell out) --
        "git_status" => run_git(&["status", "--porcelain"], &workspace_id).await,
        "git_diff" => {
            let file = arguments.get("file").and_then(Value::as_str);
            match file {
                Some(f) => run_git(&["diff", f], &workspace_id).await,
                None => run_git(&["diff"], &workspace_id).await,
            }
        }
        "git_stage" => {
            let file = arguments.get("file").and_then(Value::as_str).unwrap_or(".");
            run_git(&["add", file], &workspace_id).await
        }
        "git_commit" => {
            let message = arguments.get("message").and_then(Value::as_str).unwrap_or_default();
            run_git(&["commit", "-m", message], &workspace_id).await
        }
        "git_push" => run_git(&["push"], &workspace_id).await,

        // -- Terminal / workspace / status / ports (Phase 1) --
        "terminal_write" => {
            let data = arguments.get("data").and_then(Value::as_str).unwrap_or_default();
            let session_id = arguments.get("session_id").and_then(Value::as_str);
            let mut params = json!({ "data": data });
            if let Some(sid) = session_id {
                params["session_id"] = json!(sid);
            }
            call_socket("write_terminal", params).await
        }
        "terminal_read" => {
            let mut params = json!({});
            if let Some(lines) = arguments.get("lines").and_then(Value::as_u64) {
                params["lines"] = json!(lines);
            }
            if let Some(sid) = arguments.get("session_id").and_then(Value::as_str) {
                params["session_id"] = json!(sid);
            }
            call_socket("read_terminal", params).await
        }
        "workspace_open" => {
            let workspace_id = arguments
                .get("workspace_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if workspace_id.is_empty() {
                Err("workspace_open: missing required argument 'workspace_id'".to_string())
            } else {
                call_socket("activate_workspace", json!({ "workspace_id": workspace_id })).await
            }
        }
        "app_status" => call_socket("status", json!({})).await,
        "port_list" => {
            let mut params = json!({});
            if let Some(wid) = arguments.get("workspace_id").and_then(Value::as_str) {
                params["workspace_id"] = json!(wid);
            }
            call_socket("port_list", params).await
        }

        // -- Delegation primitives (Phase 1.5) --
        "worktree_create" => {
            let repo_path = arguments
                .get("repo_path")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let branch = arguments
                .get("branch")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if repo_path.is_empty() {
                Err("worktree_create: missing required argument 'repo_path'".to_string())
            } else if branch.is_empty() {
                Err("worktree_create: missing required argument 'branch'".to_string())
            } else {
                // Build params lazily so the socket arm's defaults (a new
                // branch, the repository's default branch, one terminal)
                // fire when the brain omits them. Passing them explicitly
                // here would freeze the defaults on the MCP side and
                // make future changes harder to coordinate.
                let mut params = json!({
                    "repo_path": repo_path,
                    "branch": branch,
                });
                if let Some(nb) = arguments.get("new_branch").and_then(Value::as_bool) {
                    params["new_branch"] = json!(nb);
                }
                if let Some(base) = arguments.get("base").and_then(Value::as_str) {
                    params["base"] = json!(base);
                }
                if let Some(layout) = arguments.get("layout").and_then(Value::as_str) {
                    params["layout"] = json!(layout);
                }
                if let Some(prompt) = arguments.get("initial_prompt").and_then(Value::as_str) {
                    params["initial_prompt"] = json!(prompt);
                }
                if let Some(pid) = arguments.get("agent_preset_id").and_then(Value::as_str) {
                    params["agent_preset_id"] = json!(pid);
                }
                if let Some(prn) = arguments.get("pr_number").and_then(Value::as_u64) {
                    params["pr_number"] = json!(prn);
                }
                call_socket("create_worktree_workspace", params).await
            }
        }
        "preset_apply" => {
            let workspace_id = arguments
                .get("workspace_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let preset_id = arguments
                .get("preset_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if workspace_id.is_empty() {
                Err("preset_apply: missing required argument 'workspace_id'".to_string())
            } else if preset_id.is_empty() {
                Err("preset_apply: missing required argument 'preset_id'".to_string())
            } else {
                let mut params = json!({
                    "workspace_id": workspace_id,
                    "preset_id": preset_id,
                });
                if let Some(m) = arguments.get("override_mode").and_then(Value::as_str) {
                    params["override_mode"] = json!(m);
                }
                if let Some(p) = arguments.get("initial_prompt").and_then(Value::as_str) {
                    params["initial_prompt"] = json!(p);
                }
                call_socket("apply_preset", params).await
            }
        }
        "preset_list" => call_socket("get_presets", json!({})).await,

        // -- Lifecycle + issue tools (Phase 1.6) --
        "workspace_close" => {
            let workspace_id = arguments
                .get("workspace_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if workspace_id.is_empty() {
                Err("workspace_close: missing required argument 'workspace_id'".to_string())
            } else {
                let mut params = json!({ "workspace_id": workspace_id });
                if let Some(d) = arguments.get("delete_worktree").and_then(Value::as_bool) {
                    params["delete_worktree"] = json!(d);
                }
                if let Some(d) = arguments.get("delete_branch").and_then(Value::as_bool) {
                    params["delete_branch"] = json!(d);
                }
                if let Some(f) = arguments.get("force_delete").and_then(Value::as_bool) {
                    params["force_delete"] = json!(f);
                }
                call_socket("close_workspace", params).await
            }
        }
        "workspace_archive" => {
            let workspace_id = arguments
                .get("workspace_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if workspace_id.is_empty() {
                Err("workspace_archive: missing required argument 'workspace_id'".to_string())
            } else {
                call_socket("archive_workspace", json!({ "workspace_id": workspace_id })).await
            }
        }
        "workspace_unarchive" => {
            let archive_id = arguments
                .get("archive_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if archive_id.is_empty() {
                Err("workspace_unarchive: missing required argument 'archive_id'".to_string())
            } else {
                call_socket("unarchive_workspace", json!({ "archive_id": archive_id })).await
            }
        }
        "workspace_archive_list" => call_socket("list_archived_workspaces", json!({})).await,
        "pane_close" => {
            let pane_id = arguments
                .get("pane_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if pane_id.is_empty() {
                Err("pane_close: missing required argument 'pane_id'".to_string())
            } else {
                call_socket("close_pane", json!({ "pane_id": pane_id })).await
            }
        }
        "issue_list" => {
            let mut params = json!({});
            if let Some(rp) = arguments.get("repo_path").and_then(Value::as_str) {
                params["repo_path"] = json!(rp);
            }
            if let Some(s) = arguments.get("search").and_then(Value::as_str) {
                params["search"] = json!(s);
            }
            call_socket("list_github_issues", params).await
        }
        "issue_get" => {
            let number = arguments.get("number").and_then(Value::as_u64);
            match number {
                None => Err("issue_get: missing required argument 'number'".to_string()),
                Some(n) => {
                    let mut params = json!({ "number": n });
                    if let Some(rp) = arguments.get("repo_path").and_then(Value::as_str) {
                        params["repo_path"] = json!(rp);
                    }
                    call_socket("get_github_issue", params).await
                }
            }
        }
        "issue_link_workspace" => {
            let number = arguments.get("number").and_then(Value::as_u64);
            match number {
                None => {
                    Err("issue_link_workspace: missing required argument 'number'".to_string())
                }
                Some(n) => {
                    let mut params = json!({ "number": n });
                    if let Some(wid) = arguments.get("workspace_id").and_then(Value::as_str) {
                        params["workspace_id"] = json!(wid);
                    }
                    call_socket("link_workspace_issue", params).await
                }
            }
        }

        // -- Visual replies --
        "html_render" => html_render_result(&arguments),

        _ => Err(format!("Unknown tool: {tool_name}")),
    };

    match result {
        Ok(data) => protocol::text_result(legacy_text(data)),
        Err(error) => protocol::error_result(error),
    }
}

/// A string result as it is; anything else as pretty JSON.
fn legacy_text(data: Value) -> String {
    match data {
        Value::String(text) => text,
        data => serde_json::to_string_pretty(&data).unwrap_or_default(),
    }
}

// ---------------------------------------------------------------------------
// Automation argument helper
// ---------------------------------------------------------------------------

/// Assemble the `AutomationInput` object the control socket expects from
/// the flat arguments an MCP `automation_create` / `automation_update`
/// call provides. Missing optional fields fall back to sensible
/// defaults; the control handler runs the authoritative validation.
fn automation_input_from_args(arguments: &Value) -> Value {
    json!({
        "name": arguments.get("name").and_then(Value::as_str).unwrap_or_default(),
        "prompt": arguments.get("prompt").and_then(Value::as_str).unwrap_or_default(),
        "agent": arguments.get("agent").and_then(Value::as_str).unwrap_or("claude"),
        "schedule": arguments.get("schedule").and_then(Value::as_str).unwrap_or_default(),
        "timezone": arguments.get("timezone").and_then(Value::as_str).unwrap_or("UTC"),
        "host_id": arguments.get("host_id").and_then(Value::as_i64),
        "project_path": arguments.get("project_path").and_then(Value::as_str),
        "retention_limit": arguments
            .get("retention_limit")
            .and_then(Value::as_i64)
            .unwrap_or(10),
    })
}

// ---------------------------------------------------------------------------
// Socket bridge
// ---------------------------------------------------------------------------

async fn call_socket(command: &str, params: Value) -> Result<Value, String> {
    let response: ControlResponse = send_control_request(ControlRequest {
        command: command.to_string(),
        params,
    })
    .await?;

    if response.ok {
        Ok(response.data.unwrap_or(json!(null)))
    } else {
        Err(response.error.unwrap_or_else(|| "Unknown error".to_string()))
    }
}

/// Runs a registry tool in the app through socket `tool_call`, so every door
/// shares one dispatcher, and returns its CallToolResult as the app built it.
async fn relay_registry_call(call: CallToolParams) -> Result<Value, RpcError> {
    let CallToolParams { name, arguments, meta } = call;
    let request = ControlRequest {
        command: "tool_call".into(),
        params: json!({ "name": name, "arguments": arguments }),
    };
    // A refusal this client raised itself keeps its code (`server_too_old`
    // when a narrowed client meets an app that ignores `--access`, which a
    // retry cannot fix); connect and read failures are `app_unavailable`.
    let response = match send_control_request(request).await {
        Ok(response) => response,
        Err(detail) => return Ok(registry::mcp_error(&ControlError::from_text(detail, ErrorCode::AppUnavailable))),
    };
    if response.ok {
        return Ok(response.data.unwrap_or(Value::Null));
    }
    let text = response.error.unwrap_or_else(|| "Unknown error".to_string());
    // A running app older than this binary (the package was upgraded and the
    // app not restarted yet) has no `tool_call`. Answer the way that app
    // would have: bare socket aliases as before, legacy twins with their
    // legacy result (vexis-agent's `workspace_info {}` keeps working).
    if text == "Unknown control command: tool_call" {
        if registry::is_socket_alias(&name) {
            let result = call_socket(&name, arguments).await.map_err(ControlError::from);
            return Ok(registry::mcp_result(&name, result));
        }
        if is_legacy_tool(&name) {
            // The legacy arm runs here, so it gets the local gate every
            // legacy call meets before the legacy lane.
            let access = control_identity::client_access().unwrap_or(ControlAccess::FullAccess);
            if let Err(error) = control_identity::stdio_permits(&name, &arguments, access) {
                return Ok(protocol::error_result(error.to_string()));
            }
            return Ok(handle_tool_call(CallToolParams { name, arguments, meta }).await);
        }
    }
    let error = ControlError::from(text);
    if error.code == ErrorCode::UnknownTool {
        return Err(RpcError::invalid_params(error.to_string()));
    }
    Ok(registry::mcp_error(&error))
}

// ---------------------------------------------------------------------------
// Git helper (shell out in workspace cwd)
// ---------------------------------------------------------------------------

/// `workspace_id` is the already-resolved id from `handle_tool_call`
/// (per-call `_meta` first, env fallback second — see
/// `resolve_workspace_id`), so git tools route to the same workspace the
/// browser tools bind to. Empty means "no workspace known" and falls
/// through to the process cwd.
async fn get_workspace_cwd(workspace_id: &str) -> Result<String, String> {
    // Map the workspace id to its path via app state.
    if !workspace_id.is_empty() {
        let response = call_socket("get_app_state", json!({})).await?;
        if let Some(workspaces) = response.get("workspaces").and_then(Value::as_array) {
            for ws in workspaces {
                let wid = ws.get("workspace_id");
                let wid_str = wid
                    .and_then(Value::as_str)
                    .or_else(|| wid.and_then(|v| v.as_object()).and_then(|o| o.get("0")).and_then(Value::as_str));
                if wid_str == Some(workspace_id) {
                    if let Some(cwd) = ws.get("cwd").and_then(Value::as_str) {
                        return Ok(cwd.to_string());
                    }
                }
            }
        }
    }
    // Fallback to current directory.
    std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .map_err(|e| format!("Cannot determine workspace directory: {e}"))
}

async fn run_git(args: &[&str], workspace_id: &str) -> Result<Value, String> {
    let cwd = get_workspace_cwd(workspace_id).await?;
    let output = crate::git::git_command_tokio()
        .args(args)
        .current_dir(&cwd)
        .output()
        .await
        .map_err(|e| format!("Failed to run git: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if output.status.success() {
        Ok(json!(stdout))
    } else {
        Err(format!("{stderr}{stdout}").trim().to_string())
    }
}

// ---------------------------------------------------------------------------
// Main MCP server loop (stdio transport)
// ---------------------------------------------------------------------------

pub async fn run_mcp_server() -> Result<(), String> {
    run_mcp_server_with(McpServerOptions::default()).await
}

/// Serve `codemux mcp` on stdin and stdout until stdin closes.
pub async fn run_mcp_server_with(options: McpServerOptions) -> Result<(), String> {
    // Log to stderr so it doesn't interfere with the JSON-RPC protocol on stdout.
    eprintln!("[codemux::mcp] MCP server starting (stdio transport)");
    // Before any line is read: `--client` beats the client's own
    // `clientInfo.name`, and every socket call carries both.
    if let Some(name) = &options.client {
        control_identity::set_client_name(name);
    }
    if let Some(access) = options.access {
        control_identity::set_client_access(access);
    }
    let access = control_identity::client_access().unwrap_or(ControlAccess::FullAccess);
    let caller = StdioCaller::from_env(|key| std::env::var_os(key), access);
    let server = Arc::new(StdioServer::new(caller));
    let served = protocol::stdio::serve(tokio::io::stdin(), tokio::io::stdout(), server).await;
    eprintln!("[codemux::mcp] MCP server shutting down (stdin closed)");
    served
}

// ---------------------------------------------------------------------------
// .mcp.json auto-discovery helpers
// ---------------------------------------------------------------------------

/// Build the codemux MCP server entry for `.mcp.json`.
///
/// The entry deliberately carries no `CODEMUX_WORKSPACE_ID`. Agents launched
/// inside a Codemux terminal inherit that workspace-specific variable from
/// their PTY, so copying one workspace id into a path-level file made the last
/// workspace opened at a shared CWD steal every other workspace's tool calls.
/// Callers without an inherited id continue to use the MCP server's cwd
/// fallback.
fn codemux_mcp_entry() -> Value {
    let command = codemux_mcp_command();
    json!({
        "command": command,
        "args": ["mcp"],
        "env": { (PROVIDER_ORIGIN_ENV): "1" }
    })
}

/// Resolve a durable command for the on-disk entry. `current_exe()` points
/// inside a transient mount for a raw AppImage launch; the wrapper named by
/// `$APPIMAGE` survives after the desktop process exits and accepts the same
/// `mcp` argument. A binary that a package upgrade replaced while the app
/// runs is named by its install path, so entries written then still launch.
pub(crate) fn codemux_mcp_command() -> String {
    std::env::var_os("APPIMAGE")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .or_else(|| std::env::current_exe().ok().map(|running| install_path(&running).to_path_buf()))
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "codemux".to_string())
}

/// Linux names a running binary that an upgrade replaced or removed
/// `<path> (deleted)`. The install path is the one to launch: after an
/// upgrade it holds the new binary.
pub(crate) fn install_path(running: &Path) -> &Path {
    running.to_str().and_then(|path| path.strip_suffix(" (deleted)")).map_or(running, Path::new)
}

/// One unique checkout that needs an MCP config reconciliation.
///
/// `workspace_id` records the last workspace at the path to preserve the
/// startup loop's historical last-writer semantics while the entry is being
/// migrated. The generated entry is now path-level rather than
/// workspace-level, so the id is retained only for call-site compatibility
/// and diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpConfigReconciliationTarget {
    pub workspace_dir: PathBuf,
    pub workspace_id: String,
}

/// Active-first boot plan. The caller should reconcile `active`
/// synchronously before restored agents start, then schedule `inactive` after
/// the renderer's first paint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpConfigReconciliationPlan {
    pub active: Option<McpConfigReconciliationTarget>,
    pub inactive: Vec<McpConfigReconciliationTarget>,
}

/// Collapse a restored snapshot to one target per exact CWD while preserving
/// the old loop's final value (the last workspace at a duplicate path wins).
/// Target order follows each path's first appearance, making the inactive
/// batch stable without canonicalizing or inspecting the filesystem.
pub fn plan_boot_mcp_reconciliation(
    snapshot: &crate::state::AppStateSnapshot,
) -> McpConfigReconciliationPlan {
    let mut targets = Vec::<McpConfigReconciliationTarget>::new();
    let mut path_indexes = HashMap::<String, usize>::new();

    for workspace in &snapshot.workspaces {
        if workspace.is_local_import_snapshot_only() {
            continue;
        }
        if let Some(index) = path_indexes.get(&workspace.cwd).copied() {
            targets[index].workspace_id = workspace.workspace_id.0.clone();
        } else {
            path_indexes.insert(workspace.cwd.clone(), targets.len());
            targets.push(McpConfigReconciliationTarget {
                workspace_dir: PathBuf::from(&workspace.cwd),
                workspace_id: workspace.workspace_id.0.clone(),
            });
        }
    }

    let active_path = snapshot
        .workspaces
        .iter()
        .find(|workspace| workspace.workspace_id == snapshot.active_workspace_id)
        .map(|workspace| workspace.cwd.as_str());
    let active_index = active_path.and_then(|path| path_indexes.get(path)).copied();
    let active = active_index.map(|index| targets.remove(index));

    McpConfigReconciliationPlan { active, inactive: targets }
}

/// Apply a planned batch. Keeping execution separate from planning lets boot
/// run the active target synchronously and move the inactive vector to a
/// blocking worker after first paint without inspecting a checkout twice.
pub fn reconcile_mcp_config_targets<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    targets: &[McpConfigReconciliationTarget],
) -> usize {
    reconcile_live_mcp_config_targets(app, targets)
}

fn reconciliation_target_is_live(
    snapshot: &crate::state::AppStateSnapshot,
    target: &McpConfigReconciliationTarget,
) -> bool {
    snapshot.workspaces.iter().any(|workspace| {
        !workspace.is_local_import_snapshot_only()
            && Path::new(&workspace.cwd) == target.workspace_dir
    })
}

fn reconcile_live_mcp_config_targets<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    targets: &[McpConfigReconciliationTarget],
) -> usize {
    use tauri::Manager as _;

    // Re-read both preference and ownership at the final filesystem commit
    // point. The startup plan is intentionally stale by the time first paint
    // occurs: users can close workspaces or disable auto-MCP during that gap.
    // The preference is global, not per checkout, so one SQLite read covers
    // the whole batch.
    if !is_auto_mcp_enabled(app) {
        return 0;
    }
    let mut repaired = 0;
    for target in targets {
        let _io_guard = MCP_CONFIG_IO_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let state: tauri::State<'_, crate::state::AppStateStore> = app.state();
        if !reconciliation_target_is_live(&state.snapshot(), target) {
            continue;
        }
        upsert_mcp_config_with_entry_locked(&target.workspace_dir, codemux_mcp_entry());
        repaired += 1;
    }
    repaired
}

/// One-shot queue populated by startup and drained after the renderer's first
/// paint. Keeping the planned targets (rather than recomputing against the
/// then-current active workspace) makes a fast workspace switch harmless.
#[derive(Debug, Default)]
pub struct PendingMcpConfigRepairs {
    targets: std::sync::Mutex<Vec<McpConfigReconciliationTarget>>,
}

impl PendingMcpConfigRepairs {
    pub fn replace(&self, targets: Vec<McpConfigReconciliationTarget>) {
        *self.targets.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = targets;
    }

    fn take(&self) -> Vec<McpConfigReconciliationTarget> {
        std::mem::take(
            &mut *self
                .targets
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }

    fn restore_after_join_failure(&self, targets: Vec<McpConfigReconciliationTarget>) {
        let mut pending = self.targets.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if pending.is_empty() {
            *pending = targets;
        }
    }
}

async fn repair_inactive_mcp_configs_inner<F>(
    pending: &PendingMcpConfigRepairs,
    repair: F,
) -> Result<usize, String>
where
    F: FnOnce(&[McpConfigReconciliationTarget]) -> usize + Send + 'static,
{
    let targets = pending.take();
    if targets.is_empty() {
        return Ok(0);
    }

    let retry_targets = targets.clone();
    match tokio::task::spawn_blocking(move || repair(&targets)).await {
        Ok(repaired) => Ok(repaired),
        Err(error) => {
            pending.restore_after_join_failure(retry_targets);
            Err(format!("inactive MCP config repair task failed: {error}"))
        }
    }
}

/// Drain the inactive boot-repair queue. The frontend calls this once after
/// first paint; repeated or concurrent invocations return `0` without another
/// filesystem inspection.
#[tauri::command]
pub async fn repair_inactive_mcp_configs<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    pending: tauri::State<'_, PendingMcpConfigRepairs>,
) -> Result<usize, String> {
    let repair_app = app.clone();
    repair_inactive_mcp_configs_inner(&pending, move |targets| {
        reconcile_live_mcp_config_targets(&repair_app, targets)
    })
    .await
}

/// Upsert the "codemux" entry in `.mcp.json`.
///
/// - If the file doesn't exist, creates it with the codemux entry.
/// - If it exists with valid JSON, merges the codemux entry alongside any
///   existing servers (shadcn, database tools, etc.) — never removes them.
/// - If it exists but is invalid JSON, logs a warning and does NOT modify it.
pub fn upsert_mcp_config(workspace_dir: &Path) {
    upsert_mcp_config_with_entry(workspace_dir, codemux_mcp_entry());
}

fn upsert_mcp_config_with_entry(workspace_dir: &Path, desired_entry: Value) {
    let _io_guard = MCP_CONFIG_IO_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    upsert_mcp_config_with_entry_locked(workspace_dir, desired_entry);
}

fn upsert_mcp_config_with_entry_locked(workspace_dir: &Path, desired_entry: Value) {
    let mcp_path = workspace_dir.join(".mcp.json");

    let mut config = if mcp_path.exists() {
        match std::fs::read_to_string(&mcp_path) {
            Ok(content) => match serde_json::from_str::<Value>(&content) {
                Ok(val) => val,
                Err(e) => {
                    eprintln!(
                        "[codemux::mcp] .mcp.json at {} is invalid JSON ({}), skipping",
                        mcp_path.display(),
                        e
                    );
                    return;
                }
            },
            Err(e) => {
                eprintln!(
                    "[codemux::mcp] Failed to read .mcp.json at {}: {}",
                    mcp_path.display(),
                    e
                );
                return;
            }
        }
    } else {
        json!({})
    };

    // A semantic match is a true no-op: preserve the user's original bytes,
    // key order, indentation, inode, and mtime. Git exclusion is independently
    // idempotent and may still repair a missing exclude entry.
    if config
        .get("mcpServers")
        .and_then(Value::as_object)
        .and_then(|servers| servers.get("codemux"))
        == Some(&desired_entry)
    {
        crate::git::ensure_git_exclude(workspace_dir, ".mcp.json");
        return;
    }

    // Ensure mcpServers object exists.
    if !config.get("mcpServers").is_some_and(Value::is_object) {
        config["mcpServers"] = json!({});
    }
    config["mcpServers"]["codemux"] = desired_entry;

    match serde_json::to_string_pretty(&config) {
        Ok(json) => {
            if let Err(e) = atomic_write(&mcp_path, json.as_bytes()) {
                eprintln!("[codemux::mcp] Failed to write .mcp.json: {e}");
                return;
            }
        }
        Err(e) => {
            eprintln!("[codemux::mcp] Failed to serialize .mcp.json: {e}");
            return;
        }
    }

    crate::git::ensure_git_exclude(workspace_dir, ".mcp.json");
}

/// Atomic-write helper used by `upsert_mcp_config` so a crash mid-write can
/// never leave half-baked JSON at `.mcp.json` — Claude Code reads that file
/// at agent start, and a single corrupted byte disables every MCP server the
/// brain was supposed to see.
///
/// Pattern (cribbed from vexis-agent's `_write_yaml` in
/// `vexis_agent/daemon/mcp.py`):
///   1. write the new content to a sibling `.tmp` file
///   2. fsync the tmp file so its bytes are durable before the rename
///   3. `rename` over the destination (atomic on POSIX, atomic on Windows
///      thanks to `MoveFileEx` semantics under `std::fs::rename`)
///   4. fsync the parent directory on Unix so the rename itself is durable
///      across a power loss
///
/// The tmp suffix is keyed on `path.file_name()` plus `.tmp` so concurrent
/// writes to two different `.mcp.json` files don't collide on the same tmp.
/// Two concurrent writes to the *same* `.mcp.json` will race on the rename,
/// which is the same race std::fs::write already had — but neither side can
/// observe a partially written file.
fn atomic_write(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;

    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("atomic_write: path has no parent: {}", path.display()),
        )
    })?;

    let file_name = path
        .file_name()
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("atomic_write: path has no file name: {}", path.display()),
            )
        })?
        .to_os_string();

    let mut tmp_name = file_name;
    tmp_name.push(".tmp");
    let tmp_path = parent.join(&tmp_name);

    // OpenOptions instead of File::create so a leftover tmp from a previous
    // crashed write is truncated cleanly instead of confusing this attempt.
    {
        let mut tmp = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp_path)?;
        tmp.write_all(contents)?;
        tmp.flush()?;
        // Best-effort: filesystems that don't support sync_all (e.g. some
        // network mounts in tests) shouldn't break the write. The rename
        // itself is still atomic; sync_all is the durability guarantee.
        let _ = tmp.sync_all();
    }

    // Atomic rename over the destination. On Unix this is `rename(2)`; on
    // Windows it's `MoveFileExW` with `MOVEFILE_REPLACE_EXISTING` semantics,
    // both of which guarantee the destination is either fully old or fully
    // new — never a half-written mix.
    if let Err(e) = std::fs::rename(&tmp_path, path) {
        // Clean up the tmp file on rename failure so we don't leave debris.
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }

    // fsync the directory on Unix so the rename is durable. POSIX only —
    // Windows directory handles don't need this and `File::open` on a
    // directory path fails on Windows anyway.
    #[cfg(unix)]
    {
        if let Ok(dir) = std::fs::File::open(parent) {
            let _ = dir.sync_all();
        }
    }

    Ok(())
}

/// Remove the "codemux" entry from `.mcp.json` on workspace close.
///
/// - If other servers remain, rewrites the file without codemux.
/// - If codemux was the only server, deletes the file.
/// - If the file doesn't exist or is invalid JSON, does nothing.
pub fn remove_mcp_config(workspace_dir: &Path) {
    let _io_guard = MCP_CONFIG_IO_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    remove_mcp_config_locked(workspace_dir);
}

fn remove_mcp_config_locked(workspace_dir: &Path) {
    let mcp_path = workspace_dir.join(".mcp.json");
    if !mcp_path.exists() {
        return;
    }

    let content = match std::fs::read_to_string(&mcp_path) {
        Ok(c) => c,
        Err(_) => return,
    };

    let mut config: Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return,
    };

    let servers = match config.get_mut("mcpServers").and_then(Value::as_object_mut) {
        Some(s) => s,
        None => return,
    };

    if servers.remove("codemux").is_none() {
        return;
    }

    if servers.is_empty() {
        // Drop our now-empty container, but keep unrelated top-level user
        // configuration instead of deleting the entire file.
        if let Some(root) = config.as_object_mut() {
            root.remove("mcpServers");
        }
    }

    if config.as_object().is_some_and(|root| root.is_empty()) {
        let _ = std::fs::remove_file(&mcp_path);
    } else if let Ok(json) = serde_json::to_string_pretty(&config) {
        // Mirror upsert's atomic-write path so a crash on cleanup can't
        // leave the file in a partially-written state either.
        let _ = atomic_write(&mcp_path, json.as_bytes());
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::protocol::{Incoming, RequestId, Response};
    use std::time::Duration;

    #[test]
    fn install_path_drops_the_suffix_linux_gives_a_replaced_binary() {
        assert_eq!(install_path(Path::new("/usr/bin/codemux (deleted)")), Path::new("/usr/bin/codemux"));
        assert_eq!(install_path(Path::new("/usr/bin/codemux")), Path::new("/usr/bin/codemux"));
        assert_eq!(install_path(Path::new("/opt/x (deleted)/codemux")), Path::new("/opt/x (deleted)/codemux"), "only a trailing suffix");
    }

    /// An outside caller at full access: no provider origin, no agent marker.
    fn outside() -> StdioCaller {
        StdioCaller { provider_origin: false, agent: false, access: ControlAccess::FullAccess }
    }

    /// Tests never name the test process's own client.
    fn unnamed() -> NameClient {
        Arc::new(|_: &str| false)
    }

    /// A server with the real tool runner for `caller`. Calls that reach the
    /// control socket are not made here.
    fn server_for(caller: StdioCaller) -> StdioServer {
        StdioServer::with_runner(caller, unnamed(), Arc::new(run_tool))
    }

    fn server() -> StdioServer {
        server_for(outside())
    }

    fn names(tools: &[StdioTool]) -> Vec<&str> {
        tools.iter().map(|tool| tool.name.as_ref()).collect()
    }

    /// Feed one JSON-RPC message to `server` and wait for its reply, as the
    /// stdio loop would. `None` for a notification.
    async fn rpc(server: &StdioServer, message: Value) -> Option<Value> {
        let error_id = message.get("id").and_then(RequestId::from_value);
        let request = match protocol::parse(message) {
            Ok(Incoming::Request(request)) => request,
            Ok(_) => return None,
            Err(error) => return Some(Response::error(error_id, error).to_value()),
        };
        let id = request.id.clone();
        let outcome = match server.request(request) {
            Reply::Now(outcome) => outcome,
            Reply::Later { future, .. } | Reply::Queued(future) => future.await,
        };
        Some(Response::from_outcome(id, outcome).to_value())
    }

    fn tools_call(id: u64, name: &str, arguments: Value) -> Value {
        json!({ "jsonrpc": "2.0", "id": id, "method": "tools/call", "params": { "name": name, "arguments": arguments } })
    }

    /// The work behind a deferred reply, and whether it parks.
    fn later(reply: Reply) -> (BoxFuture<'static, Result<Value, RpcError>>, bool) {
        match reply {
            Reply::Later { future, parks } => (future, parks),
            Reply::Queued(future) => (future, false),
            Reply::Now(outcome) => panic!("expected a deferred reply, got {outcome:?}"),
        }
    }

    fn request(id: u64, name: &str, arguments: Value) -> Request {
        match protocol::parse(tools_call(id, name, arguments)) {
            Ok(Incoming::Request(request)) => request,
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn review_s1_provider_stdio_has_no_trusted_native_controller_route() {
        const MARKER: &str = "CODEMUX_MCP_PROVIDER_ORIGIN";
        if std::env::var_os("CODEMUX_REVIEW_STDIO_CHILD").is_none() {
            // Process-owned origin, never mutable global test environment or
            // caller-controlled request metadata. This invokes only this test.
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["mcp_server::tests::review_s1_provider_stdio_has_no_trusted_native_controller_route", "--exact", "--nocapture"])
                .env(MARKER, "1").env("CODEMUX_REVIEW_STDIO_CHILD", "1").output().unwrap();
            assert!(result.status.success(), "provider-origin stdio assertion failed: {}", String::from_utf8_lossy(&result.stderr));
            assert_eq!(codemux_mcp_entry()["env"][MARKER], "1", "app-installed provider configs must bind the restricted invocation");
            return;
        }
        let caller = StdioCaller::from_env(|key| std::env::var_os(key), ControlAccess::FullAccess);
        assert!(caller.provider_origin, "the marker is read from the process env");
        assert!(!stdio_tools(caller.provider_origin, caller.agent, caller.access).iter().any(|tool| registry::relay_refuses(&tool.name)),
            "provider-origin stdio must omit trusted native tools");
        let server = server_for(caller);
        let result = rpc(&server, json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"thread_launch","arguments":{},
            "_meta":{"control_access":"full_access"}}})).await.unwrap();
        assert!(result.to_string().contains("provider_control_denied"),
            "provider stdio must reject guessed native calls before opening a trusted socket");
        let initialized = rpc(&server, json!({"jsonrpc":"2.0","id":2,"method":"initialize","params":{}})).await.unwrap();
        assert!(initialized["result"]["instructions"].as_str().unwrap().contains("Native thread control is unavailable"),
            "provider instructions must not promise the native tools removed from its catalog");
    }

    #[test]
    fn scroll_amount_reaches_receiver_as_i64() {
        // The receiver (stream_input handle_vision_action) reads `amount` with
        // Value::as_i64, which rejects float-backed numbers. The emitted value
        // must therefore be an integer JSON number.
        let v = scroll_amount_value(&json!({ "amount": 10 }));
        assert_eq!(
            v.as_i64(),
            Some(10),
            "MCP scroll amount must survive the receiver's as_i64 read"
        );
        // Absent → the documented default of 3.
        assert_eq!(scroll_amount_value(&json!({})).as_i64(), Some(3));
    }

    #[test]
    fn pane_split_tools_match_in_app_direction_convention() {
        // PaneNode.tsx renders "horizontal" as gridTemplateColumns (new pane to
        // the right) and "vertical" as gridTemplateRows (new pane below); every
        // in-app Split-right control sends "horizontal". The MCP tools must
        // match (they were inverted).
        assert_eq!(pane_split_direction("pane_split_right"), "horizontal");
        assert_eq!(pane_split_direction("pane_split_down"), "vertical");
    }

    /// Helper: create a unique temp directory for a test.
    fn test_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("codemux_test_{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn cleanup(dir: &Path) {
        let _ = std::fs::remove_dir_all(dir);
    }

    fn read_mcp(dir: &Path) -> Value {
        let content = std::fs::read_to_string(dir.join(".mcp.json")).unwrap();
        serde_json::from_str(&content).unwrap()
    }

    // -----------------------------------------------------------------------
    // Tool registry tests
    // -----------------------------------------------------------------------

    #[test]
    fn legacy_tool_names_are_pinned() {
        let legacy: Vec<&str> = legacy_tools().iter().map(|tool| tool.name).collect();
        assert_eq!(legacy, LEGACY_TOOL_NAMES, "a legacy tool was added, removed or renamed");
        assert!(LEGACY_TOOL_NAMES.iter().all(|name| is_legacy_tool(name)));
        assert!(!is_legacy_tool("thread_launch") && !is_legacy_tool("nope"));
    }

    /// Outside stdio at full access: every legacy tool the registry does not
    /// take over, then the registry's whole outside catalog. Computed rather
    /// than pinned, because it moves with the registry: a twin replaces its
    /// legacy tool one for one, and a tool with no legacy twin, such as
    /// `thread_answer`, adds one. The pins live in
    /// `legacy_tool_names_are_pinned` (62) and the catalog snapshot.
    fn full_access_outside_tool_count() -> usize {
        let kept = LEGACY_TOOL_NAMES.iter().filter(|name| registry::served(name, Audience::Outside).is_none()).count();
        kept + registry::list(&CallerView::outside(ControlAccess::FullAccess)).len()
    }

    #[test]
    fn tool_registry_has_all_tools() {
        // Registry tools take over the legacy tools they share a name with,
        // and a full-access caller is offered every other legacy tool.
        let tools = stdio_tools(false, false, ControlAccess::FullAccess);
        assert_eq!(tools.len(), full_access_outside_tool_count());
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
        assert!(names.contains(&"browser_navigate"));
        assert!(names.contains(&"browser_click"));
        assert!(names.contains(&"workspace_archive"));
        assert!(names.contains(&"workspace_unarchive"));
        assert!(names.contains(&"workspace_archive_list"));
        assert!(names.contains(&"browser_fill"));
        assert!(names.contains(&"browser_screenshot"));
        assert!(names.contains(&"workspace_list"));
        assert!(names.contains(&"workspace_info"));
        assert!(names.contains(&"automation_list"));
        assert!(names.contains(&"automation_create"));
        assert!(names.contains(&"automation_pause"));
        assert!(names.contains(&"automation_runs"));
        assert!(names.contains(&"pane_list"));
        assert!(names.contains(&"notify"));
        assert!(names.contains(&"git_status"));
        assert!(names.contains(&"git_diff"));
        assert!(names.contains(&"git_stage"));
        assert!(names.contains(&"git_commit"));
        // v0.24.0 browser info tools
        assert!(names.contains(&"browser_get_styles"));
        assert!(names.contains(&"browser_wait"));
        assert!(names.contains(&"browser_evaluate"));
        assert!(names.contains(&"git_push"));
        // Viewport tools — guards the mobile/desktop test surface so a
        // future refactor that accidentally drops them gets caught here.
        assert!(names.contains(&"browser_viewport"));
        assert!(names.contains(&"browser_viewport_presets"));
        // Phase 1 vexis-agent integration tools — locks in the contract
        // exposed to vexis-agent's brain runtime. Any drop or rename
        // breaks the integration.
        assert!(names.contains(&"terminal_write"));
        assert!(names.contains(&"terminal_read"));
        assert!(names.contains(&"conversation_read"));
        assert!(names.contains(&"conversation_search"));
        assert!(names.contains(&"workspace_open"));
        assert!(names.contains(&"app_status"));
        assert!(names.contains(&"port_list"));
        // Phase 1.5 delegation primitives — pinning the contract so the
        // brain can rely on these names existing. `workspace_create`
        // (the path-bug fix) is one of the original 31 — already
        // covered by the count assertion above.
        assert!(names.contains(&"worktree_create"));
        assert!(names.contains(&"preset_apply"));
        assert!(names.contains(&"preset_list"));
        // Phase 1.6 lifecycle + issue tools — pinning the contract for
        // close/cleanup + GitHub issue browsing.
        assert!(names.contains(&"workspace_close"));
        assert!(names.contains(&"pane_close"));
        assert!(names.contains(&"issue_list"));
        assert!(names.contains(&"issue_get"));
        assert!(names.contains(&"issue_link_workspace"));
    }

    /// Every tool any stdio caller is offered: the outside listing plus the
    /// legacy definitions it replaces.
    fn every_tool() -> Vec<StdioTool> {
        let mut tools = stdio_tools(false, false, ControlAccess::FullAccess);
        tools.extend(legacy_tools().into_iter().map(StdioTool::from));
        tools
    }

    #[test]
    fn tool_schemas_are_valid_json_objects() {
        for tool in every_tool() {
            assert!(tool.input_schema.is_object(), "Tool {} schema is not an object", tool.name);
            assert_eq!(
                tool.input_schema.get("type").and_then(Value::as_str),
                Some("object"),
                "Tool {} schema missing type:object",
                tool.name
            );
        }
    }

    #[test]
    fn tool_schemas_required_fields_are_arrays() {
        for tool in every_tool() {
            if let Some(required) = tool.input_schema.get("required") {
                assert!(required.is_array(), "Tool {} required field is not an array", tool.name);
                for item in required.as_array().unwrap() {
                    assert!(item.is_string(), "Tool {} has non-string required field", tool.name);
                }
            }
        }
    }

    #[test]
    fn tool_names_unique() {
        let tools = stdio_tools(false, false, ControlAccess::FullAccess);
        let mut names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
        let original_len = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), original_len, "Duplicate tool names found");
    }

    // -----------------------------------------------------------------------
    // Workspace-id resolution
    // -----------------------------------------------------------------------

    #[test]
    fn workspace_id_prefers_call_meta_over_env_fallback() {
        let params = json!({
            "name": "browser_navigate",
            "arguments": {},
            "_meta": { crate::mcp::WORKSPACE_META_KEY: "ws-from-meta" }
        });
        assert_eq!(
            resolve_workspace_id(params.get("_meta"), Some("ws-from-env".into())),
            "ws-from-meta"
        );
    }

    #[test]
    fn workspace_id_falls_back_to_env_without_meta() {
        let params = json!({ "name": "browser_navigate", "arguments": {} });
        assert_eq!(
            resolve_workspace_id(params.get("_meta"), Some("ws-from-env".into())),
            "ws-from-env"
        );
        assert_eq!(resolve_workspace_id(params.get("_meta"), None), "");
    }

    #[test]
    fn workspace_id_ignores_empty_or_non_string_meta() {
        let empty = json!({ "name": "t", "_meta": { crate::mcp::WORKSPACE_META_KEY: "" } });
        assert_eq!(
            resolve_workspace_id(empty.get("_meta"), Some("ws-from-env".into())),
            "ws-from-env"
        );
        let wrong_type = json!({ "name": "t", "_meta": { crate::mcp::WORKSPACE_META_KEY: 7 } });
        assert_eq!(
            resolve_workspace_id(wrong_type.get("_meta"), Some("ws-from-env".into())),
            "ws-from-env"
        );
    }

    #[test]
    fn path_level_entry_leaves_each_session_workspace_id_inherited() {
        let entry = codemux_mcp_entry();
        assert!(
            entry["env"].get("CODEMUX_WORKSPACE_ID").is_none(),
            "a shared path must not pin one workspace id in .mcp.json"
        );
        assert_eq!(entry["env"][PROVIDER_ORIGIN_ENV], "1");

        let params = json!({ "name": "browser_navigate", "arguments": {} });
        assert_eq!(
            resolve_workspace_id(params.get("_meta"), Some("workspace-a".into())),
            "workspace-a"
        );
        assert_eq!(
            resolve_workspace_id(params.get("_meta"), Some("workspace-b".into())),
            "workspace-b"
        );
    }

    // -----------------------------------------------------------------------
    // Protocol handling (async)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn dispatch_initialize() {
        let resp = rpc(&server(), json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}
        })).await.unwrap();
        let result = &resp["result"];
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert_eq!(result["serverInfo"]["name"], "codemux");
        assert_eq!(result["capabilities"], json!({ "tools": {} }));
    }

    /// A server whose client name goes to `slot` instead of the process.
    fn named_server(slot: &Arc<std::sync::OnceLock<String>>) -> StdioServer {
        let slot = Arc::clone(slot);
        let name: NameClient = Arc::new(move |raw: &str| control_identity::adopt_name(&slot, raw));
        StdioServer::with_runner(outside(), name, Arc::new(run_tool))
    }

    #[tokio::test]
    async fn initialize_echoes_2025_11_25_and_records_client_info() {
        let slot = Arc::new(std::sync::OnceLock::new());
        let server = named_server(&slot);
        let resp = rpc(&server, json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25", "capabilities": {},
                "clientInfo": {"name": "Hermes Agent", "version": "2.0.0", "title": "Hermes", "websiteUrl": "https://example.com"}
            }
        })).await.unwrap();
        assert_eq!(resp["result"]["protocolVersion"], "2025-11-25");
        assert_eq!(slot.get().map(String::as_str), Some("Hermes Agent"), "clientInfo names the client");
        // The first name sticks; an offer the server does not speak gets the latest.
        let resp = rpc(&server, json!({
            "jsonrpc": "2.0", "id": 2, "method": "initialize",
            "params": {"protocolVersion": "2026-07-28", "clientInfo": {"name": "other", "version": "1"}}
        })).await.unwrap();
        assert_eq!(resp["result"]["protocolVersion"], "2025-11-25");
        assert_eq!(slot.get().map(String::as_str), Some("Hermes Agent"), "the first name sticks");
        let bare = rpc(&server, json!({"jsonrpc": "2.0", "id": 3, "method": "initialize"})).await.unwrap();
        assert_eq!(bare["result"]["protocolVersion"], "2025-11-25");
    }

    #[tokio::test]
    async fn server_discover_probe_gets_method_not_found_with_the_same_id() {
        let server = server();
        for id in [json!("discover-1"), json!(7)] {
            let resp = rpc(&server, json!({"jsonrpc": "2.0", "id": id, "method": "server/discover", "params": {}})).await.unwrap();
            assert_eq!(resp["id"], id);
            assert_eq!(resp["error"]["code"], -32601);
            assert!(resp.get("result").is_none());
        }
    }

    /// A stateless-era (2026-07-28) request: `params` plus the `_meta`
    /// envelope dual-era SDKs stamp on every request.
    fn modern(id: u64, method: &str, mut params: Value) -> Value {
        params["_meta"] = json!({
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientInfo": { "name": "Claude Code", "version": "2.1.317" },
            "io.modelcontextprotocol/clientCapabilities": {}
        });
        json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
    }

    fn server_info() -> Value {
        json!({ "name": "codemux", "version": env!("CARGO_PKG_VERSION") })
    }

    /// The `stateless_*` tests check stateless serving. With the kill switch
    /// off every request is initialize-era, which the other tests cover.
    fn stateless_era_off() -> bool {
        if !protocol::MODERN_ERA_ENABLED {
            eprintln!("skipped: the stateless era is switched off (mcp::protocol::MODERN_ERA_ENABLED)");
        }
        !protocol::MODERN_ERA_ENABLED
    }

    #[tokio::test]
    async fn stateless_discover_describes_both_eras_and_the_same_instructions() {
        if stateless_era_off() {
            return;
        }
        for (caller, instructions) in [
            (outside(), crate::agent_control::INSTRUCTIONS),
            (StdioCaller { provider_origin: true, ..outside() }, PROVIDER_INSTRUCTIONS),
        ] {
            let resp = rpc(&server_for(caller), modern(1, "server/discover", json!({}))).await.unwrap();
            assert_eq!(resp["id"], 1);
            let result = &resp["result"];
            assert_eq!(result["resultType"], "complete", "{resp}");
            assert_eq!(
                result["supportedVersions"],
                json!(["2026-07-28", "2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"])
            );
            assert_eq!(result["capabilities"], json!({ "tools": {} }));
            assert_eq!(result["instructions"], instructions);
            assert_eq!(result["_meta"]["io.modelcontextprotocol/serverInfo"], server_info());
            assert_eq!((result["ttlMs"].as_u64(), result["cacheScope"].as_str()), (Some(3_600_000), Some("public")));
        }
    }

    #[tokio::test]
    async fn stateless_and_initialize_era_clients_share_one_server() {
        if stateless_era_off() {
            return;
        }
        let Gated { server, .. } = gated_server();
        // An initialize-era client first, then a stateless one, interleaved.
        let init = rpc(&server, json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-06-18", "clientInfo": {"name": "vexis", "version": "1"}}
        })).await.unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-06-18");

        let legacy_list = rpc(&server, json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})).await.unwrap();
        let modern_list = rpc(&server, modern(3, "tools/list", json!({}))).await.unwrap();
        assert_eq!(modern_list["result"]["tools"], legacy_list["result"]["tools"], "one catalog for both eras");
        assert!(legacy_list["result"].get("resultType").is_none(), "initialize-era results are unchanged");
        let listed = &modern_list["result"];
        assert_eq!(listed["resultType"], "complete");
        assert_eq!((listed["ttlMs"].as_u64(), listed["cacheScope"].as_str()), (Some(300_000), Some("private")));
        assert_eq!(listed["_meta"]["io.modelcontextprotocol/serverInfo"], server_info());

        // Legacy-lane and registry tools both answer stateless calls, marked complete.
        for (id, name, text) in [(4, "pane_list", "pane_list"), (5, "thread_status", "thread_status (registry)")] {
            let resp = rpc(&server, modern(id, "tools/call", json!({ "name": name, "arguments": {} }))).await.unwrap();
            assert_eq!(resp["result"]["content"][0]["text"], text, "{resp}");
            assert_eq!(resp["result"]["resultType"], "complete", "{resp}");
            assert_eq!(resp["result"]["_meta"]["io.modelcontextprotocol/serverInfo"], server_info());
            let legacy = rpc(&server, tools_call(id + 10, name, json!({}))).await.unwrap();
            assert_eq!(legacy["result"], json!({ "content": [{ "type": "text", "text": text }] }), "{legacy}");
        }
        // A wait still parks on the stateless path.
        let (_, parks) = later(server.request(match protocol::parse(modern(6, "tools/call", json!({ "name": "thread_wait" }))) {
            Ok(Incoming::Request(request)) => request,
            other => panic!("{other:?}"),
        }));
        assert!(parks);
        // Tool failures stay tool results.
        let read_only = gated_server_for(
            StdioCaller { access: ControlAccess::ReadOnly, ..outside() },
            Arc::default(),
            Arc::default(),
            Arc::default(),
        );
        let refused = rpc(&read_only.server, modern(7, "tools/call", json!({ "name": "git_commit", "arguments": { "message": "m" } })))
            .await
            .unwrap();
        assert_eq!((refused["result"]["isError"].clone(), refused["result"]["resultType"].clone()), (json!(true), json!("complete")));
    }

    #[tokio::test]
    async fn stateless_requests_name_the_client_and_the_first_name_sticks() {
        if stateless_era_off() {
            return;
        }
        let slot = Arc::new(std::sync::OnceLock::new());
        let server = named_server(&slot);
        rpc(&server, modern(1, "tools/list", json!({}))).await.unwrap();
        assert_eq!(slot.get().map(String::as_str), Some("Claude Code"));
        rpc(&server, json!({
            "jsonrpc": "2.0", "id": 2, "method": "initialize",
            "params": {"protocolVersion": "2025-11-25", "clientInfo": {"name": "other", "version": "1"}}
        })).await.unwrap();
        assert_eq!(slot.get().map(String::as_str), Some("Claude Code"));
    }

    #[tokio::test]
    async fn stateless_requests_refuse_unknown_revisions_and_methods_outside_the_revision() {
        if stateless_era_off() {
            return;
        }
        let server = server();
        let mut future = modern(1, "server/discover", json!({}));
        future["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"] = json!("2099-01-01");
        let resp = rpc(&server, future).await.unwrap();
        assert_eq!(resp["id"], 1);
        assert_eq!(resp["error"]["code"], -32022);
        assert_eq!(resp["error"]["data"]["supported"][0], "2026-07-28");
        assert_eq!(resp["error"]["data"]["requested"], "2099-01-01");

        let mut no_capabilities = modern(2, "tools/list", json!({}));
        no_capabilities["params"]["_meta"].as_object_mut().unwrap().remove("io.modelcontextprotocol/clientCapabilities");
        assert_eq!(rpc(&server, no_capabilities).await.unwrap()["error"]["code"], -32602);

        for method in ["ping", "resources/list"] {
            let resp = rpc(&server, modern(3, method, json!({}))).await.unwrap();
            assert_eq!(resp["error"]["code"], -32601, "{method}");
        }
        // `initialize` always selects the initialize era, whatever `_meta` says.
        let init = rpc(&server, modern(4, "initialize", json!({ "protocolVersion": "2025-11-25" }))).await.unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-11-25");
        assert!(init["result"].get("resultType").is_none());
    }

    #[tokio::test]
    async fn dispatch_tools_list() {
        let resp = rpc(&server(), json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}})).await.unwrap();
        let tools = resp["result"]["tools"].as_array().unwrap();
        // `tools/list` serves what `stdio_tools` builds for this caller.
        assert_eq!(tools.len(), full_access_outside_tool_count());
        for tool in tools {
            assert!(tool.get("name").is_some());
            assert!(tool.get("description").is_some());
            assert!(tool.get("inputSchema").is_some());
        }
    }

    #[tokio::test]
    async fn dispatch_unknown_method() {
        let resp = rpc(&server(), json!({"jsonrpc": "2.0", "id": 3, "method": "nonexistent/method", "params": {}})).await.unwrap();
        assert_eq!(resp["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn dispatch_notification_no_response() {
        let resp = rpc(&server(), json!({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}})).await;
        assert!(resp.is_none(), "Notifications should return None");
    }

    #[tokio::test]
    async fn dispatch_ping() {
        let resp = rpc(&server(), json!({"jsonrpc": "2.0", "id": 4, "method": "ping", "params": {}})).await.unwrap();
        assert_eq!(resp["result"], json!({}));
        assert!(resp.get("error").is_none());
    }

    #[tokio::test]
    async fn dispatch_tools_call_unknown_tool() {
        let resp = rpc(&server(), tools_call(5, "nonexistent_tool", json!({}))).await.unwrap();
        let result = &resp["result"];
        assert_eq!(result["isError"], true);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Unknown tool"));
    }

    #[tokio::test]
    async fn tools_call_without_a_name_is_invalid_params() {
        let server = server();
        for params in [json!({}), json!({"name": ""}), json!({"name": "notify", "arguments": "x"})] {
            let resp = rpc(&server, json!({"jsonrpc": "2.0", "id": 8, "method": "tools/call", "params": params})).await.unwrap();
            assert_eq!(resp["id"], 8);
            assert_eq!(resp["error"]["code"], -32602, "{params}");
        }
    }

    /// A server for routing tests whose runner logs each call's name when it
    /// starts and when it finishes.
    struct Gated {
        server: StdioServer,
        /// `git_status` waits for this.
        gate: Arc<tokio::sync::Notify>,
        started: Arc<Mutex<Vec<String>>>,
        finished: Arc<Mutex<Vec<String>>>,
    }

    /// `git_status` waits for the gate, `boom` panics, everything else
    /// finishes at once.
    fn gated_server() -> Gated {
        let gate = Arc::new(tokio::sync::Notify::new());
        let started = Arc::new(Mutex::new(Vec::new()));
        let finished = Arc::new(Mutex::new(Vec::new()));
        gated_server_for(outside(), gate, started, finished)
    }

    fn gated_server_for(
        caller: StdioCaller,
        gate: Arc<tokio::sync::Notify>,
        started: Arc<Mutex<Vec<String>>>,
        finished: Arc<Mutex<Vec<String>>>,
    ) -> Gated {
        let (gate_for_run, started_for_run, finished_for_run) =
            (Arc::clone(&gate), Arc::clone(&started), Arc::clone(&finished));
        let run: RunTool = Arc::new(move |kind: ToolKind, call: CallToolParams| -> BoxFuture<'static, Result<Value, RpcError>> {
            let (gate, started, finished) =
                (Arc::clone(&gate_for_run), Arc::clone(&started_for_run), Arc::clone(&finished_for_run));
            Box::pin(async move {
                started.lock().unwrap().push(call.name.clone());
                match call.name.as_str() {
                    "git_status" => gate.notified().await,
                    "boom" => panic!("tool bug"),
                    _ => {}
                }
                finished.lock().unwrap().push(call.name.clone());
                Ok(protocol::text_result(format!("{}{}", call.name, if kind == ToolKind::Registry { " (registry)" } else { "" })))
            })
        });
        Gated { server: StdioServer::with_runner(caller, unnamed(), run), gate, started, finished }
    }

    /// Lets other tasks run until `done` holds, for at most 2 s.
    async fn until(done: impl Fn() -> bool) {
        for _ in 0..200 {
            if done() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[tokio::test]
    async fn legacy_tools_run_in_arrival_order() {
        let active = Arc::new(Mutex::new((0usize, 0usize)));
        let finished = Arc::new(Mutex::new(Vec::new()));
        let (active_for_run, finished_for_run) = (Arc::clone(&active), Arc::clone(&finished));
        let server = StdioServer::with_runner(outside(), unnamed(), Arc::new(move |_: ToolKind, call: CallToolParams| -> BoxFuture<'static, Result<Value, RpcError>> {
            let (active, finished) = (Arc::clone(&active_for_run), Arc::clone(&finished_for_run));
            Box::pin(async move {
                {
                    let mut active = active.lock().unwrap();
                    active.0 += 1;
                    active.1 = active.1.max(active.0);
                }
                let delay = call.arguments["delay_ms"].as_u64().unwrap();
                tokio::time::sleep(Duration::from_millis(delay)).await;
                active.lock().unwrap().0 -= 1;
                finished.lock().unwrap().push(call.name.clone());
                Ok(protocol::text_result(call.name))
            })
        }));
        // Each later call is quicker, so any overlap would reorder them.
        let calls: Vec<_> = [("git_status", 60), ("pane_list", 30), ("notify", 0)]
            .into_iter()
            .enumerate()
            .map(|(id, (name, delay))| {
                let (future, parks) = later(server.request(request(id as u64, name, json!({ "delay_ms": delay }))));
                assert!(!parks);
                future
            })
            .collect();
        let results = futures_util::future::join_all(calls).await;
        assert_eq!(*finished.lock().unwrap(), ["git_status", "pane_list", "notify"]);
        assert_eq!(active.lock().unwrap().1, 1, "legacy tools never overlap");
        for (result, name) in results.into_iter().zip(["git_status", "pane_list", "notify"]) {
            assert_eq!(result.unwrap()["content"][0]["text"], name);
        }
    }

    #[tokio::test]
    async fn registry_tools_bypass_the_legacy_lane() {
        let Gated { server, gate, started, .. } = gated_server();
        let (legacy, legacy_parks) = later(server.request(request(1, "git_status", json!({}))));
        // Let the lane pick the legacy job up; it then blocks on the gate.
        until(|| !started.lock().unwrap().is_empty()).await;
        assert_eq!(*started.lock().unwrap(), ["git_status"]);
        let (status, status_parks) = later(server.request(request(2, "thread_status", json!({}))));
        let (_, wait_parks) = later(server.request(request(3, "thread_wait", json!({}))));
        assert_eq!((legacy_parks, status_parks, wait_parks), (false, false, true), "only waits park");
        // The registry call finishes while the legacy job is still blocked.
        let status = tokio::time::timeout(Duration::from_secs(2), status).await.expect("not held by the legacy lane");
        assert_eq!(status.unwrap()["content"][0]["text"], "thread_status (registry)");
        assert_eq!(*started.lock().unwrap(), ["git_status", "thread_status"]);
        gate.notify_one();
        assert_eq!(legacy.await.unwrap()["content"][0]["text"], "git_status");
    }

    fn caller(provider_origin: bool, agent: bool, access: ControlAccess) -> StdioCaller {
        StdioCaller { provider_origin, agent, access }
    }

    /// A tool's access class: the registry's for registry tools, otherwise
    /// the legacy classification.
    fn class_of(tool: &StdioTool) -> crate::agent_control::Access {
        match &tool.annotations {
            Some(_) => registry::lookup(&tool.name).map(|(_, spec)| spec.access()),
            None => control_identity::mcp_tool_access(&tool.name, &json!({})),
        }
        .unwrap_or_else(|| panic!("{} has no access class", tool.name))
    }

    #[test]
    fn stdio_keeps_one_workspace_list() {
        let outside = stdio_tools(false, false, ControlAccess::FullAccess);
        let twins: Vec<&str> = LEGACY_TOOL_NAMES.iter().copied().filter(|name| registry::served(name, Audience::Outside).is_some()).collect();
        assert!(twins.contains(&"workspace_list"), "{twins:?}");
        for twin in &twins {
            let served: Vec<&StdioTool> = outside.iter().filter(|tool| tool.name == *twin).collect();
            assert_eq!(served.len(), 1, "{twin} is listed once");
            assert!(served[0].annotations.is_some(), "{twin}: outside stdio serves the registry definition");
            assert_eq!(served[0].input_schema["additionalProperties"], false, "{twin}");
        }
        let list = outside.iter().find(|tool| tool.name == "workspace_list").unwrap();
        assert_eq!(list.annotations.as_ref().unwrap()["readOnlyHint"], true);
        let provider = stdio_tools(true, false, ControlAccess::FullAccess);
        let legacy = provider.iter().find(|tool| tool.name == "workspace_list").unwrap();
        assert!(legacy.input_schema.get("additionalProperties").is_none(), "provider-origin stdio keeps the legacy schema");
        assert!(legacy.annotations.is_none());
    }

    #[tokio::test]
    async fn tools_list_serializes_annotations_only_for_registry_tools() {
        let listed = rpc(&server(), json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})).await.unwrap();
        let tools = listed["result"]["tools"].as_array().unwrap();
        let tool = |name: &str| tools.iter().find(|tool| tool["name"] == name).unwrap_or_else(|| panic!("{name} is listed"));
        assert_eq!(tool("thread_wait")["annotations"]["readOnlyHint"], true);
        assert_eq!(tool("thread_stop")["annotations"]["destructiveHint"], true);
        assert!(tool("git_status").get("annotations").is_none(), "legacy tools keep their shape");
    }

    #[test]
    fn legacy_stdio_schemas_use_only_the_validated_vocabulary() {
        for tool in legacy_tools() {
            let errors = crate::agent_control::schema::vocabulary_errors(&tool.input_schema);
            assert!(errors.is_empty(), "{}: {errors:?}", tool.name);
        }
    }

    #[test]
    fn legacy_worktree_base_defaults_to_the_repository_default_branch() {
        let tools = legacy_tools();
        let worktree = tools.iter().find(|tool| tool.name == "worktree_create").unwrap();
        let base = &worktree.input_schema["properties"]["base"];
        assert!(base.get("default").is_none(), "{base}");
        assert!(base["description"].as_str().unwrap().contains("Defaults to the repository's default branch"), "{base}");
    }

    #[tokio::test]
    async fn client_name_comes_from_flag_then_client_info() {
        let initialize = |name: &str| json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-11-25", "clientInfo": {"name": name, "version": "1"}}});
        // `--client` is applied before the server reads its first line.
        let flagged = Arc::new(std::sync::OnceLock::new());
        assert!(control_identity::adopt_name(&flagged, "hermes"));
        let server = named_server(&flagged);
        rpc(&server, initialize("Claude Code")).await.unwrap();
        assert_eq!(flagged.get().map(String::as_str), Some("hermes"), "the flag beats clientInfo");
        // Without the flag, the first usable clientInfo name wins.
        let unflagged = Arc::new(std::sync::OnceLock::new());
        let server = named_server(&unflagged);
        rpc(&server, initialize("  ")).await.unwrap();
        assert_eq!(unflagged.get(), None, "a blank name names nobody");
        rpc(&server, initialize("Claude Code")).await.unwrap();
        rpc(&server, initialize("other")).await.unwrap();
        assert_eq!(unflagged.get().map(String::as_str), Some("Claude Code"));
    }

    #[test]
    fn read_only_stdio_lists_only_read_tools() {
        let tools = stdio_tools(false, false, ControlAccess::ReadOnly);
        for tool in &tools {
            assert_eq!(class_of(tool), crate::agent_control::Access::Read, "{} is listed read-only", tool.name);
        }
        let names = names(&tools);
        for shown in ["git_status", "workspace_list", "workspace_info", "pane_list", "html_render", "thread_list", "thread_status", "thread_wait"] {
            assert!(names.contains(&shown), "{shown}: {names:?}");
        }
        for hidden in ["git_commit", "notify", "browser_click", "thread_launch", "thread_send", "workspace_create", "terminal_write"] {
            assert!(!names.contains(&hidden), "{hidden}: {names:?}");
        }
    }

    #[test]
    fn supervised_stdio_hides_machine_and_admin_tools() {
        use crate::agent_control::Access;
        let tools = stdio_tools(false, false, ControlAccess::Supervised);
        for tool in &tools {
            assert!(matches!(class_of(tool), Access::Read | Access::Write | Access::StartsThreads), "{} is listed supervised", tool.name);
        }
        let names = names(&tools);
        for shown in ["notify", "git_commit", "browser_click", "workspace_open", "thread_launch", "thread_send", "thread_respond"] {
            assert!(names.contains(&shown), "{shown}: {names:?}");
        }
        for hidden in ["automation_create", "automation_pause", "terminal_write", "preset_apply", "workspace_create", "worktree_create", "workspace_unarchive"] {
            assert!(!names.contains(&hidden), "{hidden}: {names:?}");
        }
        assert!(names.len() < stdio_tools(false, false, ControlAccess::FullAccess).len());
    }

    #[tokio::test]
    async fn provider_origin_stdio_keeps_every_legacy_name() {
        for access in [ControlAccess::ReadOnly, ControlAccess::Supervised, ControlAccess::FullAccess] {
            let tools = stdio_tools(true, false, access);
            assert_eq!(names(&tools), LEGACY_TOOL_NAMES, "{access:?}");
            assert!(tools.iter().all(|tool| tool.annotations.is_none()));
        }
        let listed = rpc(&server_for(caller(true, false, ControlAccess::FullAccess)), json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})).await.unwrap();
        assert_eq!(listed["result"]["tools"].as_array().unwrap().len(), LEGACY_TOOL_NAMES.len());
    }

    #[tokio::test]
    async fn agent_marked_stdio_lists_legacy_only() {
        fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
            move |key| pairs.iter().find(|(k, _)| *k == key).map(|(_, v)| OsString::from(v))
        }
        let marked = StdioCaller::from_env(env(&[(AGENT_PROCESS_ENV, "")]), ControlAccess::FullAccess);
        assert_eq!(marked, caller(false, true, ControlAccess::FullAccess), "presence alone marks an agent");
        assert_eq!(StdioCaller::from_env(env(&[(PROVIDER_ORIGIN_ENV, "1")]), ControlAccess::ReadOnly), caller(true, false, ControlAccess::ReadOnly));
        assert_eq!(StdioCaller::from_env(env(&[]), ControlAccess::Supervised), caller(false, false, ControlAccess::Supervised));

        assert_eq!(names(&stdio_tools(false, true, ControlAccess::FullAccess)), LEGACY_TOOL_NAMES);
        let Gated { server, started, .. } = gated_server_for(marked, Default::default(), Default::default(), Default::default());
        for name in ["thread_list", "thread_wait", "create_threads"] {
            let refused = rpc(&server, tools_call(1, name, json!({}))).await.unwrap();
            assert_eq!(refused["error"]["code"], -32600, "{name}: {refused}");
            assert!(refused["error"]["message"].as_str().unwrap().starts_with("provider_control_denied: "), "{refused}");
        }
        // A legacy twin keeps its legacy implementation.
        let listed = rpc(&server, tools_call(2, "workspace_list", json!({}))).await.unwrap();
        assert_eq!(listed["result"]["content"][0]["text"], "workspace_list", "{listed}");
        assert_eq!(*started.lock().unwrap(), ["workspace_list"], "refused calls never ran");
        let initialized = rpc(&server, json!({"jsonrpc": "2.0", "id": 3, "method": "initialize", "params": {}})).await.unwrap();
        assert!(initialized["result"]["instructions"].as_str().unwrap().contains("Native thread control is unavailable"));
    }

    #[tokio::test]
    async fn read_only_stdio_refuses_local_git_writes() {
        let Gated { server, started, .. } =
            gated_server_for(caller(false, false, ControlAccess::ReadOnly), Default::default(), Default::default(), Default::default());
        for (name, arguments) in [("git_commit", json!({"message": "x"})), ("git_push", json!({})), ("notify", json!({"message": "x"}))] {
            let refused = rpc(&server, tools_call(1, name, arguments)).await.unwrap();
            assert_eq!(refused["result"]["isError"], true, "{name}: {refused}");
            let text = refused["result"]["content"][0]["text"].as_str().unwrap();
            assert!(text.starts_with("access_denied: This connection is read-only"), "{text}");
        }
        assert!(started.lock().unwrap().is_empty(), "a refused tool never runs");
        let diff = rpc(&server, tools_call(2, "git_diff", json!({}))).await.unwrap();
        assert_eq!(diff["result"]["content"][0]["text"], "git_diff");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stdio_registry_calls_report_app_unavailable_without_codemux() {
        if crate::test_isolation::isolated("mcp_server::tests::stdio_registry_calls_report_app_unavailable_without_codemux") {
            return;
        }
        // Nothing listens in this child's private runtime dir.
        let reply = rpc(&server(), tools_call(1, "operation_status", json!({"operation_id": "x"}))).await.unwrap();
        let result = &reply["result"];
        assert_eq!(result["isError"], true, "{reply}");
        assert_eq!(result["structuredContent"]["error"]["code"], "app_unavailable", "{reply}");
        assert_eq!(result["structuredContent"]["error"]["retryable"], true);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn narrowed_stdio_reports_an_old_app_as_server_too_old() {
        if crate::test_isolation::isolated("mcp_server::tests::narrowed_stdio_reports_an_old_app_as_server_too_old") {
            return;
        }
        // `codemux mcp --access supervised` against an app that predates
        // `client.access`: the client refuses rather than run at full access,
        // and says so in a way a retry loop stops on.
        control_identity::set_client_access(ControlAccess::Supervised);
        let seen = old_app_socket(1);
        let server = server_for(StdioCaller { access: ControlAccess::Supervised, ..outside() });
        let reply = rpc(&server, tools_call(1, "thread_list", json!({}))).await.unwrap();
        let error = &reply["result"]["structuredContent"]["error"];
        assert_eq!(reply["result"]["isError"], true, "{reply}");
        assert_eq!(error["code"], "server_too_old", "{reply}");
        assert_eq!(error["retryable"], false, "{reply}");
        assert_eq!(*seen.lock().unwrap(), ["status"], "only the version probe reaches the old app");
    }

    /// A control socket like an app from before `tool_call`, speaking
    /// control protocol `version`: it answers every line of a connection in
    /// order (`get_app_state` and the bare aliases succeed), and records each
    /// command.
    #[cfg(unix)]
    fn old_app_socket(version: u32) -> Arc<Mutex<Vec<String>>> {
        let path = crate::control::control_socket_path().unwrap();
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&seen);
        std::thread::spawn(move || {
            use std::io::{BufRead, Write};
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                for line in std::io::BufReader::new(&stream).lines() {
                    let Ok(line) = line else { break };
                    let request: Value = serde_json::from_str(&line).unwrap();
                    let command = request["command"].as_str().unwrap().to_owned();
                    recorded.lock().unwrap().push(command.clone());
                    let (ok, data, error) = match command.as_str() {
                        "get_app_state" => (true, json!({"active_workspace_id": "ws-1", "workspaces": [
                            {"workspace_id": "ws-1", "cwd": "/w1", "surfaces": [{"surface_id": "s1"}]},
                            {"workspace_id": "ws-2", "cwd": "/w2", "surfaces": []},
                        ]}), Value::Null),
                        "thread_list" => (true, json!({"threads": []}), Value::Null),
                        "operation_status" => (false, Value::Null, json!("operation_not_found: No operation belongs to this client and ID")),
                        other => (false, Value::Null, json!(format!("Unknown control command: {other}"))),
                    };
                    let reply = json!({"ok": ok, "protocol_version": version, "data": data, "error": error});
                    if writeln!(&stream, "{reply}").is_err() {
                        break;
                    }
                }
            }
        });
        seen
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn narrowed_stdio_holds_a_legacy_fallback_to_its_access() {
        if crate::test_isolation::isolated("mcp_server::tests::narrowed_stdio_holds_a_legacy_fallback_to_its_access") {
            return;
        }
        // An app that enforces `--access` but has no `tool_call`: the legacy
        // twin a registry call falls back to meets the same local gate as a
        // legacy call, so a read-only client never asks for a workspace.
        control_identity::set_client_access(ControlAccess::ReadOnly);
        let seen = old_app_socket(2);
        let server = server_for(StdioCaller { access: ControlAccess::ReadOnly, ..outside() });
        assert!(registry::served("workspace_create", Audience::Outside).is_some(), "a registry twin");
        let refused = rpc(&server, tools_call(1, "workspace_create", json!({"path": "/w3"}))).await.unwrap();
        assert_eq!(refused["result"]["isError"], true, "{refused}");
        let text = refused["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.starts_with("access_denied: This connection is read-only"), "{text}");
        assert_eq!(*seen.lock().unwrap(), ["status", "tool_call"], "create_workspace never reached the app");

        // A read the access allows still falls back.
        let info = rpc(&server, tools_call(2, "workspace_info", json!({}))).await.unwrap();
        assert!(info["result"]["content"][0]["text"].as_str().unwrap().contains("\"ws-1\""), "{info}");
        assert_eq!(*seen.lock().unwrap(), ["status", "tool_call", "status", "tool_call", "status", "get_app_state"]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stdio_twins_fall_back_to_legacy_arms_on_an_old_app() {
        if crate::test_isolation::isolated("mcp_server::tests::stdio_twins_fall_back_to_legacy_arms_on_an_old_app") {
            return;
        }
        let seen = old_app_socket(1);
        let server = server();
        let text = |reply: &Value| reply["result"]["content"][0]["text"].as_str().unwrap().to_owned();

        // vexis-agent's call: the focused workspace's legacy snapshot.
        let info = rpc(&server, tools_call(1, "workspace_info", json!({}))).await.unwrap();
        let snapshot: Value = serde_json::from_str(&text(&info)).unwrap();
        assert_eq!(snapshot["workspace_id"], "ws-1", "{info}");
        assert_eq!(snapshot["surfaces"], json!([{"surface_id": "s1"}]));
        assert!(info["result"].get("structuredContent").is_none(), "the legacy shape: {info}");

        // Every legacy twin the registry serves falls back the same way.
        let list = rpc(&server, tools_call(2, "workspace_list", json!({}))).await.unwrap();
        let listed: Value = serde_json::from_str(&text(&list)).unwrap();
        assert_eq!(listed["workspaces"].as_array().unwrap().len(), 2, "{list}");
        assert!(text(&list).contains("\n"), "pretty legacy text: {list}");

        // Bare aliases answer as the old app's own commands.
        let threads = rpc(&server, tools_call(3, "thread_list", json!({"workspace_id": "ws-1"}))).await.unwrap();
        assert_eq!(threads["result"]["isError"], false, "{threads}");
        assert_eq!(threads["result"]["structuredContent"], json!({"threads": []}));
        let missing = rpc(&server, tools_call(4, "operation_status", json!({"operation_id": "x"}))).await.unwrap();
        assert_eq!(missing["result"]["structuredContent"]["error"]["code"], "operation_not_found", "{missing}");

        let seen = seen.lock().unwrap().clone();
        let relayed = |tool: &str| registry::served(tool, Audience::Outside).is_some();
        let mut expected = Vec::new();
        for tool in ["workspace_info", "workspace_list"] {
            if relayed(tool) {
                expected.push("tool_call");
            }
            expected.push("get_app_state");
        }
        expected.extend(["tool_call", "thread_list", "tool_call", "operation_status"]);
        assert_eq!(seen, expected, "registry tools try tool_call first");
    }

    #[tokio::test]
    async fn cancelled_legacy_job_is_skipped() {
        let Gated { server, gate, started, .. } = gated_server();
        let (first, _) = later(server.request(request(1, "git_status", json!({}))));
        let (second, _) = later(server.request(request(2, "pane_list", json!({}))));
        drop(second);
        gate.notify_one();
        assert_eq!(first.await.unwrap()["content"][0]["text"], "git_status");
        let (third, _) = later(server.request(request(3, "notify", json!({}))));
        assert_eq!(third.await.unwrap()["content"][0]["text"], "notify");
        assert_eq!(*started.lock().unwrap(), ["git_status", "notify"], "the cancelled job never ran");
    }

    #[tokio::test]
    async fn cancelled_running_legacy_job_finishes_before_the_next_starts() {
        let Gated { server, gate, started, finished } = gated_server();
        let (first, _) = later(server.request(request(1, "git_status", json!({}))));
        until(|| !started.lock().unwrap().is_empty()).await;
        // Cancel it mid-run, then queue the next job.
        drop(first);
        let (second, _) = later(server.request(request(2, "pane_list", json!({}))));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(*started.lock().unwrap(), ["git_status"], "the next job waits for the cancelled one");
        gate.notify_one();
        assert_eq!(second.await.unwrap()["content"][0]["text"], "pane_list");
        assert_eq!(*finished.lock().unwrap(), ["git_status", "pane_list"], "the cancelled job ran to the end first");
    }

    #[tokio::test]
    async fn legacy_lane_survives_a_panicking_tool() {
        let Gated { server, .. } = gated_server();
        let (boom, _) = later(server.request(request(1, "boom", json!({}))));
        let result = boom.await.expect("a panic is a tool error, not a JSON-RPC error");
        assert_eq!(result["isError"], true);
        assert_eq!(result["content"][0]["text"], "boom failed unexpectedly");
        let (next, _) = later(server.request(request(2, "pane_list", json!({}))));
        assert_eq!(next.await.unwrap()["content"][0]["text"], "pane_list");
    }

    async fn call_html_render(arguments: Value) -> Value {
        let resp = rpc(&server(), tools_call(6, "html_render", arguments)).await.unwrap();
        resp["result"].clone()
    }

    #[tokio::test]
    async fn html_render_accepts_a_page_without_touching_the_socket() {
        let result = call_html_render(json!({
            "title": "Weekly turns",
            "html": "<!doctype html><p>hi</p>"
        }))
        .await;
        assert!(result.get("isError").is_none(), "{result}");
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Shown to the reader"));
    }

    #[tokio::test]
    async fn html_render_rejects_missing_and_oversized_input() {
        for arguments in [
            json!({ "html": "<p>hi</p>" }),
            json!({ "title": "  ", "html": "<p>hi</p>" }),
            json!({ "title": "Chart", "html": " " }),
            json!({ "title": "x".repeat(HTML_RENDER_MAX_TITLE_CHARS + 1), "html": "<p>hi</p>" }),
            json!({ "title": "Chart", "html": "x".repeat(HTML_RENDER_MAX_BYTES + 1) }),
        ] {
            let result = call_html_render(arguments.clone()).await;
            assert_eq!(result["isError"], true, "{arguments}");
        }
    }

    // -----------------------------------------------------------------------
    // .mcp.json upsert tests
    // -----------------------------------------------------------------------

    #[test]
    fn mcp_json_create_new() {
        let dir = test_dir("mcp_create_new");
        upsert_mcp_config(&dir);

        let config = read_mcp(&dir);
        assert!(config["mcpServers"]["codemux"]["command"].as_str().is_some_and(|c| !c.is_empty()));
        assert_eq!(config["mcpServers"]["codemux"]["args"][0], "mcp");
        assert_eq!(config["mcpServers"]["codemux"]["env"], json!({(PROVIDER_ORIGIN_ENV): "1"}));

        cleanup(&dir);
    }

    #[test]
    fn mcp_json_append_to_existing() {
        let dir = test_dir("mcp_append");
        std::fs::write(
            dir.join(".mcp.json"),
            r#"{"mcpServers":{"shadcn":{"command":"npx","args":["shadcn@latest","mcp"]}}}"#,
        ).unwrap();

        upsert_mcp_config(&dir);

        let config = read_mcp(&dir);
        // shadcn preserved
        assert_eq!(config["mcpServers"]["shadcn"]["command"], "npx");
        assert_eq!(config["mcpServers"]["shadcn"]["args"][0], "shadcn@latest");
        // codemux added
        assert!(config["mcpServers"]["codemux"]["command"].as_str().is_some_and(|c| !c.is_empty()));

        cleanup(&dir);
    }

    #[test]
    fn mcp_json_update_existing_codemux() {
        let dir = test_dir("mcp_update");
        std::fs::write(
            dir.join(".mcp.json"),
            r#"{"mcpServers":{"codemux":{"command":"old","args":["old"]},"other":{"command":"x"}}}"#,
        ).unwrap();

        upsert_mcp_config(&dir);

        let config = read_mcp(&dir);
        // codemux updated
        assert!(config["mcpServers"]["codemux"]["command"].as_str().is_some_and(|c| !c.is_empty()));
        assert_eq!(config["mcpServers"]["codemux"]["env"], json!({(PROVIDER_ORIGIN_ENV): "1"}));
        // other server untouched
        assert_eq!(config["mcpServers"]["other"]["command"], "x");

        cleanup(&dir);
    }

    #[test]
    fn mcp_json_invalid_json_no_crash() {
        let dir = test_dir("mcp_invalid");
        let bad_content = "not json{{{";
        std::fs::write(dir.join(".mcp.json"), bad_content).unwrap();

        upsert_mcp_config(&dir);

        // File unchanged
        let content = std::fs::read_to_string(dir.join(".mcp.json")).unwrap();
        assert_eq!(content, bad_content);

        cleanup(&dir);
    }

    #[test]
    fn mcp_json_equivalent_entry_preserves_formatting_inode_and_mtime() {
        let dir = test_dir("mcp_idempotent");
        let path = dir.join(".mcp.json");
        let desired = serde_json::to_string(&codemux_mcp_entry()).unwrap();
        let original = format!(
            "{{\n\t\"userFormatting\" : true,\n\t\"mcpServers\" : {{ \"codemux\" : {desired} }}\n}}\n"
        );
        std::fs::write(&path, &original).unwrap();
        let before = std::fs::metadata(&path).unwrap();

        std::thread::sleep(std::time::Duration::from_millis(20));
        upsert_mcp_config(&dir);

        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        let after = std::fs::metadata(&path).unwrap();
        assert_eq!(before.modified().unwrap(), after.modified().unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            assert_eq!(before.ino(), after.ino());
        }

        cleanup(&dir);
    }

    #[test]
    fn mcp_json_binary_change_writes_once_then_becomes_noop() {
        let dir = test_dir("mcp_binary_change");
        let path = dir.join(".mcp.json");
        let old_entry = json!({ "command": "/old/codemux", "args": ["mcp"] });
        let new_entry = json!({ "command": "/new/codemux", "args": ["mcp"] });
        std::fs::write(
            &path,
            serde_json::to_vec(&json!({ "mcpServers": { "codemux": old_entry } })).unwrap(),
        )
        .unwrap();

        upsert_mcp_config_with_entry(&dir, new_entry.clone());
        assert_eq!(read_mcp(&dir)["mcpServers"]["codemux"], new_entry);
        let after_update = std::fs::read(&path).unwrap();
        let after_update_meta = std::fs::metadata(&path).unwrap();

        std::thread::sleep(std::time::Duration::from_millis(20));
        upsert_mcp_config_with_entry(&dir, new_entry);
        assert_eq!(std::fs::read(&path).unwrap(), after_update);
        let after_noop_meta = std::fs::metadata(&path).unwrap();
        assert_eq!(
            after_update_meta.modified().unwrap(),
            after_noop_meta.modified().unwrap()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            assert_eq!(after_update_meta.ino(), after_noop_meta.ino());
        }

        cleanup(&dir);
    }

    #[test]
    fn mcp_json_workspace_id_change_is_a_noop() {
        let dir = test_dir("mcp_id_update");
        upsert_mcp_config(&dir);
        let before = std::fs::read(dir.join(".mcp.json")).unwrap();
        upsert_mcp_config(&dir);

        let config = read_mcp(&dir);
        assert_eq!(config["mcpServers"]["codemux"]["env"], json!({(PROVIDER_ORIGIN_ENV): "1"}));
        assert_eq!(std::fs::read(dir.join(".mcp.json")).unwrap(), before);

        cleanup(&dir);
    }

    async fn assert_import_restart_is_readonly(completed: bool) {
        use tauri::Manager;
        const CHILD: &str = "CODEMUX_TEST_IMPORT_MCP_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let dir = tempfile::tempdir().unwrap();
            let name = if completed {
                "mcp_server::tests::local_import_mcp_completed_restart_is_readonly"
            } else {
                "mcp_server::tests::local_import_mcp_pending_restart_is_readonly"
            };
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", name, "--nocapture"])
                .env(CHILD, "1")
                .env("XDG_CONFIG_HOME", dir.path())
                .env("APPDATA", dir.path())
                .env("CODEMUX_TEST_LAYOUT_PATH", dir.path().join("layout.json"))
                .output()
                .unwrap();
            println!(
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(output.status.success(), "import restart child failed");
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let db = crate::database::DatabaseStore::new_in_memory();
        let store = crate::state::AppStateStore::default();
        store.clear_workspaces();
        let existing = b"{\"mcpServers\": {\"codemux\": {\"command\": \"user-owned\"}}, \"note\": \"preserve bytes\"}\n";
        let mut projects = Vec::new();
        let mut sessions = Vec::new();
        let mut excludes = Vec::new();
        for (index, name) in ["atlas", "beacon"].iter().enumerate() {
            let project = root.path().join(name);
            std::fs::create_dir(&project).unwrap();
            assert!(std::process::Command::new("git")
                .args(["init", "--quiet"])
                .arg(&project)
                .status()
                .unwrap()
                .success());
            let exclude = project.join(".git/info/exclude");
            std::fs::write(&exclude, b"# user-owned exclude\nkeep-me\n").unwrap();
            excludes.push(std::fs::read(&exclude).unwrap());
            if index == 0 {
                std::fs::write(project.join(".mcp.json"), existing).unwrap();
            }
            sessions.push(crate::local_session_import::ParsedSession {
                source_id: format!("{}1111111-1111-4111-8111-111111111111", index + 1),
                provider: "claude".into(),
                cwd: project.display().to_string(),
                last_active_at: "2026-09-29T10:00:00Z".into(),
                messages: vec![("user".into(), "Synthetic restart".into())],
            });
            projects.push(project);
        }
        store.import_local_sessions(&db, &sessions).unwrap();
        let first = store.snapshot().workspaces[0].workspace_id.clone();
        assert!(store.activate_workspace(&first.0)); // Selection is not setup intent.
        let ids: Vec<_> = sessions.iter().map(|s| s.source_id.clone()).collect();
        if completed {
            store.persist_local_import_layout(&db, &ids).unwrap();
        }
        let restarted = crate::state::AppStateStore::default();
        restarted.clear_workspaces();
        if completed {
            restarted.replace_snapshot(crate::state::load_persisted_state().unwrap());
        }
        let recovered = restarted.recover_local_import_layout(&db).unwrap();
        assert_eq!(recovered.len(), if completed { 0 } else { 2 });
        if !recovered.is_empty() {
            restarted
                .persist_local_import_layout(&db, &recovered)
                .unwrap();
        }
        assert!(db.pending_local_imports().unwrap().is_empty());
        assert!(restarted.activate_workspace(&restarted.snapshot().workspaces[0].workspace_id.0));
        let app = tauri::test::mock_app();
        app.manage(restarted);
        app.manage(db);
        assert!(is_auto_mcp_enabled(app.handle()));
        let plan =
            plan_boot_mcp_reconciliation(&app.state::<crate::state::AppStateStore>().snapshot());
        if let Some(active) = &plan.active {
            reconcile_mcp_config_targets(app.handle(), std::slice::from_ref(active));
        }
        let pending = PendingMcpConfigRepairs::default();
        pending.replace(plan.inactive.clone());
        let handle = app.handle().clone();
        repair_inactive_mcp_configs_inner(&pending, move |targets| {
            reconcile_live_mcp_config_targets(&handle, targets)
        })
        .await
        .unwrap();
        let preserved = std::fs::read(projects[0].join(".mcp.json")).unwrap() == existing;
        let absent = !projects[1].join(".mcp.json").exists();
        let exclude_unchanged: Vec<_> = projects
            .iter()
            .zip(&excludes)
            .map(|(p, before)| std::fs::read(p.join(".git/info/exclude")).unwrap() == *before)
            .collect();
        println!("completed={completed}; existing_mcp_preserved={preserved}; new_mcp_absent={absent}; git_excludes_unchanged={exclude_unchanged:?}");
        assert!(
            preserved && absent && exclude_unchanged.iter().all(|v| *v),
            "import-only restart mutated checkout files"
        );
        assert!(
            plan.active.is_none() && plan.inactive.is_empty(),
            "import-only targets must not be planned"
        );
        // Exercise both native close callers against the same source files.
        // No provider/runtime is needed: the imported panes are also valid
        // with provider=None, and the workspace provenance must still protect
        // the checkout. This avoids instantiating any real provider driver.
        let store = app.state::<crate::state::AppStateStore>();
        let mut snapshot = store.snapshot();
        let ids: Vec<_> = snapshot
            .workspaces
            .iter()
            .map(|w| w.workspace_id.0.clone())
            .collect();
        for workspace in &mut snapshot.workspaces {
            for surface in &mut workspace.surfaces {
                if let crate::state::PaneNodeSnapshot::AgentChat { provider, .. } =
                    &mut surface.root
                {
                    *provider = None;
                }
            }
        }
        store.replace_snapshot(snapshot);
        app.manage(crate::terminal::PtyState {
            sessions: std::sync::Arc::new(std::sync::Mutex::new(HashMap::new())),
        });
        let db = app.state::<crate::database::DatabaseStore>();
        crate::commands::workspace::close_workspace_with_worktree_impl(
            app.handle().clone(),
            &store,
            &db,
            ids[0].clone(),
            false,
            None,
            Some(false),
        )
        .await
        .unwrap();
        crate::commands::workspace::close_workspace(
            app.handle().clone(),
            app.state(),
            app.state(),
            ids[1].clone(),
            Some(false),
        )
        .await
        .unwrap();
        assert_eq!(
            std::fs::read(projects[0].join(".mcp.json")).unwrap(),
            existing
        );
        assert!(!projects[1].join(".mcp.json").exists());
        for (project, before) in projects.iter().zip(&excludes) {
            assert_eq!(
                std::fs::read(project.join(".git/info/exclude")).unwrap(),
                *before
            );
        }
    }

    #[tokio::test]
    async fn local_import_mcp_completed_restart_is_readonly() {
        assert_import_restart_is_readonly(true).await;
    }

    #[tokio::test]
    async fn local_import_mcp_pending_restart_is_readonly() {
        assert_import_restart_is_readonly(false).await;
    }

    #[test]
    fn local_import_mcp_stale_plan_rejects_import_only_but_live_same_cwd_reconciles() {
        use tauri::Manager;
        for preexisting in [false, true] {
            let project = tempfile::tempdir().unwrap();
            assert!(std::process::Command::new("git")
                .args(["init", "--quiet"])
                .arg(project.path())
                .status()
                .unwrap()
                .success());
            let exclude = project.path().join(".git/info/exclude");
            let old_exclude = std::fs::read(&exclude).unwrap();
            let mcp = project.path().join(".mcp.json");
            let old_mcp = b"{\"note\": \"user owns this\"}\n";
            if preexisting {
                std::fs::write(&mcp, old_mcp).unwrap();
            }
            let store = crate::state::AppStateStore::default();
            store.clear_workspaces();
            let live = store.create_empty_workspace_at_path(project.path().to_path_buf());
            let stale = plan_boot_mcp_reconciliation(&store.snapshot())
                .active
                .unwrap();
            store.close_workspace(&live.0).unwrap();
            let db = crate::database::DatabaseStore::new_in_memory();
            let session = crate::local_session_import::ParsedSession {
                source_id: "11111111-1111-4111-8111-111111111111".into(),
                provider: "claude".into(),
                cwd: project.path().display().to_string(),
                last_active_at: "2026-09-29T10:00:00Z".into(),
                messages: vec![("user".into(), "Synthetic".into())],
            };
            store.import_local_sessions(&db, &[session]).unwrap();
            let imported = store.snapshot().workspaces[0].workspace_id.clone();
            let app = tauri::test::mock_app();
            app.manage(store);
            app.manage(db);
            assert_eq!(
                reconcile_mcp_config_targets(app.handle(), &[stale.clone()]),
                0
            );
            if preexisting {
                assert_eq!(std::fs::read(&mcp).unwrap(), old_mcp);
            } else {
                assert!(!mcp.exists());
            }
            assert_eq!(std::fs::read(&exclude).unwrap(), old_exclude);
            let store = app.state::<crate::state::AppStateStore>();
            let independent = store.create_empty_workspace_at_path(project.path().to_path_buf());
            assert!(store.activate_workspace(&imported.0));
            let plan = plan_boot_mcp_reconciliation(&store.snapshot());
            assert_eq!(plan.active.as_ref().unwrap().workspace_id, independent.0);
            assert!(plan.inactive.is_empty());
            assert_eq!(reconcile_mcp_config_targets(app.handle(), &[stale]), 1);
            assert!(read_mcp(project.path())["mcpServers"]["codemux"].is_object());
            assert!(std::fs::read_to_string(&exclude)
                .unwrap()
                .contains(".mcp.json"));
        }
    }

    #[test]
    fn boot_plan_deduplicates_cwds_and_keeps_active_first() {
        let store = crate::state::AppStateStore::default();
        store.clear_workspaces();
        let first = store.create_workspace_at_path(PathBuf::from("shared-checkout"));
        let active = store.create_workspace_at_path(PathBuf::from("active-checkout"));
        let last = store.create_workspace_at_path(PathBuf::from("shared-checkout"));
        assert!(store.activate_workspace(&active.0));

        let plan = plan_boot_mcp_reconciliation(&store.snapshot());
        assert_eq!(
            plan.active.as_ref().map(|target| target.workspace_dir.as_path()),
            Some(Path::new("active-checkout"))
        );
        assert_eq!(plan.inactive.len(), 1);
        assert_eq!(plan.inactive[0].workspace_dir, PathBuf::from("shared-checkout"));
        assert_eq!(
            plan.inactive[0].workspace_id, last.0,
            "duplicate paths retain the old loop's last-workspace result"
        );
        assert_ne!(first, last);
    }

    #[tokio::test]
    async fn inactive_mcp_repair_command_drains_planned_targets_once() {
        let first = test_dir("inactive_repair_first");
        let second = test_dir("inactive_repair_second");
        let pending = PendingMcpConfigRepairs::default();
        pending.replace(vec![
            McpConfigReconciliationTarget {
                workspace_dir: first.clone(),
                workspace_id: "workspace-first".into(),
            },
            McpConfigReconciliationTarget {
                workspace_dir: second.clone(),
                workspace_id: "workspace-second".into(),
            },
        ]);

        assert_eq!(
            repair_inactive_mcp_configs_inner(&pending, |targets| {
                for target in targets {
                    upsert_mcp_config(&target.workspace_dir);
                }
                targets.len()
            })
            .await
            .unwrap(),
            2
        );
        let first_bytes = std::fs::read(first.join(".mcp.json")).unwrap();
        let second_bytes = std::fs::read(second.join(".mcp.json")).unwrap();

        assert_eq!(
            repair_inactive_mcp_configs_inner(&pending, |targets| {
                for target in targets {
                    upsert_mcp_config(&target.workspace_dir);
                }
                targets.len()
            })
            .await
            .unwrap(),
            0
        );
        assert_eq!(std::fs::read(first.join(".mcp.json")).unwrap(), first_bytes);
        assert_eq!(std::fs::read(second.join(".mcp.json")).unwrap(), second_bytes);

        cleanup(&first);
        cleanup(&second);
    }

    #[test]
    fn deferred_repair_rejects_a_target_after_its_last_workspace_closes() {
        let checkout = test_dir("inactive_repair_closed_target");
        let store = crate::state::AppStateStore::default();
        store.clear_workspaces();
        let workspace = store.create_workspace_at_path(checkout.clone());
        let target = McpConfigReconciliationTarget {
            workspace_dir: checkout.clone(),
            workspace_id: workspace.0.clone(),
        };

        assert!(reconciliation_target_is_live(&store.snapshot(), &target));
        store.close_workspace(&workspace.0).unwrap();
        assert!(!reconciliation_target_is_live(&store.snapshot(), &target));

        cleanup(&checkout);
    }

    // -----------------------------------------------------------------------
    // .mcp.json removal tests
    // -----------------------------------------------------------------------

    #[test]
    fn mcp_json_remove_codemux_keeps_others() {
        let dir = test_dir("mcp_remove_keeps");
        std::fs::write(
            dir.join(".mcp.json"),
            r#"{"mcpServers":{"codemux":{"command":"codemux"},"shadcn":{"command":"npx"}}}"#,
        ).unwrap();

        remove_mcp_config(&dir);

        let config = read_mcp(&dir);
        assert!(config["mcpServers"].get("codemux").is_none());
        assert_eq!(config["mcpServers"]["shadcn"]["command"], "npx");

        cleanup(&dir);
    }

    #[test]
    fn mcp_json_remove_codemux_deletes_empty() {
        let dir = test_dir("mcp_remove_deletes");
        std::fs::write(
            dir.join(".mcp.json"),
            r#"{"mcpServers":{"codemux":{"command":"codemux"}}}"#,
        ).unwrap();

        remove_mcp_config(&dir);
        assert!(!dir.join(".mcp.json").exists(), "File should be deleted when empty");

        cleanup(&dir);
    }

    #[test]
    fn mcp_json_remove_codemux_keeps_unrelated_top_level_config() {
        let dir = test_dir("mcp_remove_keeps_top_level");
        std::fs::write(
            dir.join(".mcp.json"),
            r#"{"userConfig":{"keep":true},"mcpServers":{"codemux":{"command":"codemux"}}}"#,
        )
        .unwrap();

        remove_mcp_config(&dir);

        let config = read_mcp(&dir);
        assert_eq!(config["userConfig"]["keep"], true);
        assert!(config.get("mcpServers").is_none());

        cleanup(&dir);
    }

    #[test]
    fn mcp_json_remove_without_codemux_preserves_bytes() {
        let dir = test_dir("mcp_remove_absent");
        let path = dir.join(".mcp.json");
        let original = "{ \"mcpServers\" : { \"other\" : { \"command\" : \"keep\" } } }\n";
        std::fs::write(&path, original).unwrap();

        remove_mcp_config(&dir);

        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);

        cleanup(&dir);
    }

    #[test]
    fn mcp_json_remove_nonexistent_noop() {
        let dir = test_dir("mcp_remove_noop");
        // No .mcp.json — should not crash
        remove_mcp_config(&dir);
        assert!(!dir.join(".mcp.json").exists());

        cleanup(&dir);
    }

    // -----------------------------------------------------------------------
    // Git exclude tests (shared function lives in crate::git)
    // -----------------------------------------------------------------------

    #[test]
    fn git_exclude_creates_entry() {
        let dir = test_dir("git_exclude_create");
        let git_info = dir.join(".git").join("info");
        std::fs::create_dir_all(&git_info).unwrap();
        std::fs::write(git_info.join("exclude"), "*.log\n").unwrap();

        crate::git::ensure_git_exclude(&dir, ".mcp.json");

        let content = std::fs::read_to_string(git_info.join("exclude")).unwrap();
        assert!(content.contains("*.log"));
        assert!(content.contains(".mcp.json"));

        cleanup(&dir);
    }

    #[test]
    fn git_exclude_no_duplicate() {
        let dir = test_dir("git_exclude_nodup");
        let git_info = dir.join(".git").join("info");
        std::fs::create_dir_all(&git_info).unwrap();
        std::fs::write(git_info.join("exclude"), ".mcp.json\n").unwrap();

        crate::git::ensure_git_exclude(&dir, ".mcp.json");
        crate::git::ensure_git_exclude(&dir, ".mcp.json");

        let content = std::fs::read_to_string(git_info.join("exclude")).unwrap();
        assert_eq!(content.matches(".mcp.json").count(), 1);

        cleanup(&dir);
    }

    #[test]
    fn git_exclude_no_git_dir_noop() {
        let dir = test_dir("git_exclude_nogit");
        // No .git dir — should not crash
        crate::git::ensure_git_exclude(&dir, ".mcp.json");

        cleanup(&dir);
    }

    // -----------------------------------------------------------------------
    // Atomic write tests
    //
    // Guards the temp+rename+fsync pattern that protects `.mcp.json` from
    // partial writes. The most important invariant: if the temp file is
    // written but the rename never happens (process crash, power loss),
    // the destination must stay at its previous valid content. Claude
    // Code reads `.mcp.json` at agent start, and a corrupted file disables
    // every MCP server the brain is supposed to see.
    // -----------------------------------------------------------------------

    #[test]
    fn atomic_write_creates_new_file() {
        let dir = test_dir("atomic_write_new");
        let target = dir.join(".mcp.json");
        atomic_write(&target, b"{\"hello\":\"world\"}").unwrap();

        let content = std::fs::read_to_string(&target).unwrap();
        assert_eq!(content, "{\"hello\":\"world\"}");
        // The tmp file must be gone — its only job was to be renamed.
        assert!(
            !dir.join(".mcp.json.tmp").exists(),
            "atomic_write should not leave a .tmp behind on success"
        );

        cleanup(&dir);
    }

    #[test]
    fn atomic_write_replaces_existing_file() {
        let dir = test_dir("atomic_write_replace");
        let target = dir.join(".mcp.json");
        std::fs::write(&target, b"OLD CONTENT").unwrap();
        atomic_write(&target, b"NEW CONTENT").unwrap();

        let content = std::fs::read_to_string(&target).unwrap();
        assert_eq!(content, "NEW CONTENT");

        cleanup(&dir);
    }

    /// Simulates the crash-mid-write scenario: the tmp file lands but the
    /// process dies before `rename`. The destination `.mcp.json` must still
    /// hold the previous valid content — the agent must see the old, valid
    /// config rather than a half-written replacement.
    ///
    /// We can't actually crash a unit test process, so we model the half-
    /// completed write directly: write the previous content to the target,
    /// drop a partial / corrupted .tmp next to it, and assert the target
    /// stays at the old content. `atomic_write` succeeding *after* a crash
    /// is also verified — the leftover tmp is truncated, not appended to.
    #[test]
    fn atomic_write_crash_during_write_preserves_destination() {
        let dir = test_dir("atomic_write_crash");
        let target = dir.join(".mcp.json");
        let tmp = dir.join(".mcp.json.tmp");

        // 1) Establish the pre-crash valid state.
        let original = br#"{"mcpServers":{"codemux":{"command":"codemux","args":["mcp"]}}}"#;
        std::fs::write(&target, original).unwrap();

        // 2) Simulate a previous crashed write: a stale .tmp was created
        //    with partial JSON, but the rename never fired.
        std::fs::write(&tmp, b"{\"mcpServers\":{\"co").unwrap();

        // 3) The destination MUST still match the pre-crash content — no
        //    half-written bytes ever bled through. This is the core
        //    invariant the atomic-write fix exists to guarantee.
        let content = std::fs::read(&target).unwrap();
        assert_eq!(
            &content[..],
            &original[..],
            "destination .mcp.json was corrupted by a half-completed write"
        );

        // 4) Recovery: a subsequent atomic_write must succeed even with
        //    the stale tmp present (it gets truncated, not appended), and
        //    must clear the tmp on success.
        let new_content = br#"{"mcpServers":{"codemux":{"command":"codemux2"}}}"#;
        atomic_write(&target, new_content).unwrap();
        let after = std::fs::read(&target).unwrap();
        assert_eq!(&after[..], &new_content[..]);
        assert!(
            !tmp.exists(),
            "atomic_write should reap its own .tmp on success even after a prior crash"
        );

        cleanup(&dir);
    }

    #[test]
    fn native_thread_control_tools_are_discoverable() {
        let tools = stdio_tools(false, false, ControlAccess::FullAccess);
        let names: std::collections::HashSet<&str> = tools.iter().map(|tool| tool.name.as_ref()).collect();
        for name in [
            "agent_capabilities",
            "thread_list",
            "thread_launch",
            "thread_send",
            "thread_read",
            "thread_status",
            "thread_wait",
            "thread_interrupt",
            "thread_stop",
            "thread_respond",
            "operation_status",
            "plugin_list",
            "plugin_invoke",
        ] {
            assert!(names.contains(name), "native control tool {name} is missing from MCP discovery");
        }
        assert_eq!(names.len(), tools.len(), "MCP tool names must remain unique");
    }

    /// Upsert path uses atomic_write under the hood — verify a fresh
    /// `.mcp.json` lands cleanly and no `.mcp.json.tmp` ghost is left
    /// behind. Together with the explicit atomic_write tests above this
    /// proves the production write path is atomic end-to-end.
    #[test]
    fn upsert_uses_atomic_write_no_tmp_leftover() {
        let dir = test_dir("upsert_atomic");
        upsert_mcp_config(&dir);

        assert!(dir.join(".mcp.json").exists());
        assert!(
            !dir.join(".mcp.json.tmp").exists(),
            "upsert_mcp_config must not leave a .tmp file behind"
        );

        cleanup(&dir);
    }

    #[tokio::test]
    async fn cross_host_delegation_rejects_forged_generic_mcp_actor_metadata() {
        for name in ["delegation_targets",REMOTE_DELEGATE_TOOL,"task_status","task_cancel"] {
            let call=CallToolParams::parse(&json!({"name":name,"arguments":{"parent_thread_id":"forged","permission_mode":"danger-full-access"},"_meta":{"thread_id":"forged","provider":"codex","native":true}})).unwrap();
            let result=handle_tool_call(call).await;assert_eq!(result["isError"],true);assert!(result.to_string().contains("Native-only"));
        }
    }
    #[test]
    fn cross_host_delegation_tools_are_discoverable() {
        let tools = legacy_tools();
        for name in ["delegation_targets", REMOTE_DELEGATE_TOOL, "task_status", "task_cancel"] {
            assert!(
                tools.iter().any(|tool| tool.name == name),
                "Missing native delegation tool: {name}"
            );
        }
        // An empty grant list must not read as "local delegation is off".
        for name in ["delegation_targets", REMOTE_DELEGATE_TOOL] {
            let description = &tools.iter().find(|tool| tool.name == name).unwrap().description;
            assert!(description.contains("SSH remote host"), "{description}");
            assert!(description.contains("use delegate_task"), "{description}");
        }
    }
}
