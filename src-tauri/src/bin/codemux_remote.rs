//! `codemux-remote` — server-side binary.
//!
//! Runs on the remote host the laptop pushes workspaces to, **and**
//! (new in this revision) hosts a headless Codemux daemon so an
//! agent running on the host can drive Codemux through MCP — create
//! workspaces, list them, write to terminals — without any UI.
//!
//! Subcommands:
//!
//! - `pty-daemon --socket <path>` — the original Unix-socket PTY
//!   daemon Codemux pushes workspaces into. Unchanged.
//! - `scheduler` — automation scheduler. Unchanged.
//! - `serve` — long-running headless daemon, axum HTTP on 127.0.0.1
//!   with bearer auth from a manifest. New in this revision.
//! - `serve status` — read the manifest and report whether the
//!   daemon is up. New.
//! - `serve stop` — find the daemon via the manifest and SIGTERM it.
//!   New.
//! - `mcp` — stdio MCP server that talks to the local `serve`
//!   daemon over HTTP. Configure your CLI agent to launch this. New.
//! - `version` — JSON version string. The laptop's bootstrap probe
//!   parses it. Unchanged.
//!
//! Unix-only by design: the existing PTY daemon uses Unix-domain
//! sockets and the new `serve` mode wraps headless features that
//! were never wired for Windows. The Windows build path is a no-op
//! stub at the top of this file.

#![cfg_attr(not(unix), allow(unused_imports, dead_code))]

#[cfg(not(unix))]
fn main() -> std::process::ExitCode {
    eprintln!("codemux-remote is a Unix-only binary (daemon uses Unix sockets).");
    eprintln!("Building it on Windows produces this no-op stub. The cloud-push");
    eprintln!("feature requires a Unix-side daemon on the remote host.");
    std::process::ExitCode::from(1)
}

#[cfg(unix)]
use std::path::PathBuf;
#[cfg(unix)]
use std::process::ExitCode;

#[cfg(unix)]
use clap::{Parser, Subcommand};

/// Codemux remote agent.
#[cfg(unix)]
#[derive(Parser)]
#[command(
    name = "codemux-remote",
    version,
    about = "Headless Codemux daemon + PTY proxy for remote hosts.",
    long_about = "Runs on the remote host the laptop's Codemux pushes \
                  workspaces to, and (with `serve`) hosts a headless \
                  Codemux daemon so an agent on the host can drive \
                  Codemux through MCP."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[cfg(unix)]
#[derive(Subcommand)]
enum Command {
    /// Run as the PTY daemon, binding a Unix socket at `--socket`.
    /// This is what the laptop's SSH bootstrap runs.
    PtyDaemon {
        /// Absolute path of the Unix socket to bind.
        #[arg(long)]
        socket: PathBuf,
    },
    /// Run the automation scheduler: poll the account for this host's
    /// automations, fire those that are due, and run the agent.
    Scheduler,
    /// Print version info as JSON. The laptop's bootstrap probe uses
    /// this to confirm a working installation before attempting a
    /// daemon start.
    Version,
    /// Run the headless Codemux daemon (axum HTTP on loopback,
    /// bearer auth from manifest). An agent on this host points its
    /// MCP client at `codemux-remote mcp` to drive it.
    Serve(ServeArgs),
    /// Run the stdio MCP server that bridges an agent CLI to the
    /// local `serve` daemon over HTTP.
    Mcp {
        /// State directory the daemon used. Defaults to the same
        /// path `serve` defaults to (~/.local/share/codemux-remote).
        #[arg(long)]
        state_dir: Option<PathBuf>,
    },
    /// Workspace-registry helpers that talk to the local `serve`
    /// daemon over loopback HTTP. Used by the desktop's push flow
    /// (run remotely via SSH) so a pushed workspace shows up in the
    /// daemon's `workspace_list` without any manual MCP call.
    Workspace {
        #[command(subcommand)]
        subcommand: WorkspaceSubcommand,
    },
}

