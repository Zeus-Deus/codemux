//! SSH transport for the cloud-push feature (step 2d).
//!
//! Three pieces:
//!
//! - `probe` — fast read-only check (reachable? `codemux-remote`
//!   installed?). Used by the "Test connection" button in
//!   Settings → Hosts and by the bootstrap-install flow.
//! - `bootstrap` — scp the architecture-matched `codemux-remote`
//!   binary to a host that doesn't have it, chmod, verify.
//! - `tunnel` — spawn `ssh -L <local>:<remote> ... codemux-remote
//!   pty-daemon` and expose the local Unix-socket path. The existing
//!   `PtyDaemonClient::connect(&local_path)` then works exactly as
//!   it does for the local in-app daemon — zero changes to the
//!   client code.
//!
//! Why shell out to the system `ssh` rather than using a Rust SSH
//! library (russh, libssh2): the user already has SSH configured
//! the way they want it — keys in `~/.ssh/`, ssh-agent running,
//! known_hosts populated, `Host` blocks in `~/.ssh/config`. Shelling
//! out reuses all of that without us having to re-implement
//! key-parsing, agent integration, or config-file parsing. The
//! tradeoff is process-spawn overhead per connect, which is
//! negligible for our cadence (a tunnel persists per workspace, not
//! per request).
//!
//! Unix-only: the bootstrap + tunnel paths use Unix sockets on the
//! laptop side. Windows support is gated alongside the rest of the
//! Windows cloud-push port.

#![cfg(unix)]

pub mod bootstrap;
pub mod opencode_db_sync;
pub mod probe;
pub mod push;
pub mod registry;
pub mod tunnel;
pub mod tunnel_supervisor;

pub use bootstrap::{bootstrap_remote, BootstrapResult};
pub use opencode_db_sync::{pull_opencode_session, sync_opencode_session};
pub use probe::{probe_host, ProbeOutcome};
pub use push::{
    claude_project_dir_name, conventional_remote_path,
    conventional_remote_path_keyed, pull_workspace_back, push_workspace,
    PullOptions, PullResult, PushOptions, PushResult,
};
pub use registry::{
    client_for_workspace, forget_workspace_client, get_supervisor,
    install_supervisor, local_socket_for_workspace,
    remote_socket_for_workspace, shutdown_supervisor,
    spawn_tunnel_status_forwarder,
};
pub use tunnel::{spawn_ssh_tunnel, TunnelHandle};
pub use tunnel_supervisor::{TunnelStatus, TunnelSupervisor};
