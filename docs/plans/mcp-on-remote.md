# Headless Codemux: MCP on `codemux-remote`

- Purpose: Let an agent running on a remote host (VPS, home server, anywhere `codemux-remote` is installed) drive Codemux the same way an agent on the desktop drives it through the MCP — create workspaces, open them, run presets, write to terminals — so the worktree and metadata exist on the server, and the desktop can later pull the workspace back.
- Audience: Anyone touching `src-tauri/src/control.rs`, `src-tauri/src/mcp_server.rs`, `src-tauri/src/bin/codemux_remote.rs`, the SSH push/pull flow, or the Tauri state stores.
- Authority: Active work plan only. Behavior that lands moves to `docs/features/remote-hosts.md`.
- Update when: Any of the steps below lands or an open question is resolved.
- Read next: `docs/features/remote-hosts.md`, `docs/features/automations.md`, `src-tauri/src/control.rs`, `src-tauri/src/mcp_server.rs`.

## Goal

Promote `codemux-remote` from a slim PTY-daemon binary into a full **headless Codemux daemon** that owns its own workspace registry, control endpoint, and MCP server — so an agent on a VPS can `codemux-remote workspace create / open / preset apply / terminal write` through the same MCP tool surface the desktop exposes. When the user pulls a workspace from that host, the desktop imports both the worktree files (existing rsync path) **and** the workspace metadata (new control-protocol path), so the workspace lands in the left sidebar exactly as if it had been created locally.

**Ship v1 SSH-only. Design for a future optional cloud relay (paid tier) without building it.** No relay code, no Better Auth coupling, no org/team data model in v1 — but the transport, identity, and schema choices below are deliberately picked so adding a relay later is purely additive, not a rewrite. See "Designing for an optional future relay" below for the constraints this places on v1.

## Why not just copy Superset (and why we'll borrow the layering anyway)

Superset's design is clean but requires:

1. A central cloud MCP endpoint (`api.superset.sh`) that all agents must reach.
2. A WebSocket relay (Fly-hosted) that every host-service tunnels out to.
3. Org/JWT auth on every tool call.

That shape exists because Superset is a team product — teammate A on a MacBook needs to control teammate B's home server they have no shell on. It's a *product requirement*, not an architectural quality signal. We don't have that requirement today, and we already have SSH, so the cheap v1 is "give `codemux-remote` a headless control server, point its MCP at it locally, tunnel into it from the desktop over SSH." No cloud.

But we **do** want to borrow Superset's *layering* so a paid-tier cloud relay is a future bolt-on, not a rewrite. Specifically: `packages/host-service` exposes loopback HTTP + bearer token; `packages/mcp-v2/src/host-service-client.ts::hostServiceCall` just does `fetch()` against either a loopback URL or a relay URL — same client code, two transports. We mirror this. The daemon never knows whether it's being called by a local desktop, an SSH-forwarded desktop, or (someday) a relay forwarding requests from a phone.

## Architecture (target shape)

```
┌─────────────────────────── DESKTOP ──────────────────────────────┐
│                                                                  │
│  Tauri app                                                       │
│   ├─ UI (React)                                                  │
│   ├─ AppStateStore  ──┐                                          │
│   ├─ PtyState         │                                          │
│   ├─ DatabaseStore    ├──>  codemux_core::ControlServer  <──┐    │
│   ├─ PresetStoreState ┘         │ axum, 127.0.0.1:<port>   │    │
│   └─ ssh push/pull              │ bearer-token auth         │    │
│                                                             │    │
│   `codemux mcp`  ─── stdio MCP → HTTP POST ─────────────────┘    │
│                                                                  │
└──────────────────────────────────────────────────────────────────┘
                              │
                              │  SSH
                              ▼
┌─────────────────────────── REMOTE HOST ──────────────────────────┐
│                                                                  │
│  codemux-remote serve  (NEW — long-running daemon)               │
│   ├─ AppStateStore (headless impl)                               │
│   ├─ PtyState                                                    │
│   ├─ DatabaseStore  (SQLite at ~/.local/share/codemux-remote/)   │
│   ├─ PresetStoreState                                            │
│   └─ codemux_core::ControlServer  (axum, 127.0.0.1:<port>,       │
│        bearer-token auth, manifest at <state-dir>/manifest.json) │
│                                                                  │
│  codemux-remote pty-daemon --socket ...  (existing, unchanged)   │
│  codemux-remote mcp  (NEW — stdio MCP → local control sock)      │
│                                                                  │
│  agent CLI (claude, codex, etc.) configured with                 │
│      mcp.codemux = `codemux-remote mcp`                          │
└──────────────────────────────────────────────────────────────────┘
```