#[cfg(unix)]
#[derive(Subcommand)]
enum WorkspaceSubcommand {
    /// Register a workspace in the daemon's registry. Calls the
    /// `workspace_create` tool on the local daemon. Exits 0 with the
    /// workspace id printed to stdout (JSON) on success.
    Register {
        /// Absolute path of the workspace's working directory.
        #[arg(long)]
        path: String,
        /// Human-readable name. Defaults to basename of `--path`.
        #[arg(long)]
        name: Option<String>,
        /// Git branch (optional).
        #[arg(long)]
        branch: Option<String>,
        /// Originating project root if this is a worktree (optional).
        #[arg(long)]
        project_root: Option<String>,
        /// State directory of the daemon. Defaults to the same path
        /// `serve` defaults to.
        #[arg(long)]
        state_dir: Option<PathBuf>,
        /// Retry connecting to the daemon for up to this many seconds
        /// before giving up. Useful right after `systemctl --user
        /// start codemux-remote` when the daemon may still be coming
        /// up.
        #[arg(long, default_value = "10")]
        connect_timeout_secs: u64,
    },
    /// Print every workspace in the daemon's SQLite registry as JSON
    /// on stdout. Reads the database directly — no running daemon is
    /// required. The desktop's host-inventory poller invokes this
    /// over SSH so workspaces created on the host (via MCP tools or
    /// the desktop's push flow) become visible across the user's
    /// account without an explicit push from each device.
    ///
    /// Stable contract: stdout is exactly one JSON object of shape
    /// `{"host_id":"<gethostname>","workspaces":[<Workspace>,...]}`,
    /// where each Workspace matches `remote::workspace::Workspace`.
    /// Stderr is unused on success; non-zero exit means the registry
    /// could not be opened.
    List {
        /// State directory of the daemon. Defaults to the same path
        /// `serve` defaults to.
        #[arg(long)]
        state_dir: Option<PathBuf>,
    },
}

#[cfg(unix)]
#[derive(clap::Args)]
struct ServeArgs {
    #[command(subcommand)]
    subcommand: Option<ServeSubcommand>,

    /// Explicit port. Defaults to picking an ephemeral free port and
    /// recording it in the manifest.
    #[arg(long)]
    port: Option<u16>,

    /// State directory the daemon writes manifest / db / log under.
    /// Defaults to ~/.local/share/codemux-remote.
    #[arg(long)]
    state_dir: Option<PathBuf>,
}

#[cfg(unix)]
#[derive(Subcommand)]
enum ServeSubcommand {
    /// Print the daemon's manifest (endpoint, pid, started_at, …) if
    /// a manifest exists and the pid is alive. Exits 1 if not.
    Status {
        #[arg(long)]
        state_dir: Option<PathBuf>,
    },
    /// Send SIGTERM to the running daemon (read from the manifest).
    Stop {
        #[arg(long)]
        state_dir: Option<PathBuf>,
    },
}

#[cfg(unix)]
fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        None | Some(Command::Version) => {
            let payload = serde_json::json!({
                "name": "codemux-remote",
                "version": env!("CARGO_PKG_VERSION"),
                "protocol_version": codemux_lib::pty_daemon::PROTOCOL_VERSION,
            });
            println!("{}", payload);
            ExitCode::SUCCESS
        }
        Some(Command::PtyDaemon { socket }) => run_pty_daemon(socket),
        Some(Command::Scheduler) => run_scheduler(),
        Some(Command::Serve(args)) => {
            // Subcommand short-circuits: `serve status`, `serve stop`.
            if let Some(sub) = args.subcommand {
                return match sub {
                    ServeSubcommand::Status { state_dir } => run_serve_status(state_dir),
                    ServeSubcommand::Stop { state_dir } => run_serve_stop(state_dir),
                };
            }
            run_serve(args.port, args.state_dir)
        }
        Some(Command::Mcp { state_dir }) => run_mcp(state_dir),
        Some(Command::Workspace { subcommand }) => match subcommand {
            WorkspaceSubcommand::Register {
                path,
                name,
                branch,
                project_root,
                state_dir,
                connect_timeout_secs,
            } => run_workspace_register(
                path,
                name,
                branch,
                project_root,
                state_dir,
                connect_timeout_secs,
            ),
            WorkspaceSubcommand::List { state_dir } => run_workspace_list(state_dir),
        },
    }
}

