use crate::control::{send_control_request, ControlRequest};
use clap::{Parser, Subcommand};
use serde_json::{json, Value};

pub mod ports;

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
    /// Tell Codemux this pane is watching something in the background — a CI
    /// run, a tailed process, a PR poll — so its workspace shows the calm
    /// "Monitoring" status instead of looking finished or stuck at "Working".
    ///
    /// Provider-agnostic: any agent with a shell can call this, whatever it is
    /// running under. The pane is taken from `CODEMUX_PANE_ID` /
    /// `CODEMUX_WORKSPACE_ID`, which Codemux injects into terminal panes and
    /// chat-session subprocesses, so the agent flags its OWN pane with no
    /// arguments. Remember to `codemux monitor stop` when the watch ends.
    Monitor {
        #[command(subcommand)]
        command: MonitorCommand,
    },
    /// Reserve host ports that no other worktree will be handed, so several
    /// worktrees of one project can each bring up their own stack instead of
    /// fighting over 4200/8000/5432.
    ///
    /// `codemux ports allocate <name>` prints a free port and remembers it:
    /// the same worktree asking for the same name always gets the same
    /// number, so an ephemeral compose file written against it keeps working
    /// across restarts. Works with no project setup and changes nothing in
    /// the repository — reach for it instead of asking to stop someone
    /// else's stack.
    Ports {
        #[command(subcommand)]
        command: ports::PortsCommand,
    },
    /// Run Codemux headless as a web-remote server — no desktop GUI. Boots the
    /// full backend, binds the web-remote server, and prints a scannable
    /// pairing QR + link. Ideal over SSH: expose your desktop's UI to a phone
    /// or laptop without a display attached. Long-lived: runs until Ctrl-C /
    /// SIGTERM. Refuses to start if a GUI or another `serve` already holds the
    /// control endpoint on this machine.
    Serve {
        /// Which interfaces to expose the server on: `all` (every interface),
        /// `tailscale` (tailnet + loopback only), or `loopback` (this machine
        /// only). Defaults to `all` on first run, or the persisted scope if
        /// remote access was already configured.
        #[arg(long, value_parser = ["all", "tailscale", "loopback"])]
        scope: Option<String>,
        /// Port to bind. Defaults to the persisted port (4377).
        #[arg(long)]
        port: Option<u16>,
        /// Also enable the from-anywhere iroh relay transport (end-to-end
        /// encrypted), so a device off your LAN/tailnet can still reach this
        /// instance.
        #[arg(long)]
        relay: bool,
    },
    /// Set this machine up for remote access in one command: sign in, turn on
    /// from-anywhere (relay) access, and install a background service so
    /// Codemux stays reachable after you log out. Safe to re-run — it reports
    /// the steps that were already done instead of undoing them.
    ///
    /// With a Codemux instance already running here it drives that instance
    /// over the control socket instead of installing a service (two backends
    /// on one machine would fight over the port and the database).
    Connect {
        #[command(subcommand)]
        command: Option<ConnectCommand>,
        /// Account email, if a sign-in is needed. Omit to be prompted.
        #[arg(long)]
        email: Option<String>,
        /// Which interfaces the local server binds: `all`, `tailscale`, or
        /// `loopback`. Leave unset to keep the current scope (`all` on a
        /// first run). From-anywhere access works regardless of this.
        #[arg(long, value_parser = ["all", "tailscale", "loopback"])]
        scope: Option<String>,
        /// Port to bind. Leave unset to keep the current port (default 4377).
        #[arg(long)]
        port: Option<u16>,
    },
    /// Sign this machine into a Codemux account. Works from a bare shell —
    /// no GUI, no display, and no running Codemux instance required — so a
    /// VPS reached over SSH can be signed in before `codemux serve --relay`
    /// registers it as a device. Prompts for the email and password
    /// (password input is never echoed) and stores the same encrypted
    /// session the desktop app writes.
    Login {
        /// Account email. Omit to be prompted for it.
        #[arg(long)]
        email: Option<String>,
        /// Sign in with an already-issued session token instead of a
        /// password. The escape hatch for an account that has no password
        /// (signed up through GitHub): sign in on a machine with a browser,
        /// copy the session token, and paste it here. The token is verified
        /// against the Codemux API before anything is stored.
        #[arg(long, conflicts_with = "email")]
        token: Option<String>,
        /// Print the current session instead of signing in (same output as
        /// `codemux whoami`).
        #[arg(long)]
        status: bool,
    },
    /// Clear the cached account session on this machine.
    Logout,
    /// Print the account this machine is signed in as. Exits 1 when there
    /// is no live session.
    Whoami,
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
pub enum ConnectCommand {
    /// Show what remote access is currently set up on this machine: the
    /// signed-in account, the persisted configuration, the background
    /// service, and whether Codemux is running here.
    Status,
    /// Turn from-anywhere access off and remove the background service. The
    /// account session is left alone — run `codemux logout` to clear that.
    #[command(alias = "disconnect")]
    Off,
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
    /// Turn web remote access on (binds the server), optionally selecting which
    /// interfaces to expose it on and the port. Prints the resulting status and
    /// the recommended endpoint so you can immediately run `codemux remote pair`.
    /// Requires the desktop app to be running.
    Enable {
        /// Which interfaces to expose the server on: `all` (every interface),
        /// `tailscale` (tailnet + loopback only), or `loopback` (this machine
        /// only). Leave unset to keep the current scope.
        #[arg(long, value_parser = ["all", "tailscale", "loopback"])]
        scope: Option<String>,
        /// Port to bind. Leave unset to keep the current port (default 4377).
        #[arg(long)]
        port: Option<u16>,
    },
    /// Turn web remote access off — unbinds the server and severs every live
    /// connection immediately. Requires the desktop app to be running.
    Disable,
}

