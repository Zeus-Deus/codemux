//! Tauri commands for the Hosts feature (Settings → Hosts).
//!
//! These wrap the `DatabaseStore` CRUD with the right error shape for
//! the frontend. Sync push is fire-and-forget after each mutation:
//! every successful write triggers a background `hosts_sync::push` so
//! the user's other devices see the change within seconds. If sync
//! fails (offline, server down), the row stays marked `dirty` locally
//! and `hosts_sync::pull` will retry on next foreground.
//!
//! SSH credentials are NEVER part of any payload here. The frontend
//! only sends `name` + `ssh_target`; auth is the OS's job
//! (`~/.ssh/config`, agent, keys).

use crate::database::{DatabaseStore, HostRecord};
use serde::{Deserialize, Serialize};
use tauri::State;

#[derive(Debug, Serialize, Deserialize)]
pub struct HostView {
    pub id: i64,
    pub server_id: Option<String>,
    pub name: String,
    pub ssh_target: String,
    pub created_at: String,
    pub updated_at: String,
    pub dirty: bool,
}

impl From<HostRecord> for HostView {
    fn from(r: HostRecord) -> Self {
        Self {
            id: r.id,
            server_id: r.server_id,
            name: r.name,
            ssh_target: r.ssh_target,
            created_at: r.created_at,
            updated_at: r.updated_at,
            dirty: r.dirty,
        }
    }
}

#[tauri::command]
pub fn hosts_list(db: State<'_, DatabaseStore>) -> Vec<HostView> {
    db.list_hosts().into_iter().map(Into::into).collect()
}

#[tauri::command]
pub fn hosts_add(
    app: tauri::AppHandle,
    db: State<'_, DatabaseStore>,
    name: String,
    ssh_target: String,
) -> Result<HostView, String> {
    let name = name.trim().to_string();
    let ssh_target = ssh_target.trim().to_string();
    if name.is_empty() {
        return Err("Host name cannot be empty".into());
    }
    if ssh_target.is_empty() {
        return Err("SSH target cannot be empty".into());
    }
    if name.len() > 200 {
        return Err("Host name is too long (max 200 chars)".into());
    }
    if ssh_target.len() > 500 {
        return Err("SSH target is too long (max 500 chars)".into());
    }
    let record = db.insert_host(&name, &ssh_target)?;
    schedule_background_sync(app);
    Ok(record.into())
}

#[tauri::command]
pub fn hosts_update(
    app: tauri::AppHandle,
    db: State<'_, DatabaseStore>,
    id: i64,
    name: String,
    ssh_target: String,
) -> Result<HostView, String> {
    let name = name.trim().to_string();
    let ssh_target = ssh_target.trim().to_string();
    if name.is_empty() {
        return Err("Host name cannot be empty".into());
    }
    if ssh_target.is_empty() {
        return Err("SSH target cannot be empty".into());
    }
    let record = db.update_host(id, &name, &ssh_target)?;
    schedule_background_sync(app);
    Ok(record.into())
}

#[tauri::command]
pub fn hosts_delete(
    app: tauri::AppHandle,
    db: State<'_, DatabaseStore>,
    id: i64,
) -> Result<(), String> {
    db.delete_host(id)?;
    schedule_background_sync(app);
    Ok(())
}

/// Test whether the configured SSH target is reachable.
///
/// This is a stub for step 2a — actual SSH probing lands in step 2d
/// alongside the bootstrap flow. We return a clear "not implemented
/// yet" result so the UI can show a helpful message instead of a
/// hang. The frontend can already render the button + result panel
/// against this contract.
#[tauri::command]
pub fn hosts_test_connection(
    _db: State<'_, DatabaseStore>,
    id: i64,
) -> Result<HostTestResult, String> {
    let _ = id;
    Ok(HostTestResult {
        ok: false,
        message: "SSH connection testing ships in a follow-up. The host \
                  record is saved; transport wiring is the next step."
            .into(),
    })
}

#[derive(Debug, Serialize)]
pub struct HostTestResult {
    pub ok: bool,
    pub message: String,
}

/// Fire-and-forget background sync attempt. Reads the cached auth token
/// off-thread so the Tauri command returns immediately; if sync fails
/// the row stays `dirty` and the next foreground pull will retry. Never
/// errors back to the frontend — the local write already succeeded and
/// that's the user's mental model ("I added a host"). Sync failure is
/// a soft, recoverable condition we surface elsewhere (Settings →
/// Account → "Last synced N minutes ago").
fn schedule_background_sync(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        if let Err(error) = crate::hosts_sync::try_sync_with_app(&app).await {
            eprintln!("[codemux::hosts] background sync failed: {error}");
        }
    });
}
