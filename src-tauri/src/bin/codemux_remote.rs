//! `codemux-remote` — slim server-side binary.
//!
//! Runs on the remote host the laptop's Codemux pushes workspaces to.
//! Bundles only the PTY daemon (`codemux_lib::pty_daemon::server`) and
//! a tiny CLI wrapper — no Tauri, no webkit, no UI dependencies.
//!
//! Lifecycle:
//!
//! 1. The laptop SSHes in, scp's this binary (matched to the remote's
//!    arch + OS via `uname -sm`), and runs it under `ssh -L tunnel`.
//! 2. This process binds a Unix socket and accepts requests from the
//!    laptop's `PtyDaemonClient` — same wire protocol as the local
//!    daemon, so the client doesn't care whether it's talking over a
//!    local socket or an SSH-tunneled socket.
//! 3. Stays alive across the laptop's reconnects.  When the laptop is
//!    truly gone (host shutdown, manual stop), an idle reaper can
//!    shut us down — not yet implemented.
//!
//! Unix-only by design: the daemon's server uses `tokio::net::UnixListener`.
//! Windows servers can run as remote codemux targets once the daemon
//! grows named-pipe support — tracked alongside the desktop-side
//! Windows port.

// Unix-only — the daemon's server uses tokio::net::UnixListener which
// doesn't exist on Windows, and the cloud-push feature (the only thing
// that needs codemux-remote) is also `#[cfg(unix)]`. On Windows this
// binary compiles to a no-op stub below.
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
    about = "Slim PTY daemon Codemux pushes workspaces to.",
    long_about = "Runs on the remote host the laptop's Codemux pushes \
                  workspaces to. Same wire protocol as the local in-app \
                  daemon — the laptop's client doesn't distinguish."
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
    /// Print version info as JSON. The laptop's bootstrap probe uses
    /// this to confirm a working installation before attempting a
    /// daemon start.
    Version,
}

#[cfg(unix)]
fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        None | Some(Command::Version) => {
            // JSON form so the laptop's bootstrap can parse it
            // without grepping. Same shape Codemux uses for its
            // self-version reporting.
            let payload = serde_json::json!({
                "name": "codemux-remote",
                "version": env!("CARGO_PKG_VERSION"),
                "protocol_version": codemux_lib::pty_daemon::PROTOCOL_VERSION,
            });
            println!("{}", payload);
            ExitCode::SUCCESS
        }
        Some(Command::PtyDaemon { socket }) => run_daemon(socket),
    }
}

#[cfg(unix)]
fn run_daemon(socket: PathBuf) -> ExitCode {
    // Run the same server the in-app daemon uses. A failure inside
    // the listener (bind race, EMFILE under unusual load) is
    // surfaced to stderr so the laptop's `ssh ...` invocation sees
    // it; the process then exits non-zero so any keepalive script
    // notices.
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

#[cfg(not(unix))]
fn run_daemon(_socket: PathBuf) -> ExitCode {
    eprintln!(
        "[codemux-remote] PTY daemon mode is Unix-only for now. \
         Windows servers as Codemux push targets are tracked \
         alongside the desktop Windows port."
    );
    ExitCode::from(2)
}
