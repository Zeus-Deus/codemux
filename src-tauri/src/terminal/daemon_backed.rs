//! Daemon-backed terminal spawn path.
//!
//! Instead of `portable_pty::openpty()` + child spawn in this process, the
//! work happens inside the long-lived `codemux pty-daemon`. The resulting
//! `SessionRuntime` is marked `persistent = true` so the close and Drop paths
//! skip the kill-the-process-group step.
//!
//! Output flow:
//!     daemon child → daemon master fd → daemon mpsc → socket → client
//!     mpsc → this module's reader task → `queue_or_send_output` →
//!     Tauri channel → xterm.
//!
//! Input flow:
//!     xterm onData → write_to_pty → ordered writer queue → client.write →
//!     socket → daemon → master fd → acknowledgement.

use super::{
    emit_terminal_status_if_current, hydration_workspace_pty_env, queue_or_send_output_if_current,
    with_existing_session_runtime, DaemonReconciliation, DaemonWorkspaceRoute, PtyState,
    SessionSpawnBlocked, SessionSpawnReservation, TerminalCloseSignal,
    TerminalCloseError, TerminalLifecycleState,
    TerminalStatusPayload, DEFAULT_COLS, DEFAULT_ROWS,
};
use crate::database::{
    daemon_closes::{PendingDaemonClose, PendingTerminalReplacement},
    DatabaseStore,
};
use crate::workspace_service::Checkpoint;
use crate::pty_daemon::{DaemonSessionInfo, PtyDaemonClient, PtyDaemonError};
use crate::state::{AppStateStore, LayoutCloseIntent, PtyHydrationPlan, PtyHydrationSession, PtyHydrationWorkspace, SessionId};
use futures_util::stream::StreamExt as _;
use std::collections::{hash_map::Entry, HashMap, VecDeque};
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Manager, Runtime, State};
use uuid::Uuid;

enum DaemonSpawnClaim {
    Hydrate {
        token: Uuid,
        pending_close: Option<Arc<TerminalCloseSignal>>,
    },
    Restart(SessionSpawnReservation),
}

#[derive(Default)]
pub(super) struct DaemonHydrationResult {
    pub failures: Vec<(String, DaemonHydrationFailure)>,
    pub restarted_locally: bool,
}

enum DaemonSpawnOutcome {
    DaemonShell,
    LocalRestart { ready: bool },
}

enum LocalRestartReplay {
    None,
    AttachSurvivingTokenless,
    Retired,
    UseReplacement,
}

/// Should `spawn_pty_for_session_via_daemon` synthesize a "relaunch this
/// agent" command from the in-memory `original_command`?
///
/// True only when there's positive evidence the shell already had a prior
/// agent run — either persisted scrollback metadata (`disk_meta_present`)
/// or a captured agent session id (`agent_session_id_present`, e.g.
/// Claude's hook-captured UUID or OpenCode's push-synced session id).
/// Without one of those, this codepath is racing a fresh `apply_preset`
/// call: the preset handler will write the exact same command via its own
/// `write_command_when_ready`, so synthesizing here produces a duplicate
/// write. The second write lands inside the just-started agent's input
/// box — the Linux preset-leak bug. Windows isn't affected because the
/// whole daemon path is `#[cfg(unix)]`-gated.
pub(crate) fn should_synthesize_agent_relaunch(
    disk_meta_present: bool,
    agent_session_id_present: bool,
) -> bool {
    disk_meta_present || agent_session_id_present
}

/// Build the agent-relaunch command written into a freshly-spawned shell on
/// a remote push (or local pull-back) respawn, from the pane's original
/// command and any captured resume identifiers. The caller gates this behind
/// [`should_synthesize_agent_relaunch`]; this fn assumes the decision to
/// relaunch was already made.
///
/// Curates a minimal command (binary + resume args) rather than replaying
/// the full original, which often carries laptop-specific flags the remote
/// agent rejects:
///
/// - `claude` → `claude [--dangerously-skip-permissions] [--resume <uuid>]`.
///   `--dangerously-skip-permissions` is forwarded only when the original
///   carried it (matches the user's preset intent); `--resume <uuid>`
///   continues the conversation whose JSONLs the push flow rsynced.
/// - `opencode` → `opencode --session <id>` when a session id was synced
///   (issue #16; the receiving opencode.db got the session via
///   export/import), else `opencode --continue` (import set the session's
///   directory to this cwd, so `--continue` resumes the most-recent session
///   here). The OpenCode preset carries no skip-permissions flag, so none is
///   forwarded.
/// - any other binary → the bare binary (relaunch without resume args — the
///   prior behavior for agents without a resume integration).
///
/// Returns `None` only when there's no original command to derive a binary
/// from (a plain shell, or a preset not yet applied).
pub(crate) fn build_agent_relaunch_command(
    original_command: Option<&str>,
    claude_uuid: Option<&str>,
    opencode_session_id: Option<&str>,
) -> Option<String> {
    let original = original_command?;
    let binary = original.split_whitespace().next()?.to_string();
    match binary.as_str() {
        "claude" => {
            let mut parts = vec![binary];
            if original.contains("--dangerously-skip-permissions") {
                parts.push("--dangerously-skip-permissions".to_string());
            }
            if let Some(uuid) = claude_uuid {
                parts.push("--resume".to_string());
                parts.push(uuid.to_string());
            }
            Some(parts.join(" "))
        }
        "opencode" => {
            let mut parts = vec![binary];
            if let Some(id) = opencode_session_id {
                parts.push("--session".to_string());
                parts.push(id.to_string());
            } else {
                // No captured id (sync produced nothing, or this is a local
                // app-restart) → continue the most-recent session for the
                // cwd. Harmless when there's no session: opencode just opens
                // fresh.
                parts.push("--continue".to_string());
            }
            Some(parts.join(" "))
        }
        _ => Some(binary),
    }
}

/// Resolve the directory a remote (host-backed) workspace's terminals
/// should spawn into **on the host**. The local `cwd` is a path on this
/// device that doesn't exist remotely, so it's never used here.
///
/// Resolution order:
/// 1. `remote_cwd` — the workspace's real on-host directory. Set for
///    "open on host" / attach-in-place workspaces, where the host
///    directory was discovered by the inventory poller and can live at an
///    arbitrary path we can't reconstruct. Always preferred when present.
/// 2. The conventional, `project_uid`-keyed path
///    (`~/.codemux/worktrees/<uid>-<project>/<branch>`) — where the push
///    flow lands a pushed workspace. Used for pushed workspaces (no
///    `remote_cwd`), and matches the path `workspace_push_to_host` chose.
#[cfg(test)]
pub(crate) fn remote_spawn_cwd(owning_ws: Option<&crate::state::WorkspaceSnapshot>) -> String {
    remote_spawn_cwd_from_fields(
        owning_ws.and_then(|w| w.remote_cwd.as_deref()),
        owning_ws.and_then(|w| w.project_root.as_deref()),
        owning_ws.and_then(|w| w.git_branch.as_deref()),
        owning_ws.and_then(|w| w.project_uid.as_deref()),
    )
}

fn hydration_remote_spawn_cwd(owning_ws: &crate::state::PtyHydrationWorkspace) -> String {
    remote_spawn_cwd_from_fields(
        owning_ws.remote_cwd.as_deref(),
        owning_ws.project_root.as_deref(),
        owning_ws.git_branch.as_deref(),
        owning_ws.project_uid.as_deref(),
    )
}

fn remote_spawn_cwd_from_fields(
    remote_cwd: Option<&str>,
    project_root: Option<&str>,
    git_branch: Option<&str>,
    project_uid: Option<&str>,
) -> String {
    if let Some(remote_cwd) = remote_cwd {
        if !remote_cwd.trim().is_empty() {
            return remote_cwd.to_string();
        }
    }
    let project_name = project_root
        .and_then(|p| std::path::Path::new(p).file_name())
        .and_then(|n| n.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| "workspace".to_string());
    let branch = git_branch.unwrap_or("main");
    crate::ssh::conventional_remote_path_keyed(project_uid, &project_name, branch)
        .to_string_lossy()
        .to_string()
}

/// [`hydration_remote_spawn_cwd`] as an absolute path on the device. The
/// remote daemon expands `~` in a spawn's cwd but sets env values verbatim,
/// so env built from a `~` path would name a directory no shell can `cd`
/// into. `~` resolves against the device's `$HOME` (cached per SSH target),
/// never this computer's. If that lookup fails the terminal still starts,
/// in the `~` path, rather than not opening at all.
async fn resolve_device_spawn_dir<R: Runtime>(
    app: &AppHandle<R>,
    workspace: &PtyHydrationWorkspace,
) -> String {
    let dir = hydration_remote_spawn_cwd(workspace);
    if dir != "~" && !dir.starts_with("~/") {
        return dir;
    }
    let ssh_target = workspace.host_id.and_then(|host_id| {
        app.try_state::<crate::database::DatabaseStore>()?
            .list_hosts()
            .into_iter()
            .find(|host| host.id == host_id)
            .map(|host| host.ssh_target)
    });
    let home = match ssh_target {
        Some(ssh_target) => crate::ssh::exec::remote_home(&ssh_target).await,
        None => Err("its device is no longer configured".to_string()),
    };
    match home {
        Ok(home) => crate::ssh::exec::expand_remote_tilde(&dir, &home),
        Err(error) => {
            eprintln!(
                "[codemux::terminal::daemon_backed] couldn't resolve the device home for \
                 workspace {}; terminal env keeps {dir}: {error}",
                workspace.workspace_id
            );
            dir
        }
    }
}

const HYDRATION_CONCURRENCY: usize = 4;

/// Whether a local in-process shell can safely replace a failed daemon RPC.
/// A transport failure after sending Spawn or Attach has an unknown outcome:
/// the daemon may already own the shell even though its acknowledgement was
/// lost. Starting a local fallback in that case would duplicate the process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DaemonHydrationFailure {
    AlreadyReserved,
    RequiresGuardedRecovery,
    LegacyDaemonLocalFallback,
    OwnershipUnavailable(String),
    SafeToFallback(String),
    Ambiguous(String),
}

impl DaemonHydrationFailure {
    pub(crate) fn allows_local_fallback(&self) -> bool {
        matches!(self, Self::SafeToFallback(_) | Self::LegacyDaemonLocalFallback)
    }

    pub(crate) fn is_already_reserved(&self) -> bool {
        matches!(self, Self::AlreadyReserved)
    }
}

impl std::fmt::Display for DaemonHydrationFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyReserved => f.write_str("session already reserved by another spawn"),
            Self::RequiresGuardedRecovery => {
                f.write_str("update the terminal daemon on its host before retrying this shell")
            }
            Self::LegacyDaemonLocalFallback => {
                f.write_str("the local terminal daemon lacks revocable spawn permits")
            }
            Self::OwnershipUnavailable(message) => {
                write!(f, "Could not save terminal shell ownership: {message}. Retry when local storage is available.")
            }
            Self::SafeToFallback(message) | Self::Ambiguous(message) => f.write_str(message),
        }
    }
}

fn classify_spawn_rpc_failure(error: PtyDaemonError) -> DaemonHydrationFailure {
    let message = format!("daemon spawn: {error}");
    match error {
        // Serialization happens before the frame is written. These daemon
        // errors are also produced before `spawn_command` returns a child.
        PtyDaemonError::Serde(_) => DaemonHydrationFailure::SafeToFallback(message),
        PtyDaemonError::Daemon(ref daemon_message)
            if daemon_message == "argv is empty"
                || daemon_message.starts_with("openpty: ")
                || daemon_message.starts_with("spawn: ") => {
            DaemonHydrationFailure::SafeToFallback(message)
        }
        // A transport failure can happen after acceptance. A generic daemon
        // error is uncertain too: reader/writer setup happens after the child
        // is spawned, and "already exists" proves a daemon session is live.
        // Treat every other variant as retry-only rather than duplicating it.
        _ => DaemonHydrationFailure::Ambiguous(message),
    }
}

fn classify_attach_rpc_failure(error: PtyDaemonError) -> DaemonHydrationFailure {
    // Attach follows either a successful Spawn or a List result proving the
    // session already exists, so no attach error permits a replacement spawn.
    DaemonHydrationFailure::Ambiguous(format!("daemon attach: {error}"))
}

/// Classify a failure to acquire the workspace's daemon client. At this stage
/// errors that mean "nothing usable is listening" — connection refused,
/// missing socket, daemon spawn failure, circuit breaker open — safely permit
/// a local in-process shell, which keeps normal cold-start working when the
/// daemon is absent or disabled. A timeout or an accept-then-EOF, however,
/// means something answered the dial: possibly a stalled daemon that still
/// owns live shells, so those stay retry-only.
fn classify_client_acquisition_failure(error: PtyDaemonError) -> DaemonHydrationFailure {
    let message = format!("daemon client: {error}");
    match error {
        PtyDaemonError::Timeout | PtyDaemonError::Closed => {
            DaemonHydrationFailure::Ambiguous(message)
        }
        _ => DaemonHydrationFailure::SafeToFallback(message),
    }
}

/// Classify a failure of the workspace-level List RPC. List only runs on a
/// client whose connection already succeeded, so a daemon process exists; no
/// List error — least of all a timeout against a stalled-but-alive daemon —
/// proves the sessions are absent. Spawning local replacements here would
/// duplicate live shells and orphan their agents, so every variant is
/// retry-only, matching the spawn/attach classifiers above.
fn classify_list_rpc_failure(error: PtyDaemonError) -> DaemonHydrationFailure {
    DaemonHydrationFailure::Ambiguous(format!("daemon list: {error}"))
}

async fn list_daemon_session_map(
    client: &PtyDaemonClient,
) -> Result<HashMap<String, DaemonSessionInfo>, crate::pty_daemon::PtyDaemonError> {
    Ok(client
        .list()
        .await?
        .into_iter()
        .map(|session| (session.session_id.clone(), session))
        .collect())
}

async fn has_durable_local_fallback<R: Runtime>(
    app: &AppHandle<R>,
    workspace_id: &str,
    session_id: &str,
) -> Result<bool, DaemonHydrationFailure> {
    let app = app.clone();
    let workspace_id = workspace_id.to_owned();
    let session_id = session_id.to_owned();
    tokio::task::spawn_blocking(move || {
        app.try_state::<DatabaseStore>().map_or(Ok(false), |db| {
            db.has_local_terminal_fallback(&workspace_id, &session_id)
        })
    })
    .await
    .map_err(|error| error.to_string())
    .and_then(|result| result)
    .map_err(|error| {
        DaemonHydrationFailure::Ambiguous(format!(
            "could not read the local terminal fallback record: {error}"
        ))
    })
}

