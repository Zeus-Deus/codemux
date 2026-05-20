//! Automations — scheduled agent runs on a chosen host.
//!
//! An automation is a named prompt + agent + recurrence. The schedule
//! fires on a host the user picks; each fire creates a fresh workspace
//! and runs the agent in it.
//!
//! This module owns the recurrence engine. The rest of the feature is
//! split across the codebase by layer:
//!
//! - persistence — `database.rs` (`automations` / `automation_runs`)
//! - desktop command surface — `commands::automations`
//! - agent / MCP control surface — `control.rs` + `mcp_server.rs`
//!
//! Keeping recurrence isolated here lets the desktop (create-time
//! validation) and the host scheduler (the tick loop) call the exact
//! same logic.

pub mod executor;
pub mod recurrence;
pub mod scheduler;
