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

What is **not** in 2d (deferred to a follow-up): the "Push workspace to host" action that actually rsyncs the worktree, spawns the tunnel, and reattaches the UI to the remote daemon. The plumbing for that exists now — `TunnelHandle::local_socket()` returns a path the existing `PtyDaemonClient::connect(&path)` dials unchanged — but wiring it into the workspace push/pull UX is its own UX surface.

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

## `codemux-remote` slim binary (2c)

`src-tauri/src/bin/codemux_remote.rs`. New `[[bin]]` target in `Cargo.toml`. Same `codemux_lib` crate, no UI deps.

CLI:
```
codemux-remote version
  → {"name":"codemux-remote","version":"0.3.1","protocol_version":1}

codemux-remote pty-daemon --socket /tmp/codemux-ptyd-<rand>.sock
  → binds the socket, runs the daemon server, never returns
```

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

## Follow-ups (not in 2b–2d)

| | |
|---|---|
| Chat new-session DevicePicker wiring | Drop `<DevicePicker>` into the chat composer's entry surface. ~30 min. |
| Workspace header badge | Subtle host name pill in workspace title for non-local workspaces. ~1 hour. |
| Workspace list filter | "This device / All / per-host" dropdown matching superset's `V2WorkspacesHeader`. ~2 hours. |
| "Push workspace to host" action | rsync + tunnel spawn + reattach UI. The transport is wired; this is the UX flow that strings it together. ~1 day. |
| "Pull workspace back" action | Reverse of push. ~half day. |
| Release skill update | Cross-compile + bundling for the four `codemux-remote` targets. Concrete diff in the release pipeline. ~half day. |
| Auto-reconnect on tunnel drop | Currently the tunnel handle exits when SSH dies. A supervisor that auto-reconnects with backoff (1s→30s, watchdog) is the next layer up. Matches the pattern superset uses in `tunnel-client.ts`. |

## Important Touch Points

- `src-tauri/src/state/state_impl.rs` — `WorkspaceSnapshot.host_id`, `set_workspace_host_id`.
- `src-tauri/src/commands/hosts.rs` — `set_workspace_host`, `hosts_test_connection` (real impl), `hosts_bootstrap_install`.
- `src-tauri/src/bin/codemux_remote.rs` — slim binary entry point.
- `src-tauri/src/ssh/probe.rs` / `bootstrap.rs` / `tunnel.rs` — SSH transport.
- `src/components/hosts/device-picker.tsx` — shared pill component.
- `src/components/overlays/new-workspace-dialog.tsx` — DevicePicker wired into bottom bar.
- `src/components/settings/hosts-section.tsx` — uses real probe + bootstrap modal.
- `src/tauri/commands.ts` — new bindings: `setWorkspaceHost`, `hostsBootstrapInstall`, `HostBootstrapResult`.
- `Cargo.toml` — new `[[bin]] codemux-remote`.

## Troubleshooting

**"Reachable, but codemux-remote isn't installed yet" but the Install button does nothing:**
- Check the laptop's `src-tauri/binaries/` directory has `codemux-remote-<target>` for the host's uname. In dev builds, cross-compiles aren't usually run; the bootstrap returns `BinaryNotBundled` with the target triple it was looking for.

**Probe says "Permission denied (publickey)":**
- Your key isn't authorized on the host. Add the laptop's public key to the host's `~/.ssh/authorized_keys`. Codemux deliberately doesn't paper over this — it would mean storing your private key in our process, which is a security regression.

**Tunnel says "ssh exited before tunnel came up":**
- Usually a `-L` bind failure: the local socket already exists from a stale prior tunnel, OR the remote socket dir doesn't exist + can't be created. The tunnel command's `rm -f` + `mkdir -p` covers most of this; if it still fails, check the SSH stderr from the captured error message.
