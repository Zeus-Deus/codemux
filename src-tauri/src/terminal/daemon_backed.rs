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
//!     xterm onData → write_to_pty (sync) → `DaemonWriter::write` (fire
//!     and forget tokio task) → client.write → socket → daemon → master fd.

use super::{
    emit_terminal_status, hydration_workspace_pty_env, queue_or_send_output, with_session_runtime,
    PtyState, SessionRuntime, SessionSpawnReservation, TerminalLifecycleState,
    TerminalStatusPayload, DEFAULT_COLS, DEFAULT_ROWS,
};
use crate::pty_daemon::{DaemonSessionInfo, PtyDaemonClient, PtyDaemonError};
use crate::state::{AppStateStore, PtyHydrationPlan, PtyHydrationSession, PtyHydrationWorkspace};
use futures_util::stream::StreamExt as _;
use std::collections::HashMap;
use std::sync::Arc;
use tauri::{AppHandle, Manager, Runtime, State};

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

const HYDRATION_CONCURRENCY: usize = 4;

/// Whether a local in-process shell can safely replace a failed daemon RPC.
/// A transport failure after sending Spawn or Attach has an unknown outcome:
/// the daemon may already own the shell even though its acknowledgement was
/// lost. Starting a local fallback in that case would duplicate the process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DaemonHydrationFailure {
    AlreadyReserved,
    SafeToFallback(String),
    Ambiguous(String),
}

impl DaemonHydrationFailure {
    pub(crate) fn allows_local_fallback(&self) -> bool {
        matches!(self, Self::SafeToFallback(_))
    }

    pub(crate) fn is_already_reserved(&self) -> bool {
        matches!(self, Self::AlreadyReserved)
    }
}

