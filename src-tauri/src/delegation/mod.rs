//! App-owned cross-host tasks. Consent is local UI state, never MCP metadata.
pub mod app;
pub(crate) mod authority;
pub mod commands;
pub mod coordinator;
pub mod journal;
pub mod permissions;
pub mod transport;
pub mod types;
pub use types::*;
#[cfg(test)]
mod integration_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod ownership_tests;