#[cfg(unix)]
fn run_pty_daemon(socket: PathBuf) -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(error) => {
            eprintln!("[codemux-remote] tokio runtime: {error}");
            return ExitCode::from(2);
        }
    };
    let result = runtime.block_on(codemux_lib::pty_daemon::server::run(socket));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("[codemux-remote] daemon: {error}");
            ExitCode::from(1)
        }
    }
}

#[cfg(unix)]
fn run_scheduler() -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(error) => {
            eprintln!("[codemux-remote] tokio runtime: {error}");
            return ExitCode::from(2);
        }
    };
    runtime.block_on(scheduler_loop())
}

#[cfg(unix)]
async fn scheduler_loop() -> ExitCode {
    use codemux_lib::automations::{executor, scheduler};

    let db = match codemux_lib::database::init_database() {
        Ok(db) => db,
        Err(error) => {
            eprintln!("[codemux-remote] database: {error}");
            return ExitCode::from(1);
        }
    };

    let ceiling = (chrono::Utc::now() - chrono::Duration::hours(6)).to_rfc3339();
    let reconciled = db.reconcile_stale_runs(&ceiling);
    if reconciled > 0 {
        eprintln!("[codemux-remote] reconciled {reconciled} stale run(s)");
    }

    let token = read_scheduler_token();
    if token.is_none() {
        eprintln!(
            "[codemux-remote] no scheduler token found; running only \
             automations already in the local database"
        );
    }
    let host_id = read_scheduler_host();
    if token.is_some() && host_id.is_none() {
        eprintln!(
            "[codemux-remote] no host identity found; the account pull \
             cannot be host-scoped — skipping it to avoid running other \
             hosts' automations"
        );
    }
    eprintln!("[codemux-remote] automation scheduler started");

    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(
        scheduler::TICK_INTERVAL_SECS,
    ));
    ticker.tick().await;

    loop {
        ticker.tick().await;

        if let (Some(token), Some(host_id)) = (&token, &host_id) {
            if let Err(error) =
                codemux_lib::automations_sync::pull(token, &db, Some(host_id)).await
            {
                eprintln!("[codemux-remote] automation pull failed: {error}");
            }
        }

        let fired = scheduler::tick(&db, chrono::Utc::now(), false);
        for run in fired {
            if let Some(automation) = db.get_automation(run.automation_id) {
                let _ = db.mark_automation_run_started(run.id);
                let outcome = executor::run_fire(&automation).await;
                executor::apply_outcome(&db, run.id, &outcome);
            }
        }
    }
}

#[cfg(unix)]
fn read_scheduler_token() -> Option<String> {
    read_scheduler_file("scheduler-token")
}

#[cfg(unix)]
fn read_scheduler_host() -> Option<String> {
    read_scheduler_file("scheduler-host")
}

#[cfg(unix)]
fn read_scheduler_file(name: &str) -> Option<String> {
    let path = dirs::data_dir()?.join("codemux").join(name);
    std::fs::read_to_string(path)
        .ok()
        .map(|contents| contents.trim().to_string())
        .filter(|value| !value.is_empty())
}

// ─── New: `serve` / `mcp` subcommands ────────────────────────────

#[cfg(unix)]
fn resolve_state_dir(arg: Option<PathBuf>) -> PathBuf {
    arg.unwrap_or_else(codemux_lib::remote::config::default_state_dir)
}

#[cfg(unix)]
fn run_serve(port: Option<u16>, state_dir_arg: Option<PathBuf>) -> ExitCode {
    let state_dir = resolve_state_dir(state_dir_arg);

    // Singleton check: if a live daemon already owns the manifest,
    // refuse to start a second one. A stale manifest (pid not alive)
    // is overwritten.
    let manifest_path = codemux_lib::remote::config::manifest_path(&state_dir);
    if let Ok(Some(existing)) = codemux_lib::remote::manifest::read(&manifest_path) {
        if codemux_lib::remote::manifest::pid_alive(existing.pid) {
            eprintln!(
                "[codemux-remote] another daemon already running (pid {}, endpoint {}). \
                 Use `codemux-remote serve stop` first.",
                existing.pid, existing.endpoint
            );
            return ExitCode::from(2);
        }
        eprintln!(
            "[codemux-remote] stale manifest from pid {} (not alive); replacing",
            existing.pid
        );
        let _ = codemux_lib::remote::manifest::remove(&manifest_path);
    }

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("[codemux-remote] tokio runtime: {e}");
            return ExitCode::from(2);
        }
    };

    let result = runtime.block_on(async move { run_serve_async(port, state_dir).await });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("[codemux-remote] serve: {e}");
            ExitCode::from(1)
        }
    }
}

