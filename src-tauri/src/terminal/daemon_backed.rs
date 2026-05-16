//! Daemon-backed agent spawn path.
//!
//! Mirrors the env+command construction of `spawn_pty_for_agent_in_process`
//! but instead of `portable_pty::openpty()` + child spawn in this process,
//! the work happens inside the long-lived `codemux pty-daemon`. The
//! resulting `SessionRuntime` is marked `persistent = true` so the close
//! and Drop paths skip the kill-the-process-group step.
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
    emit_terminal_status, queue_or_send_output, remove_session_runtime, session_working_dir,
    with_session_runtime, workspace_pty_env, PtyState, SessionRuntime,
    TerminalLifecycleState, TerminalStatusPayload, DEFAULT_COLS, DEFAULT_ROWS,
};
use crate::execution::ExecutionPolicy;
use crate::pty_daemon::{ensure_daemon, PtyDaemonClient};
use crate::state::AppStateStore;
use std::sync::Arc;
use tauri::{AppHandle, Manager, State};

/// Public entrypoint. Called from `spawn_pty_for_agent` when the
/// `persistent_agents.enabled` setting is on. Returns an error if the
/// daemon can't be reached, the spawn failed, or the attach failed —
/// callers fall back to the in-process path so the user still gets a
/// working agent.
pub async fn spawn_pty_for_agent_via_daemon(
    app: AppHandle,
    session_id: String,
    workspace_id: String,
    argv: Vec<String>,
    extra_env: Vec<(String, String)>,
    execution_policy: ExecutionPolicy,
) -> Result<(), String> {
    let terminal_state: State<'_, PtyState> = app.state();
    let app_state: State<'_, AppStateStore> = app.state();
    let sessions = terminal_state.sessions.clone();

    // Same TOCTOU-resistant reservation as the in-process path.
    if !super::try_reserve_session_spawn(&sessions, &session_id) {
        return Err("session already reserved by another spawn".into());
    }

    // Reach (or spawn) the daemon BEFORE the heavy env construction so we
    // fail fast on the trivial cases (daemon binary missing, socket race).
    let client = match ensure_daemon().await {
        Ok(c) => c,
        Err(error) => {
            remove_session_runtime(&sessions, &session_id);
            return Err(format!("ensure_daemon: {error}"));
        }
    };

    let executable = argv
        .first()
        .cloned()
        .ok_or_else(|| {
            remove_session_runtime(&sessions, &session_id);
            "empty argv".to_string()
        })?;

    let prepared = crate::execution::prepare_agent_command(
        executable.clone(),
        argv.iter().skip(1).cloned().collect(),
        &session_working_dir(&app_state, &session_id),
        &execution_policy,
    );

    emit_terminal_status(
        &app,
        &sessions,
        TerminalStatusPayload {
            session_id: session_id.clone(),
            state: TerminalLifecycleState::Starting,
            message: Some(format!(
                "Starting persistent agent: {} [daemon-backed]",
                prepared.executable
            )),
            exit_code: None,
        },
    );

    let cwd = session_working_dir(&app_state, &session_id);
    let env = build_agent_env(
        &app_state,
        &workspace_id,
        &session_id,
        &extra_env,
        &execution_policy,
        &prepared,
    );

    let mut full_argv = vec![prepared.executable.clone()];
    full_argv.extend(prepared.args.iter().cloned());

    // Idempotent reattach: if the daemon already knows this session id
    // (the user reopened the app and we're being called to "spawn" what's
    // actually a session that survived the previous run), skip the spawn
    // and use the existing pid. This is what makes "close app, reopen,
    // agent still there" work end-to-end.
    let existing = match client.list().await {
        Ok(list) => list.into_iter().find(|s| s.session_id == session_id),
        Err(error) => {
            eprintln!(
                "[codemux::terminal::daemon_backed] daemon list failed during reattach \
                 check for {session_id}: {error}"
            );
            None
        }
    };

    let pid = if let Some(existing) = existing {
        eprintln!(
            "[codemux::terminal::daemon_backed] reattaching to live daemon session \
             {session_id} pid={}",
            existing.pid
        );
        existing.pid
    } else {
        match client
            .spawn(
                session_id.clone(),
                workspace_id.clone(),
                full_argv,
                cwd,
                env,
                DEFAULT_ROWS,
                DEFAULT_COLS,
            )
            .await
        {
            Ok(p) => p,
            Err(error) => {
                remove_session_runtime(&sessions, &session_id);
                emit_terminal_status(
                    &app,
                    &sessions,
                    TerminalStatusPayload {
                        session_id: session_id.clone(),
                        state: TerminalLifecycleState::Failed,
                        message: Some(format!("daemon spawn failed: {error}")),
                        exit_code: None,
                    },
                );
                return Err(format!("daemon spawn: {error}"));
            }
        }
    };

    let mut rx = match client.attach(session_id.clone()).await {
        Ok(rx) => rx,
        Err(error) => {
            // Best-effort: tell the daemon to clean up the spawn we just
            // succeeded at, since we can't actually use it.
            let _ = client.close(session_id.clone()).await;
            remove_session_runtime(&sessions, &session_id);
            emit_terminal_status(
                &app,
                &sessions,
                TerminalStatusPayload {
                    session_id: session_id.clone(),
                    state: TerminalLifecycleState::Failed,
                    message: Some(format!("daemon attach failed: {error}")),
                    exit_code: None,
                },
            );
            return Err(format!("daemon attach: {error}"));
        }
    };

    // Build a writer that funnels sync writes into the async client.
    let writer = DaemonWriter::new(client.clone(), session_id.clone());

    with_session_runtime(
        &sessions,
        &session_id,
        || SessionRuntime::new(&session_id),
        |runtime| {
            runtime.writer = Some(Box::new(writer));
            // Daemon owns the real master; we don't have a portable_pty
            // master handle. The existing reader-loop machinery never sees
            // this path — resize goes through a separate daemon call.
            runtime.master = None;
            runtime.child_pid = Some(pid);
            runtime.persistent = true;
            runtime.is_spawning = false;
        },
    );

    emit_terminal_status(
        &app,
        &sessions,
        TerminalStatusPayload {
            session_id: session_id.clone(),
            state: TerminalLifecycleState::Ready,
            message: Some(format!(
                "Persistent agent ready: {} [pid {pid}, daemon-backed]",
                prepared.executable
            )),
            exit_code: None,
        },
    );

    // Reader task — drains the daemon's mpsc and pushes bytes through the
    // same `queue_or_send_output` the in-process path uses.
    let read_sessions = sessions.clone();
    let read_session_id = session_id.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(chunk) = rx.recv().await {
            queue_or_send_output(&read_sessions, &read_session_id, chunk);
        }
        eprintln!(
            "[codemux::terminal::daemon_backed] read loop ended for session {read_session_id}"
        );
    });

    // Side-effects parity with the in-process path that we still need to
    // emit even though no `Child` lives in this process:
    //
    // - resource-monitor / process-tree views read `child_pid`; that's the
    //   daemon-side pid, which is correct (it's the actual agent process).
    // - `comm_log` setup: TODO. The in-process path tees comm log writes
    //   from inside the read loop; we'd need to do the same here. Marking
    //   as a follow-up because comm-log is OpenFlow-specific and step 1's
    //   only goal is "agents survive app close" — OpenFlow agents can opt
    //   out of persistence for now.

    Ok(())
}

