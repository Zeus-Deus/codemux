//! Shared Agent Client Protocol runtime used by CLI-backed providers.
//!
//! Cursor Agent and Grok Build both speak ACP over newline-delimited
//! JSON-RPC, but differ in their launch commands, authentication, model
//! controls, and vendor extensions.  The transport/session machinery lives
//! here so those protocol-critical paths (queueing, cancellation, permission
//! requests, and stream ordering) stay identical across both drivers.

pub mod protocol;
pub mod session;