#[cfg(unix)]
async fn run_serve_async(port: Option<u16>, state_dir: PathBuf) -> Result<(), String> {
    use codemux_lib::remote::{config, manifest, pty::PtyManager, server, workspace::WorkspaceStore};

    std::fs::create_dir_all(&state_dir).map_err(|e| format!("create state dir: {e}"))?;

    // Bind first so we know what port to write into the manifest.
    let listener = server::bind_listener(port).await?;
    let local_addr = listener.local_addr().map_err(|e| format!("local_addr: {e}"))?;
    let endpoint = format!("http://{}", local_addr);

    let host_id = manifest::current_host_id();
    let workspaces = WorkspaceStore::open(
        &config::database_path(&state_dir),
        host_id.clone(),
        config::workspaces_root(&state_dir),
    )
    .map_err(|e| format!("workspace store: {e}"))?;

    // Idempotent: backfill first-class project identity (project_uid /
    // name / kind / remote) onto any rows registered before those
    // columns existed. Does real work only once, right after an
    // upgrade; a no-op on every subsequent boot.
    match workspaces.sweep_backfill_identity() {
        Ok(0) => {}
        Ok(n) => eprintln!("[codemux-remote] backfilled project identity for {n} workspace(s)"),
        Err(e) => eprintln!("[codemux-remote] project-identity backfill skipped: {e}"),
    }

    let manifest_value = manifest::Manifest::new(endpoint.clone(), host_id);
    let manifest_path = config::manifest_path(&state_dir);
    manifest::write(&manifest_path, &manifest_value)?;
    eprintln!(
        "[codemux-remote] manifest written to {} (secret length {})",
        manifest_path.display(),
        manifest_value.secret.len()
    );

    // Best-effort: register `codemux-remote mcp` in known agent
    // MCP configs (~/.claude.json, ~/.vexis/mcp-servers.yaml) so
    // any agent CLI the user runs on this host already knows about
    // Codemux without manual config edits. Runs on every daemon
    // start because it's idempotent — if the entries are already
    // correct, no writes happen.
    //
    // The function we call is defensive: malformed config files
    // are logged and skipped, never overwritten. Missing
    // agent-specific dirs (~/.vexis nonexistent) are skipped
    // silently — we never create an opt-in we weren't asked for.
    let exec_path = std::env::current_exe()
        .unwrap_or_else(|_| std::path::PathBuf::from("codemux-remote"));
    let report = codemux_lib::remote::mcp_register::ensure_codemux_in_agent_configs(&exec_path);
    if !report.modified.is_empty() {
        eprintln!(
            "[codemux-remote] auto-registered codemux MCP in: {}",
            report
                .modified
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    for (path, reason) in &report.failed {
        eprintln!(
            "[codemux-remote] could not register in {} (skipped): {reason}",
            path.display()
        );
    }

    let started_at = manifest_value.started_at.clone();
    let secret = manifest_value.secret.clone();

    let state = std::sync::Arc::new(server::DaemonState {
        secret,
        started_at,
        workspaces,
        ptys: PtyManager::new(),
    });

    let app = server::router(std::sync::Arc::clone(&state));

    // SIGTERM/SIGINT-aware graceful shutdown so the manifest gets
    // cleaned up. On signal: drop the listener, then unlink the
    // manifest file. axum::serve with `with_graceful_shutdown`
    // does the rest.
    let manifest_path_for_shutdown = manifest_path.clone();
    let shutdown = async move {
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("install SIGTERM handler");
        let mut sigint =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
                .expect("install SIGINT handler");
        tokio::select! {
            _ = sigterm.recv() => eprintln!("[codemux-remote] SIGTERM, shutting down"),
            _ = sigint.recv()  => eprintln!("[codemux-remote] SIGINT, shutting down"),
        }
        let _ = manifest::remove(&manifest_path_for_shutdown);
    };

    eprintln!("[codemux-remote] listening on {endpoint}");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await
        .map_err(|e| format!("axum serve: {e}"))?;
    eprintln!("[codemux-remote] shutdown complete");
    Ok(())
}

#[cfg(unix)]
fn run_serve_status(state_dir_arg: Option<PathBuf>) -> ExitCode {
    let state_dir = resolve_state_dir(state_dir_arg);
    let manifest_path = codemux_lib::remote::config::manifest_path(&state_dir);
    match codemux_lib::remote::manifest::read(&manifest_path) {
        Ok(None) => {
            eprintln!(
                "[codemux-remote] no daemon manifest at {}. Not running.",
                manifest_path.display()
            );
            ExitCode::from(1)
        }
        Ok(Some(m)) => {
            let alive = codemux_lib::remote::manifest::pid_alive(m.pid);
            let payload = serde_json::json!({
                "endpoint": m.endpoint,
                "pid": m.pid,
                "started_at": m.started_at,
                "host_id": m.host_id,
                "alive": alive,
            });
            println!("{}", payload);
            if alive {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Err(e) => {
            eprintln!("[codemux-remote] {e}");
            ExitCode::from(2)
        }
    }
}

#[cfg(unix)]
fn run_serve_stop(state_dir_arg: Option<PathBuf>) -> ExitCode {
    let state_dir = resolve_state_dir(state_dir_arg);
    let manifest_path = codemux_lib::remote::config::manifest_path(&state_dir);
    let manifest = match codemux_lib::remote::manifest::read(&manifest_path) {
        Ok(Some(m)) => m,
        Ok(None) => {
            eprintln!("[codemux-remote] no daemon to stop");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("[codemux-remote] {e}");
            return ExitCode::from(2);
        }
    };
    let pid = manifest.pid as libc::pid_t;
    // SAFETY: kill is a syscall with no memory side effects.
    let rc = unsafe { libc::kill(pid, libc::SIGTERM) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ESRCH) {
            eprintln!(
                "[codemux-remote] pid {pid} is gone; removing stale manifest"
            );
            let _ = codemux_lib::remote::manifest::remove(&manifest_path);
            return ExitCode::SUCCESS;
        }
        eprintln!("[codemux-remote] kill({pid}, SIGTERM): {err}");
        return ExitCode::from(1);
    }
    eprintln!("[codemux-remote] sent SIGTERM to pid {pid}");
    ExitCode::SUCCESS
}

#[cfg(unix)]
fn run_mcp(state_dir_arg: Option<PathBuf>) -> ExitCode {
    let state_dir = resolve_state_dir(state_dir_arg);
    match codemux_lib::remote::mcp::run_stdio(state_dir) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("[codemux-remote] mcp: {e}");
            ExitCode::from(1)
        }
    }
}

/// Implementation for `codemux-remote workspace register`. Reads the
/// local daemon's manifest, POSTs `workspace_create` to its loopback
/// HTTP endpoint, prints the new workspace's id+metadata as JSON on
/// stdout. Used by the desktop's push flow to register a freshly
/// pushed worktree on the remote.
///
/// Retries until `connect_timeout_secs` so the just-started systemd
/// unit has time to come up.
#[cfg(unix)]
fn run_workspace_register(
    path: String,
    name: Option<String>,
    branch: Option<String>,
    project_root: Option<String>,
    state_dir_arg: Option<PathBuf>,
    connect_timeout_secs: u64,
) -> ExitCode {
    let state_dir = resolve_state_dir(state_dir_arg);
    let manifest_path = codemux_lib::remote::config::manifest_path(&state_dir);

    // Wait for the manifest to appear AND the daemon to answer
    // /health. systemctl returns immediately when starting a unit;
    // the daemon's actual bind happens a tick later.
    let deadline = std::time::Instant::now()
        + std::time::Duration::from_secs(connect_timeout_secs.max(1));
    let client = match reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[codemux-remote] build http client: {e}");
            return ExitCode::from(2);
        }
    };

    let manifest = loop {
        match codemux_lib::remote::manifest::read(&manifest_path) {
            Ok(Some(m)) if codemux_lib::remote::manifest::pid_alive(m.pid) => {
                // Probe /health to catch the case where the manifest
                // is fresh but the listener hasn't accepted yet.
                let healthy = client
                    .get(format!("{}/health", m.endpoint))
                    .send()
                    .map(|r| r.status().is_success())
                    .unwrap_or(false);
                if healthy {
                    break m;
                }
            }
            Ok(_) | Err(_) => {}
        }
        if std::time::Instant::now() > deadline {
            eprintln!(
                "[codemux-remote] daemon at {} did not become healthy within {}s",
                manifest_path.display(),
                connect_timeout_secs
            );
            return ExitCode::from(1);
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    };

    // POST workspace_create.
    let url = format!("{}/tools/call", manifest.endpoint);
    let mut args = serde_json::Map::new();
    args.insert("path".into(), serde_json::Value::String(path));
    if let Some(n) = name {
        args.insert("name".into(), serde_json::Value::String(n));
    }
    if let Some(b) = branch {
        args.insert("branch".into(), serde_json::Value::String(b));
    }
    if let Some(p) = project_root {
        args.insert("project_root".into(), serde_json::Value::String(p));
    }
    let body = serde_json::json!({
        "name": "workspace_create",
        "arguments": serde_json::Value::Object(args),
    });

    let response = match client
        .post(&url)
        .bearer_auth(&manifest.secret)
        .json(&body)
        .send()
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[codemux-remote] POST {url}: {e}");
            return ExitCode::from(1);
        }
    };
    let status = response.status();
    let payload: serde_json::Value = match response.json() {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[codemux-remote] decode workspace_create: {e}");
            return ExitCode::from(1);
        }
    };
    if !status.is_success() || payload.get("ok") == Some(&serde_json::Value::Bool(false)) {
        eprintln!(
            "[codemux-remote] workspace_create failed (HTTP {status}): {payload}"
        );
        return ExitCode::from(1);
    }
    // Echo the workspace JSON to stdout so the desktop can parse +
    // record the new id.
    let workspace = payload
        .get("data")
        .and_then(|d| d.get("workspace"))
        .cloned()
        .unwrap_or(payload);
    println!("{}", workspace);
    ExitCode::SUCCESS
}

