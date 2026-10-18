# Environments: every workspace visible everywhere

## Problem

A workspace exists only in the Codemux backend that created it. A workspace created on ai-node, from ai-node's own window or from a phone driving it, never appears on the laptop. The laptop's view of ai-node comes from a different program: the `codemux-remote` helper it reaches over SSH, which has its own workspace registry (`hosts_inventory.rs`). The cloud mirror (`workspaces_sync.rs`) publishes desktop workspaces without a device, so other machines show them in an "Another device" card that can only be pulled, not opened. Running chats and terminals are never visible across machines.

## Model

The model follows t3code (`~/Projects/t3code/docs/internals/remote.md`, `connection-runtime.md`).

- **Environment.** One running Codemux backend: the desktop app or headless `codemux serve`. It is the only owner of its workspaces, panes, terminals and chat threads. Nothing is copied between environments.
- **Identity.** An environment is identified by its stable `device_id` (`web_remote::registration::stable_device_id`), not by its address. Its descriptor is `{id, name, platform, app_version}`.
- **Catalog.** The account device registry (`GET /api/devices`) lists every environment that shares itself. Each client, whether desktop or browser, keeps its own catalog. A desktop is also its own *primary* environment.
- **Connection.** A client opens one live connection per enabled environment. The connection uses the existing `/ws` frame protocol (`invoke`, `listen`, `event`, `chan` frames, binary PTY frames) over iroh, authenticated by a short-lived account grant (`POST /api/devices/:id/connect`). This is the path hosted browsers already use. A desktop dials with a native iroh client, which can hole-punch directly instead of relaying. A browser dials with the existing wasm client.
- **State.** Each connection subscribes to its environment's `app-state-changed` and keeps a per-environment snapshot. The last snapshot is cached, so an offline environment still lists its workspaces, marked offline.
- **Scoped references.** Every workspace, pane and thread the UI handles carries its `environmentId`. Command calls are routed to the owning environment's connection. Local commands keep using Tauri IPC, so the primary environment pays nothing.
- **Sharing is opt-in per machine.** "Share this machine with my devices" turns on account relay mode, which registers the device and accepts account connections. The existing approval flow decides whether a new device must be approved first.

## Stack

Each step ships as its own PR on top of the previous one.

1. **Foundation (backend).** `environments` module:
   - `environment_info` returns this backend's descriptor.
   - `environments_list` returns the account catalog, marking which entry is this machine.
   - `environment_dial` / `environment_write` / `environment_close` expose a native iroh byte stream to the frontend over a Tauri channel. Minting the grant stays in Rust, so the account token never reaches the webview.
2. **Connection runtime (frontend).** An `src/environments/` registry and a supervisor per environment:
   - `RemoteTransport` over the existing `IrohSocket`, with a Tauri-backed dialer on desktop and the wasm dialer in the browser.
   - Reconnect with backoff.
   - Per-environment app state and a cached snapshot.
3. **Merged sidebar.** Workspaces from every environment appear grouped by machine, with live status (working / needs you / done) and offline rendering. A Settings toggle controls sharing.
4. **Scoped commands.** `EnvironmentScope` context, plus routing that sends workspace-, pane- and thread-scoped commands and event listeners to the owning environment. Opening a remote workspace renders its agent chat and terminals live.
5. **Remaining surfaces.** Files, editor, git, changes, browser pane and review for remote workspaces.
6. **Retire the parallel paths.** Once environments cover their use cases, remove the "Another device" orphan cards, attach-in-place SSH workspaces and the workspace cloud mirror. SSH hosts become environments by running `codemux serve` instead of `codemux-remote`, so a host and a desktop are the same kind of thing.

## Non-goals

- No shared database and no replication: each environment remains the only owner of its data.
- No change to the account API. Listing devices and minting grants already exist.
