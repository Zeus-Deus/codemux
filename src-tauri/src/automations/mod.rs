//! Automations — scheduled agent runs on a chosen host.
//!
//! An automation is a named prompt + agent + trigger. A scheduled
//! automation fires on its recurrence, on a host the user picks; a webhook
//! automation fires when its URL receives a request (`webhook`). Each fire
//! creates a fresh workspace and runs the agent in it.
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
pub mod service;
pub mod webhook;