impl std::fmt::Display for DaemonHydrationFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyReserved => f.write_str("session already reserved by another spawn"),
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
    let failures = hydrate_workspace_via_daemon(app, plan).await?;
    failures
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
pub(crate) async fn hydrate_workspace_via_daemon<R: Runtime>(
    app: AppHandle<R>,
    plan: PtyHydrationPlan,
) -> Result<Vec<(String, DaemonHydrationFailure)>, DaemonHydrationFailure> {
    let hydration_started = std::time::Instant::now();
    let terminal_state: State<'_, PtyState> = app.state();
    let sessions = terminal_state.sessions.clone();
    let pending: Vec<PtyHydrationSession> = plan
        .sessions
        .into_iter()
        .filter(|session| !super::is_session_spawn_active(&sessions, &session.session_id))
        .collect();
    if pending.is_empty() {
        crate::diagnostics::record_perf_timing(
            "pty.workspace-hydration",
            hydration_started.elapsed(),
        );
        return Ok(Vec::new());
    }

    let workspace = Arc::new(plan.workspace);
    let is_remote = workspace.host_id.is_some();
    if is_remote {
        for session in &pending {
            emit_terminal_status(
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
            );
        }
    }

    let client =
        match crate::ssh::client_for_workspace(&app, &workspace.workspace_id, workspace.host_id)
            .await
        {
            Ok(client) => client,
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
    let prep_sessions = pending.clone();
    let prepared = tokio::task::spawn_blocking(move || {
        let mut workspace_env = hydration_workspace_pty_env(&prep_workspace);
        if !is_remote {
            if let Some((shim_dir, current_exe)) = super::ensure_cli_shims() {
                let current_path = crate::execution::sanitized_child_path();
                let prefixed = super::build_child_path(&shim_dir, &current_path);
                workspace_env.push(("PATH".into(), prefixed));
                workspace_env.push(("CODEMUX_CLI_SAFE_PATH".into(), current_exe));
            }
        }
        // One directory walk for the whole workspace: the per-session lookup
        // re-read and re-parsed every meta file on disk for each pane.
        let wanted: std::collections::HashSet<&str> = prep_sessions
            .iter()
            .map(|session| session.session_id.as_str())
            .collect();
        let disk_meta: HashMap<_, _> = crate::scrollback::find_scrollback_meta_for_sessions(&wanted);
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
    for session in pending {
        let prior = existing.remove(&session.session_id);
        let meta = disk_meta.remove(&session.session_id);
        let session_id = session.session_id.clone();
        let app = app.clone();
        let workspace = workspace.clone();
        let client = client.clone();
        let workspace_env = workspace_env.clone();
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
                shell,
                session_restore_enabled,
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
    Ok(results
        .into_iter()
        .filter_map(|(session_id, result)| result.err().map(|error| (session_id, error)))
        .collect())
}

/// Daemon-backed shell spawn — the persistent equivalent of
/// `spawn_pty_for_session_in_process`. Mirrors the env construction,
/// scrollback restore, and session-adapter wiring of the in-process path
/// so user-typed commands inside the shell get the same Codemux context
/// AND reopening a previously-killed agent triggers the same
/// `claude --continue` / adapter-driven resume the in-process path does.
async fn spawn_prepared_session<R: Runtime>(
    app: AppHandle<R>,
    workspace: Arc<PtyHydrationWorkspace>,
    session: PtyHydrationSession,
    client: Arc<PtyDaemonClient>,
    existing: Option<DaemonSessionInfo>,
    disk_meta: Option<(String, String, crate::scrollback::ScrollbackMeta)>,
    workspace_env: Arc<Vec<(String, String)>>,
    shell: String,
    session_restore_enabled: bool,
) -> Result<(), DaemonHydrationFailure> {
    let session_id = session.session_id.clone();
    let entry_ts = std::time::Instant::now();
    crate::trace_cloud_push!("[trace:{session_id}] spawn_via_daemon ENTRY t=0ms");
    let terminal_state: State<'_, PtyState> = app.state();
    let app_state: State<'_, AppStateStore> = app.state();
    let sessions = terminal_state.sessions.clone();

    let Some(mut reservation) = SessionSpawnReservation::try_new(&sessions, &session_id) else {
        crate::trace_cloud_push!(
            "[trace:{session_id}] try_reserve FAILED at t={}ms",
            entry_ts.elapsed().as_millis()
        );
        return Err(DaemonHydrationFailure::AlreadyReserved);
    };
    let workspace_id = workspace.workspace_id.clone();
    let host_id = workspace.host_id;
    let is_remote = host_id.is_some();
    app_state.update_terminal_session_shell(&session_id, shell.clone());

    // ── Scrollback restore + adapter resume parity with in-process path.
    //
    // If there's saved scrollback for this session id, and the session-
    // restore setting is on, we (a) use the original cwd so CWD-scoped
    // tools like `claude --resume` find their state, and (b) capture an
    // `auto_resume_command` that we'll write into the shell after spawn.
    // Mirrors `spawn_pty_for_session_in_process` lines around 1166-1200.
    // Remote workspaces spawn into the conventional remote path
    // (`~/.codemux/worktrees/<project>/<branch>`) rather than the
    // local cwd — the workspace's `cwd` field is a local-filesystem
    // path that doesn't exist on the remote host. Local workspaces
    // keep using the local cwd as before.
    let mut effective_cwd = if is_remote {
        let computed = hydration_remote_spawn_cwd(&workspace);
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
        ("BROWSER".into(), "codemux browser open".into()),
    ];
    env.push(("CODEMUX_WORKSPACE_ID".into(), workspace_id.clone()));
    for kv in workspace_env.iter() {
        env.push(kv.clone());
    }
    if let Some(pane_id) = pane_id_for_env.as_ref() {
        env.push(("CODEMUX_PANE_ID".into(), pane_id.clone()));
    }
    if let Some(port) = crate::hooks::hook_port() {
        env.push(("CODEMUX_HOOK_PORT".into(), port.to_string()));
    }
    emit_terminal_status(
        &app,
        &sessions,
        TerminalStatusPayload {
            session_id: session_id.clone(),
            state: TerminalLifecycleState::Starting,
            message: Some(format!("Starting persistent shell: {shell}")),
            exit_code: None,
        },
    );

    // Idempotent reattach decision comes from the workspace-level List. The
    // caller performs exactly one List and maps it before any pane tasks fan
    // out, so an eight-pane activation never sends eight identical RPCs.
    let reattached;
    let pid = if let Some(existing) = existing {
        reattached = true;
        crate::trace_cloud_push!(
            "[trace:{session_id}] DECISION=reattach pid={} at t={}ms",
            existing.pid,
            entry_ts.elapsed().as_millis()
        );
        existing.pid
    } else {
        reattached = false;
        crate::trace_cloud_push!(
            "[trace:{session_id}] DECISION=fresh_spawn at t={}ms",
            entry_ts.elapsed().as_millis()
        );
        match client
            .spawn(
                session_id.clone(),
                workspace_id,
                vec![shell.clone()],
                effective_cwd,
                env,
                DEFAULT_ROWS,
                DEFAULT_COLS,
            )
            .await
        {
            Ok(pid) => pid,
            Err(error) => {
                return Err(classify_spawn_rpc_failure(error));
            }
        }
    };

    let mut rx = match client.attach(session_id.clone()).await {
        Ok(rx) => rx,
        Err(error) => {
            // Keep a successfully spawned daemon session alive. The reservation
            // guard clears the local `is_spawning` marker on return, and a retry
            // will discover this session through List and reattach idempotently.
            // Sending a background Close here races that retry and can kill the
            // shell immediately after the second attach succeeds.
            return Err(classify_attach_rpc_failure(error));
        }
    };

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
    with_session_runtime(
        &sessions,
        &session_id,
        || SessionRuntime::new(&session_id),
        |runtime| {
            runtime.writer = Some(Box::new(writer));
            runtime.master = None;
            runtime.child_pid = Some(pid);
            runtime.persistent = true;
            runtime.is_spawning = false;
            // On reattach, skip_preset_launch must ALSO be true so the
            // preset launcher (separate from auto-write) doesn't fire
            // a preset write into the live agent.
            runtime.skip_preset_launch = reattached || auto_resume_clone.is_some();
            runtime.resume_command = auto_resume_clone.clone();
            // Same as the agent path — capture the client so resize/close
            // route to the daemon that actually owns this session id.
            runtime.daemon_client = Some(client_for_runtime);
        },
    );
    reservation.commit();

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
                                emit_terminal_status(
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
                                );
                                // Don't write the command — let the bare bash
                                // prompt remain on the pane as a fallback.
                                return Ok(());
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

    emit_terminal_status(
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
        while let Some(chunk) = rx.recv().await {
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
            queue_or_send_output(&read_sessions, &read_session_id, chunk);
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
            "Shell ended",
        );
    });

    Ok(())
}

/// Adapter from sync `std::io::Write` to async `PtyDaemonClient::write`.
///
/// Writes are **fire-and-forget**: each `write` call clones the bytes,
/// spawns a tokio task that sends them to the daemon, and returns the
/// reported byte count immediately. Failures are logged but don't bubble
/// up to the caller. This matches the existing in-process behavior, where
/// `portable_pty::Writer::write` is also effectively non-blocking once
/// the OS buffer has room.
pub(crate) struct DaemonWriter {
    client: Arc<PtyDaemonClient>,
    session_id: String,
}

impl DaemonWriter {
    fn new(client: Arc<PtyDaemonClient>, session_id: String) -> Self {
        Self { client, session_id }
    }
}

impl std::io::Write for DaemonWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let client = self.client.clone();
        let session_id = self.session_id.clone();
        let data = buf.to_vec();
        // Only log on failure — the happy path fires for every
        // keystroke, which would flood stderr.
        tauri::async_runtime::spawn(async move {
            if let Err(error) = client.write(session_id.clone(), &data).await {
                eprintln!(
                    "[codemux::terminal::daemon_backed] DaemonWriter dispatch failed for \
                     {session_id}: {error}"
                );
            }
        });
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        // No-op: writes are already dispatched, and the daemon flushes the
        // master fd after every write. A blocking flush here would require
        // round-tripping the daemon, which is wrong for the sync interface.
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
}