/// Daemon-backed shell spawn — the persistent equivalent of
/// `spawn_pty_for_session_in_process`. Mirrors enough of the env construction
/// from the in-process path that user-typed commands inside the shell see
/// the same `CODEMUX_*` and workspace env they always have.
///
/// Tradeoff vs. the in-process path: the session adapter system (which
/// rewinds scrollback and offers `--resume` for matching agents) is NOT
/// wired here yet. The whole point of persistent shells is that they
/// genuinely survive — there's nothing to "resume." Scrollback may still
/// be replayed by the daemon's per-session replay buffer when the client
/// reattaches.
pub async fn spawn_pty_for_session_via_daemon(
    app: AppHandle,
    session_id: String,
) -> Result<(), String> {
    let terminal_state: State<'_, PtyState> = app.state();
    let app_state: State<'_, AppStateStore> = app.state();
    let sessions = terminal_state.sessions.clone();

    if !super::try_reserve_session_spawn(&sessions, &session_id) {
        return Err("session already reserved by another spawn".into());
    }

    let client = match ensure_daemon().await {
        Ok(c) => c,
        Err(error) => {
            remove_session_runtime(&sessions, &session_id);
            return Err(format!("ensure_daemon: {error}"));
        }
    };

    let shell = super::default_shell();
    app_state.update_terminal_session_shell(&session_id, shell.clone());

    let cwd = session_working_dir(&app_state, &session_id);
    let snapshot = app_state.snapshot();
    let owning_ws = super::find_owning_workspace(&snapshot, &session_id);
    let workspace_id = owning_ws
        .map(|w| w.workspace_id.0.clone())
        .unwrap_or_default();

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
        (
            "CODEMUX_BROWSER_CMD".into(),
            "codemux browser".into(),
        ),
        ("BROWSER".into(), "codemux browser open".into()),
    ];
    if let Some(ws) = owning_ws {
        env.push(("CODEMUX_WORKSPACE_ID".into(), ws.workspace_id.0.clone()));
        for kv in workspace_pty_env(ws) {
            env.push(kv);
        }
    } else {
        env.push((
            "CODEMUX_AGENT_CONTEXT".into(),
            crate::agent_context::build_agent_context(None, None, None, None),
        ));
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

    // Idempotent reattach for shells (same logic as agents).
    let existing = client
        .list()
        .await
        .ok()
        .and_then(|list| list.into_iter().find(|s| s.session_id == session_id));

    let pid = if let Some(existing) = existing {
        eprintln!(
            "[codemux::terminal::daemon_backed] reattaching to live shell session \
             {session_id} pid={}",
            existing.pid
        );
        existing.pid
    } else {
        match client
            .spawn(
                session_id.clone(),
                workspace_id,
                vec![shell.clone()],
                cwd,
                env,
                DEFAULT_ROWS,
                DEFAULT_COLS,
            )
            .await
        {
            Ok(pid) => pid,
            Err(error) => {
                remove_session_runtime(&sessions, &session_id);
                emit_terminal_status(
                    &app,
                    &sessions,
                    TerminalStatusPayload {
                        session_id: session_id.clone(),
                        state: TerminalLifecycleState::Failed,
                        message: Some(format!("daemon shell spawn failed: {error}")),
                        exit_code: None,
                    },
                );
                return Err(format!("daemon spawn: {error}"));
            }
        }
    };

    let mut rx = match client.attach(session_id.clone()).await {
        Ok(rx) => rx,
        Err(error) => {
            let _ = client.close(session_id.clone()).await;
            remove_session_runtime(&sessions, &session_id);
            return Err(format!("daemon attach: {error}"));
        }
    };

    let writer = DaemonWriter::new(client.clone(), session_id.clone());
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
        },
    );

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

    let read_sessions = sessions.clone();
    let read_session_id = session_id.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(chunk) = rx.recv().await {
            queue_or_send_output(&read_sessions, &read_session_id, chunk);
        }
        eprintln!(
            "[codemux::terminal::daemon_backed] shell read loop ended for {read_session_id}"
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
        tauri::async_runtime::spawn(async move {
            if let Err(error) = client.write(session_id.clone(), &data).await {
                eprintln!(
                    "[codemux::terminal::daemon_backed] write to session {session_id} \
                     failed: {error}"
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

/// Constructs the env Vec the daemon's child should inherit. Mirrors the
/// inline env construction in `spawn_pty_for_agent_in_process`, kept
/// reasonably aligned by hand. If you add env to one, add it to the
/// other; the in-process path uses `cmd.env(k, v)` against a
/// `CommandBuilder`, this path returns a Vec.
fn build_agent_env(
    app_state: &State<'_, AppStateStore>,
    workspace_id: &str,
    session_id: &str,
    extra_env: &[(String, String)],
    execution_policy: &ExecutionPolicy,
    prepared: &crate::execution::PreparedExecutionCommand,
) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = Vec::new();

    // Terminal capability advertisement (mirrors spawn_pty_for_session).
    env.push(("TERM".to_string(), "xterm-256color".to_string()));
    env.push(("COLORTERM".to_string(), "truecolor".to_string()));
    env.push(("TERM_PROGRAM".to_string(), "codemux".to_string()));
    env.push((
        "TERM_PROGRAM_VERSION".to_string(),
        env!("CARGO_PKG_VERSION").to_string(),
    ));

    // Codemux env vars.
    env.push(("CODEMUX".to_string(), "1".to_string()));
    env.push((
        "CODEMUX_VERSION".to_string(),
        env!("CARGO_PKG_VERSION").to_string(),
    ));
    env.push((
        "CODEMUX_WORKSPACE_ID".to_string(),
        workspace_id.to_string(),
    ));
    env.push(("CODEMUX_SURFACE_ID".to_string(), session_id.to_string()));
    env.push((
        "CODEMUX_BROWSER_CMD".to_string(),
        "codemux browser".to_string(),
    ));
    env.push(("BROWSER".to_string(), "codemux browser open".to_string()));

    // Workspace-derived env.
    {
        let snapshot = app_state.snapshot();
        if let Some(ws) = snapshot
            .workspaces
            .iter()
            .find(|w| w.workspace_id.0 == workspace_id)
        {
            for kv in workspace_pty_env(ws) {
                env.push(kv);
            }
        } else {
            env.push((
                "CODEMUX_AGENT_CONTEXT".to_string(),
                crate::agent_context::build_agent_context(None, None, None, None),
            ));
        }
    }

    // CLI shim path. The in-process path calls ensure_openflow_cli_shims(),
    // which is platform-gated; we mirror the same call shape so the shim
    // dir gets created (idempotent) and PATH is prefixed identically.
    if let Some((shim_dir, current_exe)) = super::ensure_openflow_cli_shims() {
        let current_path = std::env::var("PATH").unwrap_or_default();
        let prefixed_path = super::build_child_path(&shim_dir, &current_path);
        env.push(("PATH".to_string(), prefixed_path));
        env.push(("CODEMUX_CLI_SAFE_PATH".to_string(), current_exe));
    }

    // Adapter-provided env (e.g. OpenFlow agent context).
    for (k, v) in extra_env {
        env.push((k.clone(), v.clone()));
    }

    // Execution-backend signaling env.
    env.push((
        "CODEMUX_EXECUTION_BACKEND".to_string(),
        match prepared.backend {
            crate::execution::ExecutionBackendKind::HostPassthrough => "host_passthrough",
            crate::execution::ExecutionBackendKind::LinuxBubblewrap => "linux_bubblewrap",
            crate::execution::ExecutionBackendKind::MacOsSandbox => "macos_sandbox",
            crate::execution::ExecutionBackendKind::WindowsRestricted => "windows_restricted",
        }
        .to_string(),
    ));
    env.push((
        "CODEMUX_ALLOW_DESKTOP_GUI".to_string(),
        if execution_policy.allow_desktop_gui {
            "1".to_string()
        } else {
            "0".to_string()
        },
    ));
    env.push((
        "CODEMUX_ALLOW_BROWSER_AUTOMATION".to_string(),
        if execution_policy.allow_browser_automation {
            "1".to_string()
        } else {
            "0".to_string()
        },
    ));
    env.push((
        "CODEMUX_ALLOW_NETWORK".to_string(),
        if execution_policy.allow_network {
            "1".to_string()
        } else {
            "0".to_string()
        },
    ));

    // Phase-1 env-strip parity. `prepared.env_unset` is enforced by the
    // daemon by simply omitting those keys; we filter out any earlier
    // pushes that match. `prepared.env_set` overrides anything earlier.
    let unset: std::collections::HashSet<&str> =
        prepared.env_unset.iter().map(|s| s.as_str()).collect();
    env.retain(|(k, _)| !unset.contains(k.as_str()));
    for (k, v) in &prepared.env_set {
        env.push((k.clone(), v.clone()));
    }

    env
}
