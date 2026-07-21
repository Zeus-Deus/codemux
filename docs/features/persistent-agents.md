# Persistent Agents

- Purpose: Describe how shells (and the agents running inside them) keep running after the Codemux app is closed, and how a fresh launch reattaches.
- Audience: Anyone touching the PTY daemon, the spawn path, terminal lifecycle, or troubleshooting agents that died unexpectedly.
- Authority: Canonical feature doc for the persistent PTY daemon (step 1 of "cloud push").
- Update when: Daemon protocol, spawn routing, settings shape, or close/reopen behavior changes.
- Read next: `docs/features/terminal.md`, `docs/features/session-persistence.md`.

## What This Feature Is

When the user opts in via Settings → Persistent Agents, every shell Codemux spawns runs inside a long-lived subprocess called `codemux pty-daemon` instead of as a direct child of the Tauri app. Closing the app no longer kills the agent: the daemon outlives the app and the next launch adopts it, reattaches to live sessions, and the user picks up where they left off.

This is **step 1** of the wider "push workspace to cloud" feature: it solves "agents survive the local app being closed." The same daemon model is the foundation for steps 2 and 3 (push to BYO host over SSH, push to managed cloud host) — those layers will replace the local socket with a relay.

## Architecture

```
                ┌─────────────────────────────┐
                │ Tauri app (codemux)         │  closed by user → process dies
                │                             │  reopened       → adopts daemon
                │  ┌──────────────────────┐   │
                │  │ pty_daemon::client   │◀──┼── Unix socket / named pipe
                │  └──────────────────────┘   │     (JSON-lines protocol)
                └─────────────────────────────┘
                                                 ▲
                                                 │
                                                 ▼
                ┌─────────────────────────────┐
                │ codemux pty-daemon (detached subprocess)
                │  - holds master PTY fds
                │  - per-session broadcast channel + replay buffer
                │  - writes manifest with {pid, socket_path, version}
                │  ┌──────────────────────┐   │
                │  │ bash / zsh shells    │ ◀─┼── agents (claude, codex, ...)
                │  │ (children of daemon) │   │     run inside the shell as usual
                │  └──────────────────────┘   │
                └─────────────────────────────┘
```

### Spawn path

`terminal::spawn_pty_for_session` (and `spawn_pty_for_agent`) check at entry:

1. If `settings.persistent_agents.enabled` is true → route through the daemon.
2. Else if a live daemon manifest exists (i.e. the user *was* opted in and the daemon is still running) → route through the daemon anyway, so reattach works even after they toggled the setting off.
3. Else → original in-process `portable_pty::openpty` path (same behavior as before this feature).

On the daemon path, `daemon_backed::spawn_pty_for_session_via_daemon` (or `_for_agent_via_daemon`) does:

- `ensure_daemon()` — adopt the running daemon or spawn one detached (`setsid` on Unix, `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP` on Windows).
- `client.list()` — if the daemon already knows this `session_id`, skip spawn and reuse the existing pid (this is the reattach mechanism).
- Otherwise `client.spawn(...)` — daemon spawns the child, retains the master fd.
- `client.attach(session_id)` — get an mpsc receiver; spawn a tokio task that drains it into the existing `queue_or_send_output` so xterm sees bytes the same way it does for in-process PTYs.
- Build a `DaemonWriter` (impl `std::io::Write`) that funnels keystrokes into `client.write(...)` on a fire-and-forget tokio task. Slots into `SessionRuntime::writer` exactly like the in-process boxed writer.
- Mark `SessionRuntime::persistent = true` so the close paths know not to kill the pid.

### Close path

- **Pane close (user clicks X):** `terminate_pty_session` checks `persistent` first. For persistent sessions it dispatches `client.close(session_id)` to the daemon over the socket instead of `killpg(pid, SIGKILL)` — because we don't own the process group, the daemon does.
- **Window close (user closes the app):** the close handler serializes scrollback as before and exits. `Drop for SessionRuntime` checks `persistent` first and *returns early without killing* for persistent sessions. The PTYs keep running inside the daemon.

### Adoption

On Tauri startup, `pty_daemon::supervisor::ensure_daemon` is called when either the setting is on OR a manifest is present:

1. Read `~/.local/share/codemux[-dev]/pty-daemon-manifest.json`.
2. Check `kill(pid, 0)` — if the pid is dead, ignore the manifest.
3. Connect to `manifest.socket_path` and send `Hello`. Verify `protocol_version == PROTOCOL_VERSION`.
4. If everything checks out → adopt. If anything fails → spawn a fresh daemon detached, write a new manifest, connect.

