use crate::control::{send_control_request, ControlRequest};
use clap::{Parser, Subcommand};
use serde_json::{json, Value};

#[derive(Parser)]
#[command(name = "codemux", about = "Codemux desktop and control CLI")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<CommandSet>,
}

#[derive(Subcommand)]
pub enum CommandSet {
    App,
    Status,
    Json { command: String, params: Option<String> },
    Notify { message: String },
    Handoff,
    Memory {
        #[command(subcommand)]
        command: MemoryCommand,
    },
    Index {
        #[command(subcommand)]
        command: IndexCommand,
    },
    Browser {
        #[command(subcommand)]
        command: BrowserCommand,
    },
    /// GitHub issue operations
    Issue {
        #[command(subcommand)]
        command: IssueCommand,
    },
    /// Workspace operations
    Workspace {
        #[command(subcommand)]
        command: WorkspaceCommand,
    },
    /// List all available codemux commands and capabilities
    Capabilities,
    /// Start MCP server (JSON-RPC over stdio)
    Mcp,
}

#[derive(Subcommand)]
pub enum WorkspaceCommand {
    /// Re-run setup scripts (.codemuxinclude + setup commands) for a workspace
    RerunSetup {
        /// Workspace ID (defaults to active workspace)
        workspace_id: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum BrowserCommand {
    Create,
    Open { url: String },
    Snapshot {
        browser_id: Option<String>,
        /// Use DOM-based query instead of ARIA tree
        #[arg(long)]
        dom: bool,
    },
    Click { selector: String, browser_id: Option<String> },
    Fill { selector: String, value: String, browser_id: Option<String> },
    Screenshot { browser_id: Option<String> },
    ConsoleLogs { browser_id: Option<String> },
    /// Click at pixel coordinates via CDP
    ClickAt { x: f64, y: f64, #[arg(long, default_value = "left")] click_type: String, browser_id: Option<String> },
    /// Type text at coordinates or cursor position via CDP
    TypeAt { text: String, #[arg(long)] x: Option<f64>, #[arg(long)] y: Option<f64>, browser_id: Option<String> },
    /// Scroll at coordinates via CDP
    ScrollAt { x: f64, y: f64, #[arg(long, default_value = "down")] direction: String, #[arg(long, default_value = "3")] amount: i32, browser_id: Option<String> },
    /// Press a key or combo via CDP
    KeyPress { key: String, browser_id: Option<String> },
    /// Drag between coordinates via CDP
    Drag { start_x: f64, start_y: f64, end_x: f64, end_y: f64, browser_id: Option<String> },
    /// Click at coordinates using OS-level input (ydotool)
    ClickOs { x: f64, y: f64, browser_id: Option<String> },
    /// Type text using OS-level input (ydotool)
    TypeOs { text: String, #[arg(long)] x: Option<f64>, #[arg(long)] y: Option<f64>, browser_id: Option<String> },
}

#[derive(Subcommand)]
pub enum MemoryCommand {
    Show,
    Set {
        #[arg(long)]
        brief: Option<String>,
        #[arg(long)]
        goal: Option<String>,
        #[arg(long)]
        focus: Option<String>,
        #[arg(long = "constraint")]
        constraints: Vec<String>,
    },
    Add {
        kind: String,
        content: String,
        #[arg(long)]
        tool: Option<String>,
        #[arg(long)]
        session: Option<String>,
        #[arg(long = "tag")]
        tags: Vec<String>,
    },
}

#[derive(Subcommand)]
pub enum IssueCommand {
    /// List open GitHub issues for the current workspace's repo
    List {
        /// Search query (searches title and body)
        #[arg(long)]
        search: Option<String>,
        /// Issue state filter: open, closed, or all
        #[arg(long, default_value = "open")]
        state: String,
    },
    /// View a specific GitHub issue
    View {
        /// Issue number
        number: u64,
    },
    /// Link a GitHub issue to the active workspace
    Link {
        /// Issue number
        number: u64,
    },
}

#[derive(Subcommand)]
pub enum IndexCommand {
    Build,
    Status,
    Search {
        query: String,
        #[arg(long)]
        limit: Option<usize>,
    },
}

pub async fn maybe_run_cli() -> Result<bool, String> {
    let cli = Cli::parse();
    match cli.command {
        None | Some(CommandSet::App) => Ok(false),
        Some(CommandSet::Status) => {
            let response = send_control_request(ControlRequest {
                command: "status".into(),
                params: json!({}),
            })
            .await?;
            println!("{}", serde_json::to_string_pretty(&response).map_err(|error| error.to_string())?);
            Ok(true)
        }
        Some(CommandSet::Json { command, params }) => {
            let params = params
                .map(|raw| serde_json::from_str(&raw).map_err(|error| error.to_string()))
                .transpose()?
                .unwrap_or_else(|| json!({}));
            let response = send_control_request(ControlRequest { command, params }).await?;
            println!("{}", serde_json::to_string_pretty(&response).map_err(|error| error.to_string())?);
            Ok(true)
        }
        Some(CommandSet::Notify { message }) => {
            let response = send_control_request(ControlRequest {
                command: "notify".into(),
                params: json!({ "message": message }),
            })
            .await?;
            println!("{}", serde_json::to_string_pretty(&response).map_err(|error| error.to_string())?);
            Ok(true)
        }
        Some(CommandSet::Handoff) => {
            let response = send_control_request(ControlRequest {
                command: "generate_handoff".into(),
                params: json!({}),
            })
            .await?;
            println!("{}", serde_json::to_string_pretty(&response).map_err(|error| error.to_string())?);
            Ok(true)
        }
        Some(CommandSet::Memory { command }) => {
            let response = match command {
                MemoryCommand::Show => {
                    send_control_request(ControlRequest {
                        command: "get_project_memory".into(),
                        params: json!({}),
                    })
                    .await?
                }
                MemoryCommand::Set {
                    brief,
                    goal,
                    focus,
                    constraints,
                } => {
                    send_control_request(ControlRequest {
                        command: "update_project_memory".into(),
                        params: json!({
                            "update": {
                                "project_brief": brief,
                                "current_goal": goal,
                                "current_focus": focus,
                                "constraints": if constraints.is_empty() { Value::Null } else { json!(constraints) }
                            }
                        }),
                    })
                    .await?
                }
                MemoryCommand::Add {
                    kind,
                    content,
                    tool,
                    session,
                    tags,
                } => {
                    send_control_request(ControlRequest {
                        command: "add_project_memory_entry".into(),
                        params: json!({
                            "kind": normalize_memory_kind(&kind),
                            "source": "human",
                            "content": content,
                            "tool_name": tool,
                            "session_label": session,
                            "tags": tags
                        }),
                    })
                    .await?
                }
            };

            println!("{}", serde_json::to_string_pretty(&response).map_err(|error| error.to_string())?);
            Ok(true)
        }
        Some(CommandSet::Index { command }) => {
            let response = match command {
                IndexCommand::Build => {
                    send_control_request(ControlRequest {
                        command: "rebuild_index".into(),
                        params: json!({}),
                    })
                    .await?
                }
                IndexCommand::Status => {
                    send_control_request(ControlRequest {
                        command: "index_status".into(),
                        params: json!({}),
                    })
                    .await?
                }
                IndexCommand::Search { query, limit } => {
                    send_control_request(ControlRequest {
                        command: "search_index".into(),
                        params: json!({ "query": query, "limit": limit }),
                    })
                    .await?
                }
            };

            println!("{}", serde_json::to_string_pretty(&response).map_err(|error| error.to_string())?);
            Ok(true)
        }
        Some(CommandSet::Issue { command }) => {
            let response = match command {
                IssueCommand::List { search, state: _state } => {
                    let params = if let Some(ref q) = search {
                        json!({ "search": q })
                    } else {
                        json!({})
                    };
                    let response = send_control_request(ControlRequest {
                        command: "list_github_issues".into(),
                        params,
                    })
                    .await?;
                    // Pretty-print as table
                    if let Some(data) = &response.data {
                        if let Some(issues) = data.as_array() {
                            if issues.is_empty() {
                                println!("No issues found.");
                            } else {
                                for issue in issues {
                                    let num = issue["number"].as_u64().unwrap_or(0);
                                    let title = issue["title"].as_str().unwrap_or("");
                                    let state_str = issue["state"].as_str().unwrap_or("Open");
                                    println!("#{:<6} {:8} {}", num, state_str, title);
                                }
                            }
                            return Ok(true);
                        }
                    }
                    response
                }
                IssueCommand::View { number } => {
                    let response = send_control_request(ControlRequest {
                        command: "get_github_issue".into(),
                        params: json!({ "number": number }),
                    })
                    .await?;
                    if let Some(data) = &response.data {
                        let title = data["title"].as_str().unwrap_or("");
                        let state_str = data["state"].as_str().unwrap_or("Open");
                        let url = data["url"].as_str().unwrap_or("");
                        let body = data["body"].as_str().unwrap_or("(no body)");
                        let labels: Vec<&str> = data["labels"]
                            .as_array()
                            .map(|a| a.iter().filter_map(Value::as_str).collect())
                            .unwrap_or_default();

                        println!("#{} — {} [{}]", number, title, state_str);
                        if !url.is_empty() {
                            println!("{}", url);
                        }
                        if !labels.is_empty() {
                            println!("Labels: {}", labels.join(", "));
                        }
                        println!();
                        println!("{}", body);
                        return Ok(true);
                    }
                    response
                }
                IssueCommand::Link { number } => {
                    send_control_request(ControlRequest {
                        command: "link_workspace_issue".into(),
                        params: json!({ "number": number }),
                    })
                    .await?
                }
            };

            println!("{}", serde_json::to_string_pretty(&response).map_err(|error| error.to_string())?);
            Ok(true)
        }
        Some(CommandSet::Browser { command }) => {
            let ws_id = std::env::var("CODEMUX_WORKSPACE_ID").unwrap_or_default();
            let result = match command {
                BrowserCommand::Create => {
                    let response = send_control_request(ControlRequest {
                        command: "create_browser_pane".to_string(),
                        params: json!({"pane_id": ""}),
                    }).await?;
                    Ok::<_, String>(json!({ "ok": true, "data": response.data.unwrap_or(json!(null)) }))
                }
                BrowserCommand::Open { url } => {
                    let response = send_control_request(ControlRequest {
                        command: "browser_automation".into(),
                        params: json!({ "workspace_id": &ws_id, "action": { "kind": "open", "url": url } }),
                    }).await?;
                    Ok(response.data.unwrap_or(json!(null)))
                }
                BrowserCommand::Snapshot { browser_id: _, dom } => {
                    let action = if dom {
                        json!({ "kind": "eval", "script": crate::agent_browser::DOM_SNAPSHOT_SCRIPT })
                    } else {
                        json!({ "kind": "snapshot" })
                    };
                    let response = send_control_request(ControlRequest {
                        command: "browser_automation".into(),
                        params: json!({ "workspace_id": &ws_id, "action": action }),
                    }).await?;
                    Ok(response.data.unwrap_or(json!(null)))
                }
                BrowserCommand::Click { selector, browser_id: _ } => {
                    let response = send_control_request(ControlRequest {
                        command: "browser_automation".into(),
                        params: json!({ "workspace_id": &ws_id, "action": { "kind": "click", "selector": selector } }),
                    }).await?;
                    Ok(response.data.unwrap_or(json!(null)))
                }
                BrowserCommand::Fill { selector, value, browser_id: _ } => {
                    let response = send_control_request(ControlRequest {
                        command: "browser_automation".into(),
                        params: json!({ "workspace_id": &ws_id, "action": { "kind": "fill", "selector": selector, "value": value } }),
                    }).await?;
                    Ok(response.data.unwrap_or(json!(null)))
                }
                BrowserCommand::Screenshot { browser_id: _ } => {
                    let response = send_control_request(ControlRequest {
                        command: "browser_automation".into(),
                        params: json!({ "workspace_id": &ws_id, "action": { "kind": "screenshot" } }),
                    }).await?;
                    Ok(response.data.unwrap_or(json!(null)))
                }
                BrowserCommand::ConsoleLogs { browser_id: _ } => {
                    let response = send_control_request(ControlRequest {
                        command: "browser_automation".into(),
                        params: json!({ "workspace_id": &ws_id, "action": { "kind": "console" } }),
                    }).await?;
                    Ok(response.data.unwrap_or(json!(null)))
                }
                BrowserCommand::ClickAt { x, y, click_type, browser_id: _ } => {
                    let response = send_control_request(ControlRequest {
                        command: "browser_automation".into(),
                        params: json!({ "workspace_id": &ws_id, "action": { "kind": "click_at", "x": x, "y": y, "click_type": click_type } }),
                    }).await?;
                    Ok(response.data.unwrap_or(json!(null)))
                }
                BrowserCommand::TypeAt { text, x, y, browser_id: _ } => {
                    let mut action = json!({ "kind": "type_at", "text": text });
                    if let Some(xv) = x { action["x"] = json!(xv); }
                    if let Some(yv) = y { action["y"] = json!(yv); }
                    let response = send_control_request(ControlRequest {
                        command: "browser_automation".into(),
                        params: json!({ "workspace_id": &ws_id, "action": action }),
                    }).await?;
                    Ok(response.data.unwrap_or(json!(null)))
                }
                BrowserCommand::ScrollAt { x, y, direction, amount, browser_id: _ } => {
                    let response = send_control_request(ControlRequest {
                        command: "browser_automation".into(),
                        params: json!({ "workspace_id": &ws_id, "action": { "kind": "scroll_at", "x": x, "y": y, "direction": direction, "amount": amount } }),
                    }).await?;
                    Ok(response.data.unwrap_or(json!(null)))
                }
                BrowserCommand::KeyPress { key, browser_id: _ } => {
                    let response = send_control_request(ControlRequest {
                        command: "browser_automation".into(),
                        params: json!({ "workspace_id": &ws_id, "action": { "kind": "key_press", "key": key } }),
                    }).await?;
                    Ok(response.data.unwrap_or(json!(null)))
                }
                BrowserCommand::Drag { start_x, start_y, end_x, end_y, browser_id: _ } => {
                    let response = send_control_request(ControlRequest {
                        command: "browser_automation".into(),
                        params: json!({ "workspace_id": &ws_id, "action": { "kind": "drag", "start_x": start_x, "start_y": start_y, "end_x": end_x, "end_y": end_y } }),
                    }).await?;
                    Ok(response.data.unwrap_or(json!(null)))
                }
                BrowserCommand::ClickOs { x, y, browser_id: _ } => {
                    let response = send_control_request(ControlRequest {
                        command: "browser_automation".into(),
                        params: json!({ "workspace_id": &ws_id, "action": { "kind": "click_os", "x": x, "y": y } }),
                    }).await?;
                    Ok(response.data.unwrap_or(json!(null)))
                }
                BrowserCommand::TypeOs { text, x, y, browser_id: _ } => {
                    let mut action = json!({ "kind": "type_os", "text": text });
                    if let Some(xv) = x { action["x"] = json!(xv); }
                    if let Some(yv) = y { action["y"] = json!(yv); }
                    let response = send_control_request(ControlRequest {
                        command: "browser_automation".into(),
                        params: json!({ "workspace_id": &ws_id, "action": action }),
                    }).await?;
                    Ok(response.data.unwrap_or(json!(null)))
                }
            }?;
            println!("{}", serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?);
            Ok(true)
        }
        Some(CommandSet::Workspace { command }) => {
            let response = match command {
                WorkspaceCommand::RerunSetup { workspace_id } => {
                    let mut params = json!({});
                    if let Some(id) = workspace_id {
                        params["workspace_id"] = json!(id);
                    } else if let Ok(id) = std::env::var("CODEMUX_WORKSPACE_ID") {
                        params["workspace_id"] = json!(id);
                    }
                    send_control_request(ControlRequest {
                        command: "rerun_setup".into(),
                        params,
                    })
                    .await?
                }
            };
            println!("{}", serde_json::to_string_pretty(&response).map_err(|error| error.to_string())?);
            Ok(true)
        }
        Some(CommandSet::Mcp) => {
            crate::mcp_server::run_mcp_server().await?;
            Ok(true)
        }
        Some(CommandSet::Capabilities) => {
            let caps = json!({
                "version": env!("CARGO_PKG_VERSION"),
                "commands": {
                    "browser": {
                        "description": "Control the browser pane",
                        "subcommands": {
                            "open": { "args": "<url>", "description": "Navigate to a URL" },
                            "snapshot": { "args": "[--dom]", "description": "Get page element tree" },
                            "click": { "args": "<selector>", "description": "Click an element" },
                            "fill": { "args": "<selector> <value>", "description": "Type into an input" },
                            "screenshot": { "description": "Capture screenshot (base64 PNG)" },
                            "console-logs": { "description": "Get browser console output" },
                            "create": { "description": "Create a new browser pane" }
                        }
                    },
                    "memory": {
                        "description": "Project memory management",
                        "subcommands": {
                            "show": { "description": "Show project memory" },
                            "set": { "args": "--brief/--goal/--focus", "description": "Update memory fields" },
                            "add": { "args": "<kind> <content>", "description": "Add memory entry" }
                        }
                    },
                    "index": {
                        "description": "Code index for search",
                        "subcommands": {
                            "build": { "description": "Build/rebuild search index" },
                            "status": { "description": "Show index status" },
                            "search": { "args": "<query>", "description": "Search indexed code" }
                        }
                    },
                    "issue": {
                        "description": "GitHub issue operations",
                        "subcommands": {
                            "list": { "args": "[--search <query>]", "description": "List open issues for the current workspace's repo" },
                            "view": { "args": "<number>", "description": "View a specific GitHub issue" },
                            "link": { "args": "<number>", "description": "Link a GitHub issue to the active workspace" }
                        }
                    },
                    "workspace": {
                        "description": "Workspace operations",
                        "subcommands": {
                            "rerun-setup": { "args": "[workspace-id]", "description": "Re-run setup (.codemuxinclude + scripts) for a workspace" }
                        }
                    },
                    "status": { "description": "Show Codemux app status" },
                    "notify": { "args": "<message>", "description": "Send a notification to the user" },
                    "handoff": { "description": "Generate project handoff summary" },
                    "capabilities": { "description": "List all available commands (this output)" }
                },
                "environment": {
                    "CODEMUX": "Set to '1' when running inside Codemux",
                    "CODEMUX_VERSION": "Codemux version",
                    "CODEMUX_WORKSPACE_ID": "Current workspace ID",
                    "CODEMUX_BROWSER_CMD": "Command prefix for browser control",
                    "BROWSER": "Set to 'codemux browser open' for URL handling"
                }
            });
            println!("{}", serde_json::to_string_pretty(&caps).map_err(|e| e.to_string())?);
            Ok(true)
        }
    }
}

fn normalize_memory_kind(kind: &str) -> &'static str {
    match kind {
        "pinned" | "pinned_context" => "pinned_context",
        "decision" | "decisions" => "decision",
        "next" | "next_step" | "next_steps" => "next_step",
        "session" | "session_summary" => "session_summary",
        _ => "pinned_context",
    }
}