async fn replay_pending_local_restart<R: Runtime>(
    app: &AppHandle<R>,
    workspace_id: &str,
    client: &Arc<PtyDaemonClient>,
    existing: Option<&DaemonSessionInfo>,
    reservation: &SessionSpawnReservation,
) -> Result<LocalRestartReplay, DaemonHydrationFailure> {
    let session_id = &reservation.session_id;
    let read_pending = {
        let app = app.clone();
        let workspace_id = workspace_id.to_owned();
        let session_id = session_id.clone();
        tokio::task::spawn_blocking(move || {
            app.try_state::<DatabaseStore>().map_or(Ok(None), |db| {
                db.pending_local_terminal_restart(&workspace_id, &session_id)
            })
        })
        .await
        .map_err(|error| error.to_string())
        .and_then(|result| result)
    };
    let pending = match read_pending {
        Ok(Some(pending)) => pending,
        Ok(None) => return Ok(LocalRestartReplay::None),
        Err(error) if existing.is_some() => {
            log::warn!("Could not check pending local restart for {session_id}: {error}");
            return Ok(LocalRestartReplay::None);
        }
        Err(error) => return Err(DaemonHydrationFailure::Ambiguous(format!(
            "could not read the pending terminal restart: {error}"
        ))),
    };

    if let Some(recovery) = &reservation.reconciliation {
        if recovery.route.workspace_id != workspace_id
            || recovery.route.host_id.is_some()
            || recovery.route.ssh_target.is_some()
            || recovery.shells_for_close().as_slice() != pending.shell_id.as_slice()
        {
            return Ok(LocalRestartReplay::None);
        }
    }
    if pending.replacement_active {
        // An earlier attempt committed the successor before the layout
        // changed. Finish that handoff without contacting the old shell.
        return Ok(LocalRestartReplay::UseReplacement);
    }
    if pending.shell_id.is_none()
        && !pending.acknowledged
        && reservation.reconciliation.is_some()
        && existing.is_some_and(|shell| {
            shell.shell_id.is_none()
                && shell.workspace_id == workspace_id
                && shell.session_id == session_id.as_str()
        })
    {
        // The local slot survived a lost Close. The journal and inventory
        // allow reattachment to it, without replaying an unguarded Close.
        return Ok(LocalRestartReplay::AttachSurvivingTokenless);
    }
    if existing.is_some_and(|shell| pending.shell_id.is_none() || shell.shell_id != pending.shell_id) {
        // Preserve a live tokenless shell or a different guarded shell.
        return Ok(LocalRestartReplay::None);
    }
    if pending.shell_id.is_none() && !pending.acknowledged {
        // List omits in-flight spawns. The journal reserved another session
        // ID so the pane can recover without taking over this daemon identity.
        return Ok(LocalRestartReplay::UseReplacement);
    }
    let retired_shell = pending.shell_id.map(|shell_id| PendingDaemonClose {
        route: DaemonWorkspaceRoute {
            workspace_id: workspace_id.to_owned(),
            host_id: None,
            ssh_target: None,
        },
        session_id: session_id.clone(),
        shell_id,
    });

    if !pending.acknowledged || existing.is_some() {
        // Only a guarded close can be replayed without touching a replacement.
        let Some(shell_id) = pending.shell_id.filter(|_| client.supports_guarded_shells()) else {
            return Err(DaemonHydrationFailure::RequiresGuardedRecovery);
        };
        let mut target = match &reservation.reconciliation {
            Some(recovery) => Some(recovery.target.lock().await),
            None => None,
        };
        loop {
            reservation.wait_for_close().await;
            match reservation.with_current_runtime(|runtime| runtime.close_fence.is_none()) {
                Some(true) => break,
                Some(false) => continue,
                None => return Err(DaemonHydrationFailure::AlreadyReserved),
            }
        }
        if !owns_local_terminal(app, workspace_id, session_id) {
            return Err(DaemonHydrationFailure::AlreadyReserved);
        }
        if let Some(target) = &mut target {
            target.client = Some(client.clone());
        }
        client.close_owned(session_id.clone(), shell_id).await.map_err(|error| {
            DaemonHydrationFailure::Ambiguous(format!("could not replay terminal restart: {error}"))
        })?;
        if let Some(recovery) = &reservation.reconciliation {
            recovery.acknowledge_shell_closed(shell_id);
            let _ = recovery.retired_local_shell.set(super::RetiredLocalDaemonShell {
                shell_id: Some(shell_id),
            });
        }
    }

    let gate = app.state::<PtyState>().daemon_ownership_gate(session_id);
    loop {
        reservation.wait_for_close().await;
        if !reservation.is_current() {
            return Err(DaemonHydrationFailure::AlreadyReserved);
        }
        let app = app.clone();
        let workspace_id = workspace_id.to_owned();
        let session_id = session_id.clone();
        let gate = gate.clone();
        let sessions = reservation.sessions.clone();
        let restart_token = reservation.restart_token;
        let retired_shell = retired_shell.clone();
        let recorded = tokio::task::spawn_blocking(move || {
            let _lease = gate.lock().unwrap_or_else(|error| error.into_inner());
            let admitted = with_existing_session_runtime(&sessions, &session_id, |runtime| {
                runtime.restart_token == restart_token && runtime.close_fence.is_none()
            }).unwrap_or(false);
            if !admitted {
                return Ok(false);
            }
            if !owns_local_terminal(&app, &workspace_id, &session_id) {
                return Err("The terminal moved or closed while recovering its restart".to_owned());
            }
            let db = app.try_state::<DatabaseStore>()
                .ok_or_else(|| "The terminal fallback database is unavailable".to_owned())?;
            if !pending.acknowledged {
                db.acknowledge_local_terminal_restart(&workspace_id, &session_id, pending.shell_id)?;
            }
            db.remember_local_terminal_fallback_after_daemon_close(
                &workspace_id,
                &session_id,
                retired_shell.as_ref(),
            )?;
            Ok(true)
        })
        .await
        .map_err(|error| error.to_string())
        .and_then(|result| result)
        .map_err(DaemonHydrationFailure::OwnershipUnavailable)?;
        if recorded {
            return Ok(LocalRestartReplay::Retired);
        }
    }
}

async fn spawn_tokenless_restart_replacement<R: Runtime>(
    app: AppHandle<R>,
    workspace_id: String,
    mut reservation: SessionSpawnReservation,
) -> Result<bool, DaemonHydrationFailure> {
    reservation.remove_on_drop = false;
    loop {
        reservation.wait_for_close().await;
        if !reservation.is_current() {
            return Err(DaemonHydrationFailure::AlreadyReserved);
        }

        let worker_app = app.clone();
        let workspace_id = workspace_id.clone();
        let session_id = reservation.session_id.clone();
        let restart_token = reservation.restart_token;
        let sessions = reservation.sessions.clone();
        let replaced = tauri::async_runtime::spawn_blocking(move || {
            let store = worker_app.state::<AppStateStore>();
            let db = worker_app.try_state::<DatabaseStore>()
                .ok_or_else(|| "The terminal fallback database is unavailable".to_owned())?;
            let gate = worker_app.state::<PtyState>().daemon_ownership_gate(&session_id);
            let replacement_token = Uuid::new_v4();
            let mut retired_runtime = None;
            let replaced = crate::state::with_terminal_close_layout_lock(|| {
                let _lease = gate.lock().unwrap_or_else(|error| error.into_inner());
                let admitted = with_existing_session_runtime(&sessions, &session_id, |runtime| {
                    runtime.restart_token == restart_token && runtime.close_fence.is_none()
                }).unwrap_or(false);
                if !admitted {
                    return Ok(None);
                }
                if !owns_local_terminal(&worker_app, &workspace_id, &session_id) {
                    return Err("The terminal moved or closed while recovering its restart".into());
                }
                let replacement_id = db.activate_tokenless_local_replacement(&workspace_id, &session_id)?;
                let mapping = PendingTerminalReplacement {
                    workspace_id,
                    old_session_id: session_id,
                    replacement_id,
                };
                let replacement_session_id = mapping.replacement_session_id();
                let changed = store.replace_terminal_session(
                    &mapping.workspace_id,
                    &mapping.old_session_id,
                    &replacement_session_id,
                    || {
                        let mut runtimes = sessions.lock().unwrap_or_else(|error| error.into_inner());
                        if !runtimes.get(&mapping.old_session_id).is_some_and(|runtime| {
                            runtime.restart_token == restart_token && runtime.close_fence.is_none()
                        }) || runtimes.contains_key(&replacement_session_id) {
                            return false;
                        }
                        retired_runtime = runtimes.remove(&mapping.old_session_id);
                        let mut successor = super::SessionRuntime::new(&replacement_session_id);
                        successor.restart_token = replacement_token;
                        runtimes.insert(replacement_session_id.clone(), successor);
                        true
                    },
                )?;
                if !changed {
                    return Err("The terminal changed while restoring its replacement".into());
                }
                Ok::<_, String>(Some((mapping, replacement_session_id)))
            })?;
            drop(retired_runtime);
            let Some((mapping, replacement_session_id)) = replaced else {
                return Ok(None);
            };
            if let Err(error) = crate::state::persist_terminal_replacements(&store, &db, &[mapping]) {
                log::warn!("Could not persist terminal replacement for {replacement_session_id}: {error}");
            }
            crate::state::emit_app_state(&worker_app);
            Ok::<_, String>(Some(super::spawn_pty_for_session_in_process(
                worker_app.clone(),
                replacement_session_id,
                Some(replacement_token),
                true,
            )))
        })
        .await
        .map_err(|error| error.to_string())
        .and_then(|result| result)
        .map_err(DaemonHydrationFailure::OwnershipUnavailable)?;
        if let Some(ready) = replaced {
            return Ok(ready);
        }
    }
}

fn owns_local_terminal<R: Runtime>(app: &AppHandle<R>, workspace_id: &str, session_id: &str) -> bool {
    app.state::<AppStateStore>()
        .pty_hydration_plan_for_session(session_id)
        .is_some_and(|plan| plan.workspace.workspace_id == workspace_id && plan.workspace.host_id.is_none())
}

async fn forget_fallback_for_listed_shell<R: Runtime>(
    app: &AppHandle<R>,
    workspace_id: &str,
    session_id: &str,
    gate: Arc<Mutex<()>>,
) {
    let app = app.clone();
    let workspace_id = workspace_id.to_owned();
    let checked_session = session_id.to_owned();
    let result = tokio::task::spawn_blocking(move || {
        let Some(db) = app.try_state::<DatabaseStore>() else { return Ok(()); };
        let _lease = gate.lock().unwrap_or_else(|error| error.into_inner());
        db.forget_local_terminal_fallback(&workspace_id, &checked_session)
    })
    .await
    .map_err(|error| error.to_string())
    .and_then(|result| result);
    if let Err(error) = result {
        log::warn!("Could not retire local fallback proof for existing daemon shell {session_id}: {error}");
    }
}

/// Compatibility entry point for a newly-created single pane. Restored
/// workspaces call [`hydrate_workspace_via_daemon`] with all descriptors at
/// once, which is what enables the one-List invariant.
pub(crate) async fn spawn_pty_for_session_via_daemon<R: Runtime>(
    app: AppHandle<R>,
    session_id: String,
) -> Result<(), DaemonHydrationFailure> {
    let state: State<'_, AppStateStore> = app.state();
    let plan = state
        .pty_hydration_plan_for_session(&session_id)
        .ok_or_else(|| {
            DaemonHydrationFailure::SafeToFallback(
                "terminal session has no owning workspace".to_string(),
            )
        })?;
    let failures = hydrate_workspace_via_daemon(app, plan, None).await?;
    failures.failures
        .into_iter()
        .next()
        .map(|(_, error)| Err(error))
        .unwrap_or(Ok(()))
}

