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
use tauri::{Manager, State};

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

/// Push a workspace to a remote host.
///
/// Atomic contract: `host_id` is set on the workspace ONLY when the
/// rsync succeeds. The frontend can therefore call this as a single
/// command without doing an optimistic-set-then-rollback dance,
/// which used to cause a brief icon flicker on failure.
///
/// Three-step under the hood:
///   1. rsync the worktree to the conventional remote path
///      (`~/.codemux/worktrees/<sanitized-project>/<sanitized-branch>`)
///      so agents inside see the same filesystem layout they would
///      locally.
///   2. On success, stamp `workspace.host_id = host_id`.
///   3. On failure, host_id stays at its previous value (typically
///      None) and the outcome carries the captured rsync stderr.
///
/// Running PTY sessions are NOT migrated across the network — they
/// terminate cleanly, the user reopens panes on the remote, and
/// adapter-aware agents (Claude Code, Codex) auto-resume via the
/// existing scrollback adapter mechanism. This is documented in
/// `docs/features/remote-hosts.md`.
#[tauri::command]
pub async fn workspace_push_to_host(
    app: tauri::AppHandle,
    db: tauri::State<'_, DatabaseStore>,
    workspace_id: String,
    // The host to push to. The frontend passes host_id directly
    // (instead of pre-setting it on the workspace) so a failed push
    // doesn't leave the workspace in a half-remote state.
    host_id: i64,
) -> Result<crate::commands::hosts::WorkspacePushOutcome, String> {
    let app_state: tauri::State<'_, crate::state::AppStateStore> = app.state();
    let snapshot = app_state.snapshot();
    let ws = snapshot
        .workspaces
        .iter()
        .find(|w| w.workspace_id.0 == workspace_id)
        .ok_or_else(|| format!("Workspace not found: {workspace_id}"))?;
    let host = db
        .list_hosts()
        .into_iter()
        .find(|h| h.id == host_id)
        .ok_or_else(|| format!("Host {host_id} no longer exists locally"))?;

    let local_worktree = match ws.worktree_path.as_ref() {
        Some(p) => std::path::PathBuf::from(p),
        None => std::path::PathBuf::from(&ws.cwd),
    };
    if local_worktree.as_os_str().is_empty() {
        return Err("Workspace has no local path to push.".into());
    }

    let project_name = ws
        .project_root
        .as_deref()
        .and_then(|p| std::path::Path::new(p).file_name())
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "workspace".to_string());
    let branch = ws
        .git_branch
        .clone()
        .unwrap_or_else(|| "main".to_string());

    #[cfg(unix)]
    {
        let remote_path =
            crate::ssh::conventional_remote_path(&project_name, &branch);
        let remote_path_str = remote_path.to_string_lossy().to_string();
        let opts = crate::ssh::PushOptions::new(
            &host.ssh_target,
            &local_worktree,
            &remote_path_str,
        );
        let result = crate::ssh::push_workspace(opts).await;
        let outcome = match result {
            crate::ssh::PushResult::Pushed { rsync_summary, .. } => {
                // Atomicity guarantee — see fn doc. Stamp host_id
                // ONLY after rsync confirms success.
                if let Err(error) =
                    app_state.set_workspace_host_id(&workspace_id, Some(host_id))
                {
                    eprintln!(
                        "[hosts] push succeeded but host_id assignment failed: {error}"
                    );
                }
                // Spawn (or replace) the TunnelSupervisor that keeps
                // the remote daemon reachable. The supervisor handles
                // SSH flaps with its built-in exponential backoff +
                // circuit breaker. Registered by workspace id so
                // subsequent push/pull/close can find and shut it
                // down.
                let local_socket =
                    crate::ssh::local_socket_for_workspace(&workspace_id);
                let remote_socket =
                    crate::ssh::remote_socket_for_workspace(&workspace_id);
                // Forget the cached PtyDaemonClient BEFORE
                // installing the new supervisor. A re-push with a
                // stale client in cache would have the next spawn
                // attempt connect to the OLD tunnel's socket
                // (which is about to be torn down), causing the
                // shell to hang at "Starting…" forever.
                crate::ssh::forget_workspace_client(&workspace_id).await;
                let supervisor = crate::ssh::TunnelSupervisor::spawn(
                    host.ssh_target.clone(),
                    remote_socket,
                    local_socket,
                    // Absolute path via $HOME, not bare
                    // `codemux-remote`. Non-interactive SSH
                    // shells often don't have ~/.local/bin on
                    // PATH (only interactive shells do, via
                    // ~/.profile / ~/.bashrc). Bootstrap installs
                    // here, tunnel must reach here.
                    "$HOME/.local/bin/codemux-remote".to_string(),
                );
                crate::ssh::install_supervisor(&workspace_id, supervisor).await;
                // Stop-sync-restart for live PTYs: terminate the
                // workspace's existing local sessions so the
                // frontend's terminal-cache GC detects them dying
                // and triggers a respawn. The respawn goes through
                // `spawn_pty_for_session` → daemon path →
                // `client_for_workspace` which now sees host_id is
                // set and routes through the new tunnel. Same model
                // as the local persistent-daemon case described in
                // docs/features/persistent-agents.md, just over
                // SSH. Without this the user is stuck staring at
                // local sessions inside a "remote" workspace until
                // they manually close panes.
                terminate_workspace_sessions(&app, &workspace_id);
                WorkspacePushOutcome {
                    ok: true,
                    message: format!("Workspace pushed to {}", host.name),
                    remote_path: Some(remote_path_str.clone()),
                    rsync_summary: Some(rsync_summary),
                }
            }
            crate::ssh::PushResult::RsyncFailed { reason } => WorkspacePushOutcome {
                ok: false,
                message: format!("rsync failed: {reason}"),
                remote_path: None,
                rsync_summary: None,
            },
            crate::ssh::PushResult::HostUnreachable { reason } => {
                WorkspacePushOutcome {
                    ok: false,
                    message: format!("Host unreachable: {reason}"),
                    remote_path: None,
                    rsync_summary: None,
                }
            }
            crate::ssh::PushResult::LocalNotFound { path } => WorkspacePushOutcome {
                ok: false,
                message: format!("Local worktree not found at {path}"),
                remote_path: None,
                rsync_summary: None,
            },
        };
        crate::state::emit_app_state(&app);
        Ok(outcome)
    }
    #[cfg(not(unix))]
    {
        let _ = (local_worktree, project_name, branch, host);
        Ok(WorkspacePushOutcome {
            ok: false,
            message: "SSH transport is Unix-only for now.".into(),
            remote_path: None,
            rsync_summary: None,
        })
    }
}

