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

```jsonc
{
  "persistent_agents": {
    "enabled": false   // off by default; flip to true to opt in
  }
}
```

The setting only gates whether **new** sessions go through the daemon. Once a daemon is running, sessions it owns are always reattached on launch regardless of the setting — otherwise toggling the setting off would silently lose live agents.

There is no UI for this yet. Users opt in by editing `~/.local/share/codemux[-dev]/settings-cache.json` directly. Adding a Settings → Sessions toggle is a follow-up.

## What Works Today

- Shells survive Codemux app close (verified end-to-end via `npm run tauri:dev`).
- Agent processes inside those shells survive (they're children of the daemon-owned shell — kernel never sends SIGHUP because the daemon still holds the master fd).
- Fresh Codemux launch adopts the running daemon and reattaches to live sessions.
- Pane-close from the UI properly tears the agent down via the daemon (no leaked PTYs).
- Cross-platform compile (Unix path validated; Windows compiles but named-pipe + `DETACHED_PROCESS` paths haven't been exercised on a real Windows box yet).
- Integration tests (`src-tauri/tests/pty_daemon_persistence.rs`) cover the headline invariant — a child spawned through the daemon must outlive the client that spawned it.

## Current Constraints (Follow-ups)

- **Windows path is scaffolded but unvalidated.** The supervisor uses `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP` and `server.rs::run` is `#[cfg(unix)]`-gated on the listener; a Windows port (named pipes + tokio's `windows::named_pipe`) is the obvious next step.
- **No fd-handoff during daemon upgrades.** Bumping the daemon version means the user has to manually shut down the running daemon and reopen the app, which loses sessions. The superset pattern of passing PTY master fds via SCM_RIGHTS during upgrade is tracked but not implemented.
- **No crash circuit breaker.** A broken daemon will respawn indefinitely; we should cap to ~3 failures in 60 seconds like superset does.
- **Session adapter system (scrollback restore, Claude `--continue`)** is not wired for daemon-backed sessions. They get a clean slate on reattach (the daemon's per-session replay buffer feeds recent bytes back). Combining adapter resume with daemon reattach is a follow-up.
- **No comm-log piping for daemon-backed OpenFlow agents.** The in-process spawn path tees PTY output to the comm log; the daemon path skips this. OpenFlow agents should opt out of persistent mode until the comm log is wired (or just `enabled: false`).
- **Resize on daemon-backed sessions is wired but underused.** The existing `resize_pty` Tauri command flows through `runtime.master.resize`, which is `None` for persistent sessions. A dedicated daemon-side resize call exists in the protocol but isn't yet routed from the resize command.
- **Settings UI is missing.** Editing JSON is hostile. A toggle in the Settings panel is one short PR.
- **Daemon's child-exit detection is best-effort.** The read thread sees EOF and removes the session, but it can't reap the child or report a real exit code (it doesn't own the `Child` handle). The `Exited` event ships `exit_code: -1` until we wire a proper waiter.

## Important Touch Points

- `src-tauri/src/pty_daemon/protocol.rs` — wire types, `PROTOCOL_VERSION`.
- `src-tauri/src/pty_daemon/server.rs` — daemon main loop, per-session output broadcast, replay buffer.
- `src-tauri/src/pty_daemon/client.rs` — Tauri-side socket client; demuxes responses + events.
- `src-tauri/src/pty_daemon/manifest.rs` — `pty-daemon-manifest.json` read/write/atomic-replace.
- `src-tauri/src/pty_daemon/supervisor.rs` — `ensure_daemon`, adoption, spawn-detached.
- `src-tauri/src/terminal/mod.rs` — `spawn_pty_for_session` / `spawn_pty_for_agent` routing, `persistent_agents_enabled`, persistent-aware `terminate_pty_session` + `Drop for SessionRuntime`.
- `src-tauri/src/terminal/daemon_backed.rs` — the daemon-backed spawn implementations, `DaemonWriter`.
- `src-tauri/src/settings_sync.rs` — `PersistentAgentsSettings`.
- `src-tauri/src/cli.rs` — `CommandSet::PtyDaemon { socket }` subcommand wiring.
- `src-tauri/src/lib.rs` — startup adoption warmup.
- `src-tauri/tests/pty_daemon_persistence.rs` — survival + reattach integration tests.

## Troubleshooting

**Agent died with the app despite the setting being on:**
- Check `~/.local/share/codemux[-dev]/settings-cache.json` — the dev frontend currently rewrites the cache on every sync, sometimes resetting `persistent_agents.enabled` back to `false`. Set it back to `true` and restart the app. (Settings UI work will fix this.)
- Look for `[codemux::pty_daemon] startup adoption` and `[codemux::terminal::daemon_backed]` lines in the app's stderr. Absence means the spawn took the in-process path.

**Reattach didn't pick up old session:**
- Verify the daemon is still alive: `ps -p $(jq .pid ~/.local/share/codemux[-dev]/pty-daemon-manifest.json)`.
- Check the daemon's session list: connect to the socket with `nc -U ~/.local/share/codemux[-dev]/ptyd.sock` and send `{"type":"list","request_id":1}\n`.
- Stale manifests are handled by the `kill(pid, 0)` check in `supervisor::try_adopt`. If a manifest points to a dead PID, the supervisor logs and ignores it.

**How to fully reset:**
- Kill the daemon: `pkill -f "codemux pty-daemon"`.
- Remove the manifest + socket: `rm -f ~/.local/share/codemux[-dev]/{pty-daemon-manifest.json,ptyd.sock}`.
- Toggle `persistent_agents.enabled` to `false` in the settings cache.