/// Hydrate every missing terminal in one workspace using a single narrow state
/// plan, one daemon client, and exactly one daemon List. Attach/spawn RPCs fan
/// out only after that map exists and are capped to a small concurrency.
///
/// The outer caller decides local fallback vs remote Failed UI for returned
/// per-session failures. A setup/List error happens before any reservation;
/// failures after reservation are cleaned by `SessionSpawnReservation`.
/// Workspace-level errors come back pre-classified: only client-acquisition
/// errors proving the daemon is absent allow a local fallback, while List
/// failures (including timeouts) are always retry-only because the daemon
/// may still own every listed shell.
pub(super) async fn hydrate_workspace_via_daemon<R: Runtime>(
    app: AppHandle<R>,
    plan: PtyHydrationPlan,
    restart: Option<SessionSpawnReservation>,
) -> Result<DaemonHydrationResult, DaemonHydrationFailure> {
    let hydration_started = std::time::Instant::now();
    let terminal_state: State<'_, PtyState> = app.state();
    let sessions = terminal_state.sessions.clone();
    let pending: Vec<_> = match restart {
        // Reconnect already owns this session. Keep that reservation while
        // acquiring the daemon and reading its inventory.
        Some(reservation) => plan
            .sessions
            .into_iter()
            .find(|session| session.session_id == reservation.session_id)
            .filter(|_| reservation.is_current())
            .map(|session| (session, DaemonSpawnClaim::Restart(reservation)))
            .into_iter()
            .collect(),
        None => plan
            .sessions
            .into_iter()
            .filter_map(|session| {
                let (token, pending_close) =
                    match super::idle_restart_token(&sessions, &session.session_id) {
                        Ok(token) => (token, None),
                        Err(SessionSpawnBlocked::ClosePending { fence, restart_token }) =>
                            (restart_token, Some(fence)),
                        Err(SessionSpawnBlocked::Superseded) => return None,
                    };
                Some((session, DaemonSpawnClaim::Hydrate { token, pending_close }))
            })
            .collect(),
    };
    if pending.is_empty() {
        crate::diagnostics::record_perf_timing(
            "pty.workspace-hydration",
            hydration_started.elapsed(),
        );
        return Ok(DaemonHydrationResult::default());
    }

    let workspace = Arc::new(plan.workspace);
    let is_remote = workspace.host_id.is_some();
    if is_remote {
        for (session, claim) in &pending {
            let token = match claim {
                DaemonSpawnClaim::Hydrate { token, pending_close: None } => *token,
                DaemonSpawnClaim::Hydrate { pending_close: Some(_), .. } => continue,
                DaemonSpawnClaim::Restart(reservation) => reservation.restart_token,
            };
            emit_terminal_status_if_current(
                &app,
                &sessions,
                TerminalStatusPayload {
                    session_id: session.session_id.clone(),
                    state: TerminalLifecycleState::Starting,
                    message: Some(
                        "Connecting to remote host (this can take up to 20s on first connect)…"
                            .into(),
                    ),
                    exit_code: None,
                },
                Some(token),
            );
        }
    }

    // A device workspace's terminals start in an absolute directory there.
    // Resolving it can cost an SSH round trip, so it overlaps the tunnel
    // setup and is dropped if the client can't be acquired.
    let device_dir = async {
        Ok::<_, PtyDaemonError>(if is_remote {
            Some(resolve_device_spawn_dir(&app, &workspace).await)
        } else {
            None
        })
    };
    let (client, device_dir) = match tokio::try_join!(
        crate::ssh::client_for_workspace(&app, &workspace.workspace_id, workspace.host_id),
        device_dir,
    ) {
        Ok(acquired) => acquired,
        Err(error) => {
            crate::diagnostics::record_perf_timing(
                "pty.workspace-hydration",
                hydration_started.elapsed(),
            );
            return Err(classify_client_acquisition_failure(error));
        }
    };

    let list_started = std::time::Instant::now();
    let listed = list_daemon_session_map(&client).await;
    crate::diagnostics::record_perf_timing("pty.daemon-list", list_started.elapsed());
    let listed = match listed {
        Ok(listed) => listed,
        Err(error) => {
            crate::diagnostics::record_perf_timing(
                "pty.workspace-hydration",
                hydration_started.elapsed(),
            );
            return Err(classify_list_rpc_failure(error));
        }
    };
    let mut existing = listed;

    // Filesystem-backed scrollback lookup, CLI-shim creation, and workspace
    // env derivation stay off the async worker. One preparation task serves
    // every pane in the workspace.
    let prep_workspace = workspace.clone();
    let prep_sessions: Vec<_> = pending.iter().map(|(session, _)| session.clone()).collect();
    let prep_device_dir = device_dir.clone();
    let prepared = tokio::task::spawn_blocking(move || {
        let mut workspace_env =
            hydration_workspace_pty_env(&prep_workspace, prep_device_dir.as_deref());
        if !is_remote {
            if let Some((shim_dir, current_exe)) = super::ensure_cli_shims() {
                let current_path = crate::execution::sanitized_child_path();
                let prefixed = super::build_child_path(&shim_dir, &current_path);
                workspace_env.push(("PATH".into(), prefixed));
                workspace_env.push(("CODEMUX_CLI_SAFE_PATH".into(), current_exe));
            }
            if let Some((key, value)) = super::claude_plugin_env() {
                workspace_env.push((key.into(), value));
            }
        }
        // One directory walk for the whole workspace: the per-session lookup
        // re-read and re-parsed every meta file on disk for each pane.
        let wanted: std::collections::HashSet<&str> = prep_sessions
            .iter()
            .map(|session| session.session_id.as_str())
            .collect();
        let mut disk_meta = crate::scrollback::find_scrollback_meta_for_sessions(&wanted);
        if !is_remote {
            for session in &prep_sessions {
                let Some((pane_id, predecessor_id)) = session.pane_id.as_deref()
                    .zip(session.restore_predecessor_session_id.as_deref())
                else {
                    continue;
                };
                if let Entry::Vacant(entry) = disk_meta.entry(session.session_id.clone()) {
                    if let Some(meta) = crate::scrollback::load_scrollback_meta_for_predecessor(
                        &prep_workspace.workspace_id, pane_id, predecessor_id,
                    ) {
                        entry.insert(meta);
                    }
                }
            }
        }
        let shell = if is_remote {
            "bash".to_string()
        } else {
            super::default_shell()
        };
        let session_restore_enabled = crate::settings_sync::load_cache()
            .map(|settings| settings.session_restore.enabled)
            .unwrap_or(true);
        (
            Arc::new(workspace_env),
            disk_meta,
            shell,
            session_restore_enabled,
        )
    })
    .await;
    let prepared = match prepared {
        Ok(prepared) => prepared,
        Err(_) => {
            crate::diagnostics::record_perf_timing(
                "pty.workspace-hydration",
                hydration_started.elapsed(),
            );
            // The daemon is alive (List just succeeded) and may own live
            // shells, so this process-local failure is retry-only.
            return Err(DaemonHydrationFailure::Ambiguous(
                "PTY hydration preparation worker panicked".to_string(),
            ));
        }
    };
    let (workspace_env, mut disk_meta, shell, session_restore_enabled) = prepared;

    let mut work = Vec::with_capacity(pending.len());
    for (session, claim) in pending {
        let prior = existing.remove(&session.session_id);
        let meta = disk_meta.remove(&session.session_id);
        let session_id = session.session_id.clone();
        let app = app.clone();
        let workspace = workspace.clone();
        let client = client.clone();
        let workspace_env = workspace_env.clone();
        let device_dir = device_dir.clone();
        let shell = shell.clone();
        work.push(async move {
            let result = spawn_prepared_session(
                app,
                workspace,
                session,
                client,
                prior,
                meta,
                workspace_env,
                device_dir,
                shell,
                session_restore_enabled,
                claim,
            )
            .await;
            (session_id, result)
        });
    }

    let results: Vec<_> = futures_util::stream::iter(work)
        .buffer_unordered(HYDRATION_CONCURRENCY)
        .collect()
        .await;
    crate::diagnostics::record_perf_timing("pty.workspace-hydration", hydration_started.elapsed());
    let mut outcome = DaemonHydrationResult::default();
    for (session_id, result) in results {
        match result {
            Ok(DaemonSpawnOutcome::DaemonShell) => {}
            Ok(DaemonSpawnOutcome::LocalRestart { ready }) => outcome.restarted_locally |= ready,
            Err(error) => outcome.failures.push((session_id, error)),
        }
    }
    Ok(outcome)
}

#[derive(Clone)]
struct GuardedShellOwnership {
    shell: PendingDaemonClose,
    recovery: Arc<DaemonReconciliation>,
    gate: Arc<Mutex<()>>,
    sessions: Arc<Mutex<HashMap<String, super::SessionRuntime>>>,
    restart_token: Uuid,
}

#[derive(Clone, Copy)]
enum ShellResolution {
    Current,
    Rejected { allow_ordinary_retry: bool },
}

impl GuardedShellOwnership {
    fn with_current_runtime<T>(&self, work: impl FnOnce(&mut super::SessionRuntime) -> T) -> Option<T> {
        with_existing_session_runtime(&self.sessions, &self.shell.session_id, |runtime| {
            (runtime.restart_token == self.restart_token).then(|| work(runtime))
        }).flatten()
    }

    async fn remember<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        reattached: bool,
        reconciling: bool,
    ) -> Result<bool, DaemonHydrationFailure> {
        let owner = self.clone();
        let app = app.clone();
        tokio::task::spawn_blocking(move || {
            let _lease = owner.gate.lock().unwrap_or_else(|error| error.into_inner());
            let registered = owner.with_current_runtime(|runtime| {
                if runtime.close_fence.is_some() {
                    return Ok(false);
                }
                if !owner.recovery.register_shell(owner.shell.shell_id) {
                    return Err(DaemonHydrationFailure::AlreadyReserved);
                }
                Ok(true)
            }).unwrap_or(Err(DaemonHydrationFailure::AlreadyReserved))?;
            if !registered {
                return Ok(false);
            }

            // Close observes the registered ID and waits for this lease
            // before transferring ownership. The write also retires local
            // fallback proof before any RPC can create the shell.
            let remembered = app.try_state::<DatabaseStore>()
                .ok_or_else(|| "Terminal shell ownership database is unavailable".to_owned())
                .and_then(|db| db.remember_daemon_shell(&owner.shell));
            if let Err(error) = remembered {
                if !reattached {
                    owner.with_current_runtime(|runtime| {
                        owner.recovery.acknowledge_shell_closed(owner.shell.shell_id);
                        if !reconciling {
                            runtime.daemon_reconciliation = None;
                        }
                    });
                    // List omits active spawns. A fresh local shell could
                    // duplicate another shell whose outcome is unknown.
                    return Err(DaemonHydrationFailure::OwnershipUnavailable(error));
                }
                // Attachment does not create a shell. The runtime retains
                // the exact cleanup target if disk is unavailable.
                log::error!("Could not remember existing daemon shell for {}: {error}", owner.shell.session_id);
            }
            Ok(true)
        }).await.map_err(|error| DaemonHydrationFailure::OwnershipUnavailable(error.to_string()))?
    }

    async fn resolve<R: Runtime>(&self, app: &AppHandle<R>, resolution: ShellResolution) {
        let owner = self.clone();
        let app = app.clone();
        if let Err(error) = tokio::task::spawn_blocking(move || {
            let _lease = owner.gate.lock().unwrap_or_else(|error| error.into_inner());
            let can_update = owner.with_current_runtime(|runtime| {
                match resolution {
                    ShellResolution::Current => owner.recovery.retain_shell(owner.shell.shell_id),
                    ShellResolution::Rejected { allow_ordinary_retry } => {
                        owner.recovery.acknowledge_shell_closed(owner.shell.shell_id);
                        if allow_ordinary_retry {
                            runtime.daemon_reconciliation = None;
                        }
                    }
                }
                runtime.close_fence.is_none()
            }).unwrap_or(false);
            if can_update {
                if let Some(db) = app.try_state::<DatabaseStore>() {
                    let result = match resolution {
                        ShellResolution::Current => db.retain_daemon_shell(&owner.shell),
                        ShellResolution::Rejected { .. } => db.forget_daemon_shell(&owner.shell),
                    };
                    if let Err(error) = result {
                        log::error!("Could not update daemon shell ownership for {}: {error}", owner.shell.session_id);
                    }
                }
            }
        }).await {
            log::error!("Daemon shell ownership worker failed for {}: {error}", self.shell.session_id);
        }
    }
}

