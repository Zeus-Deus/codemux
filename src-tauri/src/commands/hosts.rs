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

/// Assign (or clear) the host a workspace runs on. Used by the
/// workspace header badge + the future "Push to host" action. Passes
/// the host_id straight through to the in-memory `AppState`; the
/// snapshot persists via the normal save path.
#[tauri::command]
pub fn set_workspace_host(
    app: tauri::AppHandle,
    app_state: tauri::State<'_, crate::state::AppStateStore>,
    workspace_id: String,
    host_id: Option<i64>,
) -> Result<(), String> {
    app_state.set_workspace_host_id(&workspace_id, host_id)?;
    crate::state::emit_app_state(&app);
    Ok(())
}

/// Test whether the configured SSH target is reachable, and whether
/// `codemux-remote` is already installed there.
///
/// Three observable outcomes for the UI (maps directly to
/// `HostTestResult`):
/// - reachable + installed → green light, ready to push
/// - reachable + missing binary → trigger the bootstrap-install
///   consent modal
/// - unreachable → display the SSH error verbatim so the user can
///   debug their `~/.ssh/config` / network / key access
///
/// Unix-only — the underlying `ssh::probe` module is `#[cfg(unix)]`.
/// On Windows we return a clear "not yet implemented" message; the
/// rest of the UI degrades gracefully because the daemon path is
/// also disabled on Windows.
#[tauri::command]
pub async fn hosts_test_connection(
    db: State<'_, DatabaseStore>,
    id: i64,
) -> Result<HostTestResult, String> {
    // Look up the host record by local id so the frontend doesn't
    // have to round-trip the ssh_target.
    let host = db
        .list_hosts()
        .into_iter()
        .find(|h| h.id == id)
        .ok_or_else(|| format!("Host not found: {id}"))?;

    #[cfg(unix)]
    {
        use crate::ssh::probe::{probe_host, ProbeOptions, ProbeOutcome};
        let outcome = probe_host(ProbeOptions::new(&host.ssh_target)).await;
        Ok(match outcome {
            ProbeOutcome::Reachable {
                codemux_remote_version: Some(version),
                uname,
            } => HostTestResult {
                ok: true,
                message: format!(
                    "Connected. codemux-remote v{version} is installed{}",
                    uname
                        .map(|u| format!(" ({u})"))
                        .unwrap_or_default()
                ),
                needs_install: false,
                uname: None,
            },
            ProbeOutcome::Reachable {
                codemux_remote_version: None,
                uname,
            } => HostTestResult {
                ok: false,
                message: format!(
                    "Reachable, but codemux-remote isn't installed yet{}",
                    uname
                        .as_ref()
                        .map(|u| format!(" ({u})"))
                        .unwrap_or_default()
                ),
                needs_install: true,
                uname,
            },
            ProbeOutcome::Unreachable { reason } => HostTestResult {
                ok: false,
                message: reason,
                needs_install: false,
                uname: None,
            },
        })
    }
    #[cfg(not(unix))]
    {
        let _ = host;
        Ok(HostTestResult {
            ok: false,
            message: "SSH transport is Unix-only for now. Windows support \
                      is tracked alongside the wider Windows cloud-push port."
                .into(),
            needs_install: false,
            uname: None,
        })
    }
}

#[derive(Debug, Serialize)]
pub struct HostTestResult {
    pub ok: bool,
    pub message: String,
    /// True when the probe succeeded but `codemux-remote` isn't
    /// installed. The UI uses this to switch from "show test result"
    /// to "offer the bootstrap-install modal."
    #[serde(default)]
    pub needs_install: bool,
    /// Reported `uname -sm` from the probe. Forwarded to the
    /// bootstrap-install flow so we don't have to re-probe.
    #[serde(default)]
    pub uname: Option<String>,
}

/// Bootstrap-install `codemux-remote` on a host that the probe says
/// is reachable but missing the binary. Driven by the consent modal:
/// the frontend asks the user to confirm before invoking.
///
/// Unix-only — the underlying `ssh::bootstrap` module is
/// `#[cfg(unix)]`. On Windows we return an error message.
#[tauri::command]
pub async fn hosts_bootstrap_install(
    db: State<'_, DatabaseStore>,
    id: i64,
    uname: String,
) -> Result<HostBootstrapResult, String> {
    let host = db
        .list_hosts()
        .into_iter()
        .find(|h| h.id == id)
        .ok_or_else(|| format!("Host not found: {id}"))?;

    #[cfg(unix)]
    {
        use crate::ssh::bootstrap::{
            bootstrap_remote, BootstrapOptions, BootstrapResult,
        };
        let outcome = bootstrap_remote(BootstrapOptions::new(
            &host.ssh_target,
            uname.trim(),
        ))
        .await;
        Ok(match outcome {
            BootstrapResult::Installed { reported_version } => HostBootstrapResult {
                ok: true,
                message: format!(
                    "codemux-remote v{reported_version} installed on {}",
                    host.name
                ),
            },
            BootstrapResult::BinaryNotBundled { wanted_target } => {
                HostBootstrapResult {
                    ok: false,
                    message: format!(
                        "Codemux build doesn't include codemux-remote for {wanted_target}. \
                         This is a packaging issue — please report it.",
                    ),
                }
            }
            BootstrapResult::UploadFailed { reason } => HostBootstrapResult {
                ok: false,
                message: format!("Upload failed: {reason}"),
            },
            BootstrapResult::PostInstallProbeFailed { reason } => {
                HostBootstrapResult {
                    ok: false,
                    message: format!(
                        "Installed but failed to verify: {reason}. Try testing the \
                         connection again."
                    ),
                }
            }
        })
    }
    #[cfg(not(unix))]
    {
        let _ = (host, uname);
        Ok(HostBootstrapResult {
            ok: false,
            message: "SSH transport is Unix-only for now.".into(),
        })
    }
}

#[derive(Debug, Serialize)]
pub struct HostBootstrapResult {
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