#[derive(Subcommand)]
pub enum MonitorCommand {
    /// Mark this pane as monitoring something in the background.
    Start {
        /// Short human description of what is being watched — shown next to
        /// the status ("Monitoring in the background — CI on #482").
        #[arg(long)]
        reason: Option<String>,
        /// Target a specific pane instead of `CODEMUX_PANE_ID`.
        #[arg(long)]
        pane_id: Option<String>,
    },
    /// Clear this pane's monitoring status.
    Stop {
        /// Target a specific pane instead of `CODEMUX_PANE_ID`.
        #[arg(long)]
        pane_id: Option<String>,
    },
    /// Report whether this pane currently reads as monitoring, and why.
    Status {
        /// Target a specific pane instead of `CODEMUX_PANE_ID`.
        #[arg(long)]
        pane_id: Option<String>,
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

/// What `main` should do after parsing the CLI. `serve` is special: it is a
/// long-lived foreground process that must run on the main thread, NOT a
/// control-socket round-trip, so it is signalled back to `main` instead of
/// being driven inside `maybe_run_cli`'s async body.
pub enum CliOutcome {
    /// The CLI fully handled the command; the process should exit cleanly.
    Handled,
    /// No subcommand (or `app`): fall through to launching the desktop GUI.
    LaunchGui,
    /// `codemux serve [...]`: run the headless web-remote server on the main
    /// thread (see `codemux_lib::run_serve`).
    RunServe(crate::web_remote::serve::ServeOptions),
}

pub async fn maybe_run_cli() -> Result<CliOutcome, String> {
    let cli = Cli::parse();
    // `serve` never touches the control socket — hand it back to `main` so it
    // can run the long-lived server on the main thread outside `block_on`.
    if let Some(CommandSet::Serve { scope, port, relay }) = &cli.command {
        return Ok(CliOutcome::RunServe(crate::web_remote::serve::ServeOptions {
            scope: scope.clone(),
            port: *port,
            relay: *relay,
        }));
    }
    match run_control_cli(cli).await? {
        true => Ok(CliOutcome::Handled),
        false => Ok(CliOutcome::LaunchGui),
    }
}

/// Drive every control-socket / local CLI subcommand. Returns `Ok(true)` when
/// the command was handled (process should exit) and `Ok(false)` when there is
/// no subcommand and the GUI should launch. `serve` is intercepted by
/// [`maybe_run_cli`] before this runs.
async fn run_control_cli(cli: Cli) -> Result<bool, String> {
    match cli.command {
        None | Some(CommandSet::App) => Ok(false),
        // Intercepted in `maybe_run_cli`; never reached here.
        Some(CommandSet::Serve { .. }) => {
            unreachable!("serve is handled by maybe_run_cli before run_control_cli")
        }
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
                    // action" message. The `_configured` variant makes
                    // `reset` land on the user's `browser.default_viewport`
                    // setting (read from the settings cache — the CLI is
                    // the same binary) instead of the hard-coded baseline.
                    let resolved = crate::browser_viewport::parse_spec_configured(&spec, dpr)
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
                    // `reset` reports the *actual* reset target — the
                    // user's configured default viewport when set.
                    let reset_spec = crate::browser_viewport::configured_default_spec();
                    Ok::<_, String>(json!({
                        "presets": json_presets,
                        "reset": {
                            "width": reset_spec.width,
                            "height": reset_spec.height,
                            "dpr": reset_spec.dpr,
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
        Some(CommandSet::Monitor { command }) => {
            // The pane/workspace ids come from the agent's own injected
            // environment, so the common call is a bare `codemux monitor
            // start` from inside the pane that is doing the watching. An
            // explicit `--pane-id` overrides; with neither, the backend falls
            // back to the active workspace's active pane.
            let target = |pane_id: Option<String>| {
                let mut params = json!({});
                let pane = pane_id.or_else(|| std::env::var("CODEMUX_PANE_ID").ok());
                if let Some(id) = pane.filter(|id| !id.is_empty()) {
                    params["pane_id"] = json!(id);
                }
                if let Ok(id) = std::env::var("CODEMUX_WORKSPACE_ID") {
                    if !id.is_empty() {
                        params["workspace_id"] = json!(id);
                    }
                }
                params
            };
            let (command, params) = match command {
                MonitorCommand::Start { reason, pane_id } => {
                    let mut params = target(pane_id);
                    if let Some(reason) = reason.filter(|r| !r.trim().is_empty()) {
                        params["reason"] = json!(reason);
                    }
                    ("monitor_start", params)
                }
                MonitorCommand::Stop { pane_id } => ("monitor_stop", target(pane_id)),
                MonitorCommand::Status { pane_id } => ("monitor_status", target(pane_id)),
            };
            let response = send_control_request(ControlRequest {
                command: command.into(),
                params,
            })
            .await?;
            println!("{}", serde_json::to_string_pretty(&response).map_err(|error| error.to_string())?);
            Ok(true)
        }
        // Purely local: the allocation table is a file on disk, so this works
        // from a bare shell with no Codemux instance running.
        Some(CommandSet::Ports { command }) => {
            ports::run(command)?;
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
                RemoteCommand::Enable { scope, port } => {
                    let mut params = json!({});
                    if let Some(ref s) = scope {
                        params["scope"] = json!(s);
                    }
                    if let Some(p) = port {
                        params["port"] = json!(p);
                    }
                    let response = send_control_request(ControlRequest {
                        command: "web_remote_enable".into(),
                        params,
                    })
                    .await?;
                    if !response.ok {
                        // Surface the handler's message (e.g. the tailscale
                        // "No Tailscale address found" bind error) rather than a
                        // bare `null`.
                        return Err(response
                            .error
                            .unwrap_or_else(|| "Unknown error from control endpoint".to_string()));
                    }
                    print_enable_result(&response.data.unwrap_or(json!(null)));
                }
                RemoteCommand::Disable => {
                    let response = send_control_request(ControlRequest {
                        command: "web_remote_disable".into(),
                        params: json!({}),
                    })
                    .await?;
                    if !response.ok {
                        return Err(response
                            .error
                            .unwrap_or_else(|| "Unknown error from control endpoint".to_string()));
                    }
                    println!("Web remote access disabled. Every live connection was severed.");
                }
            }
            Ok(true)
        }
        // Account commands are deliberately local: they open the same
        // SQLite store the app uses and talk to the auth API directly,
        // never the control socket. Requiring a running instance would
        // defeat the point — the whole reason `login` exists is to sign in
        // on a headless box that has nothing running yet.
        Some(CommandSet::Login {
            email,
            token,
            status,
        }) => {
            crate::auth::cli_login::run_login(email, token, status).await?;
            Ok(true)
        }
        // `connect` is local for the same reason `login` is — it exists to set
        // up a machine that has nothing running yet. It reaches for the control
        // socket only when it detects a live instance (see `web_remote::connect`).
        Some(CommandSet::Connect {
            command,
            email,
            scope,
            port,
        }) => {
            match command {
                Some(ConnectCommand::Status) => crate::web_remote::connect::run_connect_status()?,
                Some(ConnectCommand::Off) => crate::web_remote::connect::run_connect_off().await?,
                None => {
                    crate::web_remote::connect::run_connect(
                        crate::web_remote::connect::ConnectOptions { email, scope, port },
                    )
                    .await?
                }
            }
            Ok(true)
        }
        Some(CommandSet::Logout) => {
            crate::auth::cli_login::run_logout()?;
            Ok(true)
        }
        Some(CommandSet::Whoami) => {
            crate::auth::cli_login::run_whoami()?;
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
                            "enable": { "args": "[--scope all|tailscale|loopback] [--port N]", "description": "Turn web remote access on and print the reachable endpoint (requires the desktop app running)" },
                            "disable": { "description": "Turn web remote access off, severing every live connection" },
                            "pair": { "args": "[--name <label>]", "description": "Mint a one-time web-remote pairing code + QR (requires remote access enabled)" }
                        }
                    },
                    "monitor": {
                        "description": "Declare that this pane is watching something in the background (calm 'Monitoring' status). Provider-agnostic; targets CODEMUX_PANE_ID / CODEMUX_WORKSPACE_ID",
                        "subcommands": {
                            "start": { "args": "[--reason <text>] [--pane-id <id>]", "description": "Mark this pane as monitoring in the background" },
                            "stop": { "args": "[--pane-id <id>]", "description": "Clear this pane's monitoring status" },
                            "status": { "args": "[--pane-id <id>]", "description": "Report whether this pane currently reads as monitoring" }
                        }
                    },
                    "ports": {
                        "description": "Reserve host ports no other worktree will be handed, so parallel worktrees of one project can each run their own stack",
                        "subcommands": {
                            "allocate": { "args": "<name>", "description": "Print a free port reserved for this worktree under <name>; the same name always returns the same port" },
                            "list": { "description": "List the ports this worktree owns" },
                            "release": { "args": "<name>", "description": "Give a previously allocated port back" }
                        }
                    },
                    "serve": {
                        "args": "[--scope all|tailscale|loopback] [--port N] [--relay]",
                        "description": "Run headless as a web-remote server (no GUI); prints a pairing QR + link and runs until Ctrl-C"
                    },
                    "connect": {
                        "args": "[--email <address>] [--scope all|tailscale|loopback] [--port N]",
                        "description": "One-command remote-access setup: sign in, turn relay mode on, and install a background service so this machine stays reachable",
                        "subcommands": {
                            "status": { "description": "Show the account, remote-access config, background service, and running-instance state" },
                            "off": { "description": "Turn from-anywhere access off and remove the background service (stays signed in)" }
                        }
                    },
                    "login": {
                        "args": "[--email <address>] [--token <token>] [--status]",
                        "description": "Sign this machine into a Codemux account (headless-friendly; no GUI or running instance needed)"
                    },
                    "logout": { "description": "Clear the cached account session on this machine" },
                    "whoami": { "description": "Print the signed-in account; exits 1 when signed out" },
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
                    "CODEMUX_PANE_ID": "Current pane ID (the default target for `codemux monitor`)",
                    "CODEMUX_BROWSER_CMD": "Command prefix for browser control",
                    "CODEMUX_PASSWORD": "Password for a non-interactive `codemux login` (warns; for automated runs only)",
                    "CODEMUX_API_URL": "Override the Codemux API base URL (default https://api.codemux.org)",
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
pub(crate) fn print_pairing(data: &Value) {
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

/// Print the outcome of `codemux remote enable`: the port + bind scope the
/// server is now on, the recommended reachable endpoint, and a nudge toward
/// `codemux remote pair`. Reads the `ControlEnableResult` JSON the
/// `web_remote_enable` control command returns.
fn print_enable_result(data: &Value) {
    let status = &data["status"];
    let port = status["port"].as_u64().unwrap_or(0);
    let scope = status["bind_scope"].as_str().unwrap_or("all");
    let already_running = data["already_running"].as_bool().unwrap_or(false);
    let endpoint_url = data["endpoint_url"].as_str().unwrap_or_default();
    let endpoint_host = data["endpoint_host"].as_str().unwrap_or_default();
    let endpoint_kind = data["endpoint_kind"].as_str().unwrap_or_default();
    let endpoint_secure = data["endpoint_secure"].as_bool().unwrap_or(false);

    println!();
    if already_running {
        println!("Web remote access is already running.");
    } else {
        println!("Web remote access enabled.");
    }
    println!("Port:          {port}");
    let scope_note = match scope {
        "tailscale" => "tailnet + loopback only",
        "loopback" => "this machine only",
        _ => "every interface",
    };
    println!("Access scope:  {scope} ({scope_note})");
    if !endpoint_host.is_empty() {
        let secure = if endpoint_secure {
            "secure context"
        } else {
            "plain HTTP — not a browser secure context"
        };
        println!("Reachable at:  {endpoint_url}");
        println!("               {endpoint_host} ({endpoint_kind}, {secure})");
    }
    println!();
    println!("Run `codemux remote pair` to mint a pairing code for a phone or laptop.");
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
    fn account_subcommands_parse() {
        // `login` must accept both the bare interactive form and the
        // automation form; `whoami` / `logout` take no arguments. Pinned
        // because these three are the headless sign-in entry points a
        // docker/CI harness scripts against.
        let cli = Cli::try_parse_from(["codemux", "login"]).expect("bare login parses");
        assert!(matches!(
            cli.command,
            Some(CommandSet::Login {
                email: None,
                token: None,
                status: false
            })
        ));

        let cli = Cli::try_parse_from(["codemux", "login", "--email", "a@b.co"])
            .expect("login --email parses");
        match cli.command {
            Some(CommandSet::Login { email, token, status }) => {
                assert_eq!(email.as_deref(), Some("a@b.co"));
                assert_eq!(token, None);
                assert!(!status);
            }
            _ => panic!("expected login"),
        }

        // The OAuth-only escape hatch. `--token` is mutually exclusive with
        // `--email`: a bearer already identifies the account, so accepting
        // both would let the two disagree.
        let cli = Cli::try_parse_from(["codemux", "login", "--token", "sess_abc"])
            .expect("login --token parses");
        match cli.command {
            Some(CommandSet::Login { email, token, .. }) => {
                assert_eq!(token.as_deref(), Some("sess_abc"));
                assert_eq!(email, None);
            }
            _ => panic!("expected login"),
        }
        assert!(
            Cli::try_parse_from(["codemux", "login", "--email", "a@b.co", "--token", "t"])
                .is_err(),
            "--email and --token must not be combined"
        );

        let cli =
            Cli::try_parse_from(["codemux", "login", "--status"]).expect("login --status parses");
        assert!(matches!(
            cli.command,
            Some(CommandSet::Login { status: true, .. })
        ));

        assert!(matches!(
            Cli::try_parse_from(["codemux", "whoami"]).unwrap().command,
            Some(CommandSet::Whoami)
        ));
        assert!(matches!(
            Cli::try_parse_from(["codemux", "logout"]).unwrap().command,
            Some(CommandSet::Logout)
        ));
        // `status` (app status) must not be shadowed by the new account
        // commands.
        assert!(matches!(
            Cli::try_parse_from(["codemux", "status"]).unwrap().command,
            Some(CommandSet::Status)
        ));
    }

    #[test]
    fn the_command_tree_is_internally_consistent() {
        // clap's own audit: duplicate names, impossible `conflicts_with`
        // targets, and malformed value parsers are compile-time-looking bugs
        // that otherwise only surface when a user runs the command.
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    /// The `ports` module is only reachable if it is registered on
    /// `CommandSet`. Parsing the real argv guards the failure mode where the
    /// implementation exists but `codemux ports` is an unrecognized command.
    #[test]
    fn ports_subcommands_parse() {
        match Cli::try_parse_from(["codemux", "ports", "allocate", "web"])
            .expect("`codemux ports allocate web` is a known command")
            .command
        {
            Some(CommandSet::Ports {
                command: ports::PortsCommand::Allocate { name },
            }) => assert_eq!(name, "web"),
            _ => panic!("expected ports allocate"),
        }

        assert!(matches!(
            Cli::try_parse_from(["codemux", "ports", "list"])
                .expect("`codemux ports list` is a known command")
                .command,
            Some(CommandSet::Ports {
                command: ports::PortsCommand::List
            })
        ));

        match Cli::try_parse_from(["codemux", "ports", "release", "api"])
            .expect("`codemux ports release api` is a known command")
            .command
        {
            Some(CommandSet::Ports {
                command: ports::PortsCommand::Release { name },
            }) => assert_eq!(name, "api"),
            _ => panic!("expected ports release"),
        }

        // A name is required — a bare `allocate` must not silently allocate
        // something under an empty label.
        assert!(Cli::try_parse_from(["codemux", "ports", "allocate"]).is_err());
    }

    #[test]
    fn connect_subcommands_parse() {
        // The flagship path is the *bare* verb — `codemux connect` with no
        // arguments must be the whole setup, not an error asking for one.
        match Cli::try_parse_from(["codemux", "connect"]).unwrap().command {
            Some(CommandSet::Connect {
                command,
                email,
                scope,
                port,
            }) => {
                assert!(command.is_none(), "no subcommand means: set this machine up");
                assert!(email.is_none() && scope.is_none() && port.is_none());
            }
            _ => panic!("expected connect"),
        }

        match Cli::try_parse_from([
            "codemux", "connect", "--email", "a@b.co", "--scope", "tailscale", "--port", "5100",
        ])
        .unwrap()
        .command
        {
            Some(CommandSet::Connect {
                email, scope, port, ..
            }) => {
                assert_eq!(email.as_deref(), Some("a@b.co"));
                assert_eq!(scope.as_deref(), Some("tailscale"));
                assert_eq!(port, Some(5100));
            }
            _ => panic!("expected connect"),
        }

        assert!(matches!(
            Cli::try_parse_from(["codemux", "connect", "status"]).unwrap().command,
            Some(CommandSet::Connect {
                command: Some(ConnectCommand::Status),
                ..
            })
        ));
        assert!(matches!(
            Cli::try_parse_from(["codemux", "connect", "off"]).unwrap().command,
            Some(CommandSet::Connect {
                command: Some(ConnectCommand::Off),
                ..
            })
        ));
        // `disconnect` reads better in some sentences; both reach the same verb.
        assert!(matches!(
            Cli::try_parse_from(["codemux", "connect", "disconnect"]).unwrap().command,
            Some(CommandSet::Connect {
                command: Some(ConnectCommand::Off),
                ..
            })
        ));

        // The same scope values the rest of the remote surface accepts, and
        // nothing else.
        assert!(Cli::try_parse_from(["codemux", "connect", "--scope", "wan"]).is_err());
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

    #[test]
    fn print_enable_result_handles_a_full_control_result_without_panicking() {
        // Mirrors the `ControlEnableResult` JSON the `web_remote_enable` control
        // command returns, so the CLI's formatter is exercised end-to-end.
        let data = json!({
            "status": {
                "enabled": true,
                "running": true,
                "port": 4377,
                "require_approval": false,
                "bind_scope": "tailscale",
                "active_connections": 0,
                "connected_sessions": 0,
                "sessions": [],
                "update_available": false,
                "update_version": null,
                "account_mode_enabled": false,
                "trust_account_browsers": false,
                "account_signed_in": false,
                "relay_mode_enabled": false,
                "iroh_node_id": null,
                "device_registered": false,
                "device_id": null,
            },
            "endpoint_url": "http://100.101.102.103:4377",
            "endpoint_host": "100.101.102.103",
            "endpoint_kind": "tailnet",
            "endpoint_secure": false,
            "already_running": false,
        });
        print_enable_result(&data);
    }
}

