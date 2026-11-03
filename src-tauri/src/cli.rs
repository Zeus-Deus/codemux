use crate::control::{send_control_request, ControlRequest, ControlResponse};
use clap::{Parser, Subcommand};
use door::Door;
use output::Exit;
use serde_json::{json, Value};
use std::ffi::OsString;
use std::io::Write;

pub mod args;
mod capabilities;
pub mod connector;
#[cfg(test)]
mod docs_tests;
mod door;
mod output;
pub mod ports;
pub mod thread;
mod tools;
mod verbs;
pub mod workspace;

/// Help footer of the root and of `thread`: the statuses scripts branch on.
pub(crate) const EXIT_CODES_HELP: &str =
    "Exit codes: 0 done · 1 failed · 2 needs input · 3 timed out · 64 usage error. Guide: docs/outside-agents.md";

#[derive(Parser)]
#[command(name = "codemux", about = "Codemux desktop and control CLI", after_help = EXIT_CODES_HELP)]
pub struct Cli {
    /// Launch the desktop with every plugin off (same as
    /// `CODEMUX_DISABLE_PLUGINS=1`).
    #[arg(long, global = true, alias = "disable-addons")]
    pub disable_plugins: bool,
    /// Who the control socket records this invocation's calls as. Given
    /// before the command: `codemux --client hermes thread start …`.
    #[command(flatten)]
    pub caller: args::CallerArgs,
    #[command(subcommand)]
    pub command: Option<CommandSet>,
}

