//! Tauri commands for the cross-device workspace sync surface.
//!
//! The frontend reads `workspaces_sync_list` to render the
//! Workspaces overview — including workspaces that live on *other*
//! devices of the same account (rows with `workspace_id: null`).
//! Sibling-device rows show up as "lives on another device" cards
//! and can be adopted into this device via `workspaces_sync_adopt`.

use serde::Serialize;
use tauri::{Manager, State};

use crate::database::{DatabaseStore, WorkspaceSyncRecord};

/// Wire shape sent to the frontend. Matches the field naming the
/// existing UI expects (snake_case from Rust, camelCase from
/// serde-rename on JS-facing structs would be inconsistent with the
/// rest of the codebase — keep snake_case throughout).
#[derive(Debug, Clone, Serialize)]
pub struct WorkspaceSyncView {
    /// Local sync-table row id. Stable across pulls within a device.
    pub id: i64,
    /// Server-assigned id. None until the row's first successful push.
    pub server_id: Option<String>,
    /// Local workspace_id this row corresponds to. None for rows
    /// pulled from sibling devices that haven't been adopted here.
    pub workspace_id: Option<String>,
    pub title: String,
    /// Server-side host id (matches `HostView.server_id`). None means
    /// the workspace is local to the device that authored it.
    pub host_server_id: Option<String>,
    pub project_path: Option<String>,
    pub project_remote: Option<String>,
    pub git_branch: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// True when the row has unpushed changes. The UI shows a
    /// "Pending sync" pill for these.
    pub dirty: bool,
}

impl From<WorkspaceSyncRecord> for WorkspaceSyncView {
    fn from(r: WorkspaceSyncRecord) -> Self {
        WorkspaceSyncView {
            id: r.id,
            server_id: r.server_id,
            workspace_id: r.workspace_id,
            title: r.title,
            host_server_id: r.host_server_id,
            project_path: r.project_path,
            project_remote: r.project_remote,
            git_branch: r.git_branch,
            created_at: r.created_at,
            updated_at: r.updated_at,
            dirty: r.dirty,
        }
    }
}

/// List every workspace this account knows about (across all devices)
/// that this device has either authored or pulled from the server.
/// Tombstones are excluded — only live rows are returned.
///
/// Rendering rules for the UI:
/// - `workspace_id` set → the workspace also exists in this device's
///   app_state; cross-reference to render the rich local card.
/// - `workspace_id` null → lives on another device; render a
///   minimal "remote" card with title + host + branch and offer the
///   "Pull to this device" affordance (when implemented).
#[tauri::command]
pub fn workspaces_sync_list(
    db: State<'_, DatabaseStore>,
) -> Vec<WorkspaceSyncView> {
    db.list_workspaces_sync()
        .into_iter()
        .map(Into::into)
        .collect()
}

/// Trigger an immediate sync pass (pull + push). Returns when both
/// halves have finished or one has errored. Use sparingly — the
/// background loop already runs every 30 seconds.
///
/// Returns Ok(()) when the user is not signed in (sync is a no-op,
/// not an error in that case — matches the hosts and automations
/// commands).
#[tauri::command]
pub async fn workspaces_sync_now(
    app: tauri::AppHandle,
) -> Result<(), String> {
    // Snapshot + reconcile in a sync block so we don't carry the
    // State<'_> guard (which is not Send) across the await below.
    {
        let db_state: tauri::State<'_, DatabaseStore> = app.state();
        let app_state: tauri::State<'_, crate::state::AppStateStore> =
            app.state();
        let snapshot = app_state.snapshot();
        crate::workspaces_sync::reconcile_from_snapshot(
            db_state.inner(),
            &snapshot,
        )?;
    }
    crate::workspaces_sync::try_sync_with_app(&app).await
}
