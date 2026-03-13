use crate::control::{control_socket_path, ControlRequest, ControlResponse};
use clap::{Parser, Subcommand};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

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
}

#[derive(Subcommand)]
pub enum BrowserCommand {
    Create,
    Open { url: String },
    Snapshot { browser_id: Option<String> },
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
            let response = send_request(ControlRequest {
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
            let response = send_request(ControlRequest { command, params }).await?;
            println!("{}", serde_json::to_string_pretty(&response).map_err(|error| error.to_string())?);
            Ok(true)
        }
        Some(CommandSet::Notify { message }) => {
            let response = send_request(ControlRequest {
                command: "notify".into(),
                params: json!({ "message": message }),
            })
            .await?;
            println!("{}", serde_json::to_string_pretty(&response).map_err(|error| error.to_string())?);
            Ok(true)
        }
        Some(CommandSet::Handoff) => {
            let response = send_request(ControlRequest {
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
                    send_request(ControlRequest {
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
                    send_request(ControlRequest {
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
                    send_request(ControlRequest {
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
                    send_request(ControlRequest {
                        command: "rebuild_index".into(),
                        params: json!({}),
                    })
                    .await?
                }
                IndexCommand::Status => {
                    send_request(ControlRequest {
                        command: "index_status".into(),
                        params: json!({}),
                    })
                    .await?
                }
                IndexCommand::Search { query, limit } => {
                    send_request(ControlRequest {
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
                    let response = send_request(ControlRequest {
                        command: "create_browser_pane".to_string(),
                        params: json!({"pane_id": ""}),
                    }).await?;
                    Ok(json!({ "ok": true, "browser_id": response.data.unwrap_or(json!(null)) }))
                }
                BrowserCommand::Open { url } => {
                    run_agent_browser(&["open", &format!("'{}'", url), "--session", "default"])
                }
                BrowserCommand::Snapshot { browser_id: _ } => {
                    run_agent_browser(&["snapshot", "-i", "--session", "default"])
                }
                BrowserCommand::Click { selector, browser_id: _ } => {
                    run_agent_browser(&["click", &format!("'{}'", selector), "--session", "default"])
                }
                BrowserCommand::Fill { selector, value, browser_id: _ } => {
                    run_agent_browser(&["fill", &format!("'{}'", selector), &format!("'{}'", value), "--session", "default"])
                }
                BrowserCommand::Screenshot { browser_id: _ } => {
                    run_agent_browser(&["screenshot", "--session", "default"])
                }
                BrowserCommand::ConsoleLogs { browser_id: _ } => {
                    run_agent_browser(&["console", "--session", "default"])
                }
            }?;
            println!("{}", serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?);
            Ok(true)
        }
    }
}

fn run_agent_browser(args: &[&str]) -> Result<serde_json::Value, String> {
    let shell_cmd = format!("npx agent-browser {}", args.join(" "));
    let output = std::process::Command::new("sh")
        .args(["-c", &shell_cmd])
        .output()
        .map_err(|e| format!("Failed to run agent-browser: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() && !stdout.contains("✓") && !stdout.contains("{") {
        return Err(format!("agent-browser failed: {} {}", stdout, stderr));
    }

    if stdout.contains("{") {
        serde_json::from_str(&stdout).map_err(|e| format!("Failed to parse JSON: {}", e))
    } else {
        Ok(serde_json::json!({ "output": stdout, "success": output.status.success() }))
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

async fn send_request(request: ControlRequest) -> Result<ControlResponse, String> {
    let socket_path = control_socket_path()
        .ok_or_else(|| "Control socket path unavailable".to_string())?;
    let stream = UnixStream::connect(socket_path)
        .await
        .map_err(|error| format!("Failed to connect to Codemux control socket: {error}"))?;
    let (reader, mut writer) = stream.into_split();

    let payload = serde_json::to_string(&request).map_err(|error| error.to_string())?;
    writer
        .write_all(format!("{payload}\n").as_bytes())
        .await
        .map_err(|error| format!("Failed to send CLI request: {error}"))?;

    let mut lines = BufReader::new(reader).lines();
    let response = lines
        .next_line()
        .await
        .map_err(|error| format!("Failed to read CLI response: {error}"))?
        .ok_or_else(|| "No response received from Codemux".to_string())?;

    serde_json::from_str(&response).map_err(|error| format!("Invalid CLI response JSON: {error}"))
}
