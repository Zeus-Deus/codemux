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
use crate::pty_daemon::PtyDaemonClient;
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

    // Resolve the workspace + its host BEFORE picking a daemon client.
    // Remote workspaces route through their per-workspace SSH-tunneled
    // daemon; local workspaces use the singleton local daemon. Same
    // dispatch the shell spawn path uses.
    let snapshot = app_state.snapshot();
    let owning_ws = super::find_owning_workspace(&snapshot, &session_id);
    let host_id = owning_ws.and_then(|w| w.host_id);
    let is_remote = host_id.is_some();

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

    // Remote workspaces resolve their cwd to the conventional remote path
    // (`~/.codemux/worktrees/<project>/<branch>`) — the local cwd doesn't
    // exist on the remote host. Local workspaces keep their actual cwd.
    let cwd = if is_remote {
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
    let env = build_agent_env(
        &app_state,
        &workspace_id,
        &session_id,
        &extra_env,
        &execution_policy,
        &prepared,
        is_remote,
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
    let client_for_runtime = client.clone();

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
            // Capture the client so resize/close land on the right daemon
            // (local singleton or per-workspace SSH-tunneled — same client
            // we just spawned through).
            runtime.daemon_client = Some(client_for_runtime);
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
    let read_app = app.clone();
    let read_client = client.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(chunk) = rx.recv().await {
            queue_or_send_output(&read_sessions, &read_session_id, chunk);
        }
        eprintln!(
            "[codemux::terminal::daemon_backed] read loop ended for session {read_session_id}"
        );
        // Only emit Exited if WE'RE still the runtime's daemon client.
        // Otherwise this is a stale read task whose session was already
        // replaced by a fresh spawn — emitting now would clobber the
        // new spawn's Ready and leave a phantom "ended" overlay.
        super::emit_exited_if_client_owner(
            &read_app,
            &read_sessions,
            &read_session_id,
            &read_client,
            "Agent ended",
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
    let entry_ts = std::time::Instant::now();
    eprintln!(
        "[trace:{session_id}] spawn_via_daemon ENTRY t=0ms"
    );
    let terminal_state: State<'_, PtyState> = app.state();
    let app_state: State<'_, AppStateStore> = app.state();
    let sessions = terminal_state.sessions.clone();

    if !super::try_reserve_session_spawn(&sessions, &session_id) {
        eprintln!(
            "[trace:{session_id}] try_reserve FAILED at t={}ms",
            entry_ts.elapsed().as_millis()
        );
        return Err("session already reserved by another spawn".into());
    }

    // Resolve the workspace + its host assignment BEFORE picking a
    // daemon client. host_id=None → local daemon (this device).
    // host_id=Some(...) → SSH-tunneled remote daemon. Either way
    // `client_for_workspace` returns the right one (caching for
    // perf so repeated spawns in the same workspace reuse the
    // connection).
    let snapshot = app_state.snapshot();
    let owning_ws = super::find_owning_workspace(&snapshot, &session_id);
    let workspace_id = owning_ws
        .map(|w| w.workspace_id.0.clone())
        .unwrap_or_default();
    let host_id = owning_ws.and_then(|w| w.host_id);
    let is_remote = host_id.is_some();

    // Shell choice depends on local vs remote:
    // - LOCAL: use `$SHELL` from the laptop (the user's preferred shell).
    // - REMOTE: use bare `bash` — `$SHELL` on the laptop is an absolute
    //   path to the laptop's shell binary (e.g. `/usr/bin/fish`) which
    //   almost certainly doesn't exist at that path on the remote host.
    //   Sending it as argv to the remote daemon makes the spawn fail
    //   immediately, the daemon closes the session, and the read loop
    //   ends without a single byte of output. Bare `bash` (resolved via
    //   the remote daemon's PATH) is on every Linux distro and macOS.
    let shell = if is_remote {
        "bash".to_string()
    } else {
        super::default_shell()
    };
    app_state.update_terminal_session_shell(&session_id, shell.clone());

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
        let computed = crate::ssh::conventional_remote_path(&project_name, &branch)
            .to_string_lossy()
            .to_string();
        eprintln!(
            "[codemux::terminal::daemon_backed] remote cwd for {session_id}: \
             {computed} (owning_ws={}, project_root={:?}, git_branch={:?}, \
             project_name={project_name:?}, branch={branch:?})",
            owning_ws.is_some(),
            owning_ws.and_then(|w| w.project_root.clone()),
            owning_ws.and_then(|w| w.git_branch.clone()),
        );
        computed
    } else {
        session_working_dir(&app_state, &session_id)
    };
    let mut auto_resume_command: Option<String> = None;
    let mut pane_id_for_env: Option<String> = None;

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
        let disk_meta = crate::scrollback::find_scrollback_meta_for_session(&session_id);
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
        let in_memory_original = snapshot
            .terminal_sessions
            .iter()
            .find(|s| s.session_id.0 == session_id)
            .and_then(|s| s.original_command.clone());

        if is_remote {
            // Remote: keep the conventional remote cwd; relaunch with
            // a CURATED subset of the original command's args, NOT
            // the full thing. The full command often carries laptop-
            // specific args like `--system-prompt "$CODEMUX_AGENT_CONTEXT"`
            // that the agent on the remote rejects (different version,
            // different env content). What we keep:
            //   - The binary name (first whitespace token)
            //   - `--dangerously-skip-permissions` if it was set, so
            //     remote claude doesn't block on approval prompts
            //     (matches the user's local preset intent)
            //   - `--resume <uuid>` if we captured a Claude session
            //     id locally — the JSONLs were rsynced by the push
            //     flow so this actually continues the conversation
            let full = in_memory_original
                .clone()
                .or_else(|| disk_meta.as_ref().and_then(|(_, _, m)| m.original_command.clone()));
            let agent_binary = full
                .as_deref()
                .and_then(|s| s.split_whitespace().next())
                .map(|t| t.to_string());
            // Detect --dangerously-skip-permissions in the original.
            // Restrict to claude only — this flag is Claude-specific
            // and other agents (opencode, codex, gemini) would either
            // ignore it or error out. Without the binary check we'd
            // forward a meaningless / hostile flag to those agents.
            let had_skip_perms = full
                .as_deref()
                .map(|s| s.contains("--dangerously-skip-permissions"))
                .unwrap_or(false)
                && agent_binary
                    .as_deref()
                    .map(|b| b == "claude")
                    .unwrap_or(false);
            // Look up the captured Claude session UUID (if any) from
            // the in-memory snapshot's adapter_captures.
            let claude_uuid = snapshot
                .terminal_sessions
                .iter()
                .find(|s| s.session_id.0 == session_id)
                .and_then(|s| s.adapter_captures.get("claude_session_id"))
                .cloned();
            let cmd_opt = agent_binary.map(|bin| {
                let mut parts = vec![bin];
                if had_skip_perms {
                    parts.push("--dangerously-skip-permissions".to_string());
                }
                if let Some(uuid) = claude_uuid.as_ref() {
                    parts.push("--resume".to_string());
                    parts.push(uuid.clone());
                }
                parts.join(" ")
            });
            if let Some(cmd) = cmd_opt {
                eprintln!(
                    "[codemux::terminal::daemon_backed] remote relaunch for {session_id}: \
                     {cmd} (skip_perms={had_skip_perms}, has_uuid={}; \
                     in_memory={}, disk_meta={})",
                    claude_uuid.is_some(),
                    in_memory_original.is_some(),
                    disk_meta.is_some(),
                );
                auto_resume_command = Some(cmd);
            } else {
                eprintln!(
                    "[codemux::terminal::daemon_backed] remote respawn for {session_id} \
                     has no original_command (was a plain shell, or preset wasn't yet \
                     applied) — spawning bare bash"
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
            let full = in_memory_original
                .clone()
                .or_else(|| disk_meta.as_ref().and_then(|(_, _, m)| m.original_command.clone()));
            let agent_binary = full
                .as_deref()
                .and_then(|s| s.split_whitespace().next())
                .map(|t| t.to_string());
            let had_skip_perms = full
                .as_deref()
                .map(|s| s.contains("--dangerously-skip-permissions"))
                .unwrap_or(false);
            let claude_uuid = snapshot
                .terminal_sessions
                .iter()
                .find(|s| s.session_id.0 == session_id)
                .and_then(|s| s.adapter_captures.get("claude_session_id"))
                .cloned();

            // Prefer the existing scrollback+adapter pipeline when
            // BOTH disk_meta and adapter_state are available — it
            // handles all the per-adapter quirks the bare-binary
            // path doesn't. Otherwise synthesize like the remote
            // branch does.
            if let (Some(adapter_state), Some((ws_id, pane_id, meta))) = (
                app.try_state::<crate::session_adapters::AdapterState>(),
                disk_meta.as_ref(),
            ) {
                effective_cwd = super::resolve_session_cwd(
                    &meta.working_directory,
                    &effective_cwd,
                );
                if let Some(resume_command) = super::resolve_resume_command(
                    &snapshot,
                    meta,
                    &adapter_state,
                ) {
                    eprintln!(
                        "[codemux::terminal::daemon_backed] local restore via \
                         disk_meta+adapter for {session_id} at {ws_id}/{pane_id}"
                    );
                    auto_resume_command = Some(resume_command);
                }
            } else if let Some(bin) = agent_binary {
                // No disk_meta (pull-back, fresh-after-preset, etc.)
                // — synthesize from in-memory exactly like the remote
                // path. This is what makes pull-back actually relaunch
                // Claude with the just-synced conversation history.
                let mut parts = vec![bin];
                if had_skip_perms {
                    parts.push("--dangerously-skip-permissions".to_string());
                }
                if let Some(uuid) = claude_uuid.as_ref() {
                    parts.push("--resume".to_string());
                    parts.push(uuid.clone());
                }
                let cmd = parts.join(" ");
                eprintln!(
                    "[codemux::terminal::daemon_backed] local relaunch via in-memory for \
                     {session_id}: {cmd} (skip_perms={had_skip_perms}, has_uuid={})",
                    claude_uuid.is_some()
                );
                auto_resume_command = Some(cmd);
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
    let list_result = client.list().await;
    let list_snapshot = list_result.as_ref().ok().map(|v| {
        v.iter()
            .map(|s| format!("{}@pid{}", s.session_id, s.pid))
            .collect::<Vec<_>>()
            .join(",")
    });
    eprintln!(
        "[trace:{session_id}] daemon.list() at t={}ms returned: [{}]",
        entry_ts.elapsed().as_millis(),
        list_snapshot.unwrap_or_else(|| "ERR".to_string())
    );
    let existing = list_result
        .ok()
        .and_then(|list| list.into_iter().find(|s| s.session_id == session_id));

    let reattached;
    let pid = if let Some(existing) = existing {
        reattached = true;
        eprintln!(
            "[trace:{session_id}] DECISION=reattach pid={} at t={}ms",
            existing.pid,
            entry_ts.elapsed().as_millis()
        );
        existing.pid
    } else {
        reattached = false;
        eprintln!(
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
    // If we reattached to an existing daemon session, the agent (or
    // bash) is ALREADY running there. We must NOT auto-write the
    // preset/resume command — that would type the command as a chat
    // message into the running agent (the "claude" appearing as a
    // message bug). Only write on fresh_spawn where the new bash
    // genuinely needs the agent launched.
    let auto_resume_clone = if reattached {
        eprintln!(
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

    // Preflight: for remote workspaces, verify the agent binary
    // we're about to write actually exists on the remote host. If
    // it doesn't, emit a Failed lifecycle event with an actionable
    // install message INSTEAD of writing the command into bash and
    // letting the user see a confusing "bash: claude: command not
    // found" inline. Only runs for remote + fresh-spawn (not
    // reattach — if we're reattaching, the agent's already running).
    if is_remote && !reattached {
        if let Some(ref command) = auto_resume_clone {
            let binary = command
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_string();
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
                            let result = String::from_utf8_lossy(&out.stdout)
                                .trim()
                                .to_string();
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
        eprintln!(
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
    is_remote: bool,
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
    //
    // Skip for remote workspaces — the shim dir lives in the laptop's
    // filesystem and the inherited PATH would point at /home/zeus/...
    // paths that don't exist on the remote. Remote agents use the
    // remote shell's own default PATH.
    if !is_remote {
        if let Some((shim_dir, current_exe)) = super::ensure_openflow_cli_shims() {
            let current_path = std::env::var("PATH").unwrap_or_default();
            let prefixed_path = super::build_child_path(&shim_dir, &current_path);
            env.push(("PATH".to_string(), prefixed_path));
            env.push(("CODEMUX_CLI_SAFE_PATH".to_string(), current_exe));
        }
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