/// Pull a workspace back from its current host to local. Mirrors the
/// push flow: rsync remote → local, clear `host_id`. The user reopens
/// panes locally and adapter-aware agents auto-resume.
#[tauri::command]
pub async fn workspace_pull_back(
    app: tauri::AppHandle,
    db: tauri::State<'_, DatabaseStore>,
    workspace_id: String,
) -> Result<WorkspacePullOutcome, String> {
    let app_state: tauri::State<'_, crate::state::AppStateStore> = app.state();
    let snapshot = app_state.snapshot();
    let ws = snapshot
        .workspaces
        .iter()
        .find(|w| w.workspace_id.0 == workspace_id)
        .ok_or_else(|| format!("Workspace not found: {workspace_id}"))?;
    let host_id = ws
        .host_id
        .ok_or_else(|| "Workspace is already local.".to_string())?;
    let host = db
        .list_hosts()
        .into_iter()
        .find(|h| h.id == host_id)
        .ok_or_else(|| format!("Host {host_id} no longer exists locally"))?;

    let local_worktree = match ws.worktree_path.as_ref() {
        Some(p) => std::path::PathBuf::from(p),
        None => std::path::PathBuf::from(&ws.cwd),
    };
    let project_name = ws
        .project_root
        .as_deref()
        .and_then(|p| std::path::Path::new(p).file_name())
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "workspace".to_string());
    let branch = ws
        .git_branch
        .clone()
        .unwrap_or_else(|| "main".to_string());

    #[cfg(unix)]
    {
        let remote_path =
            crate::ssh::conventional_remote_path(&project_name, &branch);
        let remote_path_str = remote_path.to_string_lossy().to_string();
        let opts = crate::ssh::PullOptions::new(
            &host.ssh_target,
            &remote_path_str,
            &local_worktree,
        );
        let result = crate::ssh::pull_workspace_back(opts).await;
        let outcome = match result {
            crate::ssh::PullResult::Pulled { rsync_summary, .. } => {
                // On success: clear host_id so the workspace is local
                // again and the next pane spawn uses the local
                // pty-daemon.
                app_state.set_workspace_host_id(&workspace_id, None)?;
                // Forget the cached tunneled client BEFORE shutting
                // down the supervisor — order matters because the
                // cached client holds a socket that the supervisor
                // is about to unbind.
                crate::ssh::forget_workspace_client(&workspace_id).await;
                crate::ssh::shutdown_supervisor(&workspace_id).await;
                // Symmetric to push: terminate remote-routed PTY
                // sessions so the frontend respawns them, this time
                // routing through the local daemon (host_id is now
                // None).
                terminate_workspace_sessions(&app, &workspace_id);
                WorkspacePullOutcome {
                    ok: true,
                    message: format!("Workspace pulled back from {}", host.name),
                    rsync_summary: Some(rsync_summary),
                }
            }
            crate::ssh::PullResult::RsyncFailed { reason } => {
                WorkspacePullOutcome {
                    ok: false,
                    message: format!("rsync failed: {reason}"),
                    rsync_summary: None,
                }
            }
            crate::ssh::PullResult::HostUnreachable { reason } => {
                WorkspacePullOutcome {
                    ok: false,
                    message: format!("Host unreachable: {reason}"),
                    rsync_summary: None,
                }
            }
            crate::ssh::PullResult::RemoteNotFound { path } => {
                WorkspacePullOutcome {
                    ok: false,
                    message: format!(
                        "Remote worktree not found at {path}. The host may have \
                         been wiped or the workspace was never pushed."
                    ),
                    rsync_summary: None,
                }
            }
        };
        crate::state::emit_app_state(&app);
        Ok(outcome)
    }
    #[cfg(not(unix))]
    {
        let _ = (local_worktree, project_name, branch, host);
        Ok(WorkspacePullOutcome {
            ok: false,
            message: "SSH transport is Unix-only for now.".into(),
            rsync_summary: None,
        })
    }
}

