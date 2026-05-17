//! `codemux pty-daemon` — long-lived PTY supervisor that owns the master
//! fd outside the Tauri app's address space.
//!
//! The whole point: when the Tauri app exits, the daemon survives and the
//! PTYs it owns survive with it. On the next Tauri launch, we adopt the
//! daemon and reattach to the live sessions.
//!
//! See:
//! - `protocol.rs` — wire types
//! - `server.rs`   — the daemon binary's main loop
//! - `client.rs`   — Tauri-side socket client
//! - `manifest.rs` — adoption hint file on disk
//! - `supervisor.rs` — spawn-detached + adoption boot pattern
//!
//! Cross-platform status: Unix complete. Windows (named pipes + DETACHED
//! creation flags) scaffolded but not yet validated on a real Windows box.

pub mod client;
pub mod manifest;
pub mod protocol;
pub mod server;
pub mod supervisor;

pub use client::{PtyDaemonClient, PtyDaemonError};
pub use protocol::{DaemonSessionInfo, PROTOCOL_VERSION};
pub use supervisor::ensure_daemon;
