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
    /// List all available codemux commands and capabilities
    Capabilities,
    /// Start MCP server (JSON-RPC over stdio)
    Mcp,
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
        Some(CommandSet::Browser { command }) => {
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
                        params: json!({ "browser_id": "default", "action": { "kind": "open", "url": url } }),
                    }).await?;
                    Ok(response.data.unwrap_or(json!(null)))
                }
                BrowserCommand::Snapshot { browser_id, dom } => {
                    let bid = browser_id.as_deref().unwrap_or("default");
                    let action = if dom {
                        json!({ "kind": "eval", "script": crate::agent_browser::DOM_SNAPSHOT_SCRIPT })
                    } else {
                        json!({ "kind": "snapshot" })
                    };
                    let response = send_control_request(ControlRequest {
                        command: "browser_automation".into(),
                        params: json!({ "browser_id": bid, "action": action }),
                    }).await?;
                    Ok(response.data.unwrap_or(json!(null)))
                }
                BrowserCommand::Click { selector, browser_id } => {
                    let bid = browser_id.as_deref().unwrap_or("default");
                    let response = send_control_request(ControlRequest {
                        command: "browser_automation".into(),
                        params: json!({ "browser_id": bid, "action": { "kind": "click", "selector": selector } }),
                    }).await?;
                    Ok(response.data.unwrap_or(json!(null)))
                }
                BrowserCommand::Fill { selector, value, browser_id } => {
                    let bid = browser_id.as_deref().unwrap_or("default");
                    let response = send_control_request(ControlRequest {
                        command: "browser_automation".into(),
                        params: json!({ "browser_id": bid, "action": { "kind": "fill", "selector": selector, "value": value } }),
                    }).await?;
                    Ok(response.data.unwrap_or(json!(null)))
                }
                BrowserCommand::Screenshot { browser_id } => {
                    let bid = browser_id.as_deref().unwrap_or("default");
                    let response = send_control_request(ControlRequest {
                        command: "browser_automation".into(),
                        params: json!({ "browser_id": bid, "action": { "kind": "screenshot" } }),
                    }).await?;
                    Ok(response.data.unwrap_or(json!(null)))
                }
                BrowserCommand::ConsoleLogs { browser_id } => {
                    let bid = browser_id.as_deref().unwrap_or("default");
                    let response = send_control_request(ControlRequest {
                        command: "browser_automation".into(),
                        params: json!({ "browser_id": bid, "action": { "kind": "console" } }),
                    }).await?;
                    Ok(response.data.unwrap_or(json!(null)))
                }
            }?;
            println!("{}", serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?);
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

