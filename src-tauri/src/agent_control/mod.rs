//! Typed control of application-owned native threads.
pub mod catalog;
mod types;
mod state;
mod service;
mod guard;
pub use guard::NativeControlGuard;
pub(crate) use guard::GuardedTurnCheckpoint;
pub(crate) use guard::NativeStartCancellation;
pub use state::{NativeControlState, PendingApproval, ThreadRuntimeView};
pub(crate) use state::ThreadGate;
pub use types::{ControlAccess, ControlCaller, ControlError};
pub use catalog::{is_native_tool, is_read_only, native_tools, remote_tools, INSTRUCTIONS};

pub async fn execute<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    caller: &ControlCaller,
    tool: &str,
    args: serde_json::Value,
) -> Result<serde_json::Value, ControlError> {
    use tauri::Manager;
    service::authorize(app, caller)?;
    match tool {
        "thread_launch" => return service::launch(app, caller, args).await,
        "operation_status" => return service::operation_status(app, caller, args),
        "thread_list" => return service::thread_list(app, args),
        "thread_read" => return service::thread_read(app, args),
        "thread_status" => return service::thread_status(app, args).await,
        "thread_send" => return service::thread_send(app, caller, args).await,
        "agent_capabilities" => return service::agent_capabilities(app, caller, args).await,
        "thread_interrupt" => return service::thread_interrupt(app, caller, args).await,
        "thread_stop" => return service::thread_stop(app, caller, args).await,
        "thread_wait" => return service::thread_wait(app, caller, args).await,
        "thread_respond" => return service::thread_respond(app, caller, args).await,
        _ => {}
    }
    if tool == "workspace_list" {
        if !args.as_object().is_some_and(|args| args.is_empty()) {
            return Err(ControlError::new("invalid_arguments", "workspace_list accepts an empty object"));
        }
        let state = app.state::<crate::state::AppStateStore>().snapshot();
        let workspaces: Vec<_> = state.workspaces.iter().map(|workspace| serde_json::json!({
            "workspace_id": workspace.workspace_id.0,
            "title": workspace.title,
            "cwd": workspace.cwd,
            "project_root": workspace.project_root,
            "branch": workspace.git_branch,
            "native_control_available": !workspace.is_local_import_snapshot_only() && workspace.host_id.is_none(),
        })).collect();
        return Ok(serde_json::json!({"workspaces": workspaces}));
    }
    Err(ControlError::new("unknown_tool", "Native control is not available"))
}

#[cfg(test)]
mod tests;
#[cfg(test)]
pub(crate) mod guard_tests;
