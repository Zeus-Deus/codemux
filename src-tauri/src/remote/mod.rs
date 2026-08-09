//! Headless Codemux for `codemux-remote serve`.
//!
//! Self-contained subsystem that runs on a remote host (VPS, home
//! server, anywhere `codemux-remote` is installed) and exposes the
//! same shape of MCP tool surface the desktop's `codemux mcp` does,
//! so an agent on that host can drive Codemux locally — create
//! workspaces, list them, write to terminals — without any
//! Tauri/UI dependency.
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
pub mod config;
pub mod git;
pub mod identity;
pub mod manifest;
pub mod mcp;
pub mod mcp_register;
pub mod pty;
pub mod server;
pub mod tools;
pub mod workspace;