/// Daemon-backed shell spawn — the persistent equivalent of
/// `spawn_pty_for_session_in_process`. Mirrors the env construction,
/// scrollback restore, and session-adapter wiring of the in-process path
/// so user-typed commands inside the shell get the same Codemux context
/// AND reopening a previously-killed agent triggers the same
/// `claude --continue` / adapter-driven resume the in-process path does.
/// `device_dir` is where a device workspace's terminals start there, the
/// directory `workspace_env` names; `None` for a local workspace.
async fn spawn_prepared_session<R: Runtime>(
    app: AppHandle<R>,
    workspace: Arc<PtyHydrationWorkspace>,
    session: PtyHydrationSession,
    client: Arc<PtyDaemonClient>,
    existing: Option<DaemonSessionInfo>,
    disk_meta: Option<(String, String, crate::scrollback::ScrollbackMeta)>,
    workspace_env: Arc<Vec<(String, String)>>,
    device_dir: Option<String>,
    shell: String,
    session_restore_enabled: bool,
    claim: DaemonSpawnClaim,
) -> Result<DaemonSpawnOutcome, DaemonHydrationFailure> {
    let session_id = session.session_id.clone();
    let entry_ts = std::time::Instant::now();
    crate::trace_cloud_push!("[trace:{session_id}] spawn_via_daemon ENTRY t=0ms");
    let terminal_state: State<'_, PtyState> = app.state();
    let app_state: State<'_, AppStateStore> = app.state();
    let sessions = terminal_state.sessions.clone();

    let reservation = match claim {
        DaemonSpawnClaim::Hydrate { token, mut pending_close } => loop {
            if let Some(fence) = pending_close.take() {
                #[cfg(test)]
                super::tests::notify_hydration_waiting_for_close(&session_id);
                // A rollback leaves this generation eligible. An accepted
                // close can remove even a runtime whose generation was nil.
                if fence.wait_for_commit().await {
                    break None;
                }
            }
            match SessionSpawnReservation::try_new(&sessions, &session_id, Some(token)) {
                Ok(reservation) => break Some(reservation),
                Err(SessionSpawnBlocked::ClosePending { fence, .. }) => pending_close = Some(fence),
                Err(SessionSpawnBlocked::Superseded) => break None,
            }
        },
        DaemonSpawnClaim::Restart(reservation) => Some(reservation),
    };
    let Some(mut reservation) = reservation else {
        crate::trace_cloud_push!(
            "[trace:{session_id}] try_reserve FAILED at t={}ms",
            entry_ts.elapsed().as_millis()
        );
        return Err(DaemonHydrationFailure::AlreadyReserved);
    };
    let restart_token = reservation.restart_token;
    let reconciling = reservation.reconciliation.is_some();
    if reservation.reconciliation.as_ref().is_some_and(|recovery| {
        recovery.route.ssh_target.as_deref() != client.ssh_target()
    }) {
        return Err(DaemonHydrationFailure::Ambiguous(
            "the workspace daemon destination changed while recovering this shell".into(),
        ));
    }
    if !reservation.is_current() {
        return Err(DaemonHydrationFailure::AlreadyReserved);
    }
    if existing.as_ref().is_some_and(|shell| shell.workspace_id != workspace.workspace_id) {
        return Err(DaemonHydrationFailure::Ambiguous(
            "the daemon session belongs to a different workspace".into(),
        ));
    }
    let attach_surviving_tokenless = if !session.daemon_unused
        && workspace.host_id.is_none()
        && client.ssh_target().is_none()
        && !client.supports_prepared_spawn()
    {
        match replay_pending_local_restart(
            &app,
            &workspace.workspace_id,
            &client,
            existing.as_ref(),
            &reservation,
        ).await? {
            LocalRestartReplay::None => false,
            LocalRestartReplay::AttachSurvivingTokenless => true,
            LocalRestartReplay::Retired => {
                let ready = super::spawn_retired_local_shell(app, reservation)
                    .await
                    .map_err(DaemonHydrationFailure::Ambiguous)?;
                return Ok(DaemonSpawnOutcome::LocalRestart { ready });
            }
            LocalRestartReplay::UseReplacement => {
                let ready = spawn_tokenless_restart_replacement(
                    app,
                    workspace.workspace_id.clone(),
                    reservation,
                ).await?;
                return Ok(DaemonSpawnOutcome::LocalRestart { ready });
            }
        }
    } else {
        false
    };
    let reattached = existing.is_some();
    let shell_id = match (&existing, client.supports_guarded_shells()) {
        (Some(existing), true) => existing.shell_id,
        (Some(_), false) => None,
        (None, _) if client.supports_prepared_spawn() => Some(Uuid::new_v4()),
        (None, _) => {
            // A new shell needs a permit the daemon can revoke if its pane
            // closes before Spawn arrives. List omits in-flight spawns,
            // so a restored session needs stronger proof for local fallback.
            if workspace.host_id.is_none() && !reconciling {
                let can_fallback = if session.daemon_unused {
                    Ok(true)
                } else {
                    has_durable_local_fallback(&app, &workspace.workspace_id, &session_id).await
                };
                if !reservation.is_current() {
                    return Err(DaemonHydrationFailure::AlreadyReserved);
                }
                if can_fallback? {
                    return Err(DaemonHydrationFailure::LegacyDaemonLocalFallback);
                }
            }
            return Err(DaemonHydrationFailure::RequiresGuardedRecovery);
        }
    };
    if reconciling && shell_id.is_none() && !attach_surviving_tokenless {
        // Recovery can outlive the pane and must own its exact shell.
        return Err(DaemonHydrationFailure::RequiresGuardedRecovery);
    }
    let workspace_id = workspace.workspace_id.clone();
    let host_id = workspace.host_id;
    let is_remote = host_id.is_some();
    if !app_state.update_terminal_session_shell(&session_id, shell.clone(), true) {
        return Err(DaemonHydrationFailure::AlreadyReserved);
    }
    if reattached && shell_id.is_none() {
        // A listed shell disproves local-only ownership even when an older
        // daemon cannot supply a UUID for the guarded ownership write.
        forget_fallback_for_listed_shell(
            &app,
            &workspace_id,
            &session_id,
            terminal_state.daemon_ownership_gate(&session_id),
        ).await;
        if !reservation.is_current() {
            return Err(DaemonHydrationFailure::AlreadyReserved);
        }
    }
    let route_for_runtime = shell_id.map(|_| {
        if let Some(recovery) = &reservation.reconciliation {
            return recovery.route.clone();
        }
        let ssh_target = client.ssh_target().map(str::to_owned);
        DaemonWorkspaceRoute {
            workspace_id: workspace_id.clone(),
            host_id: host_id.filter(|_| ssh_target.is_some()),
            ssh_target,
        }
    });
    if let Some((route, shell_id)) = route_for_runtime.as_ref().zip(shell_id).filter(|_| !reconciling) {
        let recovery = Arc::new(DaemonReconciliation::new(
            route.clone(),
            Some(shell_id),
            Some(client.clone()),
        ));
        if !reservation.publish_daemon_reconciliation(recovery) {
            return Err(DaemonHydrationFailure::AlreadyReserved);
        }
    }

    // ── Scrollback restore + adapter resume parity with in-process path.
    //
    // If there's saved scrollback for this session id, and the session-
    // restore setting is on, we (a) use the original cwd so CWD-scoped
    // tools like `claude --resume` find their state, and (b) capture an
    // `auto_resume_command` that we'll write into the shell after spawn.
    // Mirrors `spawn_pty_for_session_in_process` lines around 1166-1200.
    // Remote workspaces spawn into their directory on the host
    // (`device_dir`: `remote_cwd` or the conventional
    // `~/.codemux/worktrees/<project>/<branch>`, made absolute) rather
    // than the local cwd — the workspace's `cwd` field is a
    // local-filesystem path that doesn't exist on the remote host. Local
    // workspaces keep using the local cwd as before.
    let mut effective_cwd = if let Some(computed) = device_dir {
        crate::trace_cloud_push!(
            "[codemux::terminal::daemon_backed] remote cwd for {session_id}: \
             {computed} (attach_only={}, remote_cwd={:?}, \
             project_root={:?}, git_branch={:?})",
            workspace.attach_only,
            workspace.remote_cwd.as_deref(),
            workspace.project_root.as_deref(),
            workspace.git_branch.as_deref(),
        );
        computed
    } else {
        session.cwd.clone()
    };
    let mut auto_resume_command: Option<String> = None;
    let mut pane_id_for_env = session.pane_id.clone();

    // Scrollback restore + adapter relaunch.
    //
    // LOCAL: full resume — the scrollback meta lives on this disk, the
    // adapter's captured session id (e.g. Claude's UUID) is in
    // adapter_captures, and Claude's `~/.claude/projects/<encoded-cwd>/`
    // JSONLs are reachable. So we land in the original cwd and inject
    // `<original> --resume <uuid>` so Claude continues the conversation.
    //
    // REMOTE: best-effort relaunch — same scrollback lookup (still on the
    // laptop's disk, that's fine), but we do NOT use the original local
    // cwd (path doesn't exist on the remote) and we do NOT append
    // `--resume <uuid>` (Claude's per-project JSONLs aren't synced to the
    // remote yet, so --resume would fail with "session not found").
    // Instead we inject the bare `original_command` so Claude (or
    // whichever adapter) at least starts on the remote with a fresh
    // conversation. Honest UX given today's constraint.
    //
    // TODO (Tier 2): sync `~/.claude/projects/<encoded-local-cwd>/` →
    // remote `~/.claude/projects/<encoded-remote-cwd>/` during the
    // push flow, then re-enable the `--resume <uuid>` suffix for
    // remote. Needs: (a) discover remote $HOME on first connect;
    // (b) determine Claude's path-encoding rule from its source;
    // (c) rsync the per-project JSONLs with path translation.
    if session_restore_enabled {
        if let Some((_, ref pane_id, _)) = disk_meta {
            pane_id_for_env = Some(pane_id.clone());
        }

        // For the agent-relaunch command, prefer the IN-MEMORY snapshot
        // because the disk-side scrollback meta is only persisted on
        // explicit close (via flush_cache_to_disk) — not on every
        // keystroke. So a user who opens Claude, sends one message,
        // and immediately pushes has no disk meta yet, and the disk
        // lookup returns None. The in-memory snapshot has the original
        // command from the moment the preset was applied
        // (update_terminal_session_command in commands/presets.rs).
        let in_memory_original = session.original_command.clone();

        if is_remote {
            // Remote: keep the conventional remote cwd; relaunch with a
            // CURATED subset of the original command's args, NOT the full
            // thing. The full command often carries laptop-specific args
            // like `--system-prompt "$CODEMUX_AGENT_CONTEXT"` that the agent
            // on the remote rejects (different version, different env
            // content). `build_agent_relaunch_command` keeps only the binary
            // plus the per-agent resume args:
            //   - claude → [--dangerously-skip-permissions] [--resume <uuid>]
            //     (the JSONLs were rsynced by the push flow so --resume
            //     continues the conversation).
            //   - opencode → --session <id> when the push synced one (id in
            //     adapter_captures), else --continue (issue #16; the host's
            //     opencode.db received the session via export/import).
            let full = in_memory_original.clone().or_else(|| {
                disk_meta
                    .as_ref()
                    .and_then(|(_, _, m)| m.original_command.clone())
            });
            // Resume identifiers captured in the in-memory snapshot:
            // claude's hook-captured UUID, opencode's push-synced session id.
            let claude_uuid = session.adapter_captures.get("claude_session_id").cloned();
            let opencode_session_id = session.adapter_captures.get("opencode_session_id").cloned();
            // GATE: only synthesize a relaunch command when there's evidence
            // this is genuinely a relaunch (persisted disk meta or a captured
            // agent session id). If neither is set we're racing a fresh
            // `apply_preset` call that will write the command itself —
            // synthesizing here would duplicate the write and leak the
            // command into the agent's input box (the Linux preset-leak bug).
            // See the local branch below for the full rationale.
            let has_evidence = should_synthesize_agent_relaunch(
                disk_meta.is_some(),
                claude_uuid.is_some() || opencode_session_id.is_some(),
            );
            let cmd_opt = if has_evidence {
                build_agent_relaunch_command(
                    full.as_deref(),
                    claude_uuid.as_deref(),
                    opencode_session_id.as_deref(),
                )
            } else {
                None
            };
            if let Some(cmd) = cmd_opt {
                eprintln!(
                    "[codemux::terminal::daemon_backed] remote relaunch for {session_id}: \
                     {cmd} (has_claude_uuid={}, has_opencode_sid={}; \
                     in_memory={}, disk_meta={})",
                    claude_uuid.is_some(),
                    opencode_session_id.is_some(),
                    in_memory_original.is_some(),
                    disk_meta.is_some(),
                );
                auto_resume_command = Some(cmd);
            } else if has_evidence {
                crate::trace_cloud_push!(
                    "[codemux::terminal::daemon_backed] remote respawn for {session_id} \
                     has no original_command (was a plain shell, or preset wasn't yet \
                     applied) — spawning bare bash"
                );
            } else {
                crate::trace_cloud_push!(
                    "[codemux::terminal::daemon_backed] skipping remote in-memory synthesis \
                     for {session_id}: no disk_meta and no captured agent session id — \
                     treating as a fresh preset launch (apply_preset owns the PTY write)"
                );
            }
        } else {
            // Local: use the SAME in-memory-first strategy as the
            // remote branch. Pull-back lands here (the workspace was
            // just migrated from remote → local, scrollback meta
            // isn't persisted yet because the user hasn't closed
            // the app since the migration). Reading only disk_meta
            // means a fresh shell spawns instead of relaunching the
            // agent — exactly the bug the user reported on pull-back.
            //
            // For the rare case where in_memory_original is missing
            // AND disk_meta is present (e.g. an app-restart respawn
            // before the user has interacted), we still fall back to
            // the disk path which uses the full resolve_resume_command
            // pipeline (more accurate, includes per-adapter args).
            let full = in_memory_original.clone().or_else(|| {
                disk_meta
                    .as_ref()
                    .and_then(|(_, _, m)| m.original_command.clone())
            });
            let claude_uuid = session.adapter_captures.get("claude_session_id").cloned();
            let opencode_session_id = session.adapter_captures.get("opencode_session_id").cloned();

            // Prefer the existing scrollback+adapter pipeline when
            // BOTH disk_meta and adapter_state are available — it
            // handles all the per-adapter quirks the bare-binary
            // path doesn't. Otherwise synthesize like the remote
            // branch does.
            if let (Some(adapter_state), Some((ws_id, pane_id, meta))) = (
                app.try_state::<crate::session_adapters::AdapterState>(),
                disk_meta.as_ref(),
            ) {
                effective_cwd = super::resolve_session_cwd(&meta.working_directory, &effective_cwd);
                if let Some(resume_command) = super::resolve_resume_command_from_original(
                    in_memory_original
                        .clone()
                        .or_else(|| meta.original_command.clone()),
                    meta,
                    &adapter_state,
                ) {
                    crate::trace_cloud_push!(
                        "[codemux::terminal::daemon_backed] local restore via \
                         disk_meta+adapter for {session_id} at {ws_id}/{pane_id}"
                    );
                    auto_resume_command = Some(resume_command);
                }
            } else {
                // No disk_meta + adapter pair — this branch is for
                // pull-back (workspace migrated remote → local while
                // an agent was running). We synthesize the per-agent
                // relaunch from in-memory state (claude `--resume`,
                // opencode `--session`/`--continue`) so the agent picks
                // up where it left off on the local host.
                //
                // GATE: only synthesize when there is real evidence
                // this is a *relaunch* — either a captured agent
                // session id (claude_uuid / opencode_session_id) or
                // persisted scrollback metadata (disk_meta). Otherwise
                // this is a *fresh preset launch*, and `apply_preset` is
                // about to write the exact same command via its own
                // `write_command_when_ready` call (see
                // commands/presets.rs::apply_preset → "new_tab" branch).
                // Without this gate, both writes fire and the second one
                // lands inside the just-started agent's input box — the
                // user reported seeing `claude --dangerously-skip-permissions`
                // typed into Claude Code's prompt right after launching
                // the preset on Linux. Windows isn't affected because the
                // entire daemon-backed path is `#[cfg(unix)]` and the
                // in-process spawn has no synthesis logic.
                let has_evidence = should_synthesize_agent_relaunch(
                    disk_meta.is_some(),
                    claude_uuid.is_some() || opencode_session_id.is_some(),
                );
                if has_evidence {
                    if let Some(cmd) = build_agent_relaunch_command(
                        full.as_deref(),
                        claude_uuid.as_deref(),
                        opencode_session_id.as_deref(),
                    ) {
                        eprintln!(
                            "[codemux::terminal::daemon_backed] local relaunch via in-memory \
                             for {session_id}: {cmd} (has_claude_uuid={}, has_opencode_sid={})",
                            claude_uuid.is_some(),
                            opencode_session_id.is_some(),
                        );
                        auto_resume_command = Some(cmd);
                    }
                } else {
                    crate::trace_cloud_push!(
                        "[codemux::terminal::daemon_backed] skipping in-memory synthesis for \
                         {session_id}: no disk_meta and no captured agent session id — \
                         treating as a fresh preset launch (apply_preset owns the PTY write)"
                    );
                }
            }
        }
    }

    let mut env: Vec<(String, String)> = vec![
        ("TERM".into(), "xterm-256color".into()),
        ("COLORTERM".into(), "truecolor".into()),
        ("TERM_PROGRAM".into(), "codemux".into()),
        (
            "TERM_PROGRAM_VERSION".into(),
            env!("CARGO_PKG_VERSION").into(),
        ),
        ("CODEMUX".into(), "1".into()),
        ("CODEMUX_VERSION".into(), env!("CARGO_PKG_VERSION").into()),
        ("CODEMUX_SURFACE_ID".into(), session_id.clone()),
        ("CODEMUX_SESSION_ID".into(), session_id.clone()),
        ("CODEMUX_BROWSER_CMD".into(), "codemux browser".into()),
    ];
    env.push(("CODEMUX_WORKSPACE_ID".into(), workspace_id.clone()));
    for kv in workspace_env.iter() {
        env.push(kv.clone());
    }
    if let Some(pane_id) = pane_id_for_env.as_ref() {
        env.push(("CODEMUX_PANE_ID".into(), pane_id.clone()));
    }
    if !is_remote {
        if let Some((endpoint, token)) = crate::hooks::hook_endpoint() {
            env.push(("CODEMUX_HOOK_ENDPOINT".into(), endpoint.to_owned()));
            env.push(("CODEMUX_HOOK_TOKEN".into(), token.to_owned()));
        }
    }
    emit_terminal_status_if_current(
        &app,
        &sessions,
        TerminalStatusPayload {
            session_id: session_id.clone(),
            state: TerminalLifecycleState::Starting,
            message: Some(format!("Starting persistent shell: {shell}")),
            exit_code: None,
        },
        Some(restart_token),
    );

    // A Claude typed in this shell must not find an entry an older build
    // wrote above its folder (see `mcp_cleanup::before_launch`).
    if !reattached && !is_remote {
        crate::mcp_cleanup::before_launch(std::path::Path::new(&effective_cwd)).await;
    }

    // Publish the exact cleanup target before sending an effectful RPC.
    // Closing the pane starts a separate worker that waits for this lock.
    // Keeping it until install or failure orders cleanup after Spawn/Attach.
    let guarded_target = reservation.reconciliation.as_ref().zip(shell_id).map(|(recovery, shell_id)| {
        GuardedShellOwnership {
            shell: PendingDaemonClose {
                route: recovery.route.clone(),
                session_id: session_id.clone(),
                shell_id,
            },
            recovery: recovery.clone(),
            gate: terminal_state.daemon_ownership_gate(&session_id),
            sessions: sessions.clone(),
            restart_token,
        }
    });
    let cleanup_target = if let Some(ownership) = &guarded_target {
        let mut target = ownership.recovery.target.lock().await;
        loop {
            reservation.wait_for_close().await;
            if ownership.remember(&app, reattached, reconciling).await? {
                break;
            }
        }
        target.client = Some(client.clone());
        Some(target)
    } else {
        None
    };
    // Idempotent reattach decision comes from the workspace-level List. The
    // caller performs exactly one List and maps it before any pane tasks fan
    // out, so an eight-pane activation never sends eight identical RPCs.
    let spawn_result = if let Some(existing) = existing {
        crate::trace_cloud_push!(
            "[trace:{session_id}] DECISION=reattach pid={} at t={}ms",
            existing.pid,
            entry_ts.elapsed().as_millis()
        );
        Ok(existing.pid)
    } else {
        crate::trace_cloud_push!(
            "[trace:{session_id}] DECISION=fresh_spawn at t={}ms",
            entry_ts.elapsed().as_millis()
        );
        if !reservation.is_current() {
            return Err(DaemonHydrationFailure::AlreadyReserved);
        }
        let argv = vec![shell.clone()];
        let ownership = guarded_target.as_ref().ok_or(DaemonHydrationFailure::RequiresGuardedRecovery)?;
        let shell_id = ownership.shell.shell_id;
        if let Err(error) = client.prepare_owned_spawn(session_id.clone(), shell_id).await {
            let occupied = matches!(&error, PtyDaemonError::Daemon(message)
                if message == &format!("session {session_id} already exists in daemon"));
            let failure = classify_spawn_rpc_failure(error);
            if occupied || failure.allows_local_fallback() {
                ownership.resolve(&app, ShellResolution::Rejected {
                    allow_ordinary_retry: !reconciling && failure.allows_local_fallback(),
                }).await;
            }
            return Err(failure);
        }
        // List omits in-flight spawns. Prepare checks their slot and
        // replaces any unused permit, so this acknowledgement proves
        // the previous attempt cannot produce a shell.
        ownership.resolve(&app, ShellResolution::Current).await;
        if !reservation.is_current() {
            return Err(DaemonHydrationFailure::AlreadyReserved);
        }
        client.spawn_prepared(
            session_id.clone(), shell_id, workspace_id, argv, effective_cwd, env,
            DEFAULT_ROWS, DEFAULT_COLS,
        ).await
    };

    if !reservation.is_current() {
        return Err(DaemonHydrationFailure::AlreadyReserved);
    }
    let pid = match spawn_result {
        Ok(pid) => pid,
        Err(error) => {
            let failure = classify_spawn_rpc_failure(error);
            if let Some(ownership) = guarded_target.as_ref().filter(|_| failure.allows_local_fallback()) {
                // This attempt did not create a shell. Keep unresolved IDs
                // from earlier attempts while restoring normal fallback.
                ownership.resolve(&app, ShellResolution::Rejected { allow_ordinary_retry: !reconciling }).await;
            }
            return Err(failure);
        }
    };
    if attach_surviving_tokenless {
        loop {
            reservation.wait_for_close().await;
            match reservation.with_current_runtime(|runtime| runtime.close_fence.is_none()) {
                Some(true) => break,
                Some(false) => continue,
                None => return Err(DaemonHydrationFailure::AlreadyReserved),
            }
        }
        if !owns_local_terminal(&app, &workspace.workspace_id, &session_id) {
            return Err(DaemonHydrationFailure::AlreadyReserved);
        }
    }
    let attachment = match shell_id {
        Some(shell_id) => client.attach_owned(session_id.clone(), shell_id).await,
        None => client.attach(session_id.clone()).await,
    };
    let mut rx = match attachment {
        Ok(rx) => rx,
        Err(error) => {
            // Keep a successfully spawned daemon session available for retry.
            // The cleanup target survives the failure for an explicit close.
            return Err(classify_attach_rpc_failure(error));
        }
    };
    if let Some(ownership) = guarded_target.as_ref().filter(|_| reattached) {
        // A guarded attach proves this is the installed shell. Keep prior
        // identities through failed attachments, where List can be stale.
        ownership.resolve(&app, ShellResolution::Current).await;
    }

    let writer = DaemonWriter::new(client.clone(), session_id.clone());
    // If we reattached to an existing daemon session, the agent (or
    // bash) is ALREADY running there. We must NOT auto-write the
    // preset/resume command — that would type the command as a chat
    // message into the running agent (the "claude" appearing as a
    // message bug). Only write on fresh_spawn where the new bash
    // genuinely needs the agent launched.
    let auto_resume_clone = if reattached {
        crate::trace_cloud_push!(
            "[trace:{session_id}] reattached — suppressing auto-write of resume command"
        );
        None
    } else {
        auto_resume_command.clone()
    };
    let client_for_runtime = client.clone();
    let installed = with_existing_session_runtime(&sessions, &session_id, |runtime| {
        if runtime.restart_token != restart_token {
            return None;
        }
        runtime.daemon_writer = Some(writer.clone());
        runtime.writer = Some(Box::new(writer));
        runtime.master = None;
        runtime.child_pid = Some(pid);
        runtime.persistent = true;
        runtime.is_spawning = false;
        runtime.daemon_reconciliation = None;
        runtime.daemon_shell_id = shell_id;
        runtime.daemon_route = route_for_runtime;
        // On reattach, skip_preset_launch must ALSO be true so the
        // preset launcher (separate from auto-write) doesn't fire
        // a preset write into the live agent.
        runtime.skip_preset_launch = reattached || auto_resume_clone.is_some();
        runtime.resume_command = auto_resume_clone.clone();
        // Same as the agent path — capture the client so resize/close
        // route to the daemon that actually owns this session id.
        runtime.daemon_client = Some(client_for_runtime);
        Some(super::recompute_flow_paused(runtime))
    }).flatten();
    let Some(flow_paused) = installed else {
        return Err(DaemonHydrationFailure::AlreadyReserved);
    };
    drop(cleanup_target);
    reservation.commit();
    if flow_paused {
        super::forward_flow_control_to_daemon(&sessions, &session_id);
    }

    // Preflight: for remote workspaces, verify the agent binary
    // we're about to write actually exists on the remote host. If
    // it doesn't, emit a Failed lifecycle event with an actionable
    // install message INSTEAD of writing the command into bash and
    // letting the user see a confusing "bash: claude: command not
    // found" inline. Only runs for remote + fresh-spawn (not
    // reattach — if we're reattaching, the agent's already running).
    if is_remote && !reattached {
        if let Some(ref command) = auto_resume_clone {
            let binary = command.split_whitespace().next().unwrap_or("").to_string();
            if !binary.is_empty() {
                if let Some(host_id_val) = host_id {
                    let host = app
                        .state::<crate::database::DatabaseStore>()
                        .list_hosts()
                        .into_iter()
                        .find(|h| h.id == host_id_val);
                    if let Some(host) = host {
                        let check_cmd = format!(
                            "command -v {} >/dev/null 2>&1 && echo OK || echo MISSING",
                            crate::commands::hosts::shell_word_quote(&binary)
                        );
                        let check = tokio::process::Command::new("ssh")
                            .arg("-o")
                            .arg("BatchMode=yes")
                            .arg("-o")
                            .arg("ConnectTimeout=5")
                            .arg(&host.ssh_target)
                            .arg(&check_cmd)
                            .output()
                            .await;
                        if let Ok(out) = check {
                            let result = String::from_utf8_lossy(&out.stdout).trim().to_string();
                            if result == "MISSING" {
                                eprintln!(
                                    "[codemux::terminal::daemon_backed] preflight: \
                                     {binary} is not installed on {} — surfacing \
                                     Failed status instead of writing doomed command",
                                    host.name
                                );
                                emit_terminal_status_if_current(
                                    &app,
                                    &sessions,
                                    TerminalStatusPayload {
                                        session_id: session_id.clone(),
                                        state: TerminalLifecycleState::Failed,
                                        message: Some(format!(
                                            "{binary} isn't installed on {}. Install it \
                                             on the host (see the agent's docs), then \
                                             push the workspace again.",
                                            host.name
                                        )),
                                        exit_code: None,
                                    },
                                    Some(restart_token),
                                );
                                // Don't write the command — let the bare bash
                                // prompt remain on the pane as a fallback.
                                return Ok(DaemonSpawnOutcome::DaemonShell);
                            }
                            // result == "OK" → proceed to write.
                            // result == anything-else (SSH error, etc.) →
                            // proceed anyway; transient SSH failures
                            // shouldn't block legitimate spawns.
                        }
                    }
                }
            }
        }
    }

    // Send the resume command via the same write-when-ready path the
    // in-process spawn uses. Because our `DaemonWriter` is already in
    // `runtime.writer`, this lands at the daemon, which writes to the
    // master fd; the shell sees it as if the user typed it.
    //
    // Already gated to None on reattach above, so this no-ops in the
    // reattach path even though we still iterate the if-let.
    if let Some(command) = auto_resume_clone {
        let sessions_for_command = sessions.clone();
        let session_id_for_command = session_id.clone();
        crate::commands::presets::write_command_when_ready(
            sessions_for_command,
            session_id_for_command,
            command,
            120,
        );
    }

    emit_terminal_status_if_current(
        &app,
        &sessions,
        TerminalStatusPayload {
            session_id: session_id.clone(),
            state: TerminalLifecycleState::Ready,
            message: Some(format!(
                "Persistent shell ready: {shell} [pid {pid}, daemon-backed]"
            )),
            exit_code: None,
        },
        Some(restart_token),
    );

    // ── Reader task: drain the daemon's mpsc into queue_or_send_output AND
    // feed the adapter line scanner so agents like Claude Code can capture
    // their session ID for `--resume`. Parity with the in-process read
    // loop's line buffer at terminal/mod.rs:1377.
    let adapter_clone: Option<crate::session_adapters::AdapterState> = app
        .try_state::<crate::session_adapters::AdapterState>()
        .map(|s| s.inner().clone());
    let original_cmd = session.original_command.clone();
    let has_scanner = if let (Some(ref adapter), Some(ref cmd)) = (&adapter_clone, &original_cmd) {
        adapter.start_scanner(&session_id, cmd).is_some()
    } else {
        false
    };

    let read_sessions = sessions.clone();
    let read_session_id = session_id.clone();
    let scanner_session_id = session_id.clone();
    let read_app = app.clone();
    let read_client = client.clone();
    tauri::async_runtime::spawn(async move {
        let mut line_buf: Vec<u8> = Vec::new();
        let mut reset_before_replay = reconciling && reattached;
        while let Some(mut chunk) = rx.recv().await {
            // Adapter scanner (cheap when has_scanner=false).
            if has_scanner {
                if let Some(ref adapter) = adapter_clone {
                    for &byte in &chunk {
                        if byte == b'\n' {
                            let line = String::from_utf8_lossy(&line_buf);
                            let clean = super::strip_ansi_codes(&line);
                            adapter.scan_line(&scanner_session_id, &clean);
                            line_buf.clear();
                        } else if byte != b'\r' {
                            line_buf.push(byte);
                        }
                    }
                }
            }
            if std::mem::take(&mut reset_before_replay) {
                // Mounted viewers already rendered the old stream. Reset
                // before replaying the daemon's snapshot into those viewers.
                chunk = [crate::pty_daemon::server::RESYNC_PREFIX, &chunk].concat();
            }
            queue_or_send_output_if_current(&read_sessions, &read_session_id, chunk, Some(restart_token));
        }
        crate::trace_cloud_push!(
            "[codemux::terminal::daemon_backed] shell read loop ended for {read_session_id}"
        );
        // Skip emit if this is a stale read task whose session was
        // already replaced by a fresh spawn. See `emit_exited_if_client_owner`.
        super::emit_exited_if_client_owner(
            &read_app,
            &read_sessions,
            &read_session_id,
            &read_client,
            restart_token,
            "Shell ended",
        );
    });

    Ok(DaemonSpawnOutcome::DaemonShell)
}