The same `codemux_core` library is embedded by both the Tauri app and the `codemux-remote` binary. Tauri-specific things (UI events, the menubar, the system tray) become a thin adapter layer on top of `codemux_core`. The remote binary plugs in a different adapter that no-ops UI events and routes notifications to logs or a webhook.

## Active Priorities

### Step ordering (what blocks what)

```
Step 1 (extract codemux_core)
   │
   ├──> Step 2 (codemux-remote serve)
   │       │
   │       ├──> Step 3 (codemux-remote mcp)
   │       ├──> Step 4 (trim headless tool surface)
   │       ├──> Step 5 (desktop pull workspace)
   │       └──> Step 8 (process supervision)
   │
   ├──> Step 7 (auth + manifest, both desktop and remote)
   │       │
   │       └──> Step 6 (desktop --host flag, depends on manifest format)
   │
   └──> Step 9 (desktop transport retrofit) — see below
```

Step 1 is the prerequisite for everything. Steps 2 + 7 can be done in parallel after Step 1. Step 6 is the last user-facing piece. The whole sequence is incrementally shippable — Steps 1–4 alone deliver "agent on a server can drive Codemux locally," which is the headline feature.

### Step 1 — Extract `codemux_core` from the Tauri crate

Today every piece of MCP-relevant state lives behind a `tauri::State<'_, T>` lookup off `AppHandle`. The first job is to break that coupling without changing behavior.

Concretely:

1. Create a new internal crate `src-tauri/codemux-core/` (or a `core/` module reachable from both `[lib]` and `[[bin]] codemux-remote`).
2. Move into it:
   - `state.rs` (`AppStateStore`, workspace records, active-workspace tracking).
   - `database.rs` (the SQLite-backed `DatabaseStore`).
   - `terminal.rs` (PTY state — already mostly UI-free; just unhook the `app.emit` calls).
   - `presets/`, `automations/`, `worktree.rs`.
   - `control.rs` (transport + `dispatch_request`).
   - `mcp_server.rs` (the MCP shim).
3. Replace every `app.state::<X>()` lookup inside `dispatch_request` with a single explicit `CoreContext` struct that holds `Arc<AppStateStore>`, `Arc<DatabaseStore>`, `Arc<PtyState>`, `Arc<PresetStoreState>`, …, plus a `dyn UiNotifier` trait object for the few `app.emit(...)` calls (currently used by `emit_app_state`, preset progress, automation status).
4. The Tauri side keeps an `impl UiNotifier for TauriUiBridge` that calls `app.emit`. The remote side provides `impl UiNotifier for NullNotifier` (no-op + structured log) or eventually a `WebhookNotifier`.
5. **Add an `Identity` argument to every `dispatch_request` call.** Today it's just `Identity::Local` (parsed from the bearer token — any caller with the secret is "local"). The struct exists so future cloud-relay-forwarded requests can carry `Identity::Cloud { user_id, org_id, role }` without changing handler signatures. v1 handlers ignore the value; they just need to accept the parameter.
6. Acceptance: desktop build still passes `npm run verify`; `cargo check -p codemux-core` passes with no `tauri` dependency in `codemux-core/Cargo.toml`; every dispatch handler takes `(ctx: &CoreContext, identity: &Identity, params: T)`.

### Step 2 — Headless `codemux-remote serve`

In `src-tauri/src/bin/codemux_remote.rs`, add a `serve` subcommand:

```text
codemux-remote serve
  [--port <n>]             # default: pick a free port, write to manifest
  [--bind 127.0.0.1]       # never default to 0.0.0.0; SSH tunnels are the path
  [--state-dir <path>]     # default ~/.local/share/codemux-remote
  [--no-daemonize]         # for systemd / debugging
  [--notify-webhook <url>] # POST agent-needs-attention events to this URL
```

What it does:

1. Boots `codemux_core` with `NullNotifier` (or `WebhookNotifier` if `--notify-webhook` set).
2. Opens / creates SQLite at `<state-dir>/codemux.db` — same schema the desktop uses, since the `DatabaseStore` code is shared.
3. Spawns an **axum HTTP server** on loopback. Each handler is a thin wrapper that parses the bearer token → `Identity::Local`, dispatches to the same `codemux_core::dispatch_request` function the desktop uses.
4. Writes `<state-dir>/manifest.json` with `{ endpoint: "http://127.0.0.1:<port>", secret, pid, started_at, host_id }`, mode `0600` (matches Superset's pattern in `packages/host-service/src/daemon/manifest.ts`).
5. Stays running until SIGTERM. On exit, drain in-flight requests, snapshot active workspaces, flush DB, remove the manifest.

A second subcommand, `codemux-remote serve status`, reads the manifest and prints `{ endpoint, pid, workspaces: N, started_at }` — used by the desktop to probe whether a host already has a daemon up.

Acceptance: `codemux-remote serve` + `curl -H "Authorization: Bearer $(cat ~/.local/share/codemux-remote/manifest.json | jq -r .secret)" http://127.0.0.1:<port>/control -d '{"command":"workspace_list","params":{}}'` returns an empty list (and creating one then re-listing shows it).

Why HTTP and not the existing Unix-socket JSON-line protocol: see "Designing for an optional future relay" below. Short version: HTTP is what makes the relay a future bolt-on instead of a rewrite. Same code complexity (axum vs `tokio::net::UnixListener`), strictly more optionality.

### Step 3 — `codemux-remote mcp`

Mirror what `codemux mcp` does in `src-tauri/src/cli.rs` (line 571–574) — boot the stdio MCP server pointed at the local control endpoint. Because `mcp_server.rs` lives in `codemux_core` now, this is a thin subcommand that:

1. Reads `<state-dir>/manifest.json` to get the endpoint URL + bearer secret.
2. Boots the MCP server with a `HttpControlClient` that POSTs to that endpoint.

The desktop's `codemux mcp` works the same way against the desktop manifest. Both clients hit the same `codemux_core` dispatcher; the only thing that varies is the URL.

Acceptance: on a remote host, configure the agent (e.g. Claude Code) with `"codemux": { "command": "codemux-remote", "args": ["mcp"] }` in its MCP config; the agent can `workspace_create`, `workspace_list`, `preset_apply`, `terminal_write`, `terminal_read`. Those workspaces appear in `codemux-remote serve`'s SQLite and are reachable via HTTP from anything else (CLI, SSH-tunnelled desktop, future relay).

### Step 4 — Trim the headless MCP tool surface

`mcp_server.rs` advertises 50+ tools. Some don't make sense headless:

| Tool family | Headless behavior |
|---|---|
| `mcp__codemux__workspace_*` | Keep, all server-side. |
| `mcp__codemux__terminal_*` | Keep, PTY works headless. |
| `mcp__codemux__preset_*` | Keep. |
| `mcp__codemux__automation_*` | Keep — automations on a server are arguably the killer app. |
| `mcp__codemux__git_*` | Keep. |
| `mcp__codemux__issue_*` | Keep (it shells to `gh`). |
| `mcp__codemux__pane_*` | **Drop on headless** — no panes without a UI. |
| `mcp__codemux__browser_*` | **Drop on headless** — no browser pane. |
| `mcp__codemux__notify` | Replace UI toast with webhook POST + journald log. |
| `mcp__codemux__app_status` | Keep, but `panes`/`browser` keys report `unavailable`. |

Mechanism: each tool's `define-tool` macro takes a `headless_supported: bool`. The MCP server filters its `tools/list` response based on a `Mode::Desktop | Mode::Headless` value passed in at construction.

Acceptance: an agent talking to `codemux-remote mcp` only sees the headless-supported tools in its tool catalog; calling `browser_navigate` returns a clean "not supported on headless host" MCP error rather than a hang or a panic.

### Step 5 — Desktop "pull workspace" learns the control protocol

Today (`docs/features/remote-hosts.md` line 17) "Push workspace to host" rsyncs the worktree, spawns the remote PTY daemon, attaches the local UI. There is **no** pull path that imports a workspace **created on** the remote — because nothing creates workspaces on the remote yet.

After Step 2 there will be. New flow:

1. Desktop opens an SSH tunnel that forwards the remote HTTP port: `ssh -L 127.0.0.1:<local-port>:127.0.0.1:<remote-port> <host>`. The remote port + secret come from `ssh <host> cat ~/.local/share/codemux-remote/manifest.json`.
2. Desktop GETs `http://127.0.0.1:<local-port>/control` with the bearer secret, sends `workspace_list` — gets back the full list of workspaces the remote daemon owns.
3. User picks one; desktop sends `workspace_info <id>` — gets path, branch, base commit, agent session ID, preset, notes, linked issue.
4. Desktop rsyncs the worktree from `remote:<path>` to a local path under its workspace tree.
5. Desktop inserts a new row into its **own** SQLite with the imported metadata, marks it `imported_from_host=<host_id>`, and emits `app_state` so the new workspace shows up in the left sidebar.
6. Optional: desktop sends `workspace_close <id>` to the remote (or leaves it; "pull" can be a copy, not a move).

Touch points: `src-tauri/src/ssh/` (new `pull.rs` alongside `push.rs`), the existing "Push workspace to host" command UI gets a sibling "Pull workspace from host".

Acceptance: agent on remote runs `workspace_create` via MCP, makes commits, sleeps. Desktop opens, clicks "Pull from <host>", picks the workspace, sees it in the sidebar with the right branch and last-commit info. Opening it shows the imported worktree.

### Step 6 — Desktop CLI can target a remote host

Once the remote runs the same control server, the desktop CLI (`codemux workspace ...`, `codemux mcp`) should be able to address it by host id. Mirror Superset's `--host` flag:

```text
codemux --host homelab workspace create ./repo
codemux --host homelab mcp                          # routes MCP over SSH tunnel
```

Implementation: the control client now takes a URL + secret. When `--host <id>` is set:

1. Look up the host in `ssh::registry` — get host_id, manifest path, cached secret.
2. Open (or reuse) an SSH `-L` tunnel forwarding a free local port to `127.0.0.1:<remote-port>`.
3. Point the HTTP client at `http://127.0.0.1:<local-tunnel-port>` with the bearer secret.

This means the **same** MCP server (`codemux mcp`) can be configured on the desktop agent to control either the desktop or any registered host — `codemux mcp --host homelab` and your desktop Claude Code is now running workspaces on the VPS.

Acceptance: from desktop terminal, `codemux --host homelab workspace list` returns the remote's workspaces; round-trip latency stays under ~500ms on a typical broadband link.

### Step 7 — Auth + safety

Right now the desktop control socket has no auth — same-user filesystem perms (`0o600` parent dir) are the trust boundary, matching `gh`/`ssh-agent` precedent. That's fine on a single-user laptop. On a multi-tenant VPS it isn't.

For the headless daemon:

1. On first `codemux-remote serve` boot, generate a 32-byte secret, store it inside `<state-dir>/manifest.json` (the same file that holds the endpoint URL, mode `0o600`).
2. Every HTTP request must carry `Authorization: Bearer <secret>`. The axum middleware validates and tags the request with `Identity::Local`.
3. `codemux-remote mcp` reads the secret from the manifest and injects it on every POST.
4. SSH-forwarded callers on the desktop side read the manifest over SSH (`ssh <host> cat ~/.local/share/codemux-remote/manifest.json`) on first connect and cache the endpoint + secret in the host record.
5. The desktop's existing control endpoint gets the same treatment for consistency — write a manifest, require a bearer token. Existing local callers (Tauri-side MCP boot) read the same manifest. This makes the desktop and remote endpoints byte-for-byte identical in shape; the only difference is what writes to UI.

Acceptance: spinning up `codemux-remote serve` on a host and then `curl` -ing the endpoint without the secret returns `401`; `codemux-remote mcp` works because it has the secret.

### Step 8 — Process supervision

`codemux-remote serve` needs to survive SSH disconnects and reboots, otherwise the "agent works on it overnight" story breaks.

Two options, picked at install time by the bootstrap modal (`docs/features/remote-hosts.md` §"bootstrap.rs"):

- **systemd user unit** (preferred when `systemctl --user` works): `~/.config/systemd/user/codemux-remote.service` with `Restart=on-failure`, `loginctl enable-linger` the user.
- **nohup + screen-less detach** fallback: a small `codemux-remote serve --daemonize` mode that double-forks, redirects stdio to `<state-dir>/serve.log`, writes a pidfile.

Acceptance: SSH out, `systemctl --user status codemux-remote` shows active; reboot the host, daemon comes back up; workspaces persist (already covered by SQLite).

### Step 9 — Migrate the desktop's existing control endpoint to HTTP+manifest

Called out in Step 7 but big enough to deserve its own step. The desktop today binds a Unix socket at `$XDG_RUNTIME_DIR/codemux.sock` (or named pipe on Windows) and accepts unauthenticated JSON-line requests. After Step 1+7, the desktop also runs the axum HTTP server on loopback with a bearer token from `~/.codemux/manifest.json`.

What changes:

1. `control::spawn_control_server` (lib.rs:593) starts the axum server instead of the JSON-line listener.
2. `codemux mcp` reads `~/.codemux/manifest.json` to find the endpoint and secret. Existing user MCP configs (`{"command": "codemux", "args": ["mcp"]}`) keep working — the change is internal.
3. The Unix-socket path can be kept temporarily as a compatibility shim that proxies to the HTTP endpoint, or dropped outright if no external tools depend on it (likely none — the socket is meant to be internal).
4. Windows: same axum server, no more named-pipe code path. Strict simplification.

Migration concerns:

- **Existing in-flight workspaces.** The SQLite schema is unchanged in v1 (the `owner_id` column is new and nullable). No data migration required.
- **In-flight MCP sessions during upgrade.** An agent process holding a long-lived MCP connection will see the socket disappear; it'll need to reconnect when the user restarts the desktop. Acceptable; agents already handle this.
- **External scripts hitting the socket directly.** Anything outside the Codemux org doing this is undocumented usage; we don't owe it compatibility. Mention in release notes.

Acceptance: existing `codemux mcp` flow works against the new HTTP endpoint without the user changing their agent config; `npm run verify` passes; Windows build still produces a working binary.

### Step 10 — Testing strategy

Per-step acceptance criteria above cover the happy path. Below is the test-suite shape this work needs to add or extend.

- **`codemux-core` unit tests.** Every `dispatch_request` handler gets a unit test against an in-memory `CoreContext` (SQLite `:memory:`, `NullNotifier`, no PTY). Catches state-management regressions without spinning up a real daemon. Aim: 80%+ coverage of the dispatcher.
- **Auth middleware tests.** `cargo test` for the axum layer: missing header → 401, wrong secret → 401, correct secret → 200, `Identity::Local` propagated to handler.
- **`codemux-remote serve` binary tests.** Extend `src-tauri/tests/codemux_remote_binary.rs` (currently 3 tests per `docs/features/remote-hosts.md`) with: `serve` writes a manifest then accepts HTTP; `serve status` reads it back; `serve` exits cleanly on SIGTERM and removes the manifest; two concurrent `serve` instances on the same state-dir refuse to start the second.
- **End-to-end SSH flow.** A new `src-tauri/tests/remote_mcp_roundtrip.rs` that boots `codemux-remote serve` in a child process, opens a local HTTP client against it (skipping SSH for test speed), exercises `workspace_create` → `workspace_list` → `workspace_info` → `workspace_close`. Covers Steps 2 + 5's read path.
- **Headless tool surface.** Test that `tools/list` over the MCP stdio interface returns only headless-supported tools when `Mode::Headless` is set; `browser_*` calls return the right "not supported" error.
- **Cross-platform.** Windows CI must run the same axum/manifest tests. The desktop build also has to keep working after Step 9; verify in CI on Linux + macOS + Windows.

## Designing for an optional future relay (do not build in v1)

The four design choices below are what make a future paid-tier cloud relay an additive feature, not a rewrite. v1 implements all of them with the *local* meaning and ignores the cloud meaning. Cost today is roughly zero; cost of retrofitting later if we skip them is high.

| Choice | v1 behavior | Future cloud meaning | Cost to do now |
|---|---|---|---|
| **HTTP transport** | axum on loopback, bearer token from manifest. | Relay terminates user's TLS, forwards HTTP request body through a WebSocket tunnel to the host's loopback HTTP. Same dispatcher on the host side. | Same lines of code as Unix-socket JSON-lines. |
| **`Identity` argument on every handler** | Always `Identity::Local`. Handlers ignore it. | Relay verifies the JWT, attaches `Identity::Cloud { user_id, org_id, role }` as a forwarded header, daemon trusts the relay's verdict. | ~20 lines (one struct + one middleware). |
| **Nullable `owner_id` on workspaces table** | Always `NULL`. | Populated with the creating user's ID; future ACL checks gate `workspace_open` etc. | One column in the schema. |
| **Daemon does not import Better Auth / VPS-side libs** | Daemon trusts its local bearer secret, full stop. | Relay (a separate binary on the VPS) talks to Better Auth, daemon talks to relay. | Discipline — just don't add the dependency. |

What a future v2 relay would look like *if* we ever build it (sketch only; not on the v1 roadmap):

1. `codemux-relay` binary deployed to the existing Hetzner VPS, alongside Better Auth and Postgres. ~1 Fly-style WebSocket server: hosts open outbound WS, cloud HTTP requests get routed to the right host's WS.
2. `codemux-remote serve --tunnel-to https://api.codemux.com --org-token <jwt>` — opt-in flag, never default. Host opens a WebSocket out to the relay, identifies itself as `org_id/host_id`.
3. MCP endpoint at `api.codemux.com/mcp` accepts tool calls with a user JWT, looks up the user's hosts, routes to the chosen one through the relay.
4. Phone/web client gets a `codemux mcp --cloud` mode for "control my hosts from anywhere."

Estimated VPS load even at 1000 paying users × 5 hosts each = 5000 idle WebSockets, ~80 MB RAM, ~500 KB/s heartbeats — fits comfortably on the existing Hetzner box without scaling out. PTY data does not transit the relay; only control-plane tool-call envelopes do (a few KB per call). This is the same load model Superset runs on Fly.

Again: **do not build this in v1.** The point of listing it is to lock in the four design choices in the table above, then stop. The relay is a future paid product (team collaboration, "control from phone without SSH"), not a v1 dependency.

## Open Questions

- **Should workspaces be moved on pull, or copied?** Copy is safer (server still has the worktree if pull fails). Move keeps state-of-the-world clean. Probably ship as copy with an opt-in "pull and remove from host" toggle later.
- **Agent sessions across hosts.** When the desktop pulls a workspace, the agent's session (Claude Code conversation file under `~/.claude/projects/...`) lives in the remote `$HOME`. Do we rsync that too? Likely yes — there's prior art in the existing push flow that "synchronizes the Claude conversation across local/remote ends" (per `remote-hosts.md` line 17). Make pull symmetric.
- **Concurrent agents on the same workspace on different hosts.** Out of scope; the first version refuses pull if the remote workspace is "active" (has a running agent PTY). User must stop the agent first.
- **Webhook payload shape for the headless notifier.** Likely just `{ kind, workspace_id, message, ts }`; needs a tiny doc once it lands so users can build phone notifiers.
- **Cross-host workspace IDs.** Workspace UUIDs need to be globally unique so an imported workspace can't collide with a local one. Current schema uses UUID v4 — fine. But we should record `origin_host_id` so the UI can show "imported from homelab".
- **Distribution of the new `serve` subcommand.** The release pipeline already cross-compiles `codemux-remote` for four targets (`docs/features/remote-hosts.md` line 78). The headless serve will increase binary size (it now embeds SQLite, the worktree manager, etc.). Probably still under 30 MB; verify before shipping.
- **Is `codemux-remote` still a good name?** Once it grows a workspace registry it's more "codemux-server" than "codemux-remote". Rename or alias? Defer to release-time bikeshed.
- **Upgrade flow for existing hosts.** When the desktop ships a newer `codemux-remote` binary in `src-tauri/binaries/`, hosts running an older version need to be re-bootstrapped (re-scp the new binary + restart the systemd unit). Today the probe step in `docs/features/remote-hosts.md` checks the version but doesn't auto-upgrade. Probably needs a "Reinstall on host" button in Settings → Hosts. Out of scope for v1; track as a follow-up.
- **Concurrent `serve` instances on the same machine.** If a user runs `codemux-remote serve` on the same machine as their desktop (weird but possible), both processes try to manage workspaces and may stomp on each other. Resolution: keep state dirs strictly separate (`~/.codemux/` desktop vs `~/.local/share/codemux-remote/` headless), and have `serve` refuse to start if its own manifest's pid is still alive (singleton check, ~10 lines).
- **Agent CLI installed on the remote.** `terminal_write` will only succeed if `claude`, `codex`, etc. are actually installed on the host. The bootstrap step should probe for the user's configured agent and warn if missing; or — more pragmatic — let the first `workspace_create` fail with a clear error message and document the prerequisite. Probably the latter for v1.

## Likely Touch Points

- `src-tauri/src/control.rs` — strip `AppHandle`, take `CoreContext`.
- `src-tauri/src/mcp_server.rs` — move out of Tauri crate; add `Mode::Desktop | Mode::Headless`.
- `src-tauri/src/state.rs` — drop `tauri::Manager` calls, expose `Arc<AppStateStore>`.
- `src-tauri/src/database.rs` — already Tauri-free in spirit; verify.
- `src-tauri/src/terminal/` — strip the few `app.emit` calls behind `UiNotifier`.
- `src-tauri/src/bin/codemux_remote.rs` — add `serve`, `serve status`, `mcp` subcommands.
- `src-tauri/src/ssh/push.rs` and a new sibling `src-tauri/src/ssh/pull.rs`.
- `src-tauri/src/ssh/registry.rs` — persist `secret` per host, persist `control_socket_path`.
- `src-tauri/src/cli.rs` — `--host` global flag.
- `src/components/hosts/` — Pull-workspace UI alongside the existing Push.
- `docs/features/remote-hosts.md` — update once §"`codemux-remote` slim binary" stops being slim.
- `Cargo.toml` — new `[[bin]] codemux-remote` deps (sqlite, etc.); possibly a new `codemux-core` member crate.

## Already Landed

- Slim `codemux-remote` binary with `version`, `pty-daemon --socket`, `scheduler` (commit history under `docs/features/remote-hosts.md`).
- SSH push of a workspace from desktop to host, with PTY tunnel and Claude-conversation sync (commit `8c72b44`).
- Cross-compile + bundling of `codemux-remote` for four targets via the release pipeline.
- Desktop `control.rs` control socket + `mcp_server.rs` MCP shim — the implementation we're about to lift into a shared crate.
- **Headless `codemux-remote serve` daemon (this branch).** Self-contained module at `src-tauri/src/remote/` with:
  - `manifest.rs` — atomic-write manifest.json (endpoint, bearer secret, pid, started_at, host_id, owner_id), mode 0600. Pid liveness via `kill(0)`.
  - `auth.rs` — axum middleware enforcing `Authorization: Bearer <secret>` with constant-time comparison; tags every request with `Identity::Local`.
  - `identity.rs` — `Identity` enum with `Local` (v1) and reserved `Cloud { user_id, org_id, role }` for the future relay.
  - `workspace.rs` — self-contained SQLite registry at `<state-dir>/codemux.db` with nullable `owner_id` column for future relay use.
  - `pty.rs` — minimal portable-pty wrapper, per-terminal ring buffer (1 MiB cap).
  - `server.rs` — axum HTTP routes: `GET /health`, `GET /tools/list`, `POST /tools/call`.
  - `mcp.rs` — JSON-RPC 2.0 stdio MCP server (`initialize`, `tools/list`, `tools/call`) forwarding to the local daemon over HTTP.
  - `tools/mod.rs` — 12 headless tools: workspace_{create,list,info,update,close}, worktree_create (added `v0.7.5`, backed by `remote/git.rs`), terminal_{spawn,write,read,list,close}, app_status.
- **CLI subcommands on `codemux-remote`**: `serve` (long-running daemon with graceful SIGTERM/SIGINT shutdown), `serve status`, `serve stop`, `mcp` (stdio MCP for an agent CLI).
- **Tests**: 26 unit tests in the `remote` module + 8 end-to-end integration tests in `src-tauri/tests/codemux_remote_serve_mcp.rs` covering the full HTTP roundtrip, the MCP stdio roundtrip (the headline test), and edge cases (auth required, singleton check, status JSON, missing-daemon error path, PTY echo roundtrip).
- **Example systemd user unit** at `scripts/codemux-remote.service.example` with install instructions.

## Auto-provisioning on push (the "it just works" flow — landed)

The desktop push flow is now end-to-end zero-touch:

- `ssh::bootstrap::bootstrap_remote` (pre-existing) installs the binary.
- `ssh::bootstrap::provision_serve` (NEW) installs + starts the
  `codemux-remote.service` systemd user unit and enables lingering.
- `ssh::push::push_workspace` (extended) calls
  `provision_workspace_mcp_config` to drop a `.mcp.json` in the pushed
  workspace dir so any CLI agent launched there auto-discovers Codemux
  via `codemux-remote mcp`.
- After rsync, `ssh::bootstrap::register_workspace_on_remote` SSHes in
  and runs `codemux-remote workspace register --path ...` (a new
  subcommand on the remote binary that calls the local daemon's
  `workspace_create` tool over loopback HTTP) so the pushed workspace
  shows in `workspace_list` from any agent.

Net effect: user clicks "Push workspace to host" once, the rest is
automated. They never see the words "manifest" or "systemd."

## Explicitly deferred to follow-up PRs

- **Step 1 (extract codemux_core)** and **Step 9 (migrate desktop transport)** — the desktop's `control.rs`/`mcp_server.rs`/state stores are not touched in this branch. Reconnaissance showed ~98K LOC of Rust across ~143 files with ~403 occurrences of Tauri couplings; doing it cleanly is a multi-PR effort. The headless daemon ships *without* needing it because it has its own self-contained registry, server, and tools. Code-sharing dedup becomes a future refactor when the two implementations have actually diverged enough to be worth merging.
- **Step 5 (desktop pull workspace)** — desktop UI for "pull from host" is unbuilt; the existing rsync push path is unchanged. The wire protocol it needs (HTTP `workspace_list` + `workspace_info`) is already up and tested on the daemon side. Adding the desktop pull-flow code is mostly UI + an SSH-tunnel wrapper around an already-working HTTP client.
- **Step 6 (desktop CLI --host flag)** — same gating: the daemon side is done, the desktop CLI wiring is the missing piece.
- **Phone control without SSH** — v1 recommends Tailscale (documented in `docs/features/remote-hosts.md`). The optional cloud relay (paid tier) is deferred as a separate v2 project; the four design constraints listed above (HTTP transport, `Identity` passthrough, nullable `owner_id`, no Better-Auth coupling) keep that future path additive.

## Notes

- This is the first step toward "Codemux works the same on any machine I can ssh into," which subsumes a chunk of the automations roadmap (`docs/plans/automations.md`) — a server-side automation is just "this headless Codemux runs a preset and pushes a PR" with no daemon-lifecycle distinction.
- We are intentionally **not** introducing a cloud relay. If we ever want a "control my home Codemux from my phone without SSH" story, the right shape is a thin per-user relay that proxies between an authenticated phone client and the user's headless daemon — additive, not on the critical path here.
- Superset's `apps/api/MCP_TOOLS.md` and `packages/host-service/src/tunnel/` are worth a re-read before Step 5/6; their `buildHostRoutingKey` + WebSocket-routed tRPC envelope is the most mature prior art for "MCP on host X targets host Y," even if we don't use their transport.
