//! Typed control of application-owned native threads.
pub mod catalog;
mod types;
pub(crate) mod access;
pub(crate) mod error;
pub(crate) mod schema;
pub(crate) mod permissions;
pub(crate) mod native;
mod state;
pub(crate) mod service;
mod guard;
pub mod delivery;
pub mod registry;
pub(crate) mod wait;
pub(crate) mod outcome;
pub(crate) mod workspace_tools;
pub use guard::NativeControlGuard;
pub(crate) use guard::GuardedTurnCheckpoint;
pub(crate) use guard::NativeStartCancellation;
pub use state::{NativeControlState, PendingApproval, ThreadRuntimeView};
pub(crate) use state::ThreadGate;
pub(crate) use types::AdmittedAuthority;
pub use access::{Access, Audience, CallerKind, CallerView};
pub use error::{ControlError, ErrorCode};
pub use types::{CallerPrincipal, ClientName, ControlAccess, ControlCaller};
pub use catalog::INSTRUCTIONS;

/// Runs a registry tool on the explicit-argument route: the socket aliases,
/// HTTP and in-process callers such as the plugin platform.
pub async fn execute<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    caller: &ControlCaller,
    tool: &str,
    args: serde_json::Value,
) -> Result<serde_json::Value, ControlError> {
    registry::dispatch(app, caller, registry::Route::Outside, tool, args).await
}

#[cfg(test)]
pub(crate) mod tests;
#[cfg(test)]
mod wait_tests;
#[cfg(test)]
mod workspace_tests;
#[cfg(test)]
mod docs_tests;
#[cfg(test)]
pub(crate) mod guard_tests;

pub mod selection;