pub(super) struct PreparedDaemonClose {
    route: DaemonWorkspaceRoute,
    session_id: String,
    shell_ids: Vec<Uuid>,
    recovery: Option<Arc<DaemonReconciliation>>,
}

impl PreparedDaemonClose {
    fn records(&self) -> impl Iterator<Item = PendingDaemonClose> + '_ {
        self.shell_ids.iter().map(move |&shell_id| PendingDaemonClose {
            route: self.route.clone(),
            session_id: self.session_id.clone(),
            shell_id,
        })
    }
}

/// One queue and worker per workspace daemon route. A closed pane's cleanup
/// stays in the journal until the daemon acknowledges it.
#[derive(Default)]
pub(super) struct PendingDaemonCloses {
    routes: Mutex<HashMap<DaemonWorkspaceRoute, VecDeque<PreparedDaemonClose>>>,
}

/// Collect unresolved identities without changing a live reservation.
pub(super) fn prepare_closed_pane_cleanup(
    session_id: &str,
    recovery: &Arc<DaemonReconciliation>,
) -> PreparedDaemonClose {
    PreparedDaemonClose {
        route: recovery.route.clone(),
        session_id: session_id.to_owned(),
        shell_ids: recovery.shells_for_close(),
        recovery: Some(recovery.clone()),
    }
}

