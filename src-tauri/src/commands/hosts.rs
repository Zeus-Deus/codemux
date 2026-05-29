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

/// Turn the host-side `git`/`gh` probe output into a human note for the
/// connection-test message. Pure — unit-tested.
///
/// This is the automations preflight: an automation that triages PRs or
/// issues needs the host to have `git` and an authenticated `gh`, so the
/// "Test connection" result flags it up at setup time rather than at the
/// first 9am run.
fn interpret_github_probe(stdout: &str) -> String {
    let git_ok = stdout.contains("GIT_OK");
    let gh_ok = stdout.contains("GH_OK");
    if !git_ok {
        " · ⚠ git not found — automations on this host need git installed"
            .to_string()
    } else if !gh_ok {
        " · ⚠ gh not signed in — run `gh auth login` on this host for \
         PR/issue automations"
            .to_string()
    } else {
        " · GitHub access ready".to_string()
    }
}

/// SSH to the host and probe `git` + `gh` in one round-trip. Returns a
/// note to append to the connection-test message, or an empty string if
/// the probe itself could not run.
#[cfg(unix)]
async fn probe_host_github(ssh_target: &str) -> String {
    let output = tokio::process::Command::new("ssh")
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("ConnectTimeout=10")
        .arg(ssh_target)
        .arg(
            "git --version >/dev/null 2>&1 && echo GIT_OK; \
             gh auth status >/dev/null 2>&1 && echo GH_OK",
        )
        .output()
        .await;
    match output {
        Ok(out) => interpret_github_probe(&String::from_utf8_lossy(&out.stdout)),
        Err(_) => String::new(),
    }
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
            } => {
                // Automations preflight — flag a host that can't reach
                // GitHub before an automation silently fails at run time.
                let github_note = probe_host_github(&host.ssh_target).await;

                // Version-aware upgrade detection: if the host's
                // codemux-remote is older than the one bundled with
                // this Codemux, surface it as `needs_install: true`
                // so the UI shows the Install button (which then
                // re-uploads + restarts via the same path as a fresh
                // install). Without this, an upgraded Codemux would
                // silently keep using stale codemux-remote on every
                // host the user never explicitly pushed to.
                let our_version = env!("CARGO_PKG_VERSION");
                let upgrade_available = version != our_version;
                let uname_suffix = uname.as_ref().map(|u| format!(" ({u})")).unwrap_or_default();
                if upgrade_available {
                    HostTestResult {
                        ok: true,
                        message: format!(
                            "codemux-remote v{version} on host, v{our_version} bundled — \
                             Install to upgrade{uname_suffix}{github_note}"
                        ),
                        needs_install: true,
                        uname,
                    }
                } else {
                    HostTestResult {
                        ok: true,
                        message: format!(
                            "Connected. codemux-remote v{version} is installed{uname_suffix}{github_note}"
                        ),
                        needs_install: false,
                        uname: None,
                    }
                }
            }
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
    app: tauri::AppHandle,
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
        let outcome = bootstrap_remote(
            BootstrapOptions::new(&host.ssh_target, uname.trim())
                .with_app(&app),
        )
        .await;
        Ok(match outcome {
            BootstrapResult::Installed { reported_version } => {
                // Best-effort: also provision the automation scheduler
                // so the host can run automations. A failure here does
                // NOT fail the install — push-workspace works without
                // it, and not every host runs systemd. Skipped silently
                // when the user is signed out or the host has not yet
                // synced (no server id to identify it by).
                let scheduler_note = match (
                    crate::auth::load_token(&db).map(|(token, _)| token),
                    host.server_id.clone(),
                ) {
                    (Some(token), Some(server_id)) => {
                        match crate::ssh::bootstrap::provision_scheduler(
                            &host.ssh_target,
                            "~/.local/bin/codemux-remote",
                            &token,
                            &server_id,
                            std::time::Duration::from_secs(30),
                        )
                        .await
                        {
                            Ok(()) => " · automation scheduler enabled".to_string(),
                            Err(error) => {
                                eprintln!(
                                    "[codemux::hosts] scheduler provisioning failed: {error}"
                                );
                                " · (automation scheduler not enabled — see logs)"
                                    .to_string()
                            }
                        }
                    }
                    _ => String::new(),
                };

                // Always provision the headless `serve` daemon. Unlike
                // the scheduler this needs no auth token or server id —
                // it's a per-host local control plane. After this
                // returns successfully, an MCP-capable agent on the host
                // can use `codemux-remote mcp` without any manual setup.
                // Same best-effort contract: a failure logs and is
                // surfaced in the result message, doesn't fail bootstrap.
                let serve_note = match crate::ssh::bootstrap::provision_serve(
                    &host.ssh_target,
                    "~/.local/bin/codemux-remote",
                    std::time::Duration::from_secs(30),
                )
                .await
                {
                    Ok(()) => " · MCP control plane enabled".to_string(),
                    Err(error) => {
                        eprintln!(
                            "[codemux::hosts] serve provisioning failed: {error}"
                        );
                        " · (MCP control plane not enabled — see logs)".to_string()
                    }
                };

                HostBootstrapResult {
                    ok: true,
                    message: format!(
                        "codemux-remote v{reported_version} installed on {}{scheduler_note}{serve_note}",
                        host.name
                    ),
                }
            }
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
        // Auto-update the remote codemux-remote binary if our version
        // doesn't match what's installed on the host. Skipping this is
        // what made the cwd bug so painful: my fix lived in the local
        // binary but pandora was still running the May-16 build, and
        // every "the bug isn't fixed" loop was actually "the binary
        // we sent commands to didn't have the fix yet."
        //
        // Cheap when versions match (one SSH probe, ~1s). When they
        // differ we re-bootstrap (~10s) — but that only happens once
        // per Codemux version bump, and the next call is back to fast.
        //
        // For dev users editing daemon code without bumping the version
        // string, the version check passes and the stale-binary problem
        // returns. Workaround: manually re-scp, or rebuild + clear
        // ~/.local/bin/codemux-remote on the remote so the version
        // probe sees MISSING and triggers a bootstrap.
        if let Err(error) = ensure_remote_binary_current(&app, &host).await {
            // Don't block the push on this — if the auto-update fails,
            // the push may still work with the older binary. Log loudly
            // so we know to look here next time something cwd-shaped
            // misbehaves.
            eprintln!(
                "[hosts] auto-update of codemux-remote on {} failed (continuing \
                 with existing binary): {error}",
                host.name
            );
        }

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

                // Sync Claude session JSONLs from
                // ~/.claude/projects/<local-encoded>/ to the
                // remote's matching encoded path, so a fresh
                // `claude --resume <uuid>` on the remote finds the
                // conversation history. Best-effort: failure here
                // only loses continuity, never blocks the push.
                //
                // We need the REMOTE's absolute cwd (with remote
                // $HOME) for the encoded dir name. Query $HOME via
                // ssh — ~1s round trip, only when there's actually
                // local history to sync.
                let local_workspace_cwd =
                    std::path::PathBuf::from(&ws.cwd);
                if !local_workspace_cwd.as_os_str().is_empty() {
                    match tokio::process::Command::new("ssh")
                        .arg("-o")
                        .arg("BatchMode=yes")
                        .arg(&host.ssh_target)
                        .arg("echo $HOME")
                        .output()
                        .await
                    {
                        Ok(out) if out.status.success() => {
                            let remote_home =
                                String::from_utf8_lossy(&out.stdout)
                                    .trim()
                                    .to_string();
                            if !remote_home.is_empty() {
                                // Build the remote absolute cwd:
                                // <remote_home>/.codemux/worktrees/<project>/<branch>
                                let conv = crate::ssh::conventional_remote_path(
                                    &project_name,
                                    &branch,
                                );
                                let conv_str = conv.to_string_lossy();
                                let remote_rel = conv_str
                                    .strip_prefix("~/")
                                    .unwrap_or(&conv_str);
                                let remote_absolute_cwd =
                                    std::path::PathBuf::from(&remote_home)
                                        .join(remote_rel);
                                if let Err(error) = sync_claude_projects(
                                    &host.ssh_target,
                                    &local_workspace_cwd,
                                    &remote_absolute_cwd,
                                )
                                .await
                                {
                                    eprintln!(
                                        "[hosts] Claude JSONL sync failed (continuing — \
                                         agent will launch but conversation will be \
                                         fresh): {error}"
                                    );
                                }
                            }
                        }
                        Ok(out) => eprintln!(
                            "[hosts] ssh 'echo $HOME' failed (status {}); \
                             skipping Claude JSONL sync",
                            out.status
                        ),
                        Err(e) => eprintln!(
                            "[hosts] ssh 'echo $HOME' spawn failed: {e}; \
                             skipping Claude JSONL sync"
                        ),
                    }
                }

                // Close any pre-existing sessions on this workspace's
                // remote daemon BEFORE respawning. The daemon process
                // outlives the Codemux app — a session_id from a
                // previous (possibly buggy) push run is still alive on
                // the daemon, and the spawn path's reattach logic will
                // happily attach to it, inheriting its old cwd. For
                // example: an earlier push that left a bash in
                // `/home/deus` because of a cwd bug stays in
                // `/home/deus` forever, and every subsequent push that
                // hits the same session id ends up there too.
                //
                // Each workspace gets its own per-workspace tunnel +
                // its own codemux-remote pty-daemon process (different
                // socket per workspace), so closing every session on
                // this daemon only affects this workspace.
                //
                // Filter defensively by workspace_id anyway in case
                // that invariant ever changes.
                match crate::ssh::client_for_workspace(
                    &app,
                    &workspace_id,
                    Some(host_id),
                )
                .await
                {
                    Ok(remote_client) => match remote_client.list().await {
                        Ok(remote_sessions) => {
                            let mut closed = 0usize;
                            for s in remote_sessions {
                                if !s.workspace_id.is_empty()
                                    && s.workspace_id != workspace_id
                                {
                                    continue;
                                }
                                if let Err(e) =
                                    remote_client.close(s.session_id.clone()).await
                                {
                                    eprintln!(
                                        "[hosts] failed to close stale remote session \
                                         {} on push: {e}",
                                        s.session_id
                                    );
                                } else {
                                    closed += 1;
                                }
                            }
                            eprintln!(
                                "[hosts] closed {closed} stale remote session(s) for \
                                 workspace {workspace_id} before respawn"
                            );
                        }
                        Err(e) => eprintln!(
                            "[hosts] failed to list remote sessions before respawn: {e}"
                        ),
                    },
                    Err(e) => eprintln!(
                        "[hosts] failed to reach remote daemon for pre-respawn \
                         cleanup: {e} (continuing — fresh sessions will be created \
                         but stale ones may persist on the daemon)"
                    ),
                }

                // Stop-sync-restart for live PTYs: terminate the
                // workspace's existing local sessions, then
                // explicitly re-spawn each pane's session so the
                // user's terminals come back online on the remote
                // daemon. We tried "let the frontend respawn from
                // GC" originally but the frontend has no auto-
                // respawn path (the cache GC only DISPOSES dead
                // entries); without an explicit backend respawn
                // the user just sees "shell ended" overlays and
                // has to manually close + reopen every pane.
                //
                // `spawn_pty_for_session` is idempotent per
                // session id (gated by `try_reserve_session_spawn`),
                // and now routes through `client_for_workspace`
                // which sees host_id is set → remote daemon →
                // fresh shells appear on the host machine.
                terminate_workspace_sessions(&app, &workspace_id);
                crate::terminal::spawn_missing_ptys_for_workspace(
                    app.clone(),
                    &workspace_id,
                );
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
    _db: tauri::State<'_, DatabaseStore>,
    workspace_id: String,
) -> Result<WorkspacePullOutcome, String> {
    workspace_pull_back_impl(app, workspace_id).await
}

/// Internal entry point for the pull-back machinery. Extracted from
/// the `#[tauri::command]` wrapper so other Rust code paths (notably
/// the cross-device adoption flow in
/// `commands::workspaces_sync::workspaces_adopt_synced`) can drive
/// the same rsync + tunnel-teardown + session-respawn pipeline
/// without going back out through the Tauri IPC layer.
///
/// Takes only `app` and looks up both `DatabaseStore` and
/// `AppStateStore` internally — this avoids the `tauri::State<'_>`
/// lifetime trap where the guard cannot cross an `.await`.
///
/// Semantics: requires the local workspace to already exist with
/// `host_id` set; rsyncs the remote worktree to the local
/// `worktree_path` (or `cwd` fallback); clears `host_id` on success;
/// tears down the SSH tunnel; respawns each pane's PTY locally.
pub async fn workspace_pull_back_impl(
    app: tauri::AppHandle,
    workspace_id: String,
) -> Result<WorkspacePullOutcome, String> {
    // Resolve State guards in a tight pre-await scope and capture
    // owned values so the State guards drop before any `.await`
    // (they are not Send).
    let (host, ws_clone, origin_path) = {
        let app_state: tauri::State<'_, crate::state::AppStateStore> = app.state();
        let db: tauri::State<'_, DatabaseStore> = app.state();
        let snapshot = app_state.snapshot();
        let ws = snapshot
            .workspaces
            .iter()
            .find(|w| w.workspace_id.0 == workspace_id)
            .ok_or_else(|| format!("Workspace not found: {workspace_id}"))?
            .clone();
        let host_id = ws
            .host_id
            .ok_or_else(|| "Workspace is already local.".to_string())?;
        let host = db
            .list_hosts()
            .into_iter()
            .find(|h| h.id == host_id)
            .ok_or_else(|| format!("Host {host_id} no longer exists locally"))?;
        // The authoritative remote source path. For a workspace an agent
        // CREATED on the host, this is its real on-host path (e.g.
        // `/home/deus/projects/passpage`) — which is NOT the
        // `~/.codemux/worktrees/<project>/<branch>` convention the desktop
        // uses for workspaces it PUSHED. Pulling from the wrong (assumed)
        // path is exactly what produced empty worktrees. Falls back to the
        // convention when the sync row has no recorded origin path (the
        // pushed-workspace case, where the two coincide).
        let origin_path = db
            .list_workspaces_sync_for_sync()
            .into_iter()
            .find(|r| r.workspace_id.as_deref() == Some(workspace_id.as_str()))
            .and_then(|r| r.origin_path);
        (host, ws, origin_path)
    };
    let app_state: tauri::State<'_, crate::state::AppStateStore> = app.state();
    // Rename for compatibility with the original function body below
    // (which references `ws` and `host`).
    let ws = &ws_clone;

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
        // Prefer the workspace's REAL recorded path on the host
        // (`origin_path`, set by the inventory poller from the daemon
        // registry). Only reconstruct the conventional path when we have
        // no recorded origin — i.e. a workspace this device pushed, where
        // the remote path IS the conventional one.
        let remote_path_str = origin_path.clone().unwrap_or_else(|| {
            crate::ssh::conventional_remote_path(&project_name, &branch)
                .to_string_lossy()
                .to_string()
        });
        let opts = crate::ssh::PullOptions::new(
            &host.ssh_target,
            &remote_path_str,
            &local_worktree,
        );
        let result = crate::ssh::pull_workspace_back(opts).await;
        let outcome = match result {
            crate::ssh::PullResult::Pulled { rsync_summary, .. } => {
                // Symmetric Claude JSONL sync (remote → local) BEFORE
                // we kill the remote and respawn locally. Without this,
                // any conversation continuation that happened on the
                // remote would be lost on pull-back.
                //
                // SAFETY: we only sync the workspace's specific
                // encoded directory (not the whole projects/ tree),
                // and we use rsync's default per-file mtime/size
                // comparison so newer files (the remote's continued
                // session) overwrite older ones (laptop's pre-push
                // version). We do NOT pass --delete, so any local-
                // only session files (e.g. older runs that never
                // went to the remote) survive untouched.
                let local_workspace_cwd =
                    std::path::PathBuf::from(&ws.cwd);
                if !local_workspace_cwd.as_os_str().is_empty() {
                    match tokio::process::Command::new("ssh")
                        .arg("-o")
                        .arg("BatchMode=yes")
                        .arg(&host.ssh_target)
                        .arg("echo $HOME")
                        .output()
                        .await
                    {
                        Ok(out) if out.status.success() => {
                            let remote_home =
                                String::from_utf8_lossy(&out.stdout)
                                    .trim()
                                    .to_string();
                            if !remote_home.is_empty() {
                                let conv = crate::ssh::conventional_remote_path(
                                    &project_name,
                                    &branch,
                                );
                                let conv_str = conv.to_string_lossy();
                                let remote_rel = conv_str
                                    .strip_prefix("~/")
                                    .unwrap_or(&conv_str);
                                let remote_absolute_cwd =
                                    std::path::PathBuf::from(&remote_home)
                                        .join(remote_rel);
                                if let Err(error) = pull_claude_projects(
                                    &host.ssh_target,
                                    &remote_absolute_cwd,
                                    &local_workspace_cwd,
                                )
                                .await
                                {
                                    eprintln!(
                                        "[hosts] Claude JSONL pull-back failed \
                                         (continuing — agent will launch with whatever \
                                         conversation history was already local): {error}"
                                    );
                                }
                            }
                        }
                        Ok(out) => eprintln!(
                            "[hosts] ssh 'echo $HOME' failed on pull-back (status {}); \
                             skipping Claude JSONL sync",
                            out.status
                        ),
                        Err(e) => eprintln!(
                            "[hosts] ssh 'echo $HOME' spawn failed on pull-back: {e}; \
                             skipping Claude JSONL sync"
                        ),
                    }
                }

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
                // sessions and immediately respawn each pane's
                // session on the local daemon (host_id is now
                // None, so `client_for_workspace` returns the
                // local singleton). Same agent-caveat as push —
                // see the long comment in `workspace_push_to_host`.
                terminate_workspace_sessions(&app, &workspace_id);
                crate::terminal::spawn_missing_ptys_for_workspace(
                    app.clone(),
                    &workspace_id,
                );
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
        let _ = (local_worktree, project_name, branch, host, origin_path);
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

/// Sync the laptop's per-workspace Claude session JSONLs to the
/// matching encoded directory on the remote host, so a fresh
/// `claude --resume <uuid>` invocation on the remote finds the
/// conversation history we built up locally.
///
/// Returns Ok(()) on success OR on benign "nothing to sync" (no
/// local sessions for this workspace). Returns Err on actual
/// rsync/SSH failure. Caller decides whether to propagate or
/// warn-and-continue — for now we warn-and-continue because the
/// agent will still launch (just without continuity), which is a
/// strictly better outcome than blocking the push.
#[cfg(unix)]
async fn sync_claude_projects(
    ssh_target: &str,
    local_cwd: &std::path::Path,
    remote_cwd: &std::path::Path,
) -> Result<(), String> {
    use tokio::process::Command;

    // Step 1: figure out the laptop-side source dir. If no Claude
    // session has ever been started in this workspace, the dir
    // doesn't exist — nothing to sync, success.
    let local_home = std::env::var("HOME")
        .map_err(|_| "HOME env var not set on laptop".to_string())?;
    let local_dir_name = crate::ssh::claude_project_dir_name(local_cwd);
    let local_source = std::path::PathBuf::from(&local_home)
        .join(".claude")
        .join("projects")
        .join(&local_dir_name);
    if !local_source.exists() {
        eprintln!(
            "[hosts] no local Claude session history for this workspace \
             ({}); skipping JSONL sync",
            local_source.display()
        );
        return Ok(());
    }

    // Step 2: compute the remote-side destination dir name. The
    // encoded path uses the REMOTE's absolute cwd (with remote
    // $HOME), not the laptop's.
    let remote_dir_name = crate::ssh::claude_project_dir_name(remote_cwd);
    // Use `~/.claude/projects/<encoded>/` on the remote — rsync
    // tilde-expands via the remote shell.
    let remote_dest = format!("{ssh_target}:~/.claude/projects/{remote_dir_name}/");

    // Step 3: ensure the remote dest dir exists. rsync creates the
    // LAST path component but not parents; mkdir -p covers the
    // ~/.claude/projects/<encoded>/ chain in one shot.
    let mkdir = Command::new("ssh")
        .arg("-o")
        .arg("BatchMode=yes")
        .arg(ssh_target)
        .arg(format!(
            "mkdir -p ~/.claude/projects/{}",
            shell_word_quote(&remote_dir_name)
        ))
        .status()
        .await
        .map_err(|e| format!("ssh mkdir spawn: {e}"))?;
    if !mkdir.success() {
        return Err(format!("mkdir on remote failed (status: {mkdir})"));
    }

    // Step 4: rsync the JSONLs. Use trailing slash on source so
    // contents (not the dir itself) land at the destination. No
    // --delete because the remote may have OTHER sessions started
    // there that we don't want to wipe.
    let source_with_slash = format!("{}/", local_source.display());
    let rsync = Command::new("rsync")
        .arg("-a")
        .arg("--no-owner")
        .arg("--no-group")
        .arg("-e")
        .arg("ssh -o BatchMode=yes")
        .arg(&source_with_slash)
        .arg(&remote_dest)
        .status()
        .await
        .map_err(|e| format!("rsync spawn: {e}"))?;
    if !rsync.success() {
        return Err(format!("rsync failed (status: {rsync})"));
    }
    eprintln!(
        "[hosts] synced Claude session JSONLs: {} → {}",
        local_source.display(),
        remote_dest
    );
    Ok(())
}

/// Minimal shell-quote for the encoded dir name. Encoded paths
/// contain only `[A-Za-z0-9_-]` so this is mostly defensive; we
/// just escape single quotes the standard way and wrap in single
/// quotes. Also used by terminal::daemon_backed for the agent-
/// binary preflight check.
pub(crate) fn shell_word_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Symmetric to `sync_claude_projects` but pulls remote → local.
/// Called from the pull-back flow so any conversation that
/// continued on the remote comes back with the workspace files.
///
/// SAFETY (the explicit thing the user asked us to be careful about):
/// - Scoped to ONE specific encoded directory (this workspace's),
///   never the whole `~/.claude/projects/` tree
/// - NO `--delete` flag — we don't want to nuke local-only files
///   (older sessions, local-only experiments). The union of local
///   and remote files exists after pull
/// - Rsync's default per-file mtime/size comparison picks the
///   newer copy when both sides have the same UUID (the remote
///   one is newer because that's where the continuation happened)
///
/// Errors are non-fatal — the agent will still launch locally,
/// just without the remote-side continuation.
#[cfg(unix)]
async fn pull_claude_projects(
    ssh_target: &str,
    remote_cwd: &std::path::Path,
    local_cwd: &std::path::Path,
) -> Result<(), String> {
    use tokio::process::Command;

    let local_home = std::env::var("HOME")
        .map_err(|_| "HOME env var not set on laptop".to_string())?;
    let local_dir_name = crate::ssh::claude_project_dir_name(local_cwd);
    let local_dest = std::path::PathBuf::from(&local_home)
        .join(".claude")
        .join("projects")
        .join(&local_dir_name);
    // mkdir the local destination if it doesn't exist (first time
    // pulling a workspace whose Claude sessions never ran locally).
    if !local_dest.exists() {
        if let Err(e) = std::fs::create_dir_all(&local_dest) {
            return Err(format!("create local dest: {e}"));
        }
    }

    let remote_dir_name = crate::ssh::claude_project_dir_name(remote_cwd);
    // Check the remote dir exists first — if not, nothing to pull.
    let probe = Command::new("ssh")
        .arg("-o")
        .arg("BatchMode=yes")
        .arg(ssh_target)
        .arg(format!(
            "test -d ~/.claude/projects/{} && echo EXISTS || echo MISSING",
            shell_word_quote(&remote_dir_name)
        ))
        .output()
        .await
        .map_err(|e| format!("ssh probe spawn: {e}"))?;
    let probe_out = String::from_utf8_lossy(&probe.stdout).trim().to_string();
    if probe_out != "EXISTS" {
        eprintln!(
            "[hosts] no remote Claude session history at \
             ~/.claude/projects/{remote_dir_name}/ on {ssh_target}; \
             skipping pull-back of JSONLs"
        );
        return Ok(());
    }

    let remote_source = format!(
        "{ssh_target}:~/.claude/projects/{remote_dir_name}/"
    );
    let local_dest_with_slash = format!("{}/", local_dest.display());
    let rsync = Command::new("rsync")
        .arg("-a")
        .arg("--no-owner")
        .arg("--no-group")
        .arg("-e")
        .arg("ssh -o BatchMode=yes")
        .arg(&remote_source)
        .arg(&local_dest_with_slash)
        .status()
        .await
        .map_err(|e| format!("rsync spawn: {e}"))?;
    if !rsync.success() {
        return Err(format!("rsync failed (status: {rsync})"));
    }
    eprintln!(
        "[hosts] pulled Claude session JSONLs back: {} → {}",
        remote_source,
        local_dest.display()
    );
    Ok(())
}

/// Probe the remote `codemux-remote` binary's version. If it
/// doesn't match what we'd ship from this Codemux build, re-bootstrap
/// (scp the current binary + chmod + verify) so the daemon spawn the
/// supervisor's about to make uses the up-to-date binary. Also kills
/// any running daemon on the remote so the next SSH `exec` can bind
/// the same socket without an "address in use" conflict.
///
/// Returns Ok on either "already current, nothing to do" or "updated
/// successfully." Returns Err only when the bootstrap attempt itself
/// failed (network down, no bundled binary for the target uname, etc.).
/// Caller decides whether to propagate or warn-and-continue.
#[cfg(unix)]
async fn ensure_remote_binary_current(
    app: &tauri::AppHandle,
    host: &crate::database::HostRecord,
) -> Result<(), String> {
    use std::process::Stdio;
    use tokio::process::Command;

    // Step 1: probe the installed binary's version.
    let probe = Command::new("ssh")
        .arg("-o")
        .arg("BatchMode=yes")
        .arg("-o")
        .arg("ConnectTimeout=10")
        .arg(&host.ssh_target)
        .arg("$HOME/.local/bin/codemux-remote --version 2>/dev/null || echo MISSING")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("version probe: spawn ssh: {e}"))?;
    let stdout = String::from_utf8_lossy(&probe.stdout).trim().to_string();
    // `codemux-remote --version` prints `codemux-remote X.Y.Z` to stdout.
    let remote_version = stdout
        .strip_prefix("codemux-remote ")
        .map(|s| s.trim().to_string());
    let our_version = env!("CARGO_PKG_VERSION");
    if remote_version.as_deref() == Some(our_version) {
        eprintln!(
            "[hosts] {} already has codemux-remote {our_version} — skipping bootstrap",
            host.name
        );
        return Ok(());
    }
    eprintln!(
        "[hosts] {} needs bootstrap: remote_version={:?} our_version={our_version}",
        host.name, remote_version
    );

    // Step 2: figure out the remote uname so we can pick the right
    // bundled binary.
    let uname_output = Command::new("ssh")
        .arg("-o")
        .arg("BatchMode=yes")
        .arg(&host.ssh_target)
        .arg("uname -s -m")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("uname probe: spawn ssh: {e}"))?;
    let uname = String::from_utf8_lossy(&uname_output.stdout).trim().to_string();
    if uname.is_empty() {
        return Err("uname probe returned empty string".into());
    }

    // Step 3: kill any running pty-daemon. Otherwise the freshly-bootstrapped
    // binary won't actually be used until the next SSH-spawn cycle, and
    // a stale daemon still bound to the workspace's Unix socket would
    // make that next spawn fail with "address in use." The narrow `-f`
    // pattern only matches the SSH-spawned pty-daemon — user-launched
    // `codemux-remote mcp` or `serve` invocations are spared. `serve`
    // is restarted via systemctl in step 5 instead.
    let _ = Command::new("ssh")
        .arg("-o")
        .arg("BatchMode=yes")
        .arg(&host.ssh_target)
        .arg("pkill -f 'codemux-remote pty-daemon' 2>/dev/null || true")
        .status()
        .await;

    // Step 4: bootstrap (upload binary + verify version).
    use crate::ssh::bootstrap::{bootstrap_remote, BootstrapOptions, BootstrapResult};
    let outcome = bootstrap_remote(
        BootstrapOptions::new(&host.ssh_target, &uname).with_app(app),
    )
    .await;
    match outcome {
        BootstrapResult::Installed { reported_version } => {
            eprintln!(
                "[hosts] bootstrapped {} → codemux-remote {reported_version}",
                host.name
            );
        }
        BootstrapResult::BinaryNotBundled { wanted_target } => {
            return Err(format!(
                "this Codemux build doesn't include a codemux-remote for {wanted_target}"
            ));
        }
        BootstrapResult::UploadFailed { reason } => {
            return Err(format!("upload: {reason}"));
        }
        BootstrapResult::PostInstallProbeFailed { reason } => {
            return Err(format!("verify: {reason}"));
        }
    }

    // Step 5: re-provision the headless `serve` daemon. This is
    // idempotent — it rewrites the systemd unit (so a unit-content
    // change in this Codemux version takes effect immediately), runs
    // daemon-reload, and **restarts** the unit (not just `enable
    // --now` — restart kills the old process so the new binary on
    // disk actually starts running). If we skipped this, an upgraded
    // codemux-remote on disk would coexist with an old `serve`
    // process in memory until the next host reboot, which would
    // confuse anyone debugging "why doesn't my new MCP tool show up
    // after I updated Codemux."
    //
    // Best-effort: a failure here doesn't fail the upgrade. The
    // binary is current; the user can `systemctl --user restart
    // codemux-remote` themselves if needed.
    if let Err(error) = crate::ssh::bootstrap::provision_serve(
        &host.ssh_target,
        "~/.local/bin/codemux-remote",
        std::time::Duration::from_secs(30),
    )
    .await
    {
        eprintln!(
            "[hosts] re-provisioning serve on {} after upgrade failed (continuing): {error}",
            host.name
        );
    } else {
        eprintln!("[hosts] re-provisioned codemux-remote.service on {}", host.name);
    }

    Ok(())
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
#[cfg(unix)]
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
        // Use the keep-channel variant so the frontend's xterm output
        // channel survives the kill-and-respawn. Otherwise the user
        // has to tab-switch away and back to see the respawned PTY's
        // output (claude UI, shell prompt, etc.) — the regular
        // terminate clears the output_channel and a fresh spawn gets
        // a fresh runtime with no channel, so all post-respawn output
        // buffers in pending_output until something forces a re-attach.
        crate::terminal::terminate_pty_session_keep_channel(
            &pty_state.sessions,
            &sid,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::interpret_github_probe;

    #[test]
    fn github_probe_reports_ready_when_git_and_gh_are_present() {
        let note = interpret_github_probe("GIT_OK\nGH_OK\n");
        assert!(note.contains("ready"));
        assert!(!note.contains("⚠"));
    }

    #[test]
    fn github_probe_flags_missing_git() {
        // No GIT_OK — git itself is absent.
        let note = interpret_github_probe("GH_OK\n");
        assert!(note.contains("⚠"));
        assert!(note.contains("git"));
    }

    #[test]
    fn github_probe_flags_unauthenticated_gh() {
        // git is present but `gh auth status` failed.
        let note = interpret_github_probe("GIT_OK\n");
        assert!(note.contains("⚠"));
        assert!(note.contains("gh auth login"));
    }

    #[test]
    fn github_probe_flags_an_empty_probe_as_missing_git() {
        assert!(interpret_github_probe("").contains("⚠"));
    }
}
