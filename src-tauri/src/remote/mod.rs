//! The host side of `codemux-remote serve` and `codemux-remote mcp`.
//!
//! Self-contained subsystem that runs on a managed host (VPS, home
//! server, anywhere `codemux-remote` is installed) with no Tauri/UI
//! dependency. It lets an agent on that host manage the host's own
//! folders, git worktrees and shells through MCP; the workspaces it
//! registers show up under Devices in the Codemux app. It is not the
//! desktop's `codemux mcp`: it has its own tools, does not run Codemux
//! chats and does not control the Codemux app. It listens on loopback
//! only, so it is not a door into the app from other machines either.
//!
//! Design constraints baked in for a future optional cloud relay
//! (paid tier, not on the v1 roadmap):
//!
//! 1. **HTTP transport, not Unix-socket JSON-lines.** A future relay
//!    can forward HTTP through a WebSocket tunnel without the daemon
//!    knowing. Same dispatcher whether the caller is local, SSH-tunnelled
//!    desktop, or cloud-routed.
//! 2. **`Identity` argument on every handler.** Today every request
//!    tags as `Identity::Local` (any caller with the manifest secret).
//!    Tomorrow a relay verifies a JWT, attaches `Identity::Cloud { … }`,
//!    handler signatures don't change.
//! 3. **Bearer-token auth via manifest.json.** 32-byte secret in the
//!    `<state-dir>/manifest.json`, mode 0600. Loopback HTTP only by
//!    default; SSH tunnels are the path for remote access in v1.
//! 4. **No Better-Auth / VPS-side coupling.** The daemon trusts its
//!    local secret, full stop. A future relay would be a separate
//!    binary on the VPS that talks to Better Auth and forwards a
//!    trusted identity header to the daemon.
//!
//! Unix-only — codemux-remote itself is Unix-only (the existing
//! pty_daemon::server uses `tokio::net::UnixListener`). The serve
//! mode reuses that constraint.

#![cfg(unix)]

pub mod auth;
pub mod checkout;
pub mod config;
pub mod git;
pub mod host_status;
pub mod identity;
pub mod manifest;
pub mod mcp;
pub mod mcp_register;
pub mod pty;
pub mod server;
pub mod tools;
pub mod tasks;
pub mod workspace;
