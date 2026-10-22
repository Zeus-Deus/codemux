//! Single test executable for the integration tests that are safe to share a
//! process: none of these modules mutate process environment variables, change
//! the working directory or re-execute the test binary.
//!
//! Every file directly under `tests/` is its own executable, and each one links
//! the whole `codemux_lib` crate plus its dependencies. Grouping these modules
//! into one target turns ~30 links of the full crate into one, which dominated
//! `cargo test` build time and target-dir size. Tests that need process
//! isolation (env/cwd mutation, self re-exec, or referenced by name from CI and
//! scripts) stay as separate files in `tests/`.
//!
//! Run one module with `cargo test --test integration <module>::`.

mod agent_delegation;
mod app_command_wiring_contract;
mod claude_adapter;
mod codemux_remote_binary;
mod codemux_remote_inventory;
mod codemux_remote_serve_mcp;
mod codemux_ssh_roundtrip;
mod codex_adapter;
mod cursor_adapter;
mod custom_acp_corrections;
mod custom_acp_definition;
mod custom_acp_runtime;
mod delegation_startup;
mod files_commands_ipc;
mod git_operations;
mod github_operations;
mod gitlab_live;
mod grok_adapter;
mod gui_leak_prevention;
mod managed_bridge_boundary;
mod managed_native_boundary;
mod managed_opencode_boundary;
mod mcp_bridge;
mod remote_task_protocol;
mod sidecar_ping;
mod sidecar_sdk;
mod skills_command;
mod skills_real_disk_smoke;
mod workflow_chat_script;
mod workflow_live_canary;