#[derive(Debug, Serialize)]
pub struct WorkspacePushOutcome {
    pub ok: bool,
    pub message: String,
    pub remote_path: Option<String>,
    pub rsync_summary: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct WorkspacePullOutcome {
    pub ok: bool,
    pub message: String,
    pub rsync_summary: Option<String>,
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

/// Terminate every PTY session belonging to the given workspace.
///
/// Called from both push (so existing local sessions stop and the
/// frontend respawns them, this time routed through the tunnel)
/// and pull (symmetric — terminate remote-routed sessions so they
/// respawn locally). The frontend's terminal-cache GC detects the
/// session dying and re-mounts the pane, which goes through
/// `spawn_pty_for_session` → routing chooses the right daemon
/// based on the workspace's current host_id.
///
/// Walks the workspace's pane tree via the existing helper and
/// invokes `terminate_pty_session` on every collected session id.
/// For persistent (daemon-backed) sessions, the terminate path
/// already routes the kill through the daemon — see
/// `terminal::terminate_pty_session`.
fn terminate_workspace_sessions(
    app: &tauri::AppHandle,
    workspace_id: &str,
) {
    let app_state: tauri::State<'_, crate::state::AppStateStore> = app.state();
    let pty_state: tauri::State<'_, crate::terminal::PtyState> = app.state();
    let snapshot = app_state.snapshot();
    let session_ids: Vec<String> = snapshot
        .workspaces
        .iter()
        .find(|w| w.workspace_id.0 == workspace_id)
        .map(|w| crate::state::collect_terminal_sessions(&w.surfaces))
        .unwrap_or_default();
    for sid in session_ids {
        crate::terminal::terminate_pty_session(&pty_state.sessions, &sid);
    }
}
