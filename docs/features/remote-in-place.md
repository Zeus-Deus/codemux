# Operate a Remote Workspace In Place ("Open on host")

- Purpose: Describe how the desktop opens and drives a **host-backed workspace directly on its host** — terminal/agent running on the host, output streaming live — **without rsync-copying the files down to this device**, and how that survives app close.
- Audience: Anyone touching the Workspaces overview actions, the daemon-backed terminal path, the SSH tunnel, or the host-inventory/sync model.
- Authority: Canonical feature doc for the "attach in place" remote-operation capability (issue #64).
- Read next: `docs/features/remote-hosts.md`, `docs/features/workspaces-sync.md`, `docs/features/persistent-agents.md`.

## What This Feature Is

The third remote-workspace mode, alongside the two that already existed:

1. **Push** a local workspace to a host (rsync up, then drive it over the tunnel) — `docs/features/remote-hosts.md`.
2. **Pull** a host/sibling workspace down to this device (rsync down, work locally) — `docs/features/workspaces-sync.md`.
3. **Open on host (this feature)** — drive a host workspace **in place**: a terminal/agent runs on the host in the workspace's real on-host directory, output streams live to the desktop, and **nothing lands under `~/.codemux/` locally**. Closing the app (or the laptop lid) leaves the host process running; reopening re-attaches.

This is the "Superset-style remote operation" capability: use a remote machine's workspace from the desktop without ever materialising a local copy.

## Current Model

### The attach-in-place workspace

`AppStateStore::create_remote_attach_workspace` (in `src-tauri/src/state/state_impl.rs`) makes a normal local `WorkspaceSnapshot` with three things that mark it as in-place:

- `host_id: Some(..)` — routes every terminal spawn through the host's SSH-tunneled pty-daemon (the existing daemon-backed path), exactly like a pushed workspace.
- `remote_cwd: Some(<host path>)` — the workspace's **real** directory on the host. The daemon-backed terminal path (`terminal/daemon_backed.rs::remote_spawn_cwd`) prefers this over the reconstructed `conventional_remote_path`, so it works even for workspaces an agent created at an arbitrary path.
- `attach_only: true` — "operated in place, no local files." Gates every filesystem-bound code path off (git enrichment, push, pull-back, worktree deletion); the only teardown is a detach.

There is **no rsync** and nothing is written under `~/.codemux/` on this device. The workspace has a single ready-to-use terminal pane; opening it spawns a shell on the host.

### The command

`workspace_open_on_host(sync_row_id)` (`src-tauri/src/commands/workspaces_sync.rs`):

1. Looks up the host-backed `workspaces_sync` row by its local id.
2. Requires `host_server_id` and resolves it to a **locally-configured** host (so the SSH tunnel can reach it) — otherwise it returns `host_not_configured`.
3. Resolves the on-host directory via `resolve_open_on_host_cwd`: the daemon-reported `origin_path` (authoritative; set by the inventory poller) with the `project_uid`-keyed conventional path as a fallback.
4. Idempotently re-activates an existing in-place view of the same host+path, else creates one and activates it.

It deliberately does **not** link the sync row to the new local workspace, and `reconcile_from_snapshot` **skips** `attach_only` workspaces — so opening in place never creates a duplicate cloud row, and closing it never soft-deletes the host's published row.

### Persistence (survive app close → reattach)

The host pty-daemon must outlive the SSH connection for "close the app, the agent keeps working" to hold. `ssh::tunnel::build_remote_command` makes the remote command:

- **Reuse** a still-running daemon: if `<socket>.pid` names a live process and the socket exists, just hold the `-L` forward open (`exec sleep`). On the desktop, `client.list()` then finds the surviving sessions and reattaches.
- Else **spawn the daemon detached** — `setsid` (Linux) with a `nohup` fallback (macOS), stdio redirected — so an SSH channel close can't SIGHUP it, then hold the forward.

The pty-daemon writes/removes `<socket>.pid` on bind / clean exit (`pty_daemon/server.rs`). Its existing 1-hour idle-reaper cleans up an abandoned detached daemon. This persistence is universal to all SSH-tunneled daemons (push and open-on-host), and is a strict improvement for push too (a WiFi flap no longer loses host sessions).

### UI

The Workspaces overview's host-backed sibling row (`workspace-overview-row.tsx` → `RemoteRow`) gains an **"Open on host"** action, enabled when the row's `host_server_id` resolves to a host configured on this device. The created in-place workspace renders a sky **"on host"** badge and offers only **"Close (leave running on host)"** — never delete/push/pull. `use-overview-items.ts` dedupes: a sibling row already opened in place (same `host_server_id` + `origin_path`) is hidden so the overview shows one card, not two.

## What Works Today

- Open a host-backed workspace's terminal in the desktop with **no local copy** created.
- Commands run on the host; output streams live over the existing SSH-tunneled pty-daemon.
- App close → the host pty-daemon survives (detached) → reopen re-tunnels and `client.list()` reattaches the live sessions.
- Works from any device signed in that has the host configured (each attaches independently over its own tunnel).
- The action is gated to hosts configured locally; rows on a host this device can't reach surface the existing "Pull to this device" path instead.

## Current Constraints

- "Open on host" requires the workspace's host to be configured in Settings → Hosts on this device (we need an SSH path to it). Sibling-device-only workspaces (no host) still use Pull.
- Local-FS surfaces (file tree, Changes panel, ports) are empty for an attach-only workspace — its files live only on the host. The terminal/agent is the supported surface today.
- Mid-session tunnel reconnect (WiFi flap *while the app stays open*) reuses the surviving host daemon, but the cached `PtyDaemonClient` reconnect polish is still the open item tracked in `docs/features/remote-hosts.md` (the daemon now survives the flap, which is the prerequisite).
- `origin_path` is a local-only sync column populated by the host-inventory poller; a device that has never polled the host (and has no `origin_path`) falls back to the conventional keyed path.

## Important Touch Points

- `src-tauri/src/state/state_impl.rs` — `WorkspaceSnapshot.remote_cwd` + `attach_only`; `create_remote_attach_workspace`; `all_workspace_cwds`/`active_workspace_cwd` skip `attach_only`.
- `src-tauri/src/terminal/daemon_backed.rs` — `remote_spawn_cwd` (prefers `remote_cwd`), used by both the agent and shell spawn paths.
- `src-tauri/src/ssh/tunnel.rs` — `build_remote_command` (reuse-or-spawn-detached + hold forward), `build_tunnel_argv`.
- `src-tauri/src/pty_daemon/server.rs` — `pid_file_for` / `write_pid_file` / `remove_pid_file` (per-socket liveness pidfile).
- `src-tauri/src/commands/workspaces_sync.rs` — `workspace_open_on_host`, `resolve_open_on_host_cwd`, `OpenOnHostOutcome`; `WorkspaceSyncView.origin_path`.
- `src-tauri/src/workspaces_sync.rs` — `reconcile_from_snapshot` skips `attach_only`.
- `src/components/workspaces-overview/workspace-overview-row.tsx` — "Open on host" action + "on host" badge + detach-only close.
- `src/components/workspaces-overview/use-overview-items.ts` — sibling-row dedup against an open in-place view.
- `src/tauri/commands.ts` / `src/tauri/types.ts` — `workspaceOpenOnHost`, `OpenOnHostOutcome`, `WorkspaceSnapshot.remote_cwd`/`attach_only`, `WorkspaceSyncView.origin_path`.

## Notes

- Keep this file about current truth. The persistence change (detached daemon + pidfile reuse) is shared with the push flow — describe behavior changes there in `docs/features/remote-hosts.md`.
- Test coverage: `ssh::tunnel` (remote-command persistence markers), `pty_daemon::server` (pidfile path), `state_impl` (`create_remote_attach_workspace`), `terminal::daemon_backed` (`remote_spawn_cwd`), `commands::workspaces_sync` (`resolve_open_on_host_cwd`), `workspaces_sync` (reconcile skip), and the frontend `open-on-host-action.test.tsx` + `use-overview-items.test.ts` dedup + `sibling-device-row.test.tsx` badge.
