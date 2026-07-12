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
    /// Web remote-access operations
    Remote {
        #[command(subcommand)]
        command: RemoteCommand,
    },
    /// Print recent lines from the desktop app's log file
    Logs {
        /// Number of lines from the end of the log to print
        #[arg(long, default_value_t = 200)]
        tail: usize,
    },
    /// Check the local environment for common problems (file dialogs,
    /// logs). Works without a running Codemux instance.
    Doctor,
    /// List all available codemux commands and capabilities
    Capabilities,
    /// Start MCP server (JSON-RPC over stdio)
    Mcp,
    /// Run as the persistent PTY daemon (internal subcommand spawned by the
    /// Tauri app; long-lived process that owns agent PTYs so they survive
    /// the app being closed).
    PtyDaemon {
        /// Absolute path of the Unix socket to bind. The Tauri app passes
        /// this when spawning the daemon.
        #[arg(long)]
        socket: std::path::PathBuf,
    },
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
pub enum RemoteCommand {
    /// Mint a one-time pairing code for the web remote server and print a
    /// scannable link + terminal QR. Requires remote access to be enabled in
    /// Settings first. Handy over SSH: pair a phone/laptop without opening
    /// the desktop GUI.
    Pair {
        /// Optional label for the paired device. Used as a fallback name if
        /// the connecting browser doesn't provide one of its own.
        #[arg(long)]
        name: Option<String>,
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
    /// Set the browser viewport to a preset (mobile / tablet / desktop / ...),
    /// a custom WxH like `390x844`, or `reset` to return to the default.
    /// Use `codemux browser viewport-presets` to list every preset.
    Viewport {
        /// Preset name, `WxH` dimensions, or `reset`.
        spec: String,
        /// Override device-pixel-ratio (e.g. `2` for retina). Defaults to
        /// the preset's natural DPR (3.0 for phones, 2.0 for tablets,
        /// 1.0 for desktop / custom dimensions).
        #[arg(long)]
        dpr: Option<f64>,
        browser_id: Option<String>,
    },
    /// List the available viewport presets and their CSS dimensions.
    ViewportPresets,
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
            // Best-effort cwd so the control layer can resolve the owning
            // workspace by path when `CODEMUX_WORKSPACE_ID` is absent — e.g.
            // Bash subprocesses of the agent-chat sidecar (whose env we don't
            // inject) or any other env-less caller. Empty string on error;
            // the handler treats empty `cwd` as "no hint" and falls back to
            // today's behaviour. See `resolve_workspace_id_by_cwd` in
            // control.rs.
            let cwd = std::env::current_dir()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();

            // Build the shared params object for every `browser_automation`
            // request: the env-injected `workspace_id` (may be empty), the
            // `cwd` fallback hint, and the per-command `action`. Factored so
            // the two routing params are added in exactly one place instead
            // of being hand-copied across every command below.
            let make_params = |action: Value| {
                json!({ "workspace_id": &ws_id, "cwd": &cwd, "action": action })
            };

            // Helper: surface the handler's error message instead of silently
            // returning `null` when `response.ok == false`. Without this, every
            // failed browser command in the running app showed up to the user
            // (and any agent reading stdout) as the literal string "null" with
            // no diagnostic — which is exactly what the Windows debug session
            // surfaced when the control-pipe layer started working.
            fn unwrap_response(response: crate::control::ControlResponse) -> Result<Value, String> {
                if !response.ok {
                    return Err(response
                        .error
                        .unwrap_or_else(|| "Unknown error from control endpoint".to_string()));
                }
                Ok(response.data.unwrap_or(json!(null)))
            }

            let result = match command {
                BrowserCommand::Create => {
                    let response = send_control_request(ControlRequest {
                        command: "create_browser_pane".to_string(),
                        params: json!({"pane_id": ""}),
                    }).await?;
                    if !response.ok {
                        return Err(response
                            .error
                            .unwrap_or_else(|| "Unknown error from control endpoint".to_string()));
                    }
                    Ok::<_, String>(json!({ "ok": true, "data": response.data.unwrap_or(json!(null)) }))
                }
                BrowserCommand::Open { url } => {
                    let response = send_control_request(ControlRequest {
                        command: "browser_automation".into(),
                        params: make_params(json!({ "kind": "open", "url": url })),
                    }).await?;
                    unwrap_response(response)
                }
                BrowserCommand::Snapshot { browser_id: _, dom } => {
                    let action = if dom {
                        json!({ "kind": "eval", "script": crate::agent_browser::DOM_SNAPSHOT_SCRIPT })
                    } else {
                        json!({ "kind": "snapshot" })
                    };
                    let response = send_control_request(ControlRequest {
                        command: "browser_automation".into(),
                        params: make_params(action),
                    }).await?;
                    unwrap_response(response)
                }
                BrowserCommand::Click { selector, browser_id: _ } => {
                    let response = send_control_request(ControlRequest {
                        command: "browser_automation".into(),
                        params: make_params(json!({ "kind": "click", "selector": selector })),
                    }).await?;
                    unwrap_response(response)
                }
                BrowserCommand::Fill { selector, value, browser_id: _ } => {
                    let response = send_control_request(ControlRequest {
                        command: "browser_automation".into(),
                        params: make_params(json!({ "kind": "fill", "selector": selector, "value": value })),
                    }).await?;
                    unwrap_response(response)
                }
                BrowserCommand::Screenshot { browser_id: _ } => {
                    let response = send_control_request(ControlRequest {
                        command: "browser_automation".into(),
                        params: make_params(json!({ "kind": "screenshot" })),
                    }).await?;
                    unwrap_response(response)
                }
                BrowserCommand::ConsoleLogs { browser_id: _ } => {
                    let response = send_control_request(ControlRequest {
                        command: "browser_automation".into(),
                        params: make_params(json!({ "kind": "console" })),
                    }).await?;
                    unwrap_response(response)
                }
                BrowserCommand::ClickAt { x, y, click_type, browser_id: _ } => {
                    let response = send_control_request(ControlRequest {
                        command: "browser_automation".into(),
                        params: make_params(json!({ "kind": "click_at", "x": x, "y": y, "click_type": click_type })),
                    }).await?;
                    unwrap_response(response)
                }
                BrowserCommand::TypeAt { text, x, y, browser_id: _ } => {
                    let mut action = json!({ "kind": "type_at", "text": text });
                    if let Some(xv) = x { action["x"] = json!(xv); }
                    if let Some(yv) = y { action["y"] = json!(yv); }
                    let response = send_control_request(ControlRequest {
                        command: "browser_automation".into(),
                        params: make_params(action),
                    }).await?;
                    unwrap_response(response)
                }
                BrowserCommand::ScrollAt { x, y, direction, amount, browser_id: _ } => {
                    let response = send_control_request(ControlRequest {
                        command: "browser_automation".into(),
                        params: make_params(json!({ "kind": "scroll_at", "x": x, "y": y, "direction": direction, "amount": amount })),
                    }).await?;
                    unwrap_response(response)
                }
                BrowserCommand::KeyPress { key, browser_id: _ } => {
                    let response = send_control_request(ControlRequest {
                        command: "browser_automation".into(),
                        params: make_params(json!({ "kind": "key_press", "key": key })),
                    }).await?;
                    unwrap_response(response)
                }
                BrowserCommand::Drag { start_x, start_y, end_x, end_y, browser_id: _ } => {
                    let response = send_control_request(ControlRequest {
                        command: "browser_automation".into(),
                        params: make_params(json!({ "kind": "drag", "start_x": start_x, "start_y": start_y, "end_x": end_x, "end_y": end_y })),
                    }).await?;
                    unwrap_response(response)
                }
                BrowserCommand::ClickOs { x, y, browser_id: _ } => {
                    let response = send_control_request(ControlRequest {
                        command: "browser_automation".into(),
                        params: make_params(json!({ "kind": "click_os", "x": x, "y": y })),
                    }).await?;
                    unwrap_response(response)
                }
                BrowserCommand::TypeOs { text, x, y, browser_id: _ } => {
                    let mut action = json!({ "kind": "type_os", "text": text });
                    if let Some(xv) = x { action["x"] = json!(xv); }
                    if let Some(yv) = y { action["y"] = json!(yv); }
                    let response = send_control_request(ControlRequest {
                        command: "browser_automation".into(),
                        params: make_params(action),
                    }).await?;
                    unwrap_response(response)
                }
                BrowserCommand::Viewport { spec, dpr, browser_id: _ } => {
                    // Resolve preset / WxH locally first so we can surface
                    // a friendly error (with the full preset list) instead
                    // of letting the socket call return a generic "Unknown
                    // action" message.
                    let resolved = crate::browser_viewport::parse_spec(&spec, dpr)
                        .map_err(|e| e.to_string())?;
                    // Shared socket-action builder — keeps CLI and MCP
                    // payloads byte-identical so a future field bump
                    // can't ship to one surface and not the other.
                    let action = crate::browser_viewport::socket_action(resolved);
                    let response = send_control_request(ControlRequest {
                        command: "browser_automation".into(),
                        params: make_params(action),
                    }).await?;
                    let data = unwrap_response(response)?;
                    // Echo back what we applied — useful for agents
                    // chaining `viewport` → `screenshot`.
                    Ok::<_, String>(json!({
                        "applied": {
                            "width": resolved.width,
                            "height": resolved.height,
                            "dpr": resolved.dpr,
                            "spec": spec,
                        },
                        "result": data,
                    }))
                }
                BrowserCommand::ViewportPresets => {
                    let presets = crate::browser_viewport::list_presets();
                    let json_presets: Vec<_> = presets
                        .iter()
                        .map(|p| json!({
                            "name": p.name,
                            "width": p.spec.width,
                            "height": p.spec.height,
                            "dpr": p.spec.dpr,
                            "description": p.description,
                        }))
                        .collect();
                    Ok::<_, String>(json!({
                        "presets": json_presets,
                        "reset": {
                            "width": crate::browser_viewport::RESET_SPEC.width,
                            "height": crate::browser_viewport::RESET_SPEC.height,
                            "dpr": crate::browser_viewport::RESET_SPEC.dpr,
                        },
                        "custom": "Use `WxH` like `390x844` for a custom viewport, plus `--dpr N` for retina.",
                    }))
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
        Some(CommandSet::Remote { command }) => {
            match command {
                RemoteCommand::Pair { name } => {
                    let mut params = json!({});
                    if let Some(ref label) = name {
                        params["name"] = json!(label);
                    }
                    let response = send_control_request(ControlRequest {
                        command: "web_remote_pair".into(),
                        params,
                    })
                    .await?;
                    if !response.ok {
                        // Surface the handler's message (e.g. "Remote access is
                        // not enabled — enable it in Settings first") instead of
                        // a bare `null`.
                        return Err(response
                            .error
                            .unwrap_or_else(|| "Unknown error from control endpoint".to_string()));
                    }
                    print_pairing(&response.data.unwrap_or(json!(null)));
                }
            }
            Ok(true)
        }
        Some(CommandSet::Logs { tail }) => {
            // Purely local — reads the file tauri-plugin-log writes, so
            // it works even when (especially when) the app won't start.
            match crate::app_logs::app_log_file() {
                Some(path) if path.exists() => {
                    eprintln!("# {}", path.display());
                    for line in
                        crate::app_logs::tail_lines(&path, tail).map_err(|e| e.to_string())?
                    {
                        println!("{line}");
                    }
                }
                Some(path) => {
                    println!(
                        "No log file at {} yet. It is created the first time the desktop app runs.",
                        path.display()
                    );
                }
                None => println!("Could not resolve the platform log directory."),
            }
            Ok(true)
        }
        Some(CommandSet::Doctor) => {
            crate::doctor::run().await;
            Ok(true)
        }
        Some(CommandSet::Mcp) => {
            crate::mcp_server::run_mcp_server().await?;
            Ok(true)
        }
        Some(CommandSet::PtyDaemon { socket }) => {
            // The daemon's `run` only returns on a fatal listener error;
            // it never returns Ok. Translate into a CLI error string so the
            // outer harness logs it and the process exits non-zero.
            //
            // Windows: not yet implemented; print a clear message rather
            // than a link error. The Tauri side never spawns this on
            // Windows because `daemon_path_viable()` is false there.
            #[cfg(unix)]
            {
                crate::pty_daemon::server::run(socket).await?;
                Ok(true)
            }
            #[cfg(not(unix))]
            {
                let _ = socket;
                Err("codemux pty-daemon is Unix-only for now".to_string())
            }
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
                            "create": { "description": "Create a new browser pane" },
                            "viewport": {
                                "args": "<preset|WxH|reset> [--dpr N]",
                                "description": "Set viewport for mobile/tablet/desktop testing"
                            },
                            "viewport-presets": { "description": "List available viewport presets" }
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
                    "remote": {
                        "description": "Web remote-access operations",
                        "subcommands": {
                            "pair": { "args": "[--name <label>]", "description": "Mint a one-time web-remote pairing code + QR (requires remote access enabled)" }
                        }
                    },
                    "status": { "description": "Show Codemux app status" },
                    "notify": { "args": "<message>", "description": "Send a notification to the user" },
                    "handoff": { "description": "Generate project handoff summary" },
                    "logs": { "args": "[--tail <n>]", "description": "Print recent lines from the desktop app's log file" },
                    "doctor": { "description": "Diagnose the local environment (file dialogs, logs)" },
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

/// Pretty-print the `web_remote_pair` result for a human at a terminal: a
/// scannable QR of the pairing URL, then the link, token, endpoint, and
/// expiry. The QR encodes the same `http://host:port/#pair=<token>` URL the
/// link shows, so a phone camera can pair without any typing.
fn print_pairing(data: &Value) {
    let url = data["pairing_url"].as_str().unwrap_or_default();
    let token = data["token"].as_str().unwrap_or_default();
    let expires_at = data["expires_at"].as_str().unwrap_or_default();
    let host = data["host"].as_str().unwrap_or_default();
    let kind = data["endpoint_kind"].as_str().unwrap_or_default();
    let secure = data["secure"].as_bool().unwrap_or(false);

    println!();
    match render_qr(url) {
        Some(qr) => println!("{qr}"),
        None => println!("(QR unavailable — use the link below)\n"),
    }
    println!("Pairing link:  {url}");
    println!("Token:         {token}");
    if !host.is_empty() {
        let scope = if secure {
            "secure context"
        } else {
            "plain HTTP — not a browser secure context"
        };
        println!("Endpoint:      {host} ({kind}, {scope})");
    }
    if !expires_at.is_empty() {
        println!("Expires:       {expires_at}");
    }
    println!();
    println!("Scan the QR with your phone — or open the link in any browser on a");
    println!("device that can reach this machine — to pair it. One-time use.");
}

/// Render `text` to a compact terminal QR using the pure-Rust `qrcode`
/// crate's built-in unicode renderer (two rows of modules per text line).
/// Returns `None` if the text is too large to encode, in which case the
/// caller falls back to printing just the link.
fn render_qr(text: &str) -> Option<String> {
    use qrcode::render::unicode;
    use qrcode::{EcLevel, QrCode};

    // Medium EC keeps the code compact while tolerating a smudged terminal /
    // camera glare; the pairing URL is short so this always fits.
    let code = QrCode::with_error_correction_level(text, EcLevel::M).ok()?;
    Some(
        code.render::<unicode::Dense1x2>()
            .quiet_zone(true)
            .build(),
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_qr_encodes_the_pairing_url() {
        // The pairing URL is short, so it always encodes; the unicode
        // renderer yields a non-trivial block of half-height module rows.
        let url = "http://100.101.102.103:4377/#pair=deadbeefdeadbeefdeadbeefdeadbeef";
        let qr = render_qr(url).expect("pairing URL encodes to a QR");
        assert!(qr.lines().count() > 8, "renders multiple module rows");
        assert!(
            qr.chars().any(|c| c == '█' || c == '▀' || c == '▄' || c == ' '),
            "uses the unicode Dense1x2 block glyphs"
        );
    }

    #[test]
    fn print_pairing_handles_a_full_control_result_without_panicking() {
        // Mirrors the `ControlPairing` JSON the `web_remote_pair` control
        // command returns, so the CLI's formatter is exercised end-to-end.
        let data = json!({
            "pairing_url": "http://100.101.102.103:4377/#pair=abc123",
            "host": "100.101.102.103",
            "port": 4377,
            "token": "abc123",
            "expires_at": "2026-07-12T10:00:00Z",
            "secure": false,
            "endpoint_kind": "tailnet",
        });
        print_pairing(&data);
    }
}

