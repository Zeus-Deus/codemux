//! Shared Agent Client Protocol runtime used by CLI-backed providers.
//!
//! Cursor Agent, Grok Build and Google Antigravity all speak ACP over
//! newline-delimited JSON-RPC, but differ in their launch commands,
//! authentication, model controls, and vendor extensions. The
//! transport/session machinery lives here so those protocol-critical paths
//! (queueing, cancellation, permission requests, and stream ordering) stay
//! identical across the drivers.

pub mod mcp;
pub mod protocol;
pub mod session;
pub mod slash_commands;
