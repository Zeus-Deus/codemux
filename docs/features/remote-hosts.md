# Remote Hosts (cloud-push steps 2b–2d)

- Purpose: Describe the device-picker UI, the slim `codemux-remote` server binary, and the SSH transport stack that lets the laptop push workspaces to user-owned SSH hosts.
- Audience: Anyone touching the new-workspace dialog, the Settings → Hosts pane, the `codemux-remote` binary, or the SSH transport.
- Authority: Canonical feature doc for steps 2b/2c/2d of the cloud-push series. Builds on 2a (`persistent-agents.md`) + Step 1 (`persistent-agents.md`).
- Update when: The DevicePicker shape, the codemux-remote CLI, the SSH probe/bootstrap/tunnel protocol, or the workspace `host_id` model change.
- Read next: `docs/features/persistent-agents.md`, `docs/features/hosts.md` (when 2a's doc is split out).

## What These Steps Ship

| Step | Surface | What lands |
|---|---|---|
| **2b** | UI + data model | `host_id: Option<i64>` on `WorkspaceSnapshot`. Shared `<DevicePicker>` pill component. New-workspace dialog gains the picker in its bottom bar. `set_workspace_host` Tauri command. |
| **2c** | Binary | New `[[bin]] codemux-remote` target. Slim CLI with `version` + `pty-daemon --socket` subcommands. Reuses `codemux_lib::pty_daemon::server::run` — same wire protocol as the in-app daemon. |
| **2d** | SSH transport | `ssh::probe`, `ssh::bootstrap`, `ssh::tunnel` modules. Real `hosts_test_connection` (replaces the 2a stub). `hosts_bootstrap_install` Tauri command + consent modal in the Hosts pane. |

The "Push workspace to host" action that was originally deferred from 2d has since landed (`8c72b44`). The push flow rsyncs the worktree to the host, spawns the remote daemon, attaches the local UI through the SSH-forwarded socket, and synchronizes the **Claude conversation** across local/remote ends. New transport-side modules: `ssh::push`, `ssh::registry`, `ssh::tunnel_supervisor`.

## DevicePicker (2b)

`src/components/hosts/device-picker.tsx`. Compact pill matching superset-sh's shape:

```
┌─ [💻 Local Device ▾] ─┐    (selected: local)
└────────────────────────┘

┌─ [🖥 homelab • ▾] ────┐    (selected: remote, online dot)
└────────────────────────┘
```

Dropdown structure:
```
○ Local Device                    ✓
─────────────────
▸ Other Hosts                       (submenu)
    ● homelab
    ○ vps-fra (offline)
```

Rules:
- `hostId === null` means local. Local never gets an online dot ("tautologically online" — the app itself is the local host).
- Remote hosts get a dot. Until SSH probe is wired (i.e. until the user has clicked "Test connection" in Settings → Hosts), every remote shows as offline-style.
- The picker reads from `hostsList()` on mount. If listing fails (DB not initialized, auth issues), it falls back to local-only — **never throws**, because a crash would break the surrounding new-workspace dialog.

Where it's wired today:
- New-workspace dialog (leftmost in the bottom bar, ahead of the agent picker).

Wiring **deferred** to a follow-up (small UX work, no architectural risk):
- Chat new-session entry surface.
- Workspace header badge for non-local workspaces.
- Workspace list filter dropdown.

## `codemux-remote` binary

`src-tauri/src/bin/codemux_remote.rs`. `[[bin]]` target in `src-tauri/Cargo.toml`. Same `codemux_lib` crate, no UI deps.

CLI:
```
codemux-remote version
  → {"name":"codemux-remote","version":"<v>","protocol_version":1}

codemux-remote pty-daemon --socket /tmp/codemux-ptyd-<rand>.sock
  → binds the Unix socket the laptop's "Push workspace" tunnel uses,
    runs the PTY daemon server, never returns.

codemux-remote scheduler
  → runs the Automations reconcile + pull + tick + execute loop on an
    always-on host; never returns.

codemux-remote serve [--port <n>] [--state-dir <path>]
  → runs the headless Codemux daemon: axum HTTP server on 127.0.0.1
    with bearer-token auth (manifest at <state-dir>/manifest.json,
    mode 0600). Tracks workspaces in <state-dir>/codemux.db. Survives
    SIGTERM/SIGINT cleanly (manifest is removed on shutdown). v1
    listens loopback-only; the desktop tunnels in over SSH.

codemux-remote serve status [--state-dir <path>]
  → prints {endpoint, pid, started_at, host_id, alive} as JSON.
    Exit 0 if alive, exit 1 otherwise.

codemux-remote serve stop [--state-dir <path>]
  → SIGTERM the running daemon (pid read from manifest).

codemux-remote mcp [--state-dir <path>]
  → stdio JSON-RPC MCP server. Reads <state-dir>/manifest.json for the
    daemon's endpoint + secret, then bridges agent CLI tool calls to
    the daemon's HTTP API. Configure your CLI agent with:
    {"command": "codemux-remote", "args": ["mcp"]}
```

### `serve` mode overview

The `serve` subcommand is the headless equivalent of running the
desktop Codemux app on this host — an MCP-aware agent on this machine
can drive Codemux locally without any UI. Module layout:

- `src-tauri/src/remote/manifest.rs` — manifest read/write, pid liveness.
- `src-tauri/src/remote/auth.rs` — bearer-token axum middleware,
  attaches `Identity::Local` to the request.
- `src-tauri/src/remote/identity.rs` — `Identity::Local | Cloud{…}`.
  v1 only ever produces `Local`; the `Cloud` variant is here so a
  future optional relay layer can forward verified identity without
  changing handler signatures.
- `src-tauri/src/remote/workspace.rs` — SQLite workspace registry
  with nullable `owner_id` column for future relay use.
- `src-tauri/src/remote/pty.rs` — minimal portable-pty wrapper with
  per-terminal ring buffer.
- `src-tauri/src/remote/server.rs` — axum routes (`/health`,
  `/tools/list`, `/tools/call`).
- `src-tauri/src/remote/mcp.rs` — stdio MCP server.
- `src-tauri/src/remote/tools/mod.rs` — 11-tool catalog.

Headless tool surface (advertised via `tools/list`):

| Tool | Purpose |
|---|---|
| `workspace_create` | Register a new workspace. |
| `workspace_list` | All workspaces, newest first. |
| `workspace_info` | One workspace by id. |
| `workspace_update` | Mutate name/branch/notes. |
| `workspace_close` | Remove from registry. |
| `terminal_spawn` | Spawn a shell PTY. |
| `terminal_write` | Write bytes to PTY stdin. |
| `terminal_read` | Read accumulated PTY output. |
| `terminal_list` | List open PTYs. |
| `terminal_close` | SIGHUP the shell. |
| `app_status` | Daemon version, uptime, counts. |

Deliberately *not* in the headless surface: `pane_*`, `browser_*`
(no UI on a headless host).

### Process supervision

Sample systemd user unit at `scripts/codemux-remote.service.example`.
Install:

```
cp scripts/codemux-remote.service.example ~/.config/systemd/user/codemux-remote.service
loginctl enable-linger $USER
systemctl --user daemon-reload
systemctl --user enable --now codemux-remote.service
```

`Restart=on-failure`, `StandardError=journal`, `loginctl enable-linger`
so the daemon survives ssh logouts and reboots.

### Designing for a future optional cloud relay

`docs/plans/mcp-on-remote.md` details the four design choices baked
into v1 so that a paid-tier cloud relay (team collaboration, "control
from phone without SSH") can be added later as a purely additive
feature, not a rewrite: HTTP transport + bearer-token, `Identity`
passthrough, nullable `owner_id`, no Better-Auth coupling in the
daemon. None of those four costs anything today.

### Auto-provisioning on push (the "it just works" flow)

The desktop's push flow does the following automatically, so the user
never has to know about manifests, secrets, or systemd:

1. **Binary install.** `ssh::bootstrap::bootstrap_remote` uploads the
   matching `codemux-remote-<target>` to `~/.local/bin/codemux-remote`
   and chmods it. Upload uses a `ssh ... 'cat > <path>'` pipeline
   instead of `scp` because OpenSSH 9+ broke tilde-expansion in
   `scp` destination paths; piping through stdin sidesteps the issue
   entirely. Skipped if `version` already returns the right value.
2. **`serve` systemd unit.** `ssh::bootstrap::provision_serve` writes
   `~/.config/systemd/user/codemux-remote.service`, runs
   `loginctl enable-linger` so it survives logout, and
   `systemctl --user enable --now codemux-remote`. The daemon binds an
   ephemeral loopback port and writes its manifest under
   `~/.local/share/codemux-remote/`.
3. **`.mcp.json` in the pushed workspace.** `ssh::push::push_workspace`
   drops a `.mcp.json` into the rsynced workspace directory pointing
   `codemux` at `codemux-remote mcp`. Any CLI agent (Claude Code,
   Codex, Gemini) launched in that directory on the host auto-discovers
   Codemux as an MCP server with zero further config.
4. **Workspace registration.** After rsync,
   `ssh::bootstrap::register_workspace_on_remote` runs `codemux-remote
   workspace register --path ... --name ... --branch ...` on the host
   via SSH. That command talks to the local daemon over loopback HTTP
   and inserts the workspace into its registry, so it shows in
   `workspace_list` from any MCP-aware agent on the host.

Net effect: user clicks "Push workspace to host" once, gets back a
working MCP control plane on the remote without ever having to know
the words "manifest" or "systemd."

### User-level MCP auto-register (`remote/mcp_register.rs`)

Per-workspace `.mcp.json` only covers directories the user pushed.
For repos cloned directly on the host, ad-hoc scratch dirs, and
services running in arbitrary places, an agent CLI needs a
**user-level** (not per-workspace) MCP config that names
`codemux-remote` as a server. `codemux-remote serve` writes that
once on every startup, idempotently, into every supported agent
config it finds present on the host:

| Path | Format |
|---|---|
| `~/.claude.json` (top-level `mcpServers`) | Claude Code |
| `~/.codex/config.toml` (`[mcp_servers.codemux]`) | Codex |
| `~/.cursor/mcp.json` (top-level `mcpServers`) | Cursor |

Safety contract:

1. **Idempotent.** If the codemux entry is already present (same
   command + args), the file is left untouched.
2. **Atomic.** Writes go through a sibling `.tmp` file + rename, so
   a crash mid-write can never leave the user's agent config in a
   half-baked state.
3. **No corruption.** Unparseable files (broken JSON/TOML) are
   logged and skipped — never overwritten.
4. **Skip missing tools.** A user who doesn't have Claude Code
   won't have `~/.claude.json`; we don't create directories the
   user never opted into.

### Background host-upgrade poller (`hosts_upgrade.rs`)

Users don't think about "upgrading a helper binary on a remote
host." They think "I updated Codemux." So `hosts_upgrade::spawn` is
called once during app setup (~5 s after `setup` so the UI is
responsive first) and walks every registered SSH host:

1. `probe_host` → returns the host's installed `codemux-remote`
   version (or "not installed").
2. If that version differs from `env!("CARGO_PKG_VERSION")`,
   `bootstrap_remote` re-uploads the bundled binary (same code path
   the Install button uses) and `provision_serve` re-applies the
   systemd unit. `provision_serve` is idempotent so a host that's
   already up to date pays only one SSH version probe.
3. Best-effort by design — offline host, flaky tunnel, missing
   bundled-binary target — any of these log and move on; the task
   never fails the app.

Consent was implicitly granted the first time the user bootstrapped
the host; re-uploading a newer version of the same binary to the
same location is not a meaningful trust escalation.

### Controlling the daemon from your phone (Tailscale)

v1's transport is SSH-only — there is no cloud relay. The cleanest
way to drive your home/VPS Codemux daemon from a phone today is
**Tailscale**:

1. Install Tailscale on both the host running `codemux-remote serve`
   and your phone (Tailscale has iOS and Android apps).
2. Both devices join the same tailnet.
3. From any agent app on the phone that can SSH (or any web shell
   pointed at the host's tailnet name), `ssh <host>` works without
   any port forwarding or DDNS.

This costs nothing, works today, and is what we recommend until the
optional cloud relay (paid tier) ships. The relay would replace the
SSH-from-phone requirement, not the Tailscale-or-similar mesh — some
users will always prefer mesh VPN over a hosted relay for privacy.

The `scheduler` subcommand was added by the Automations feature, not by
2c. Host bootstrap provisions it — it writes the scheduler token + host
identity and registers a systemd user service so it survives reboots.
The scheduler loop, host routing, and service provisioning are owned by
`docs/features/automations.md`; this doc only covers the binary and the
SSH transport it shares.

Cross-compile targets (CI work, not in this commit — flagged for the release skill):
- `x86_64-unknown-linux-gnu` — most servers + home labs
- `aarch64-unknown-linux-gnu` — Raspberry Pi, AWS Graviton
- `x86_64-apple-darwin` — older Intel Macs
- `aarch64-apple-darwin` — Apple Silicon Macs

The four binaries are bundled into the laptop's Codemux app as `src-tauri/binaries/codemux-remote-<target>`. The bootstrap step picks the matching one based on the remote's `uname -sm` and scp's it.

## SSH transport (2d)

Three modules under `src-tauri/src/ssh/` (Unix-only — Windows gracefully skips):

### `probe.rs` — "is this host usable?"

`probe_host(opts)` shells out to `ssh -o BatchMode=yes -o ConnectTimeout=N`, runs a single combined command on the remote:
```sh
printf 'UNAME: ' ; uname -sm
if command -v codemux-remote >/dev/null 2>&1 ; then
  printf 'CMR: ' ; codemux-remote version
else
  printf 'CMR: NOT_INSTALLED\n'
fi
```

Parses the output into one of three outcomes:
- `Reachable { codemux_remote_version: Some(...), uname: Some(...) }` — green light.
- `Reachable { codemux_remote_version: None, uname: Some(...) }` — host is up, binary missing → triggers the bootstrap-install consent modal.
- `Unreachable { reason }` — SSH itself failed (DNS, refused, auth, timeout). `reason` is the SSH stderr so the user can debug.

Critical SSH flags (locked in via unit tests):
- `BatchMode=yes` — never prompt for a password (would hang the probe).
- `ConnectTimeout=N` — bound how long an unreachable host can stall us.
- `StrictHostKeyChecking=accept-new` — first-time hosts add to known_hosts; subsequent key changes still fail closed.

### `bootstrap.rs` — install `codemux-remote` on a fresh host

Runs after the user clicks "Install" in the consent modal. Four steps:

1. Map the probe's `uname -sm` to the matching target triple (e.g. `Linux x86_64` → `x86_64-unknown-linux-gnu`).
2. Find the bundled binary at `src-tauri/binaries/codemux-remote-<target>`. Returns `BinaryNotBundled` if the cross-compile step didn't run (dev builds without the release pipeline).
3. `ssh ... mkdir -p` the install dir → `scp` the binary to `~/.local/bin/codemux-remote` → `ssh ... chmod +x`.
4. Re-probe via `ssh ... codemux-remote version` to verify the install worked. Parse out the reported version.

Returns `BootstrapResult::Installed { reported_version }` on success; one of three failure variants otherwise, each with a specific error message the UI surfaces verbatim.

### `tunnel.rs` — SSH-tunneled daemon

`spawn_ssh_tunnel(opts, timeout)` spawns:
```
ssh -o BatchMode=yes \
    -o ServerAliveInterval=30 \
    -o ServerAliveCountMax=3 \
    -o ExitOnForwardFailure=yes \
    -o StreamLocalBindUnlink=yes \
    -L /tmp/local.sock:/tmp/codemux-ptyd-abc.sock \
    user@host \
    'rm -f /tmp/codemux-ptyd-abc.sock ; mkdir -p "$(dirname /tmp/codemux-ptyd-abc.sock)" ; exec codemux-remote pty-daemon --socket /tmp/codemux-ptyd-abc.sock'
```

Returns a `TunnelHandle` whose `local_socket()` is the path the existing `PtyDaemonClient::connect(&path)` dials. **Same client code, different socket path** — that's the whole point of building the daemon protocol as Unix-socket-only from the start.

Reconnect cadence is the caller's job for now: a push-then-detach vs. an interactive session want different reconnect policies, so we don't bake one into the handle.

## Settings → Hosts pane upgrade (in 2d)

The pane built in 2a now uses the real probe + bootstrap:

- **Test connection** → calls `hosts_test_connection`, surfaces the result inline.
- **Install button** appears when the probe reports `needs_install: true`. Opens a `window.confirm` modal that names the binary, says it's ~8MB and runs in the user's account (no root), and links to the source repo. On confirm → calls `hosts_bootstrap_install` and surfaces the result.

## Test coverage

- **DevicePicker** (`src/components/hosts/device-picker.test.tsx`): 7 tests — local label, custom local label, remote selection, fallback when configured hostId is missing, dropdown open with Local Device entry, Other Hosts submenu, graceful failure when `hostsList` rejects.
- **codemux-remote binary** (`src-tauri/tests/codemux_remote_binary.rs`): 3 tests — `version` subcommand prints valid JSON, no-subcommand defaults to version, end-to-end spawn-and-reap via `PtyDaemonClient` against the real binary.
- **SSH probe** (`src-tauri/src/ssh/probe.rs::tests`): 5 tests — argv construction (BatchMode + ConnectTimeout + StrictHostKeyChecking + target + command position), parsing reachable+installed, reachable+missing, unparseable version, empty payload.
- **SSH bootstrap** (`src-tauri/src/ssh/bootstrap.rs::tests`): 3 tests — `target_for_uname` covers all four release targets including aliases (`amd64`, `arm64`), returns None for unsupported (FreeBSD/Windows/garbage), trims whitespace.
- **SSH tunnel** (`src-tauri/src/ssh/tunnel.rs::tests`): 4 tests — required ssh flags locked in, `-L` forwarding spec contains both paths, remote command is the last arg, target comes before remote command.

All 22 new tests pass alongside the prior suite (1382 lib tests, 1721 frontend tests, no regressions; one pre-existing env-related lib failure unrelated to this change).

## Follow-ups (remaining after the push action landed)

| | |
|---|---|
| Chat new-session DevicePicker wiring | Drop `<DevicePicker>` into the chat composer's entry surface. ~30 min. |
| Workspace header badge | Subtle host name pill in workspace title for non-local workspaces. ~1 hour. |
| Workspace list filter | "This device / All / per-host" dropdown matching superset's `V2WorkspacesHeader`. ~2 hours. |
| "Pull workspace back" action | Reverse of push. ~half day. |
| Release skill update | Cross-compile + bundling for the four `codemux-remote` targets. Concrete diff in the release pipeline. ~half day. |
| Tunnel auto-reconnect polish | `ssh::tunnel_supervisor` is in place; tune the backoff (1s→30s, watchdog) and surface the reconnect state in the UI. |

Landed since the original 2b–2d cut: **"Push workspace to host" action** (`8c72b44`) — rsync the worktree, spawn the remote daemon, attach the local UI through the SSH-forwarded socket, and synchronize the Claude conversation across local/remote ends.

## Important Touch Points

- `src-tauri/src/state/state_impl.rs` — `WorkspaceSnapshot.host_id`, `set_workspace_host_id`.
- `src-tauri/src/commands/hosts.rs` — `set_workspace_host`, `hosts_test_connection` (real impl), `hosts_bootstrap_install`.
- `src-tauri/src/bin/codemux_remote.rs` — binary entry point. Subcommands: `version`, `pty-daemon`, `scheduler`, `serve` (+ `status`, `stop`), `mcp`, `workspace register`. Unix-only — Windows builds a no-op stub.
- `src-tauri/src/remote/` — headless MCP daemon module: `manifest.rs` (atomic write + pid liveness), `auth.rs` (bearer-token axum middleware), `identity.rs` (`Local | Cloud { user_id, org_id, role }`), `workspace.rs` (self-contained SQLite registry with nullable `owner_id`), `pty.rs` (portable-pty wrapper + 1 MiB ring buffer), `server.rs` (axum routes), `mcp.rs` (stdio JSON-RPC bridge), `mcp_register.rs` (auto-write into agent configs on startup), `tools/mod.rs` (11-tool catalog), `config.rs` (state-dir resolution).
- `src-tauri/src/ssh/probe.rs` / `bootstrap.rs` / `tunnel.rs` / `tunnel_supervisor.rs` / `push.rs` / `registry.rs` — SSH transport (push action + reconnect supervisor + zero-touch provisioning of the `serve` daemon).
- `src-tauri/src/hosts_upgrade.rs` — background re-bootstrap poller that runs once ~5 s after app start.
- `src/components/hosts/device-picker.tsx` — shared pill component.
- `src/components/overlays/new-workspace-dialog.tsx` — DevicePicker wired into bottom bar.
- `src/components/settings/hosts-section.tsx` — uses real probe + bootstrap modal.
- `src/tauri/commands.ts` — new bindings: `setWorkspaceHost`, `hostsBootstrapInstall`, `HostBootstrapResult`.
- `Cargo.toml` — `[[bin]] codemux-remote`; embedded `axum`, `tower`, `rusqlite`, `portable-pty` for the headless daemon.
- `scripts/codemux-remote.service.example` — sample systemd user unit (used both by manual install and `provision_serve`).
- `src-tauri/tests/codemux_remote_serve_mcp.rs` — 8 end-to-end tests covering the full HTTP roundtrip, the MCP stdio roundtrip, auth required, singleton check, status JSON, missing-daemon error path, PTY echo.

## Troubleshooting

**"Reachable, but codemux-remote isn't installed yet" but the Install button does nothing:**
- Check the laptop's `src-tauri/binaries/` directory has `codemux-remote-<target>` for the host's uname. In dev builds, cross-compiles aren't usually run; the bootstrap returns `BinaryNotBundled` with the target triple it was looking for.

**Probe says "Permission denied (publickey)":**
- Your key isn't authorized on the host. Add the laptop's public key to the host's `~/.ssh/authorized_keys`. Codemux deliberately doesn't paper over this — it would mean storing your private key in our process, which is a security regression.

**Tunnel says "ssh exited before tunnel came up":**
- Usually a `-L` bind failure: the local socket already exists from a stale prior tunnel, OR the remote socket dir doesn't exist + can't be created. The tunnel command's `rm -f` + `mkdir -p` covers most of this; if it still fails, check the SSH stderr from the captured error message.