pub(super) struct CommittedDaemonClose {
    intent: LayoutCloseIntent,
    requests: Vec<PreparedDaemonClose>,
    restored: Vec<PendingDaemonClose>,
}

impl CommittedDaemonClose {
    /// Registration takes the same sessions lock as the close caller.
    /// Seal admission once the exact identities have reached SQLite.
    pub(super) fn seal(&self) {
        for request in &self.requests {
            if let Some(recovery) = &request.recovery {
                recovery.seal_for_close();
            }
        }
    }

    pub(super) fn finish<R: Runtime>(self, app: &AppHandle<R>) {
        // Archive becomes visible after the shared close finishes.
        if !matches!(&self.intent, LayoutCloseIntent::Archive(_)) {
            crate::state::persist_pending_layout_closes(app, vec![self.intent]);
        }
        for request in self.requests {
            queue_closed_pane_cleanup(app.clone(), request);
        }
        resume_pending_daemon_closes(app, self.restored);
    }
}

pub(super) fn commit_layout_close(
    db: &DatabaseStore,
    intent: LayoutCloseIntent,
    workspace_id: &str,
    closed_sessions: &[SessionId],
    mut requests: Vec<PreparedDaemonClose>,
    checkpoint: &Checkpoint,
) -> Result<CommittedDaemonClose, TerminalCloseError> {
    let mut restored = Vec::new();
    let closes: Vec<_> = requests.iter().flat_map(PreparedDaemonClose::records).collect();
    let owners = db.record_layout_close_with_shell_ownership(&intent, Some(workspace_id), closed_sessions, &closes, checkpoint)
        .map_err(|error| match error {
            TerminalCloseError::Failed(message) => format!("Could not journal layout close: {message}").into(),
            error => error,
        })?;
    for owner in owners {
        let live = requests.iter_mut().find(|request| {
            request.route == owner.route && request.session_id == owner.session_id
        });
        if let Some(request) = live {
            if !request.shell_ids.contains(&owner.shell_id) {
                request.shell_ids.push(owner.shell_id);
            }
        } else {
            restored.push(owner);
        }
    }
    Ok(CommittedDaemonClose { intent, requests, restored })
}

pub(super) fn queue_closed_pane_cleanup<R: Runtime>(app: AppHandle<R>, request: PreparedDaemonClose) {
    if request.shell_ids.is_empty() {
        return;
    }
    let pending = app.state::<PtyState>().pending_daemon_closes.clone();
    let route = request.route.clone();
    {
        let mut queues = pending.routes.lock().unwrap_or_else(|error| error.into_inner());
        match queues.entry(route.clone()) {
            Entry::Occupied(mut entry) => {
                entry.get_mut().push_back(request);
                return;
            }
            Entry::Vacant(entry) => {
                entry.insert(VecDeque::from([request]));
            }
        }
    }
    tauri::async_runtime::spawn(drain_pending_daemon_closes(app, pending, route));
}

pub(crate) fn resume_pending_daemon_closes<R: Runtime>(
    app: &AppHandle<R>,
    jobs: Vec<PendingDaemonClose>,
) {
    let mut grouped = HashMap::<_, Vec<_>>::new();
    for job in jobs {
        grouped.entry((job.route, job.session_id)).or_default().push(job.shell_id);
    }
    for ((route, session_id), shell_ids) in grouped {
        queue_closed_pane_cleanup(app.clone(), PreparedDaemonClose {
            route,
            session_id,
            shell_ids,
            recovery: None,
        });
    }
}

async fn drain_pending_daemon_closes<R: Runtime>(
    app: AppHandle<R>,
    pending: Arc<PendingDaemonCloses>,
    route: DaemonWorkspaceRoute,
) {
    let mut cleanup_tunnel = tokio::sync::Mutex::new(None);
    loop {
        let request = pending.routes.lock().unwrap_or_else(|error| error.into_inner())
            .get_mut(&route).and_then(VecDeque::pop_front);
        if let Some(request) = request {
            // A live restart may have sent one of these RPCs before the pane
            // was closed. Let it finish before sending CloseOwned. The seal
            // taken by prepare prevents any later RPC publishing a new UUID.
            let captured_client = match request.recovery {
                Some(recovery) => recovery.target.lock().await.client.clone(),
                None => None,
            };
            for &shell_id in &request.shell_ids {
                retry_guarded_close(|| async {
                    let acquired = match captured_client.as_ref().filter(|client| !client.is_closed()) {
                        Some(client) => Ok(client.clone()),
                        None => {
                            if let Some(ssh_target) = &route.ssh_target {
                                let mut tunnel = cleanup_tunnel.lock().await;
                                if tunnel.is_none() {
                                    *tunnel = Some(crate::ssh::registry::CleanupDaemonTunnel::start(
                                        &route.workspace_id, ssh_target,
                                    ).await);
                                }
                                tunnel.as_mut().expect("cleanup tunnel initialized")
                                    .client(&route.workspace_id).await
                            } else if route.host_id.is_none() {
                                crate::pty_daemon::ensure_daemon().await
                            } else {
                                return false;
                            }
                        }
                    };
                    let Ok(client) = acquired else { return false };
                    client.supports_guarded_shells()
                        && client.close_owned(request.session_id.clone(), shell_id).await.is_ok()
                }).await;
            }
            let app = app.clone();
            let session_id = request.session_id.clone();
            let result = tokio::task::spawn_blocking(move || {
                app.state::<DatabaseStore>().retire_completed_daemon_closes(
                    &request.route, &request.session_id, &request.shell_ids,
                )
            }).await.map_err(|error| error.to_string()).and_then(|result| result);
            if let Err(error) = result {
                log::error!("Could not retire acknowledged terminal close for {session_id}: {error}");
            }
            continue;
        }

        // Release only the worker's tunnel. The workspace may have moved
        // to another host and installed a different active connection.
        if let Some(tunnel) = cleanup_tunnel.get_mut().take() {
            tunnel.shutdown().await;
        }
        let mut queues = pending.routes.lock().unwrap_or_else(|error| error.into_inner());
        if queues.get(&route).is_some_and(VecDeque::is_empty) {
            queues.remove(&route);
            return;
        }
    }
}

async fn retry_guarded_close<F>(mut attempt: impl FnMut() -> F + Send)
where
    F: Future<Output = bool> + Send,
{
    let mut delay = Duration::from_millis(250);
    loop {
        if tokio::time::timeout(Duration::from_secs(35), attempt()).await.unwrap_or(false) {
            return;
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(Duration::from_secs(30));
    }
}

/// Acknowledgement for one queued input write. Internal callers await it after
/// releasing the shared sessions lock; terminal typing drops it (see
/// `admit_pty_input`), which routes a failure to the session's next input.
pub(crate) type WriteAck = tokio::sync::oneshot::Receiver<std::io::Result<()>>;

type QueuedWrite = (Vec<u8>, tokio::sync::oneshot::Sender<std::io::Result<()>>);

/// Upper bound on input merged into one daemon write by the drain task.
const MAX_COALESCED_INPUT: usize = 64 * 1024;

/// One ordered input queue per session, shared by IPC and synchronous input.
/// Each caller receives its own daemon acknowledgement or error.
#[derive(Clone)]
pub(crate) struct DaemonWriter {
    queue: tokio::sync::mpsc::UnboundedSender<QueuedWrite>,
    /// Failure of a write whose acknowledgement nobody awaited.
    unawaited_failure: Arc<std::sync::Mutex<Option<std::io::Error>>>,
}

impl DaemonWriter {
    pub(super) fn new(client: Arc<PtyDaemonClient>, session_id: String) -> Self {
        let (queue, mut pending) = tokio::sync::mpsc::unbounded_channel::<QueuedWrite>();
        let unawaited_failure = Arc::new(std::sync::Mutex::new(None));
        let drain_failure = Arc::clone(&unawaited_failure);
        tauri::async_runtime::spawn(async move {
            while let Some((mut data, reply)) = pending.recv().await {
                // Merge input queued while the previous write was in flight,
                // so a typing burst costs one daemon round trip, not one each.
                let mut replies = vec![reply];
                while data.len() < MAX_COALESCED_INPUT {
                    let Ok((more, reply)) = pending.try_recv() else {
                        break;
                    };
                    data.extend_from_slice(&more);
                    replies.push(reply);
                }
                let result = client
                    .write(session_id.clone(), &data)
                    .await
                    .map_err(|error| match error {
                        PtyDaemonError::Io(error) => error,
                        PtyDaemonError::Closed => {
                            std::io::Error::from(std::io::ErrorKind::BrokenPipe)
                        }
                        PtyDaemonError::Timeout => {
                            std::io::Error::new(std::io::ErrorKind::TimedOut, error)
                        }
                        error => std::io::Error::other(error),
                    });
                for reply in replies {
                    let result = match &result {
                        Ok(()) => Ok(()),
                        Err(error) => Err(std::io::Error::new(error.kind(), error.to_string())),
                    };
                    if let Err(Err(error)) = reply.send(result) {
                        drain_failure
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .get_or_insert(error);
                    }
                }
            }
        });
        Self {
            queue,
            unawaited_failure,
        }
    }

    pub(super) fn take_unawaited_failure(&self) -> Option<std::io::Error> {
        self.unawaited_failure
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }

    pub(super) fn enqueue(&self, data: &[u8]) -> std::io::Result<WriteAck> {
        let (reply, ack) = tokio::sync::oneshot::channel();
        self.queue.send((data.to_vec(), reply)).map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "pty daemon writer stopped")
        })?;
        Ok(ack)
    }
}