The first call caches the connected client in a `OnceCell`; every subsequent `ensure_daemon()` returns the same `Arc<PtyDaemonClient>`.

### Wire protocol

JSON-lines over a stream socket. One message per line, base64-encoded payloads for PTY data so line framing is binary-safe. Two channels are multiplexed:

- **Request/response** correlated by `request_id`: `Hello`, `Spawn`, `Attach`, `Detach`, `Write`, `Resize`, `Close`, `List`, `Shutdown`.
- **Push events** from daemon to client: `Output { session_id, data_b64 }`, `Exited { session_id, exit_code }`.

`Frame::Response` and `Frame::Event` are the two top-level wire variants. Both define their own `type` discriminator so a `nc`-style debugging session reads naturally.

Defined in `src-tauri/src/pty_daemon/protocol.rs`. Bump `PROTOCOL_VERSION` for any incompatible shape change — adoption refuses to adopt a daemon at a different protocol version.

## Settings

**There is no setting.** Persistent agents are the default behavior of the app — every PTY spawn goes through the daemon, full stop. This is intentional: agents not dying when the app closes is a strict UX upgrade, and the upcoming cloud-push feature builds on the same mechanism.

The only escape hatch is the env var **`CODEMUX_DISABLE_PTY_DAEMON=1`**, which forces the in-process path. Treat it as a panic button for the field if a regression ever ships; normal users never need it.

## Graceful Fallback

The daemon path is **always safe**. Every error route falls back to the in-process PTY path so the user always gets a working terminal:

| Failure | Behavior |
|---|---|
| Daemon binary missing or can't spawn | log + in-process fallback |
| Socket race / connect timeout | log + in-process fallback |
| Protocol version mismatch on adoption | log + spawn fresh daemon, fall back if that fails |
| Windows (named-pipe IPC not wired yet) | in-process, every time, no daemon code touched |
| `CODEMUX_DISABLE_PTY_DAEMON=1` | in-process, no daemon code touched |
| **Crash circuit open** (3 daemon failures within 60 s) | fast-fail + in-process for rest of process lifetime |

The crash circuit prevents a broken daemon from turning into a tight respawn loop. Tracked by `pty_daemon::supervisor::{circuit_is_open, total_failures, reset_circuit}`. Resets only on app restart (intentional — recurring failures are an environment problem, not a transient hiccup).

## What Works Today

- **Default behavior:** no setting, no opt-in. Every shell goes through the daemon automatically.
- Shells + agents inside them survive Codemux app close (verified end-to-end via `npm run tauri:dev`).
- Fresh Codemux launch adopts the running daemon and reattaches to live sessions (`[codemux::terminal::daemon_backed] reattaching to live shell session ...`).
- Agent status hooks rebind across that app restart: notifier scripts try the PTY's inherited hook-server port first, then retry the current port published in `~/.codemux/hooks/active-port`, so resumed Codex/Claude/Gemini/OpenCode/Pi work updates the sidebar instead of silently targeting the dead prior app process.
- Pane-close from the UI properly tears the agent down via the daemon (no leaked PTYs).
- Session-adapter resume wired for daemon-backed sessions: reopening a Claude pane auto-types `--resume <session_id>` (or `--continue` fallback) just like the in-process path.
- Scrollback restoration: daemon-backed sessions use the same `~/.local/share/codemux[-dev]/scrollback/` cache.
- **Real exit codes** via a per-session waiter thread (no more `-1` sentinel).
- **Resize** for daemon-backed sessions routes through `client.resize` over the socket.
- **Graceful fallback at every error site** — daemon failure never breaks the terminal.
- **Crash circuit breaker** caps daemon respawn attempts.
- **Idle reaper (`v0.7.9`)**: a daemon with zero live sessions continuously for **1 hour** (checked every 60 s) removes its manifest + socket and exits, so an abandoned daemon doesn't linger and stale manifests don't confuse the next adoption. Hard guard: it re-checks the session count under the lock immediately before exit, so it can **never** reap a live session; any spawn/attach resets the idle clock by making `sessions` non-empty on the next check. Spawned from `pty_daemon::server::run` (`IDLE_REAP = 3600s`, `CHECK_INTERVAL = 60s`).
- **Comm-log tee on the daemon-backed agent path (`v0.7.9`)**: the daemon-backed reader task now tees each cleaned PTY chunk to the OpenFlow communication log, matching the in-process reader — so OpenFlow agents spawned through the daemon (the default since persistent agents) populate the comm log the orchestrator's stuck-detection reads. Both reader paths share the `terminal::comm_log_entry_for_chunk` helper. See `docs/features/openflow.md`.
- **Late-attacher exit signal**: clients that attach after a child has already exited receive an immediate `Exited` event instead of hanging.
- Integration tests (`src-tauri/tests/pty_daemon_persistence.rs` + `pty_daemon_circuit_breaker.rs`): handshake, list, child survives client disconnect, second client sees session, exit code 0 / non-zero reporting, resize round-trip, write-to-unknown error shape, circuit-breaker trip + reset.

