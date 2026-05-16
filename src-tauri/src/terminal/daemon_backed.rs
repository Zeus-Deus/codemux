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
/// `spawn_pty_for_session_in_process`. Mirrors the env construction,
/// scrollback restore, and session-adapter wiring of the in-process path
/// so user-typed commands inside the shell get the same Codemux context
/// AND reopening a previously-killed agent triggers the same
/// `claude --continue` / adapter-driven resume the in-process path does.
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

    // Resolve the workspace + its host assignment BEFORE picking a
    // daemon client. host_id=None → local daemon (this device).
    // host_id=Some(...) → SSH-tunneled remote daemon. Either way
    // `client_for_workspace` returns the right one (caching for
    // perf so repeated spawns in the same workspace reuse the
    // connection).
    let shell = super::default_shell();
    app_state.update_terminal_session_shell(&session_id, shell.clone());

    let snapshot = app_state.snapshot();
    let owning_ws = super::find_owning_workspace(&snapshot, &session_id);
    let workspace_id = owning_ws
        .map(|w| w.workspace_id.0.clone())
        .unwrap_or_default();
    let host_id = owning_ws.and_then(|w| w.host_id);
    let is_remote = host_id.is_some();

    // Emit an early "Connecting…" status for remote spawns so the
    // overlay shows progress during the tunnel + daemon-handshake
    // wait. Without this the user sees "Starting persistent shell"
    // for up to 40s with no movement — looks like a hang.
    if is_remote {
        emit_terminal_status(
            &app,
            &sessions,
            TerminalStatusPayload {
                session_id: session_id.clone(),
                state: TerminalLifecycleState::Starting,
                message: Some(
                    "Connecting to remote host (this can take up to 20s on \
                     first connect)…"
                        .into(),
                ),
                exit_code: None,
            },
        );
    }

    let client = match crate::ssh::client_for_workspace(
        &app,
        &workspace_id,
        host_id,
    )
    .await
    {
        Ok(c) => c,
        Err(error) => {
            remove_session_runtime(&sessions, &session_id);
            return Err(format!("daemon client: {error}"));
        }
    };

    // ── Scrollback restore + adapter resume parity with in-process path.
    //
    // If there's saved scrollback for this session id, and the session-
    // restore setting is on, we (a) use the original cwd so CWD-scoped
    // tools like `claude --resume` find their state, and (b) capture an
    // `auto_resume_command` that we'll write into the shell after spawn.
    // Mirrors `spawn_pty_for_session_in_process` lines around 1166-1200.
    let session_restore_enabled = crate::settings_sync::load_cache()
        .map(|s| s.session_restore.enabled)
        .unwrap_or(true);
    // Remote workspaces spawn into the conventional remote path
    // (`~/.codemux/worktrees/<project>/<branch>`) rather than the
    // local cwd — the workspace's `cwd` field is a local-filesystem
    // path that doesn't exist on the remote host. Local workspaces
    // keep using the local cwd as before.
    let mut effective_cwd = if is_remote {
        let project_name = owning_ws
            .and_then(|w| {
                w.project_root
                    .as_deref()
                    .and_then(|p| std::path::Path::new(p).file_name())
                    .and_then(|n| n.to_str())
                    .map(|s| s.to_string())
            })
            .unwrap_or_else(|| "workspace".to_string());
        let branch = owning_ws
            .and_then(|w| w.git_branch.clone())
            .unwrap_or_else(|| "main".to_string());
        crate::ssh::conventional_remote_path(&project_name, &branch)
            .to_string_lossy()
            .to_string()
    } else {
        session_working_dir(&app_state, &session_id)
    };
    let mut auto_resume_command: Option<String> = None;
    let mut pane_id_for_env: Option<String> = None;

    // Scrollback restore + adapter resume are local-machine
    // concepts (the cache lives on disk on the laptop). Skip them
    // for remote workspaces — when chat-on-remote ships we'll
    // revisit how to coordinate adapter state across the tunnel.
    if session_restore_enabled && !is_remote {
        if let Some(adapter_state) =
            app.try_state::<crate::session_adapters::AdapterState>()
        {
            if let Some((ws_id, pane_id, meta)) =
                crate::scrollback::find_scrollback_meta_for_session(&session_id)
            {
                effective_cwd =
                    super::resolve_session_cwd(&meta.working_directory, &effective_cwd);
                pane_id_for_env = Some(pane_id.clone());
                if let Some(resume_command) =
                    super::resolve_resume_command(&snapshot, &meta, &adapter_state)
                {
                    eprintln!(
                        "[codemux::terminal::daemon_backed] restored session at \
                         {ws_id}/{pane_id} for {session_id}; auto-resume armed"
                    );
                    auto_resume_command = Some(resume_command);
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
    if let Some(pane_id) = pane_id_for_env.as_ref() {
        env.push(("CODEMUX_PANE_ID".into(), pane_id.clone()));
    }
    if let Some(port) = crate::hooks::hook_port() {
        env.push(("CODEMUX_HOOK_PORT".into(), port.to_string()));
    }
    // PATH + CLI shim injection are local-machine concepts —
    // injecting the laptop's PATH into a remote shell would be
    // worse than nothing (paths to /home/zeus/... etc don't exist
    // on the remote, and the shim dir lives in the laptop's
    // filesystem). For remote workspaces the remote shell uses
    // its own default PATH from the user's ~/.bashrc / ~/.zshrc.
    if !is_remote {
        if let Some((shim_dir, current_exe)) = super::ensure_openflow_cli_shims() {
            let current_path = std::env::var("PATH").unwrap_or_default();
            let prefixed = super::build_child_path(&shim_dir, &current_path);
            env.push(("PATH".into(), prefixed));
            env.push(("CODEMUX_CLI_SAFE_PATH".into(), current_exe));
        }
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
                effective_cwd,
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
    let auto_resume_clone = auto_resume_command.clone();
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
            runtime.skip_preset_launch = auto_resume_clone.is_some();
            runtime.resume_command = auto_resume_clone;
        },
    );

    // Send the resume command via the same write-when-ready path the
    // in-process spawn uses. Because our `DaemonWriter` is already in
    // `runtime.writer`, this lands at the daemon, which writes to the
    // master fd; the shell sees it as if the user typed it.
    if let Some(command) = auto_resume_command {
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
    let original_cmd = snapshot
        .terminal_sessions
        .iter()
        .find(|s| s.session_id.0 == session_id)
        .and_then(|s| s.original_command.clone());
    let has_scanner = if let (Some(ref adapter), Some(ref cmd)) =
        (&adapter_clone, &original_cmd)
    {
        adapter.start_scanner(&session_id, cmd).is_some()
    } else {
        false
    };

    let read_sessions = sessions.clone();
    let read_session_id = session_id.clone();
    let scanner_session_id = session_id.clone();
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