/// Only `queue_pty_input` may write daemon input: it calls `enqueue` and
/// waits for the acknowledgement after releasing the sessions lock. This impl
/// exists only so the writer can fill `SessionRuntime::writer`, which marks the
/// session writable. A direct write fails fast instead of blocking, which
/// would panic on a tokio runtime thread.
impl std::io::Write for DaemonWriter {
    fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "daemon input must go through queue_pty_input",
        ))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_agent_relaunch_command, classify_attach_rpc_failure,
        classify_client_acquisition_failure, classify_list_rpc_failure,
        classify_spawn_rpc_failure, list_daemon_session_map, should_synthesize_agent_relaunch,
        DaemonHydrationFailure,
    };
    use crate::pty_daemon::protocol::{ClientRequest, Frame, ServerResponse};
    use crate::pty_daemon::{DaemonSessionInfo, PtyDaemonClient, PtyDaemonError};
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    /// Read one `Write` request off the fake daemon socket.
    async fn read_test_write(
        reader: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
    ) -> (u64, Vec<u8>) {
        use base64::Engine;

        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(1), reader.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        let ClientRequest::Write {
            request_id,
            data_b64,
            ..
        } = serde_json::from_str(&line).unwrap()
        else {
            panic!("expected Write");
        };
        (
            request_id,
            base64::engine::general_purpose::STANDARD
                .decode(data_b64)
                .unwrap(),
        )
    }

    async fn send_test_response(
        write: &mut tokio::net::unix::OwnedWriteHalf,
        response: ServerResponse,
    ) {
        let mut bytes = serde_json::to_vec(&Frame::Response(response)).unwrap();
        bytes.push(b'\n');
        write.write_all(&bytes).await.unwrap();
    }

    #[test]
    fn pending_terminal_close_persists_intent_without_a_shell() {
        use super::{DaemonReconciliation, DaemonWorkspaceRoute, DatabaseStore, LayoutCloseIntent};
        use tauri::Manager;

        let root = tempfile::tempdir().unwrap();
        let app = tauri::test::mock_app();
        app.manage(DatabaseStore::test_file(&root.path().join("journal.sqlite")));
        let recovery = std::sync::Arc::new(DaemonReconciliation::new(
            DaemonWorkspaceRoute {
                workspace_id: "workspace".into(),
                host_id: None,
                ssh_target: None,
            },
            None,
            None,
        ));
        let close = super::prepare_closed_pane_cleanup("session", &recovery);
        let intent = LayoutCloseIntent::Pane("pane".into());
        let db = app.state::<DatabaseStore>();
        db.record_layout_close(&intent, &close.records().collect::<Vec<_>>()).unwrap();
        assert!(db.pending_daemon_closes().unwrap().is_empty());
        assert_eq!(db.pending_layout_closes().unwrap(), [intent]);
    }

    #[tokio::test]
    async fn pending_terminal_close_replays_after_reopening_the_app_database() {
        const TEST: &str = "terminal::daemon_backed::tests::pending_terminal_close_replays_after_reopening_the_app_database";
        if crate::test_isolation::isolated(TEST) {
            return;
        }
        use super::{AppStateStore, DaemonReconciliation, DaemonWorkspaceRoute, DatabaseStore, LayoutCloseIntent, PtyState};
        use crate::pty_daemon::manifest::{write_manifest, DaemonManifest};
        use crate::state;
        use tauri::Manager;
        use tokio::net::UnixListener;

        let root = crate::test_isolation::isolated_root().unwrap();
        let db_path = root.join("terminal-journal.sqlite");
        let socket_path = root.join("cleanup-daemon.sock");
        let initial_store = AppStateStore::default();
        let session_id = initial_store.active_terminal_session_id().unwrap().0;
        let plan = initial_store.pty_hydration_plan_for_session(&session_id).unwrap();
        let pane_id = plan.sessions[0].pane_id.as_deref().unwrap();
        let intent = LayoutCloseIntent::Pane(pane_id.to_owned());
        let shell_ids = [uuid::Uuid::new_v4(), uuid::Uuid::new_v4()];
        let route = DaemonWorkspaceRoute {
            workspace_id: plan.workspace.workspace_id,
            host_id: None,
            ssh_target: None,
        };
        let layout_path = root.join("layout.json");
        assert!(state::load_persisted_state_checked().unwrap().is_none());
        state::flush_persisted_state(&initial_store).unwrap();
        let stale_layout = std::fs::read(&layout_path).unwrap();
        {
            let app = tauri::test::mock_app();
            app.manage(DatabaseStore::test_file(&db_path));
            let recovery = std::sync::Arc::new(DaemonReconciliation::new(route.clone(), Some(shell_ids[0]), None));
            assert!(recovery.register_shell(shell_ids[1]));
            let _rpc_in_progress = recovery.target.lock().await;
            initial_store.close_pane(pane_id).unwrap();
            let close = super::prepare_closed_pane_cleanup(&session_id, &recovery);
            app.state::<DatabaseStore>()
                .record_layout_close(&intent, &close.records().collect::<Vec<_>>()).unwrap();
            recovery.seal_for_close();
            assert!(!recovery.register_shell(uuid::Uuid::new_v4()));
            // The close command's layout save and both daemon requests are pending.
        }

        let relaunched = tauri::test::mock_app();
        relaunched.manage(PtyState::default());
        relaunched.manage(DatabaseStore::test_file(&db_path));
        let db = relaunched.state::<DatabaseStore>();
        let jobs = db.pending_daemon_closes().unwrap();
        let intents = db.pending_layout_closes().unwrap();
        assert_eq!(intents, [intent.clone()]);
        assert_eq!(jobs.len(), 2);
        assert!(jobs.iter().all(|job| job.route == route));
        // A fallback save cannot acknowledge a close against the set-aside layout.
        std::fs::write(&layout_path, b"invalid layout").unwrap();
        assert!(state::load_persisted_state_checked().unwrap().is_none());
        assert!(state::layout_recovery_pending());
        state::persist_layout_closes(&AppStateStore::default(), &db, &intents).unwrap();
        assert_eq!(db.pending_layout_closes().unwrap(), intents);
        let marker = std::fs::read_to_string(layout_path.with_extension("json.recovery-pending")).unwrap();
        let backup = root.join(marker.trim());
        assert_eq!(std::fs::read(&backup).unwrap(), b"invalid layout");
        std::fs::write(&backup, &stale_layout).unwrap();
        std::fs::rename(backup, &layout_path).unwrap();
        let restored = AppStateStore::default();
        restored.replace_snapshot(state::load_persisted_state_checked().unwrap().unwrap());
        assert!(!state::layout_recovery_pending());
        assert!(restored.pty_hydration_plan_for_session(&session_id).is_some());
        state::restore_pending_layout_closes(&restored, &intents).unwrap();
        assert!(restored.pty_hydration_plan_for_session(&session_id).is_none());
        relaunched.manage(restored);

        let listener = UnixListener::bind(&socket_path).unwrap();
        let pid = std::process::id();
        write_manifest(&DaemonManifest {
            pid,
            socket_path,
            daemon_version: env!("CARGO_PKG_VERSION").into(),
            protocol_version: crate::pty_daemon::PROTOCOL_VERSION,
            started_at: 0,
        }).unwrap();
        let (close_seen, mut seen) = tokio::sync::mpsc::channel(2);
        let (allow_ack, mut ack_allowed) = tokio::sync::mpsc::channel(2);
        let daemon_session = session_id.clone();
        let daemon = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = stream.into_split();
            let mut lines = BufReader::new(read).lines();
            let hello = lines.next_line().await.unwrap().unwrap();
            let ClientRequest::Hello { request_id } = serde_json::from_str(&hello).unwrap() else {
                panic!("expected a daemon handshake");
            };
            send_test_response(&mut write, ServerResponse::Hello {
                request_id,
                protocol_version: crate::pty_daemon::PROTOCOL_VERSION,
                daemon_pid: pid,
                daemon_version: env!("CARGO_PKG_VERSION").into(),
                host_claude_plugin: false,
                guarded_shells: true,
                prepared_spawn: true,
            }).await;

            for shell_id in shell_ids {
                let close = lines.next_line().await.unwrap().unwrap();
                let ClientRequest::CloseOwned {
                    request_id, session_id: requested_session, shell_id: requested_shell,
                } = serde_json::from_str(&close).unwrap() else {
                    panic!("expected to replay the exact guarded close");
                };
                assert_eq!(requested_session, daemon_session);
                assert_eq!(requested_shell, shell_id);
                close_seen.send(shell_id).await.unwrap();
                ack_allowed.recv().await.unwrap();
                send_test_response(&mut write, ServerResponse::Closed {
                    request_id,
                    attachment_id: None,
                }).await;
            }
        });

        let (locked, writer_locked) = std::sync::mpsc::channel();
        let (release, wait_for_release) = std::sync::mpsc::channel();
        let layout_writer = std::thread::spawn(move || {
            state::with_terminal_close_layout_lock(|| {
                locked.send(()).unwrap();
                let _ = wait_for_release.recv_timeout(Duration::from_secs(15));
                Ok::<_, String>(())
            }).unwrap();
        });
        writer_locked.recv_timeout(Duration::from_secs(5)).unwrap();
        crate::state::persist_pending_layout_closes(relaunched.handle(), intents);
        super::resume_pending_daemon_closes(relaunched.handle(), jobs);
        assert_eq!(tokio::time::timeout(Duration::from_secs(5), seen.recv()).await.unwrap(), Some(shell_ids[0]));
        allow_ack.send(()).await.unwrap();
        // Observing the second request proves the first ACK was processed.
        assert_eq!(tokio::time::timeout(Duration::from_secs(5), seen.recv()).await.unwrap(), Some(shell_ids[1]));
        assert_eq!(db.pending_daemon_closes().unwrap().len(), 2);
        assert_eq!(std::fs::read(root.join("layout.json")).unwrap(), stale_layout);
        allow_ack.send(()).await.unwrap();
        daemon.await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !db.pending_daemon_closes().unwrap().is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.unwrap();
        assert_eq!(db.pending_layout_closes().unwrap(), [intent]);
        assert_eq!(std::fs::read(root.join("layout.json")).unwrap(), stale_layout);
        release.send(()).unwrap();
        layout_writer.join().unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !db.pending_layout_closes().unwrap().is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.unwrap();
        let saved = state::load_persisted_state().unwrap();
        assert!(saved.terminal_sessions.iter().all(|session| session.session_id.0 != session_id));
    }

    #[tokio::test(start_paused = true)]
    async fn guarded_close_retries_past_two_minutes_for_the_original_shell() {
        let session_id = "closed-pane";
        let shell_id = uuid::Uuid::new_v4();
        let unavailable_until = tokio::time::Instant::now() + Duration::from_secs(125);
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let unavailable_attempts = AtomicUsize::new(0);
        let close_acknowledged = AtomicBool::new(false);

        let cleanup = super::retry_guarded_close(|| async {
            if tokio::time::Instant::now() < unavailable_until {
                unavailable_attempts.fetch_add(1, Ordering::Relaxed);
                return false;
            }

            // The outage and backoff use virtual time. Run the recovered
            // socket exchange on real time so readiness cannot race a
            // paused Tokio timer automatically advancing to its deadline.
            tokio::time::resume();
            let (client_stream, server_stream) = tokio::net::UnixStream::pair().unwrap();
            let client = PtyDaemonClient::from_test_stream(client_stream, Duration::from_secs(2));
            let daemon = tokio::spawn(async move {
                let (read, mut write) = server_stream.into_split();
                let mut lines = BufReader::new(read).lines();
                let hello = lines.next_line().await.unwrap().unwrap();
                let ClientRequest::Hello { request_id } = serde_json::from_str(&hello).unwrap() else {
                    panic!("expected the recovered daemon handshake");
                };
                send_test_response(&mut write, ServerResponse::Hello {
                    request_id,
                    protocol_version: crate::pty_daemon::protocol::PROTOCOL_VERSION,
                    daemon_pid: std::process::id(),
                    daemon_version: env!("CARGO_PKG_VERSION").into(),
                    host_claude_plugin: false,
                    guarded_shells: true,
                    prepared_spawn: true,
                }).await;

                let close = lines.next_line().await.unwrap().unwrap();
                let ClientRequest::CloseOwned {
                    request_id, session_id: requested_session, shell_id: requested_shell,
                } = serde_json::from_str(&close).unwrap() else {
                    panic!("expected a guarded close on the recovered connection");
                };
                assert_eq!(requested_session, session_id);
                assert_eq!(requested_shell, shell_id);
                send_test_response(&mut write, ServerResponse::Closed {
                    request_id,
                    attachment_id: None,
                }).await;
            });

            client.hello().await.unwrap();
            client.close_owned(session_id.into(), shell_id).await.unwrap();
            daemon.await.unwrap();
            close_acknowledged.store(true, Ordering::Relaxed);
            true
        });

        tokio::time::timeout(Duration::from_secs(160), cleanup)
            .await
            .expect("the pending close should finish when the daemon comes back");
        assert!(unavailable_attempts.load(Ordering::Relaxed) > 1);
        assert!(close_acknowledged.load(Ordering::Relaxed), "the cleanup returned before closing the shell");
    }

    #[tokio::test]
    async fn daemon_writer_reports_write_rejection() {
        let (client_stream, server_stream) = tokio::net::UnixStream::pair().unwrap();
        let client = PtyDaemonClient::from_test_stream(client_stream, Duration::from_secs(1));
        let (server_read, mut server_write) = server_stream.into_split();
        let mut reader = BufReader::new(server_read);
        let ack = super::DaemonWriter::new(client, "missing".into())
            .enqueue(b"input")
            .unwrap();
        let (request_id, _) = read_test_write(&mut reader).await;
        send_test_response(
            &mut server_write,
            ServerResponse::Error {
                request_id,
                message: "unknown session".into(),
            },
        )
        .await;
        let result = ack.await.unwrap();
        assert!(
            result.is_err(),
            "the rejected write was reported as successful"
        );
        assert!(result.unwrap_err().to_string().contains("unknown session"));
    }

    #[tokio::test]
    async fn daemon_writer_sends_each_write_after_the_previous_is_acknowledged() {
        let (client_stream, server_stream) = tokio::net::UnixStream::pair().unwrap();
        let client = PtyDaemonClient::from_test_stream(client_stream, Duration::from_secs(1));
        let (server_read, mut server_write) = server_stream.into_split();
        let mut reader = BufReader::new(server_read);
        let writer = super::DaemonWriter::new(client, "session".into());
        let first_ack = writer.enqueue(b"command").unwrap();
        let (first_id, first) = read_test_write(&mut reader).await;
        assert_eq!(first, b"command");
        // Input queued while a write is in flight goes out next, merged in order.
        let second_ack = writer.enqueue(b"\r").unwrap();
        let third_ack = writer.enqueue(b"ls").unwrap();
        let mut line = String::new();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), reader.read_line(&mut line))
                .await
                .is_err(),
            "the second write was sent before the first was acknowledged"
        );
        send_test_response(
            &mut server_write,
            ServerResponse::Written {
                request_id: first_id,
            },
        )
        .await;
        let (second_id, second) = read_test_write(&mut reader).await;
        assert_eq!(second, b"\rls");
        send_test_response(
            &mut server_write,
            ServerResponse::Written {
                request_id: second_id,
            },
        )
        .await;
        first_ack.await.unwrap().unwrap();
        second_ack.await.unwrap().unwrap();
        third_ack.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn daemon_writer_input_waits_without_holding_sessions_lock() {
        use super::super::{write_pty_input_sync, SessionRuntime};
        use std::sync::{Arc, Mutex};

        {
            let (client_stream, server_stream) = tokio::net::UnixStream::pair().unwrap();
            let client = PtyDaemonClient::from_test_stream(client_stream, Duration::from_secs(1));
            let (server_read, mut server_write) = server_stream.into_split();
            let mut reader = BufReader::new(server_read);
            let writer = super::DaemonWriter::new(client, "session".into());
            let mut runtime = SessionRuntime::new("session");
            runtime.daemon_writer = Some(writer.clone());
            runtime.writer = Some(Box::new(writer));
            let sessions = Arc::new(Mutex::new(std::collections::HashMap::from([(
                "session".into(),
                runtime,
            )])));
            let input_sessions = sessions.clone();
            let input = tokio::task::spawn_blocking(move || {
                write_pty_input_sync(&input_sessions, "session", b"input")
            });
            let (request_id, _) = read_test_write(&mut reader).await;
            assert!(
                !input.is_finished(),
                "input returned before the daemon acknowledged it"
            );
            assert!(
                sessions.try_lock().is_ok(),
                "input held up other terminals while awaiting the daemon"
            );
            send_test_response(
                &mut server_write,
                ServerResponse::Error {
                    request_id,
                    message: "unknown session".into(),
                },
            )
            .await;
            assert!(input
                .await
                .unwrap()
                .unwrap_err()
                .contains("unknown session"));
        }
    }

    #[tokio::test]
    async fn typing_is_admitted_without_waiting_for_the_daemon() {
        use super::super::{admit_pty_input, SessionRuntime};
        use std::sync::{Arc, Mutex};

        let (client_stream, server_stream) = tokio::net::UnixStream::pair().unwrap();
        let client = PtyDaemonClient::from_test_stream(client_stream, Duration::from_secs(1));
        let (server_read, mut server_write) = server_stream.into_split();
        let mut reader = BufReader::new(server_read);
        let mut runtimes = std::collections::HashMap::new();
        for id in ["slow", "other"] {
            let writer = super::DaemonWriter::new(client.clone(), id.into());
            let mut runtime = SessionRuntime::new(id);
            runtime.daemon_writer = Some(writer.clone());
            runtime.writer = Some(Box::new(writer));
            runtimes.insert(id.to_string(), runtime);
        }
        let sessions = Arc::new(Mutex::new(runtimes));

        // An unacknowledged write to one session doesn't hold up another.
        admit_pty_input(&sessions, "slow", b"a").unwrap();
        let (slow_id, slow) = read_test_write(&mut reader).await;
        assert_eq!(slow, b"a");
        admit_pty_input(&sessions, "other", b"b").unwrap();
        let (other_id, other) = read_test_write(&mut reader).await;
        assert_eq!(other, b"b");
        send_test_response(&mut server_write, ServerResponse::Written { request_id: other_id }).await;
        send_test_response(
            &mut server_write,
            ServerResponse::Error {
                request_id: slow_id,
                message: "unknown session".into(),
            },
        )
        .await;

        // The unawaited failure is reported by the session's next input,
        // which is still queued.
        let slow_writer = sessions.lock().unwrap()["slow"].daemon_writer.clone().unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        while slow_writer.unawaited_failure.lock().unwrap().is_none() {
            assert!(tokio::time::Instant::now() < deadline, "failure was never recorded");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let error = admit_pty_input(&sessions, "slow", b"c").unwrap_err();
        assert!(error.contains("unknown session"), "{error}");
        let (_, next) = read_test_write(&mut reader).await;
        assert_eq!(next, b"c");
    }

    #[tokio::test]
    async fn daemon_writer_direct_write_fails_fast_on_a_runtime_thread() {
        use std::io::Write;

        let (client_stream, _server_stream) = tokio::net::UnixStream::pair().unwrap();
        let client = PtyDaemonClient::from_test_stream(client_stream, Duration::from_secs(1));
        let error = super::DaemonWriter::new(client, "session".into())
            .write_all(b"input")
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
    }

    #[tokio::test]
    async fn daemon_writer_reports_timeout_and_disconnect() {
        for disconnected in [false, true] {
            let (client_stream, server_stream) = tokio::net::UnixStream::pair().unwrap();
            let client =
                PtyDaemonClient::from_test_stream(client_stream, Duration::from_millis(50));
            let peer = if disconnected {
                drop(server_stream);
                None
            } else {
                Some(server_stream)
            };
            let error = super::DaemonWriter::new(client, "session".into())
                .enqueue(b"input")
                .unwrap()
                .await
                .unwrap()
                .unwrap_err();
            assert_eq!(
                error.kind(),
                if disconnected {
                    std::io::ErrorKind::BrokenPipe
                } else {
                    std::io::ErrorKind::TimedOut
                }
            );
            drop(peer);
        }
    }

    #[test]
    fn only_confirmed_spawn_rejections_allow_local_fallback() {
        let rejected = classify_spawn_rpc_failure(PtyDaemonError::Daemon(
            "spawn: executable not found".into(),
        ));
        assert!(rejected.allows_local_fallback());
        assert!(matches!(
            rejected,
            DaemonHydrationFailure::SafeToFallback(_)
        ));

        let timed_out = classify_spawn_rpc_failure(PtyDaemonError::Timeout);
        assert!(!timed_out.allows_local_fallback());
        assert!(matches!(
            timed_out,
            DaemonHydrationFailure::Ambiguous(_)
        ));

        let daemon_session_exists = classify_spawn_rpc_failure(PtyDaemonError::Daemon(
            "session shell-1 already exists in daemon".into(),
        ));
        assert!(!daemon_session_exists.allows_local_fallback());
    }

    #[test]
    fn attach_failure_never_allows_a_replacement_shell() {
        let failed =
            classify_attach_rpc_failure(PtyDaemonError::Daemon("attach rejected".into()));
        assert!(!failed.allows_local_fallback());
        assert!(matches!(
            failed,
            DaemonHydrationFailure::Ambiguous(_)
        ));
    }

    #[test]
    fn list_failures_never_allow_local_fallback() {
        // List runs on a connection that already succeeded, so a daemon
        // process exists. A List timeout against a stalled-but-alive daemon
        // proves nothing about session non-existence; spawning local
        // replacements would duplicate live shells and orphan their agents.
        for error in [
            PtyDaemonError::Timeout,
            PtyDaemonError::Closed,
            PtyDaemonError::Daemon("daemon busy".into()),
            PtyDaemonError::Io(std::io::Error::from(std::io::ErrorKind::BrokenPipe)),
        ] {
            let classified = classify_list_rpc_failure(error);
            assert!(
                !classified.allows_local_fallback(),
                "List errors must stay retry-only: {classified}"
            );
            assert!(matches!(classified, DaemonHydrationFailure::Ambiguous(_)));
        }
    }

    #[test]
    fn client_acquisition_refusals_allow_fallback_but_timeouts_do_not() {
        // Nothing listening (cold start with the daemon absent/disabled,
        // circuit breaker open, spawn failure) must keep falling back so the
        // user still gets a working terminal.
        let refused = classify_client_acquisition_failure(PtyDaemonError::Io(
            std::io::Error::from(std::io::ErrorKind::ConnectionRefused),
        ));
        assert!(refused.allows_local_fallback());
        let circuit_open = classify_client_acquisition_failure(PtyDaemonError::Daemon(
            "circuit breaker open: too many recent failures".into(),
        ));
        assert!(circuit_open.allows_local_fallback());

        // A timeout or accept-then-EOF means something answered the dial —
        // possibly a stalled daemon that still owns live shells.
        let timed_out = classify_client_acquisition_failure(PtyDaemonError::Timeout);
        assert!(!timed_out.allows_local_fallback());
        let eof = classify_client_acquisition_failure(PtyDaemonError::Closed);
        assert!(!eof.allows_local_fallback());
    }

    #[tokio::test]
    async fn eight_pane_hydration_inventory_uses_exactly_one_list_rpc() {
        let (client_stream, server_stream) = tokio::net::UnixStream::pair().unwrap();
        let client = PtyDaemonClient::from_test_stream(client_stream, Duration::from_secs(1));
        let (server_read, mut server_write) = server_stream.into_split();
        let server = tokio::spawn(async move {
            let mut reader = BufReader::new(server_read);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let request: ClientRequest = serde_json::from_str(line.trim()).unwrap();
            let ClientRequest::List { request_id } = request else {
                panic!("workspace inventory must start with List")
            };
            let sessions = (0..8)
                .map(|index| DaemonSessionInfo {
                    session_id: format!("session-{index}"),
                    shell_id: None,
                    workspace_id: "workspace".into(),
                    pid: 1000 + index,
                    argv: vec!["bash".into()],
                    cwd: "/workspace".into(),
                    rows: 24,
                    cols: 80,
                    created_at: 0,
                })
                .collect();
            let mut response = serde_json::to_vec(&Frame::Response(ServerResponse::Listed {
                request_id,
                sessions,
            }))
            .unwrap();
            response.push(b'\n');
            server_write.write_all(&response).await.unwrap();
            server_write.flush().await.unwrap();

            line.clear();
            assert!(
                tokio::time::timeout(Duration::from_millis(50), reader.read_line(&mut line))
                    .await
                    .is_err(),
                "hydration inventory must not issue a second List"
            );
        });

        let inventory = list_daemon_session_map(&client).await.unwrap();
        assert_eq!(inventory.len(), 8);
        server.await.unwrap();
    }

    // ── build_agent_relaunch_command: per-agent resume synthesis ──

    #[test]
    fn relaunch_claude_with_uuid_resumes() {
        // Claude with a captured UUID → `--resume <uuid>`, skip-perms
        // forwarded because the original carried it.
        let cmd = build_agent_relaunch_command(
            Some("claude --dangerously-skip-permissions"),
            Some("uuid-abc"),
            None,
        );
        assert_eq!(
            cmd.as_deref(),
            Some("claude --dangerously-skip-permissions --resume uuid-abc")
        );
    }

    #[test]
    fn relaunch_claude_without_uuid_omits_resume() {
        let cmd = build_agent_relaunch_command(Some("claude"), None, None);
        assert_eq!(cmd.as_deref(), Some("claude"));
    }

    #[test]
    fn relaunch_opencode_with_session_id_uses_session_flag() {
        // The headline issue #16 case: a pushed/pulled opencode pane resumes
        // the exact synced session by id.
        let cmd = build_agent_relaunch_command(Some("opencode"), None, Some("ses_abc123"));
        assert_eq!(cmd.as_deref(), Some("opencode --session ses_abc123"));
    }

    #[test]
    fn relaunch_opencode_without_session_id_falls_back_to_continue() {
        // No captured id (sync found nothing / local app-restart) → continue
        // the most-recent session for the cwd.
        let cmd = build_agent_relaunch_command(Some("opencode"), None, None);
        assert_eq!(cmd.as_deref(), Some("opencode --continue"));
    }

    #[test]
    fn relaunch_opencode_never_forwards_skip_perms() {
        // The opencode preset carries no skip-permissions flag; even if the
        // original somehow did, opencode resume args stay clean.
        let cmd = build_agent_relaunch_command(
            Some("opencode --dangerously-skip-permissions"),
            None,
            Some("ses_x"),
        );
        assert_eq!(cmd.as_deref(), Some("opencode --session ses_x"));
    }

    #[test]
    fn relaunch_claude_does_not_leak_opencode_session_id() {
        // A stray opencode capture on a claude pane must not produce
        // `--session` (wrong flag for claude).
        let cmd = build_agent_relaunch_command(Some("claude"), None, Some("ses_should_ignore"));
        assert_eq!(cmd.as_deref(), Some("claude"));
    }

    #[test]
    fn relaunch_unknown_agent_returns_bare_binary() {
        // Non-resume agents relaunch as the bare binary (prior behavior).
        let cmd = build_agent_relaunch_command(Some("codex --foo bar"), None, None);
        assert_eq!(cmd.as_deref(), Some("codex"));
    }

    #[test]
    fn relaunch_none_when_no_original_command() {
        assert_eq!(build_agent_relaunch_command(None, None, None), None);
        assert_eq!(build_agent_relaunch_command(Some(""), None, None), None);
    }

    // These tests pin the gate that prevents the Linux preset-leak bug.
    //
    // Bug shape: clicking a CLI preset on Linux wrote the preset's
    // command twice — once submitted to the shell (which launched the
    // agent) and once typed into the just-started agent's input box.
    // Root cause was that the daemon-backed spawn task synthesized a
    // "relaunch the agent" command whenever the in-memory
    // `original_command` was set, racing the `apply_preset` handler's
    // own write_command_when_ready call. The gate below restricts that
    // synthesis to genuine relaunches (pull-back, restart) where one of
    // (disk_meta, captured agent UUID) is present. A fresh preset
    // launch has neither, so it falls through to apply_preset's single
    // write.
    //
    // Windows was unaffected because the entire daemon-backed path is
    // `#[cfg(unix)]`-gated — the in-process spawn has no synthesis
    // logic. So this is specifically a Linux/macOS regression.

    #[test]
    fn fresh_preset_launch_does_not_synthesize() {
        // The exact case from the user's bug report: clicking the
        // Claude Code preset in a fresh workspace. No prior session
        // ever ran on this PTY → no disk_meta, no captured UUID.
        // Synthesizing here would duplicate apply_preset's write.
        assert!(
            !should_synthesize_agent_relaunch(false, false),
            "fresh preset launch (no disk meta, no UUID) must NOT \
             synthesize — apply_preset owns the single write"
        );
    }

    #[test]
    fn restart_with_disk_meta_synthesizes() {
        // App restart: scrollback meta was flushed to disk in a prior
        // session. apply_preset is NOT being invoked — the daemon spawn
        // task is the only thing that will write a command. Must
        // synthesize so the agent relaunches.
        assert!(
            should_synthesize_agent_relaunch(true, false),
            "respawn with disk meta but no UUID must synthesize so the \
             agent restarts (e.g. non-Claude agents on app restart)"
        );
    }

    #[test]
    fn pullback_with_captured_uuid_synthesizes() {
        // Pull-back from remote: scrollback meta hasn't flushed yet
        // (user never closed), but the in-memory adapter captures hold
        // Claude's session UUID. We want `claude --resume <uuid>` to
        // fire on the local host so the conversation continues.
        assert!(
            should_synthesize_agent_relaunch(false, true),
            "pull-back with captured Claude UUID must synthesize the \
             --resume relaunch command"
        );
    }

    #[test]
    fn both_present_still_synthesizes() {
        // Belt-and-suspenders: long-running session that's been used
        // through both an app restart (disk meta) and a remote round-trip
        // (UUID). Both signals say "this is a real relaunch."
        assert!(should_synthesize_agent_relaunch(true, true));
    }

    // ── remote_spawn_cwd: where a host-backed terminal lands on the host ──

    #[test]
    fn remote_spawn_cwd_prefers_remote_cwd_for_attach_in_place() {
        // The whole point of "open on host": the terminal spawns in the
        // workspace's REAL on-host directory (which can be an arbitrary,
        // agent-created path), not a reconstructed conventional path.
        let store = crate::state::AppStateStore::default();
        let wid = store.create_remote_attach_workspace(
            "demo".into(),
            1,
            "/srv/agent-made/app".into(),
            Some("feature/x".into()),
            Some("/home/me/app".into()),
            Some("uid-123".into()),
            Some("worktree".into()),
        );
        let snap = store.snapshot();
        let ws = snap
            .workspaces
            .iter()
            .find(|w| w.workspace_id.0 == wid.0)
            .expect("workspace exists");
        assert!(ws.attach_only, "open-on-host workspace is attach_only");
        assert_eq!(ws.host_id, Some(1));
        assert_eq!(ws.remote_cwd.as_deref(), Some("/srv/agent-made/app"));
        assert_eq!(
            super::remote_spawn_cwd(Some(ws)),
            "/srv/agent-made/app",
            "remote_cwd must win over the reconstructed conventional path"
        );
    }

    #[test]
    fn remote_spawn_cwd_falls_back_to_conventional_without_remote_cwd() {
        // Pushed workspaces (and any host workspace whose path we don't
        // know) fall back to the conventional `~/.codemux/worktrees/...`
        // layout. `None` exercises the safe defaults.
        let cwd = super::remote_spawn_cwd(None);
        assert!(cwd.contains("worktrees"), "got {cwd}");
        assert!(cwd.contains("workspace"), "default project name; got {cwd}");
        assert!(cwd.ends_with("main"), "default branch; got {cwd}");
    }

    #[test]
    fn pushed_terminal_env_names_absolute_device_paths() {
        // A push only stamps `host_id`, so its terminals start in the
        // conventional `~` path. The daemon expands `~` in the cwd but sets
        // env verbatim, so the env must be built from the path resolved
        // with the device's home.
        let local_worktree = "/home/zeus/.codemux/worktrees/app/feat-x";
        let ws = crate::state::PtyHydrationWorkspace {
            workspace_id: "ws-pushed".into(),
            title: "feat-x".into(),
            cwd: local_worktree.into(),
            git_branch: Some("feat-x".into()),
            worktree_path: Some(local_worktree.into()),
            project_root: Some("/home/zeus/projects/app".into()),
            project_uid: None,
            host_id: Some(4),
            remote_cwd: None,
            remote_root: None,
            attach_only: false,
        };
        let unresolved = super::hydration_remote_spawn_cwd(&ws);
        assert_eq!(unresolved, "~/.codemux/worktrees/app/feat-x");
        let device_dir = crate::ssh::exec::expand_remote_tilde(&unresolved, "/home/deus");

        let env: std::collections::HashMap<_, _> =
            super::hydration_workspace_pty_env(&ws, Some(&device_dir))
                .into_iter()
                .collect();
        let expected = "/home/deus/.codemux/worktrees/app/feat-x";
        assert_eq!(env["CODEMUX_WORKSPACE_PATH"], expected);
        assert_eq!(env["CODEMUX_ROOT_PATH"], expected);
        for (key, value) in &env {
            assert!(!value.contains("~/"), "{key}={value}");
            assert!(!value.contains("/home/zeus"), "{key}={value}");
        }
    }
}