## Current Constraints (Follow-ups)

- **Windows path is scaffolded but not wired.** The supervisor + server are `#[cfg(unix)]`-gated; on Windows the daemon path is disabled entirely and the in-process path is used (zero regression). A Windows port needs tokio's `windows::named_pipe` for the IPC and `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP` creation flags (already in `spawn_daemon_detached`'s cfg-gated branch).
- **No fd-handoff during daemon upgrades.** Bumping the daemon version means the user has to manually shut down the running daemon and reopen the app to use the new protocol; live sessions are lost. The superset pattern of passing PTY master fds via SCM_RIGHTS during upgrade is the next step.

## Important Touch Points

- `src-tauri/src/pty_daemon/protocol.rs` — wire types, `PROTOCOL_VERSION`.
- `src-tauri/src/pty_daemon/server.rs` — daemon main loop, per-session output broadcast, replay buffer, idle reaper (`v0.7.9`).
- `src-tauri/src/pty_daemon/client.rs` — Tauri-side socket client; demuxes responses + events.
- `src-tauri/src/pty_daemon/manifest.rs` — `pty-daemon-manifest.json` read/write/atomic-replace.
- `src-tauri/src/pty_daemon/supervisor.rs` — `ensure_daemon`, adoption, spawn-detached.
- `src-tauri/src/terminal/mod.rs` — `spawn_pty_for_session` / `spawn_pty_for_agent` routing, `daemon_path_viable`, persistent-aware `terminate_pty_session` + `Drop for SessionRuntime`, `resize_pty` routing.
- `src-tauri/src/terminal/daemon_backed.rs` — the daemon-backed spawn implementations, `DaemonWriter`, scrollback + adapter resume wiring, the comm-log tee in the reader task (`v0.7.9`).
- `src-tauri/src/terminal/mod.rs` — `comm_log_entry_for_chunk` shared helper used by both the in-process and daemon-backed reader paths.
- `src-tauri/src/cli.rs` — `CommandSet::PtyDaemon { socket }` subcommand wiring.
- `src-tauri/src/lib.rs` — startup adoption warmup (Unix-only).
- `src-tauri/tests/pty_daemon_persistence.rs` — survival, reattach, exit code, resize, error-handling integration tests.
- `src-tauri/tests/pty_daemon_circuit_breaker.rs` — circuit breaker unit tests.

## Troubleshooting

**Agent died with the app close:**
- Look for `[codemux::pty_daemon] startup adoption ok` and `[codemux::terminal::daemon_backed]` lines in the app's stderr. Absence means the spawn took the in-process fallback path — check the preceding log line for the reason (circuit open, daemon binary missing, socket bind failed).
- If you see `circuit OPEN: N ensure_daemon failures within 60s` — the breaker tripped. Restart the app to reset.

**Reattach didn't pick up old session:**
- Verify the daemon is still alive: `ps -p $(jq .pid ~/.local/share/codemux[-dev]/pty-daemon-manifest.json)`.
- Check the daemon's session list: connect to the socket with `socat - UNIX-CONNECT:~/.local/share/codemux[-dev]/ptyd.sock` and send `{"type":"list","request_id":1}\n`.
- Stale manifests are handled by the `kill(pid, 0)` check in `supervisor::try_adopt`. If a manifest points to a dead PID, the supervisor logs and ignores it.

**Need to disable persistent mode entirely (panic button):**
- Set `CODEMUX_DISABLE_PTY_DAEMON=1` in the environment before launching Codemux. Every PTY spawn will go through the in-process path; the daemon is never touched. This is the rollback path if a regression ever ships.

**How to fully reset state:**
- Kill the daemon: `pkill -f "codemux pty-daemon"`.
- Remove the manifest + socket: `rm -f ~/.local/share/codemux[-dev]/{pty-daemon-manifest.json,ptyd.sock}`.
- Next app launch spawns a fresh daemon automatically.