// Declaration order is help order: the app, then the control verbs, then
// the other commands, with debug and internal ones last.
#[derive(Subcommand)]
pub enum CommandSet {
    /// Open the desktop app (same as no command)
    App,
    /// Show the running app's workspaces and focused pane
    Status,
    /// List the control tools this caller can use, from the running app
    Tools {
        /// Print the MCP tool definitions as JSON.
        #[arg(long, conflicts_with = "markdown")]
        json: bool,
        /// Print a Markdown reference.
        #[arg(long)]
        markdown: bool,
    },
    /// Call one control tool with JSON arguments and print the result as JSON
    Call {
        /// Tool name, as `codemux tools` lists it.
        tool: String,
        /// Arguments as a JSON object, or - to read them from stdin (default {}).
        arguments: Option<String>,
    },
    /// Open, list, inspect and close workspaces
    Workspace {
        #[command(subcommand)]
        command: workspace::WorkspaceCommand,
    },
    /// Create git worktrees as workspaces
    Worktree {
        #[command(subcommand)]
        command: workspace::WorktreeCommand,
    },
    /// Launch agent threads, send them messages, and wait for and read
    /// their replies
    #[command(after_help = EXIT_CODES_HELP)]
    Thread {
        #[command(subcommand)]
        command: thread::ThreadCommand,
    },
    /// Run the network door that assistants on other devices sign in to
    Connector {
        #[command(subcommand)]
        command: connector::ConnectorCommand,
    },
    /// Serve Codemux tools to an MCP client over stdio
    Mcp {
        /// Name this client's calls are recorded under (default: the MCP
        /// client's own name).
        #[arg(long, value_name = "NAME", value_parser = args::client_name)]
        client: Option<String>,
        /// The most this client's calls may do (default: full-access).
        #[arg(long, value_enum, value_name = "LEVEL")]
        access: Option<args::AccessArg>,
    },
    /// Send a notification to the user
    Notify { message: String },
    /// Generate a project handoff summary
    Handoff,
    /// Project memory management
    Memory {
        #[command(subcommand)]
        command: MemoryCommand,
    },
    /// Code index for search
    Index {
        #[command(subcommand)]
        command: IndexCommand,
    },
    /// Control the browser pane
    Browser {
        #[command(subcommand)]
        command: BrowserCommand,
    },
    /// GitHub issue operations
    Issue {
        #[command(subcommand)]
        command: IssueCommand,
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
    /// Reload the Codemux window's interface without restarting the app.
    /// Recovers a blank or frozen window; the backend, terminals, and agents
    /// keep running. Same as pressing Ctrl+Alt+R in the window.
    ReloadUi,
    /// Print recent lines from the desktop app's log file
    Logs {
        /// Number of lines from the end of the log to print
        #[arg(long, default_value_t = 200)]
        tail: usize,
    },
    /// Check the local environment for common problems (file dialogs,
    /// logs). Works without a running Codemux instance.
    Doctor,
    /// Describe every command, control tool, exit code and environment
    /// variable as JSON (no running app needed)
    Capabilities,
    /// Debug: send one raw control-socket command and print the reply.
    /// Exits 1 when the reply is not ok. Scripts should prefer `codemux call`.
    Json { command: String, params: Option<String> },
    /// Call an injected chat MCP tool with the chat's own authority. Reads
    /// the credential the agent process was started with.
    McpCall {
        tool: Option<String>,
        arguments: Option<String>,
        #[arg(long)]
        list: bool,
    },
    /// Relay a stdio MCP client to the desktop app's MCP gateway. Codemux
    /// starts this for agents that only take stdio MCP servers.
    #[command(hide = true)]
    McpBridge,
    /// Run as the persistent PTY daemon (internal subcommand spawned by the
    /// Tauri app; long-lived process that owns agent PTYs so they survive
    /// the app being closed).
    #[command(hide = true)]
    PtyDaemon {
        /// Absolute path of the Unix socket to bind. The Tauri app passes
        /// this when spawning the daemon.
        #[arg(long)]
        socket: std::path::PathBuf,
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
    /// Create a new browser pane
    Create,
    /// Navigate to a URL
    Open { url: String },
    /// Get the page element tree
    Snapshot {
        #[arg(hide = true)]
        browser_id: Option<String>,
        /// Use DOM-based query instead of ARIA tree
        #[arg(long)]
        dom: bool,
    },
    /// Click an element
    Click { selector: String, #[arg(hide = true)] browser_id: Option<String> },
    /// Type into an input
    Fill { selector: String, value: String, #[arg(hide = true)] browser_id: Option<String> },
    /// Capture a screenshot (base64 PNG)
    Screenshot { #[arg(hide = true)] browser_id: Option<String> },
    /// Get the browser console output
    ConsoleLogs { #[arg(hide = true)] browser_id: Option<String> },
    /// Click at pixel coordinates via CDP
    ClickAt { x: f64, y: f64, #[arg(long, default_value = "left")] click_type: String, #[arg(hide = true)] browser_id: Option<String> },
    /// Type text at coordinates or cursor position via CDP
    TypeAt { text: String, #[arg(long)] x: Option<f64>, #[arg(long)] y: Option<f64>, #[arg(hide = true)] browser_id: Option<String> },
    /// Scroll at coordinates via CDP
    ScrollAt { x: f64, y: f64, #[arg(long, default_value = "down")] direction: String, #[arg(long, default_value = "3")] amount: i32, #[arg(hide = true)] browser_id: Option<String> },
    /// Press a key or combo via CDP
    KeyPress { key: String, #[arg(hide = true)] browser_id: Option<String> },
    /// Drag between coordinates via CDP
    Drag { start_x: f64, start_y: f64, end_x: f64, end_y: f64, #[arg(hide = true)] browser_id: Option<String> },
    /// Click at coordinates using OS-level input (ydotool)
    ClickOs { x: f64, y: f64, #[arg(hide = true)] browser_id: Option<String> },
    /// Type text using OS-level input (ydotool)
    TypeOs { text: String, #[arg(long)] x: Option<f64>, #[arg(long)] y: Option<f64>, #[arg(hide = true)] browser_id: Option<String> },
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
        #[arg(hide = true)]
        browser_id: Option<String>,
    },
    /// List the available viewport presets and their CSS dimensions.
    ViewportPresets,
}

#[derive(Subcommand)]
pub enum MemoryCommand {
    /// Show project memory
    Show,
    /// Update memory fields
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
    /// Add a memory entry
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
    /// Build or rebuild the search index
    Build,
    /// Show index status
    Status,
    /// Search indexed code
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
    /// The CLI fully handled the command; the process exits with this status.
    Handled(Exit),
    /// No subcommand (or `app`): fall through to launching the desktop GUI.
    LaunchGui,
    /// `codemux serve [...]`: run the headless web-remote server on the main
    /// thread (see `codemux_lib::run_serve`).
    RunServe(crate::web_remote::serve::ServeOptions),
}

pub async fn maybe_run_cli() -> Result<CliOutcome, String> {
    let cli = match parse_outcome(std::env::args_os()) {
        Ok(cli) => cli,
        Err((exit, error)) => {
            // clap picks the stream: help on stdout, rejections on stderr.
            let _ = error.print();
            return Ok(CliOutcome::Handled(exit));
        }
    };
    if cli.disable_plugins {
        std::env::set_var("CODEMUX_DISABLE_PLUGINS", "1");
    }
    // `serve` never touches the control socket — hand it back to `main` so it
    // can run the long-lived server on the main thread outside `block_on`.
    if let Some(CommandSet::Serve { scope, port, relay }) = &cli.command {
        return Ok(CliOutcome::RunServe(crate::web_remote::serve::ServeOptions {
            scope: scope.clone(),
            port: *port,
            relay: *relay,
        }));
    }
    if matches!(cli.command, None | Some(CommandSet::App)) {
        return Ok(CliOutcome::LaunchGui);
    }
    run_control_cli(cli).await.map(CliOutcome::Handled)
}

/// Parse argv without letting clap exit the process from inside `block_on`.
/// Help is exit 0 and every rejection is 64 (EX_USAGE), so a 2 only ever
/// means "needs input". The caller prints the returned clap message.
fn parse_outcome<I, T>(argv: I) -> Result<Cli, (Exit, clap::Error)>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    Cli::try_parse_from(argv).map_err(|error| {
        let exit = if error.use_stderr() { Exit::Usage } else { Exit::Done };
        (exit, error)
    })
}

/// Print a control reply as the legacy verbs always have, failing the
/// process when it is not ok. The reply still goes to stdout either way, so
/// scripts keep reading the error from it.
fn print_response(response: &ControlResponse) -> Result<Exit, String> {
    write_response(&mut std::io::stdout().lock(), response)
}

fn write_response(writer: &mut impl Write, response: &ControlResponse) -> Result<Exit, String> {
    output::write_json(writer, response)?;
    Ok(if response.ok { Exit::Done } else { Exit::Failed })
}

/// Names this process's control identity before any socket call: every
/// request then carries it (`ControlClient::current`). `codemux mcp` takes
/// its own flags first, then the root's; unnamed calls run as the owner.
fn adopt_caller(cli: &Cli) -> (Option<String>, Option<args::AccessArg>) {
    let (client, access) = match &cli.command {
        Some(CommandSet::Mcp { client, access }) => (client.clone().or(cli.caller.client.clone()), access.or(cli.caller.access)),
        _ => (cli.caller.client.clone(), cli.caller.access),
    };
    if let Some(name) = &client {
        crate::control_identity::set_client_name(name);
    }
    if let Some(access) = access {
        crate::control_identity::set_client_access(access.into());
    }
    (client, access)
}

/// Drive every control-socket / local CLI subcommand and return the exit
/// status. No subcommand, `app` and `serve` are handled by [`maybe_run_cli`]
/// before this runs.
async fn run_control_cli(cli: Cli) -> Result<Exit, String> {
    let (client, access) = adopt_caller(&cli);
    match cli.command {
        None | Some(CommandSet::App) | Some(CommandSet::Serve { .. }) => {
            unreachable!("maybe_run_cli handles the GUI launch and serve before run_control_cli")
        }
        Some(CommandSet::Tools { json, markdown }) => tools::run_tools(&Door::Socket, json, markdown).await,
        Some(CommandSet::Call { tool, arguments }) => tools::run_call(&Door::Socket, &tool, arguments.as_deref()).await,
        Some(CommandSet::Workspace { command }) => workspace::run(&Door::Socket, command).await,
        Some(CommandSet::Worktree { command }) => workspace::run_worktree(&Door::Socket, command).await,
        Some(CommandSet::Thread { command }) => thread::run(&Door::Socket, command).await,
        Some(CommandSet::Connector { command }) => connector::run(&Door::Socket, command).await,
        Some(CommandSet::Status) => {
            let response = send_control_request(ControlRequest {
                command: "status".into(),
                params: json!({}),
            })
            .await?;
            print_response(&response)
        }
        Some(CommandSet::Json { command, params }) => {
            let params = params
                .map(|raw| serde_json::from_str(&raw).map_err(|error| error.to_string()))
                .transpose()?
                .unwrap_or_else(|| json!({}));
            let response = send_control_request(ControlRequest { command, params }).await?;
            print_response(&response)
        }
        Some(CommandSet::Notify { message }) => {
            let response = send_control_request(ControlRequest {
                command: "notify".into(),
                params: json!({ "message": message }),
            })
            .await?;
            print_response(&response)
        }
        Some(CommandSet::Handoff) => {
            let response = send_control_request(ControlRequest {
                command: "generate_handoff".into(),
                params: json!({}),
            })
            .await?;
            print_response(&response)
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

            print_response(&response)
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

            print_response(&response)
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
                            return Ok(Exit::Done);
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
                        return Ok(Exit::Done);
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

            print_response(&response)
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
            output::emit_json(&result)?;
            Ok(Exit::Done)
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
            print_response(&response)
        }
        // Purely local: the allocation table is a file on disk, so this works
        // from a bare shell with no Codemux instance running.
        Some(CommandSet::Ports { command }) => {
            ports::run(command)?;
            Ok(Exit::Done)
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
            Ok(Exit::Done)
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
            Ok(Exit::Done)
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
                Some(ConnectCommand::Status) => crate::web_remote::connect::run_connect_status().await?,
                Some(ConnectCommand::Off) => crate::web_remote::connect::run_connect_off().await?,
                None => {
                    crate::web_remote::connect::run_connect(
                        crate::web_remote::connect::ConnectOptions { email, scope, port },
                    )
                    .await?
                }
            }
            Ok(Exit::Done)
        }
        Some(CommandSet::Logout) => {
            crate::auth::cli_login::run_logout()?;
            Ok(Exit::Done)
        }
        Some(CommandSet::Whoami) => {
            crate::auth::cli_login::run_whoami()?;
            Ok(Exit::Done)
        }
        Some(CommandSet::ReloadUi) => {
            let response = send_control_request(ControlRequest {
                command: "reload_ui".into(),
                params: json!({}),
            })
            .await?;
            if !response.ok {
                return Err(response
                    .error
                    .unwrap_or_else(|| "Codemux could not reload its interface".to_string()));
            }
            println!("Reloading the Codemux interface. Terminals and agents keep running.");
            Ok(Exit::Done)
        }
        Some(CommandSet::Logs { tail }) => {
            // Purely local — reads the file tauri-plugin-log writes, so
            // it works even when (especially when) the app won't start.
            match crate::app_logs::app_log_file() {
                Some(path) if path.exists() => {
                    output::note(&format!("# {}", path.display()));
                    for line in
                        crate::app_logs::tail_lines(&path, tail).map_err(|e| e.to_string())?
                    {
                        output::out(&line)?;
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
            Ok(Exit::Done)
        }
        Some(CommandSet::Doctor) => {
            crate::doctor::run().await;
            Ok(Exit::Done)
        }
        Some(CommandSet::Mcp { .. }) => {
            let options = crate::mcp_server::McpServerOptions { client, access: access.map(Into::into) };
            crate::mcp_server::run_mcp_server_with(options).await?;
            Ok(Exit::Done)
        }
        Some(CommandSet::McpBridge) => {
            crate::mcp::bridge::run().await?;
            Ok(Exit::Done)
        }
        Some(CommandSet::McpCall {
            tool,
            arguments,
            list,
        }) => {
            crate::mcp::bridge::call(tool, arguments, list).await?;
            Ok(Exit::Done)
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
                Ok(Exit::Done)
            }
            #[cfg(not(unix))]
            {
                let _ = socket;
                Err("codemux pty-daemon is Unix-only for now".to_string())
            }
        }
        Some(CommandSet::Capabilities) => {
            capabilities::print()?;
            Ok(Exit::Done)
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
    if let Some(err) = status["bind_error"].as_str() {
        // Relay mode kept remote access on without its LAN listener; don't
        // print an address nothing is listening on.
        println!("Listener:      not running — {err}");
        println!("               (retrying in the background; relay access is unaffected)");
        return;
    }
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

    #[test]
    fn usage_errors_are_64_and_help_is_0() {
        let exit_of = |argv: &[&str]| match parse_outcome(argv) {
            Ok(_) => None,
            Err((exit, _)) => Some(exit.code()),
        };

        // Help at any level prints on stdout and succeeds.
        for argv in [
            &["codemux", "--help"][..],
            &["codemux", "-h"],
            &["codemux", "help"],
            &["codemux", "help", "memory"],
            &["codemux", "status", "--help"],
            &["codemux", "remote", "enable", "--help"],
        ] {
            assert_eq!(exit_of(argv), Some(0), "{argv:?}");
        }

        // Every rejection is EX_USAGE, never clap's 2, which means "needs input".
        for argv in [
            &["codemux", "no-such-command"][..],
            &["codemux", "status", "--bogus"],
            &["codemux", "notify"],
            &["codemux", "memory"],
            &["codemux", "serve", "--scope", "wan"],
            &["codemux", "logs", "--tail", "many"],
        ] {
            assert_eq!(exit_of(argv), Some(64), "{argv:?}");
        }

        // Valid argv still parses, including the bare GUI launch.
        for argv in [
            &["codemux"][..],
            &["codemux", "--disable-plugins"],
            &["codemux", "json", "status"],
            &["codemux", "json", "conversation_read", "{\"limit\":5}"],
        ] {
            assert_eq!(exit_of(argv), None, "{argv:?}");
        }

        // The control verbs follow the same rule.
        assert_eq!(exit_of(&["codemux", "thread", "--help"]), Some(0));
        for argv in [
            &["codemux", "thread", "start", "--provider", "codex", "-m", "hi"][..],
            &["codemux", "thread", "wait", "t", "--timeout", "forever"],
            &["codemux", "call"],
            &["codemux", "--access", "root", "tools"],
        ] {
            assert_eq!(exit_of(argv), Some(64), "{argv:?}");
        }
    }

    fn parse(argv: &[&str]) -> CommandSet {
        Cli::try_parse_from(argv).unwrap_or_else(|error| panic!("{argv:?}: {error}")).command.expect("a command")
    }

    #[test]
    fn control_verbs_parse() {
        use thread::{Decision, ThreadCommand};
        let start = ["codemux", "thread", "start", "--workspace", "ws", "--provider", "codex"];
        let message = ["-m", "hi"];
        let full = [&start[..], &["--permission-mode", "read-only"], &message, &["--wait", "--timeout", "10m", "--json"]].concat();
        match parse(&full) {
            CommandSet::Thread { command: ThreadCommand::Start(args) } => {
                assert_eq!((args.provider.as_str(), args.permission_mode.as_deref(), args.message.as_str()), ("codex", Some("read-only"), "hi"));
                assert!(args.run.wait && args.out.json);
                assert_eq!(args.run.timeout, Some(std::time::Duration::from_secs(600)));
            }
            _ => panic!("expected thread start"),
        }
        // Exactly one of the permission flags, and never neither.
        let both = [&start[..], &["--permission-mode", "x", "--no-permission-mode"], &message].concat();
        assert!(Cli::try_parse_from(both).is_err());
        assert!(Cli::try_parse_from([&start[..], &message].concat()).is_err());
        assert!(Cli::try_parse_from([&start[..], &["--no-permission-mode"], &message].concat()).is_ok());
        // A worktree base needs the worktree branch.
        let base_only = [&start[..], &["--no-permission-mode", "--worktree-base", "trunk"], &message].concat();
        assert!(Cli::try_parse_from(base_only).is_err());

        // Branch deletion and force only come with worktree removal.
        assert!(Cli::try_parse_from(["codemux", "workspace", "close", "ws", "--delete-branch"]).is_err());
        assert!(Cli::try_parse_from(["codemux", "workspace", "close", "ws", "--force"]).is_err());
        assert!(Cli::try_parse_from(["codemux", "workspace", "close", "ws", "--remove-worktree", "--delete-branch", "--force"]).is_ok());

        for (word, decision) in [("allow-once", Decision::AllowOnce), ("allow-always", Decision::AllowAlways), ("deny", Decision::Deny)] {
            match parse(&["codemux", "thread", "respond", "t", "r", word]) {
                CommandSet::Thread { command: ThreadCommand::Respond(args) } => assert_eq!(args.decision, decision),
                _ => panic!("expected thread respond"),
            }
        }
        assert!(Cli::try_parse_from(["codemux", "thread", "respond", "t", "r", "allow_once"]).is_err());

        for argv in [
            &["codemux", "tools", "--json"][..],
            &["codemux", "tools", "--markdown"],
            &["codemux", "call", "thread_list", "{}"],
            &["codemux", "call", "thread_list", "-"],
            &["codemux", "workspace", "create", ".", "--select", "--request-id", "k", "--json"],
            &["codemux", "workspace", "list"],
            &["codemux", "workspace", "info"],
            &["codemux", "workspace", "rerun-setup", "ws"],
            &["codemux", "worktree", "create", "--repo", ".", "--branch", "b", "--base", "trunk"],
            &["codemux", "thread", "send", "t", "--workspace", "ws", "-m", "-", "--delivery", "queue", "--wait"],
            &["codemux", "thread", "wait", "t", "--turn", "t1", "--timeout", "90s"],
            &["codemux", "thread", "read", "t", "--all"],
            &["codemux", "thread", "list", "--limit", "5"],
            &["codemux", "thread", "status", "t", "--json"],
            &["codemux", "thread", "interrupt", "t", "--turn", "t1"],
            &["codemux", "thread", "stop", "t"],
            &["codemux", "connector", "status"],
            &["codemux", "connector", "enable", "--port", "4378", "--origin", "https://desk.example"],
            &["codemux", "connector", "issue", "--label", "bot", "--access", "supervised", "--expires", "30d"],
            &["codemux", "connector", "revoke", "--all"],
            &["codemux", "connector", "approve", "req", "--access", "read-only"],
        ] {
            parse(argv);
        }
        assert!(Cli::try_parse_from(["codemux", "tools", "--json", "--markdown"]).is_err());
        assert!(Cli::try_parse_from(["codemux", "worktree", "create", "--repo", ".", "--branch", "b", "--base", "x", "--existing"]).is_err());
        assert!(Cli::try_parse_from(["codemux", "thread", "read", "t", "--all", "--cursor", "3"]).is_err());
        assert!(Cli::try_parse_from(["codemux", "thread", "read", "t", "--limit", "101"]).is_err());
    }

    #[test]
    fn caller_flags_name_and_narrow_the_client() {
        // Root flags, before the command, for every verb.
        let cli = Cli::try_parse_from(["codemux", "--client", "Hermes Bot", "--access", "supervised", "tools"]).unwrap();
        assert_eq!(cli.caller.client.as_deref(), Some("Hermes Bot"));
        assert_eq!(cli.caller.access, Some(args::AccessArg::Supervised));
        for (spelling, access) in [
            ("read-only", args::AccessArg::ReadOnly),
            ("read_only", args::AccessArg::ReadOnly),
            ("full-access", args::AccessArg::FullAccess),
            ("full_access", args::AccessArg::FullAccess),
            ("full", args::AccessArg::FullAccess),
        ] {
            let cli = Cli::try_parse_from(["codemux", "--access", spelling, "status"]).unwrap();
            assert_eq!(cli.caller.access, Some(access), "{spelling}");
        }
        // A name with nothing usable would silently run as the owner.
        assert!(Cli::try_parse_from(["codemux", "--client", "!!!", "status"]).is_err());
        assert!(Cli::try_parse_from(["codemux", "--client", "x", "tools", "--client", "y"]).is_err());
        assert_eq!(crate::agent_control::ControlAccess::from(args::AccessArg::Supervised), crate::agent_control::ControlAccess::Supervised);
    }

    #[test]
    fn bare_mcp_still_serves_stdio() {
        // `.mcp.json` entries, the Vexis config and Settings run exactly this.
        assert!(matches!(parse(&["codemux", "mcp"]), CommandSet::Mcp { client: None, access: None }));
        match parse(&["codemux", "mcp", "--client", "hermes", "--access", "supervised"]) {
            CommandSet::Mcp { client, access } => {
                assert_eq!(client.as_deref(), Some("hermes"));
                assert_eq!(access, Some(args::AccessArg::Supervised));
            }
            _ => panic!("expected mcp"),
        }
        assert!(Cli::try_parse_from(["codemux", "mcp", "--client", "---"]).is_err());
        assert!(Cli::try_parse_from(["codemux", "mcp", "install"]).is_err());
    }

    #[test]
    fn mcp_call_still_parses() {
        // ACP prompts tell agents to run exactly these.
        assert!(matches!(parse(&["codemux", "mcp-call", "--list"]), CommandSet::McpCall { tool: None, list: true, .. }));
        match parse(&["codemux", "mcp-call", "thread_list", "{}"]) {
            CommandSet::McpCall { tool, arguments, list } => {
                assert_eq!((tool.as_deref(), arguments.as_deref(), list), (Some("thread_list"), Some("{}"), false));
            }
            _ => panic!("expected mcp-call"),
        }
    }

    #[test]
    fn internal_commands_are_hidden_but_still_run() {
        use clap::CommandFactory;
        let root = Cli::command();
        for name in ["mcp-bridge", "pty-daemon"] {
            assert!(root.find_subcommand(name).is_some_and(|command| command.is_hide_set()), "{name}");
        }
        assert!(matches!(parse(&["codemux", "pty-daemon", "--socket", "/tmp/x.sock"]), CommandSet::PtyDaemon { .. }));
        // The ignored browser ids are hidden from help but still accepted.
        assert!(matches!(parse(&["codemux", "browser", "screenshot", "default"]), CommandSet::Browser { .. }));
        let help = root.find_subcommand("browser").unwrap().find_subcommand("screenshot").unwrap().clone().render_help().to_string();
        assert!(!help.contains("BROWSER_ID"), "{help}");
    }

    #[test]
    fn ok_false_replies_exit_1_and_still_print() {
        let print = |response: &ControlResponse| {
            let mut printed = Vec::new();
            let exit = write_response(&mut printed, response).expect("writes to memory");
            (exit, String::from_utf8(printed).expect("utf-8"))
        };
        // The exact pretty JSON the legacy verbs always printed.
        let as_before = |response: &ControlResponse| {
            format!("{}\n", serde_json::to_string_pretty(response).unwrap())
        };

        let ok = ControlResponse {
            ok: true,
            protocol_version: 1,
            data: Some(json!({ "workspace_id": "ws-1" })),
            error: None,
        };
        let (exit, printed) = print(&ok);
        assert_eq!((exit, exit.code()), (Exit::Done, 0));
        assert_eq!(printed, as_before(&ok));

        let refused = ControlResponse {
            ok: false,
            protocol_version: 1,
            data: None,
            error: Some("workspace_not_found: Select an existing workspace explicitly".into()),
        };
        let (exit, printed) = print(&refused);
        assert_eq!((exit, exit.code()), (Exit::Failed, 1));
        // stdout is unchanged, so scripts still read the error from the reply.
        assert_eq!(printed, as_before(&refused));
        let reply: Value = serde_json::from_str(&printed).unwrap();
        assert_eq!(reply["ok"], json!(false));
        assert_eq!(
            reply["error"],
            json!("workspace_not_found: Select an existing workspace explicitly")
        );
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

