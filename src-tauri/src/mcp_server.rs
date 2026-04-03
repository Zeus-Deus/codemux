use crate::control::{send_control_request, ControlRequest, ControlResponse};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::Write;
use std::path::Path;

/// Check if auto-MCP config is enabled in settings (default: true).
pub fn is_auto_mcp_enabled(app: &tauri::AppHandle) -> bool {
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

async fn handle_tool_call(id: Value, params: Value) -> JsonRpcResponse {
    let tool_name = match params.get("name").and_then(Value::as_str) {
        Some(name) => name.to_string(),
        None => return JsonRpcResponse::error(id, INVALID_REQUEST, "Missing tool name"),
    };
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

    // Workspace ID for workspace-scoped browser routing.
    let workspace_id = std::env::var("CODEMUX_WORKSPACE_ID").unwrap_or_default();

    let result = match tool_name.as_str() {
        // -- Browser tools --
        "browser_navigate" => {
            let url = arguments.get("url").and_then(Value::as_str).unwrap_or_default();
            call_socket("browser_automation", json!({
                "workspace_id": &workspace_id,
                "action": { "kind": "open", "url": url }
            }))
            .await
        }
        "browser_snapshot" => {
            call_socket("browser_automation", json!({
                "workspace_id": &workspace_id,
                "action": { "kind": "eval", "script": crate::agent_browser::DOM_SNAPSHOT_SCRIPT }
            }))
            .await
        }
        "browser_accessibility_snapshot" => {
            call_socket("browser_automation", json!({
                "workspace_id": &workspace_id,
                "action": { "kind": "snapshot" }
            }))
            .await
        }
        "browser_click" => {
            let selector = arguments.get("selector").and_then(Value::as_str).unwrap_or_default();
            call_socket("browser_automation", json!({
                "workspace_id": &workspace_id,
                "action": { "kind": "click", "selector": selector }
            }))
            .await
        }
        "browser_fill" => {
            let selector = arguments.get("selector").and_then(Value::as_str).unwrap_or_default();
            let value = arguments.get("value").and_then(Value::as_str).unwrap_or_default();
            call_socket("browser_automation", json!({
                "workspace_id": &workspace_id,
                "action": { "kind": "fill", "selector": selector, "value": value }
            }))
            .await
        }
        "browser_screenshot" => {
            let result = call_socket("browser_automation", json!({
                "workspace_id": &workspace_id,
                "action": { "kind": "screenshot" }
            }))
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
            call_socket("browser_automation", json!({
                "workspace_id": &workspace_id,
                "action": { "kind": "console" }
            }))
            .await
        }

        // -- Coordinate-based vision tools --
        "browser_click_at" => {
            let x = arguments.get("x").and_then(Value::as_f64).unwrap_or(0.0);
            let y = arguments.get("y").and_then(Value::as_f64).unwrap_or(0.0);
            let ct = arguments.get("click_type").and_then(Value::as_str).unwrap_or("left");
            call_socket("browser_automation", json!({
                "workspace_id": &workspace_id,
                "action": { "kind": "click_at", "x": x, "y": y, "click_type": ct }
            })).await
        }
        "browser_type_at" => {
            let text = arguments.get("text").and_then(Value::as_str).unwrap_or_default();
            let mut action = json!({ "kind": "type_at", "text": text });
            if let Some(x) = arguments.get("x").and_then(Value::as_f64) { action["x"] = json!(x); }
            if let Some(y) = arguments.get("y").and_then(Value::as_f64) { action["y"] = json!(y); }
            call_socket("browser_automation", json!({ "workspace_id": &workspace_id, "action": action })).await
        }
        "browser_scroll_at" => {
            let x = arguments.get("x").and_then(Value::as_f64).unwrap_or(0.0);
            let y = arguments.get("y").and_then(Value::as_f64).unwrap_or(0.0);
            let dir = arguments.get("direction").and_then(Value::as_str).unwrap_or("down");
            let amt = arguments.get("amount").and_then(Value::as_f64).unwrap_or(3.0);
            call_socket("browser_automation", json!({
                "workspace_id": &workspace_id,
                "action": { "kind": "scroll_at", "x": x, "y": y, "direction": dir, "amount": amt }
            })).await
        }
        "browser_key_press" => {
            let key = arguments.get("key").and_then(Value::as_str).unwrap_or("Enter");
            call_socket("browser_automation", json!({
                "workspace_id": &workspace_id,
                "action": { "kind": "key_press", "key": key }
            })).await
        }
        "browser_drag" => {
            let sx = arguments.get("start_x").and_then(Value::as_f64).unwrap_or(0.0);
            let sy = arguments.get("start_y").and_then(Value::as_f64).unwrap_or(0.0);
            let ex = arguments.get("end_x").and_then(Value::as_f64).unwrap_or(0.0);
            let ey = arguments.get("end_y").and_then(Value::as_f64).unwrap_or(0.0);
            call_socket("browser_automation", json!({
                "workspace_id": &workspace_id,
                "action": { "kind": "drag", "start_x": sx, "start_y": sy, "end_x": ex, "end_y": ey }
            })).await
        }
        // -- OS-level input tools --
        "browser_click_os" => {
            let x = arguments.get("x").and_then(Value::as_f64).unwrap_or(0.0);
            let y = arguments.get("y").and_then(Value::as_f64).unwrap_or(0.0);
            call_socket("browser_automation", json!({
                "workspace_id": &workspace_id,
                "action": { "kind": "click_os", "x": x, "y": y }
            })).await
        }
        "browser_type_os" => {
            let text = arguments.get("text").and_then(Value::as_str).unwrap_or_default();
            let mut action = json!({ "kind": "type_os", "text": text });
            if let Some(x) = arguments.get("x").and_then(Value::as_f64) { action["x"] = json!(x); }
            if let Some(y) = arguments.get("y").and_then(Value::as_f64) { action["y"] = json!(y); }
            call_socket("browser_automation", json!({ "workspace_id": &workspace_id, "action": action })).await
        }

        // -- Browser info tools (v0.24.0) --
        "browser_get_styles" => {
            let selector = arguments.get("selector").and_then(Value::as_str).unwrap_or_default();
            call_socket("browser_automation", json!({
                "workspace_id": &workspace_id,
                "action": { "kind": "get_styles", "selector": selector }
            })).await
        }
        "browser_wait" => {
            let selector = arguments.get("selector").and_then(Value::as_str);
            let text = arguments.get("text").and_then(Value::as_str);
            let mut action = json!({ "kind": "wait" });
            if let Some(sel) = selector { action["selector"] = json!(sel); }
            if let Some(t) = text { action["text"] = json!(t); }
            call_socket("browser_automation", json!({
                "workspace_id": &workspace_id,
                "action": action
            })).await
        }
        "browser_evaluate" => {
            let script = arguments.get("script").and_then(Value::as_str).unwrap_or_default();
            call_socket("browser_automation", json!({
                "workspace_id": &workspace_id,
                "action": { "kind": "eval", "script": script }
            })).await
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
            call_socket("split_pane", json!({ "pane_id": pane_id, "direction": "vertical" })).await
        }
        "pane_split_down" => {
            let pane_id = arguments.get("pane_id").and_then(Value::as_str).unwrap_or_default();
            call_socket("split_pane", json!({ "pane_id": pane_id, "direction": "horizontal" })).await
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
            if let Err(e) = std::fs::write(&mcp_path, &json) {
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
        let _ = std::fs::write(&mcp_path, json);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(tools.len(), 29);
        let names: Vec<&str> = tools.iter().map(|t| t.name).collect();
        assert!(names.contains(&"browser_navigate"));
        assert!(names.contains(&"browser_click"));
        assert!(names.contains(&"browser_fill"));
        assert!(names.contains(&"browser_screenshot"));
        assert!(names.contains(&"workspace_list"));
        assert!(names.contains(&"workspace_info"));
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
        assert_eq!(tools.len(), 29);
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
}