/// Implementation for `codemux-remote workspace list`. Opens the
/// daemon's SQLite registry directly (no HTTP, no running daemon
/// required) and prints `{"host_id":"...","workspaces":[...]}` to
/// stdout.
///
/// Used by the desktop's host-inventory poller: every ~60 seconds the
/// desktop SSHes into every configured host and runs this command, then
/// reconciles the result into its own `workspaces_sync` table so the
/// account-wide overview surfaces host-side workspaces without each
/// device having to push from itself.
///
/// We open the store read-only as far as workspaces go — we never call
/// `create`, so the `host_id` and `workspaces_root` args to
/// `WorkspaceStore::open` are only used to materialise the schema on
/// first run (and to create the workspaces root, which is harmless).
#[cfg(unix)]
fn run_workspace_list(state_dir_arg: Option<PathBuf>) -> ExitCode {
    use codemux_lib::remote::{config, manifest, workspace::WorkspaceStore};

    let state_dir = resolve_state_dir(state_dir_arg);
    let host_id = manifest::current_host_id();
    let store = match WorkspaceStore::open(
        &config::database_path(&state_dir),
        host_id.clone(),
        config::workspaces_root(&state_dir),
    ) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[codemux-remote] open workspace store at {}: {e}", state_dir.display());
            return ExitCode::from(1);
        }
    };
    let workspaces = match store.list() {
        Ok(w) => w,
        Err(e) => {
            eprintln!("[codemux-remote] list workspaces: {e}");
            return ExitCode::from(1);
        }
    };
    let payload = serde_json::json!({
        "host_id": host_id,
        "workspaces": workspaces,
    });
    println!("{}", payload);
    ExitCode::SUCCESS
}
