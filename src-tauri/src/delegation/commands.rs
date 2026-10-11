use super::types::*;
pub fn is_command(name: &str) -> bool {
    matches!(
        name,
        "delegation_host_info"
            | "delegation_grants"
            | "delegation_authorize"
            | "delegation_revoke"
            | "delegate_task"
            | "delegation_list"
            | "delegation_read"
            | "delegation_cancel"
            | "delegation_respond"
            | "delegation_deliver"
    )
}
use serde_json::json;
use tauri::{AppHandle, Runtime};
#[tauri::command]
pub async fn delegation_host_info<R: Runtime>(
    app: AppHandle<R>,
    host_id: i64,
) -> Result<crate::remote::tasks::TaskCapabilities, String> {
    serde_json::from_value(
        super::app::invoke(&app, "delegation_host_info", json!({"hostId":host_id})).await?,
    )
    .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn delegation_grants<R: Runtime>(
    app: AppHandle<R>,
    pane_id: String,
) -> Result<Vec<Grant>, String> {
    serde_json::from_value(
        super::app::invoke(&app, "delegation_grants", json!({"paneId":pane_id})).await?,
    )
    .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn delegation_authorize<R: Runtime>(
    app: AppHandle<R>,
    pane_id: String,
    input: AuthorizeDelegationInput,
) -> Result<Grant, String> {
    serde_json::from_value(
        super::app::invoke(
            &app,
            "delegation_authorize",
            json!({"paneId":pane_id,"input":input}),
        )
        .await?,
    )
    .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn delegation_revoke<R: Runtime>(
    app: AppHandle<R>,
    pane_id: String,
    target_id: String,
) -> Result<(), String> {
    serde_json::from_value(
        super::app::invoke(
            &app,
            "delegation_revoke",
            json!({"paneId":pane_id,"targetId":target_id}),
        )
        .await?,
    )
    .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn delegate_task<R: Runtime>(
    app: AppHandle<R>,
    pane_id: String,
    input: DelegateTaskInput,
) -> Result<LocalTask, String> {
    serde_json::from_value(
        super::app::invoke(
            &app,
            "delegate_task",
            json!({"paneId":pane_id,"input":input}),
        )
        .await?,
    )
    .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn delegation_list<R: Runtime>(
    app: AppHandle<R>,
    pane_id: String,
) -> Result<Vec<LocalTask>, String> {
    serde_json::from_value(
        super::app::invoke(&app, "delegation_list", json!({"paneId":pane_id})).await?,
    )
    .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn delegation_read<R: Runtime>(
    app: AppHandle<R>,
    pane_id: String,
    task_id: String,
    cursor: i64,
) -> Result<TaskRead, String> {
    serde_json::from_value(
        super::app::invoke(
            &app,
            "delegation_read",
            json!({"paneId":pane_id,"taskId":task_id,"cursor":cursor}),
        )
        .await?,
    )
    .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn delegation_cancel<R: Runtime>(
    app: AppHandle<R>,
    pane_id: String,
    task_id: String,
) -> Result<LocalTask, String> {
    serde_json::from_value(
        super::app::invoke(
            &app,
            "delegation_cancel",
            json!({"paneId":pane_id,"taskId":task_id}),
        )
        .await?,
    )
    .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn delegation_respond<R: Runtime>(
    app: AppHandle<R>,
    pane_id: String,
    task_id: String,
    request_id: String,
    decision: crate::agent_provider::ApprovalDecision,
    original_request: Option<crate::remote::tasks::RemoteApproval>,
) -> Result<LocalTask, String> {
    serde_json::from_value(
        super::app::invoke(
            &app,
            "delegation_respond",
            json!({"paneId":pane_id,"taskId":task_id,"requestId":request_id,"decision":decision,"originalRequest":original_request}),
        )
        .await?,
    )
    .map_err(|e| e.to_string())
}
#[tauri::command]
pub async fn delegation_deliver<R: Runtime>(
    app: AppHandle<R>,
    pane_id: String,
    task_id: String,
) -> Result<LocalTask, String> {
    serde_json::from_value(
        super::app::invoke(
            &app,
            "delegation_deliver",
            json!({"paneId":pane_id,"taskId":task_id}),
        )
        .await?,
    )
    .map_err(|e| e.to_string())
}
