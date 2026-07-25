use crate::control::{send_control_request, ControlRequest, ControlResponse};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::Write;
use std::path::Path;

/// Check if auto-MCP config is enabled in settings (default: true).
pub fn is_auto_mcp_enabled<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> bool {
    use tauri::Manager;
    let db: tauri::State<'_, crate::database::DatabaseStore> = app.state();
    db.get_setting("auto_mcp_config")
        .map(|v| v != "false")
        .unwrap_or(true)
}

// ---------------------------------------------------------------------------
// JSON-RPC 2.0 types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    #[allow(dead_code)]
    jsonrpc: String,
    method: String,
    #[serde(default)]
    params: Value,
    id: Option<Value>,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
    id: Value,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

impl JsonRpcResponse {
    fn success(id: Value, result: Value) -> Self {
        Self { jsonrpc: "2.0", result: Some(result), error: None, id }
    }

    fn error(id: Value, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            result: None,
            error: Some(JsonRpcError { code, message: message.into(), data: None }),
            id,
        }
    }
}

// JSON-RPC error codes
const INVALID_REQUEST: i32 = -32600;
const METHOD_NOT_FOUND: i32 = -32601;
#[allow(dead_code)]
const INTERNAL_ERROR: i32 = -32603;
const PARSE_ERROR: i32 = -32700;

// ---------------------------------------------------------------------------
// MCP tool definition
// ---------------------------------------------------------------------------

struct McpTool {
    name: &'static str,
    description: &'static str,
    input_schema: Value,
}

fn register_tools() -> Vec<McpTool> {
    vec![
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
                        "description": "Base branch to fork from when `new_branch` is true. Defaults to \"main\".",
                        "default": "main"
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
            description: "Close a workspace by id. Runs teardown scripts, terminates PTYs, releases the workspace's virtual display, and removes Codemux's entry from the workspace's `.mcp.json`. Optionally also removes the underlying git worktree. Protected project-root checkouts can NEVER have their files deleted through this tool — `delete_worktree: true` on such a workspace errors; use `workspace_archive` instead to close it restorably.",
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
    ]
}

// ---------------------------------------------------------------------------
// MCP protocol dispatch
// ---------------------------------------------------------------------------

async fn dispatch(request: JsonRpcRequest) -> Option<JsonRpcResponse> {
    let id = match &request.id {
        Some(id) => id.clone(),
        None => {
            // Notifications (no id) get no response.
            return None;
        }
    };

    let response = match request.method.as_str() {
        "initialize" => JsonRpcResponse::success(
            id,
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": {
                    "name": "codemux",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        ),
        "tools/list" => {
            let tools: Vec<Value> = register_tools()
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "inputSchema": t.input_schema
                    })
                })
                .collect();
            JsonRpcResponse::success(id, json!({ "tools": tools }))
        }
        "tools/call" => handle_tool_call(id, request.params).await,
        "ping" => JsonRpcResponse::success(id, json!({})),
        _ => JsonRpcResponse::error(id, METHOD_NOT_FOUND, format!("Unknown method: {}", request.method)),
    };
    Some(response)
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

async fn handle_tool_call(id: Value, params: Value) -> JsonRpcResponse {
    let tool_name = match params.get("name").and_then(Value::as_str) {
        Some(name) => name.to_string(),
        None => return JsonRpcResponse::error(id, INVALID_REQUEST, "Missing tool name"),
    };
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

    // Workspace ID for workspace-scoped browser routing.
    let workspace_id = std::env::var("CODEMUX_WORKSPACE_ID").unwrap_or_default();
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

        // -- Notification tools --
        "notify" => {
            let message = arguments.get("message").and_then(Value::as_str).unwrap_or_default();
            let level = arguments.get("level").and_then(Value::as_str).unwrap_or("attention");
            call_socket("notify", json!({ "message": message, "level": level })).await
        }

        // -- Git tools (shell out) --
        "git_status" => run_git(&["status", "--porcelain"]).await,
        "git_diff" => {
            let file = arguments.get("file").and_then(Value::as_str);
            match file {
                Some(f) => run_git(&["diff", f]).await,
                None => run_git(&["diff"]).await,
            }
        }
        "git_stage" => {
            let file = arguments.get("file").and_then(Value::as_str).unwrap_or(".");
            run_git(&["add", file]).await
        }
        "git_commit" => {
            let message = arguments.get("message").and_then(Value::as_str).unwrap_or_default();
            run_git(&["commit", "-m", message]).await
        }
        "git_push" => run_git(&["push"]).await,

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
                // Build params lazily so the socket arm's defaults
                // (new_branch=true, base="main", layout="single") fire
                // when the brain omits them. Passing them explicitly
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

        _ => Err(format!("Unknown tool: {tool_name}")),
    };

    match result {
        Ok(data) => {
            let text = if data.is_string() {
                data.as_str().unwrap_or_default().to_string()
            } else {
                serde_json::to_string_pretty(&data).unwrap_or_default()
            };
            JsonRpcResponse::success(
                id,
                json!({ "content": [{ "type": "text", "text": text }] }),
            )
        }
        Err(error) => JsonRpcResponse::success(
            id,
            json!({
                "content": [{ "type": "text", "text": error }],
                "isError": true
            }),
        ),
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

// ---------------------------------------------------------------------------
// Git helper (shell out in workspace cwd)
// ---------------------------------------------------------------------------

async fn get_workspace_cwd() -> Result<String, String> {
    // Try CODEMUX_WORKSPACE_ID env var first to get workspace path via app state.
    if let Ok(workspace_id) = std::env::var("CODEMUX_WORKSPACE_ID") {
        let response = call_socket("get_app_state", json!({})).await?;
        if let Some(workspaces) = response.get("workspaces").and_then(Value::as_array) {
            for ws in workspaces {
                let wid = ws.get("workspace_id");
                let wid_str = wid
                    .and_then(Value::as_str)
                    .or_else(|| wid.and_then(|v| v.as_object()).and_then(|o| o.get("0")).and_then(Value::as_str));
                if wid_str == Some(&workspace_id) {
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

async fn run_git(args: &[&str]) -> Result<Value, String> {
    let cwd = get_workspace_cwd().await?;
    let output = tokio::process::Command::new("git")
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
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();

    // Log to stderr so it doesn't interfere with the JSON-RPC protocol on stdout.
    eprintln!("[codemux::mcp] MCP server starting (stdio transport)");

    let reader = std::io::BufReader::new(stdin.lock());
    let mut stdout = stdout.lock();

    for line in std::io::BufRead::lines(reader) {
        let line = line.map_err(|e| format!("stdin read error: {e}"))?;
        if line.trim().is_empty() {
            continue;
        }

        let request: JsonRpcRequest = match serde_json::from_str(&line) {
            Ok(req) => req,
            Err(e) => {
                let error_resp = JsonRpcResponse::error(Value::Null, PARSE_ERROR, format!("Parse error: {e}"));
                let json = serde_json::to_string(&error_resp).unwrap_or_default();
                let _ = writeln!(stdout, "{json}");
                let _ = stdout.flush();
                continue;
            }
        };

        if let Some(response) = dispatch(request).await {
            let json = serde_json::to_string(&response).unwrap_or_default();
            let _ = writeln!(stdout, "{json}");
            let _ = stdout.flush();
        }
    }

    eprintln!("[codemux::mcp] MCP server shutting down (stdin closed)");
    Ok(())
}

// ---------------------------------------------------------------------------
// .mcp.json auto-discovery helpers
// ---------------------------------------------------------------------------

/// Build the codemux MCP server entry for .mcp.json.
/// Uses the absolute path to the current binary so agents can find it
/// regardless of PATH.
fn codemux_mcp_entry(workspace_id: &str) -> Value {
    let command = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "codemux".to_string());
    json!({
        "command": command,
        "args": ["mcp"],
        "env": {
            "CODEMUX_WORKSPACE_ID": workspace_id
        }
    })
}

/// Upsert the "codemux" entry in `.mcp.json`.
///
/// - If the file doesn't exist, creates it with the codemux entry.
/// - If it exists with valid JSON, merges the codemux entry alongside any
///   existing servers (shadcn, database tools, etc.) — never removes them.
/// - If it exists but is invalid JSON, logs a warning and does NOT modify it.
pub fn upsert_mcp_config(workspace_dir: &Path, workspace_id: &str) {
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

    // Ensure mcpServers object exists.
    if !config.get("mcpServers").is_some_and(Value::is_object) {
        config["mcpServers"] = json!({});
    }
    config["mcpServers"]["codemux"] = codemux_mcp_entry(workspace_id);

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

    servers.remove("codemux");

    if servers.is_empty() {
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
    fn tool_registry_has_all_tools() {
        let tools = register_tools();
        // Tool count bumped from 39 → 44 with the Phase 1.6 lifecycle +
        // issue tools, then 44 → 52 with the eight automation tools,
        // then 52 → 55 with the workspace-archive tools
        // (workspace_archive / workspace_unarchive /
        // workspace_archive_list). Keep this number in sync with
        // register_tools() when adding new entries.
        assert_eq!(tools.len(), 55);
        let names: Vec<&str> = tools.iter().map(|t| t.name).collect();
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

    #[test]
    fn tool_schemas_are_valid_json_objects() {
        for tool in register_tools() {
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
        for tool in register_tools() {
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
        let tools = register_tools();
        let mut names: Vec<&str> = tools.iter().map(|t| t.name).collect();
        let original_len = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), original_len, "Duplicate tool names found");
    }

    // -----------------------------------------------------------------------
    // JSON-RPC response tests
    // -----------------------------------------------------------------------

    #[test]
    fn jsonrpc_response_success_format() {
        let resp = JsonRpcResponse::success(json!(1), json!({"hello": "world"}));
        let serialized = serde_json::to_value(&resp).unwrap();
        assert_eq!(serialized["jsonrpc"], "2.0");
        assert_eq!(serialized["id"], 1);
        assert!(serialized.get("result").is_some());
        assert!(serialized.get("error").is_none());
    }

    #[test]
    fn jsonrpc_response_error_format() {
        let resp = JsonRpcResponse::error(json!(2), -32600, "Bad request");
        let serialized = serde_json::to_value(&resp).unwrap();
        assert_eq!(serialized["jsonrpc"], "2.0");
        assert_eq!(serialized["id"], 2);
        assert!(serialized.get("result").is_none());
        assert_eq!(serialized["error"]["code"], -32600);
        assert_eq!(serialized["error"]["message"], "Bad request");
    }

    #[test]
    fn jsonrpc_error_omits_null_data() {
        let resp = JsonRpcResponse::error(json!(3), -32601, "Not found");
        let serialized = serde_json::to_string(&resp).unwrap();
        assert!(!serialized.contains("\"data\""), "data field should be omitted when None");
    }

    // -----------------------------------------------------------------------
    // Dispatch tests (async)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn dispatch_initialize() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "initialize".into(),
            params: json!({"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}),
            id: Some(json!(1)),
        };
        let resp = dispatch(req).await.unwrap();
        let result = resp.result.unwrap();
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert!(result["serverInfo"]["name"].as_str() == Some("codemux"));
    }

    #[tokio::test]
    async fn dispatch_tools_list() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "tools/list".into(),
            params: json!({}),
            id: Some(json!(2)),
        };
        let resp = dispatch(req).await.unwrap();
        let result = resp.result.unwrap();
        let tools = result["tools"].as_array().unwrap();
        // Bumped 39 → 44 with the Phase 1.6 lifecycle + issue tools,
        // then 44 → 52 with the eight automation tools, then 52 → 55
        // with the workspace-archive tools. See
        // tool_registry_has_all_tools for the canonical count.
        assert_eq!(tools.len(), 55);
        for tool in tools {
            assert!(tool.get("name").is_some());
            assert!(tool.get("description").is_some());
            assert!(tool.get("inputSchema").is_some());
        }
    }

    #[tokio::test]
    async fn dispatch_unknown_method() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "nonexistent/method".into(),
            params: json!({}),
            id: Some(json!(3)),
        };
        let resp = dispatch(req).await.unwrap();
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn dispatch_notification_no_response() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "notifications/initialized".into(),
            params: json!({}),
            id: None,
        };
        let resp = dispatch(req).await;
        assert!(resp.is_none(), "Notifications should return None");
    }

    #[tokio::test]
    async fn dispatch_ping() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "ping".into(),
            params: json!({}),
            id: Some(json!(4)),
        };
        let resp = dispatch(req).await.unwrap();
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }

    #[tokio::test]
    async fn dispatch_tools_call_unknown_tool() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            method: "tools/call".into(),
            params: json!({"name": "nonexistent_tool", "arguments": {}}),
            id: Some(json!(5)),
        };
        let resp = dispatch(req).await.unwrap();
        let result = resp.result.unwrap();
        assert_eq!(result["isError"], true);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Unknown tool"));
    }

    // -----------------------------------------------------------------------
    // .mcp.json upsert tests
    // -----------------------------------------------------------------------

    #[test]
    fn mcp_json_create_new() {
        let dir = test_dir("mcp_create_new");
        upsert_mcp_config(&dir, "ws-123");

        let config = read_mcp(&dir);
        assert!(config["mcpServers"]["codemux"]["command"].as_str().is_some_and(|c| !c.is_empty()));
        assert_eq!(config["mcpServers"]["codemux"]["args"][0], "mcp");
        assert_eq!(config["mcpServers"]["codemux"]["env"]["CODEMUX_WORKSPACE_ID"], "ws-123");

        cleanup(&dir);
    }

    #[test]
    fn mcp_json_append_to_existing() {
        let dir = test_dir("mcp_append");
        std::fs::write(
            dir.join(".mcp.json"),
            r#"{"mcpServers":{"shadcn":{"command":"npx","args":["shadcn@latest","mcp"]}}}"#,
        ).unwrap();

        upsert_mcp_config(&dir, "ws-456");

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

        upsert_mcp_config(&dir, "ws-new");

        let config = read_mcp(&dir);
        // codemux updated
        assert!(config["mcpServers"]["codemux"]["command"].as_str().is_some_and(|c| !c.is_empty()));
        assert_eq!(config["mcpServers"]["codemux"]["env"]["CODEMUX_WORKSPACE_ID"], "ws-new");
        // other server untouched
        assert_eq!(config["mcpServers"]["other"]["command"], "x");

        cleanup(&dir);
    }

    #[test]
    fn mcp_json_invalid_json_no_crash() {
        let dir = test_dir("mcp_invalid");
        let bad_content = "not json{{{";
        std::fs::write(dir.join(".mcp.json"), bad_content).unwrap();

        upsert_mcp_config(&dir, "ws-789");

        // File unchanged
        let content = std::fs::read_to_string(dir.join(".mcp.json")).unwrap();
        assert_eq!(content, bad_content);

        cleanup(&dir);
    }

    #[test]
    fn mcp_json_idempotent() {
        let dir = test_dir("mcp_idempotent");
        upsert_mcp_config(&dir, "ws-111");
        upsert_mcp_config(&dir, "ws-111");

        let config = read_mcp(&dir);
        let servers = config["mcpServers"].as_object().unwrap();
        assert_eq!(servers.len(), 1, "Should have exactly one server entry");
        assert!(servers.contains_key("codemux"));

        cleanup(&dir);
    }

    #[test]
    fn mcp_json_workspace_id_updated() {
        let dir = test_dir("mcp_id_update");
        upsert_mcp_config(&dir, "ws-old-id");
        upsert_mcp_config(&dir, "ws-new-id");

        let config = read_mcp(&dir);
        assert_eq!(config["mcpServers"]["codemux"]["env"]["CODEMUX_WORKSPACE_ID"], "ws-new-id");

        cleanup(&dir);
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

    /// Upsert path uses atomic_write under the hood — verify a fresh
    /// `.mcp.json` lands cleanly and no `.mcp.json.tmp` ghost is left
    /// behind. Together with the explicit atomic_write tests above this
    /// proves the production write path is atomic end-to-end.
    #[test]
    fn upsert_uses_atomic_write_no_tmp_leftover() {
        let dir = test_dir("upsert_atomic");
        upsert_mcp_config(&dir, "ws-atomic");

        assert!(dir.join(".mcp.json").exists());
        assert!(
            !dir.join(".mcp.json.tmp").exists(),
            "upsert_mcp_config must not leave a .tmp file behind"
        );

        cleanup(&dir);
    }
}
