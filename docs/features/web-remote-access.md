# Web Remote Access

- Purpose: Describe the embedded, default-off HTTP + WebSocket server that turns the desktop app into a second frontend for its own backend — a browser on another device loads the same UI bundle and drives the same running instance.
- Audience: Anyone touching the `web_remote` server, the `src/remote/` WebSocket shim, the Remote Access settings pane, the stream fan-out in terminal/agent-chat, or the web fallbacks (path picker, notifications, assets, browser-pane proxy, updater).
- Authority: Canonical feature-level reality doc for web remote access.
- Update when: The WS protocol contract, the auth model, the fan-out semantics, the endpoint enumeration, or any web fallback changes.
- Read next: `docs/plans/web-remote-access.md` (locked design contract + remaining work), `docs/features/dev-mock-runtime.md`, `docs/features/remote-hosts.md`, `docs/features/terminal.md`, `docs/features/agent-chat.md`.

## What This Feature Is

The desktop app grows an embedded HTTP + WebSocket server. When the user turns it on, a browser on another machine — a laptop, a phone on the same LAN or the user's mesh VPN — loads the *same* UI bundle the desktop runs and drives the *same* running desktop instance: the same projects, workspaces, terminals, agent chats, git state. It is a **second frontend for one backend**, not a second app.

This is a **UI transport**. It is the sibling of remote-hosts, which is a **compute transport** (same frontend, a remote backend reached over SSH). The two compose: a paired web client automatically sees and drives the desktop's remote hosts, because all of that logic already lives behind the invoke surface the web client speaks to. See "Relationship to remote hosts" below.

Default off. Nothing binds until the user enables it in Settings → Remote Access. Work continues when the browser disconnects — the persistent PTY daemon already guarantees a shell survives the app losing its UI, and the web client is just another UI.

## Current Model

### The seam: `window.__TAURI_INTERNALS__`

The entire React UI talks to its backend through exactly one object, `window.__TAURI_INTERNALS__`. The dev mock runtime (`src/dev/tauri-mock.ts`) already proved the UI runs unmodified in a plain browser against a shim of that object. Web remote access installs a **production** twin of that shim (`src/remote/shim.ts`) backed by a real WebSocket to the desktop instead of in-process fixtures. **Zero app components change** — every `invoke()`, `listen`/`emit`/`once`, `Channel`, `convertFileSrc`, and plugin call routes through the shim.

`src/main.tsx` selects the runtime before React mounts, three-way:

1. The Tauri desktop WebView injects the real internals — nothing installed, real IPC.
2. `npm run dev` in a plain browser with no real internals → dev mock.
3. A plain browser served by the desktop's web-remote server → the WebSocket shim (`src/remote/bootstrap.tsx` handles pairing, then installs the shim and connects before mounting).

Both shims stay dormant under the desktop WebView, so real-IPC behavior is byte-identical to before this feature existed. `import.meta.env.DEV` statically tree-shakes the dev-mock branch out of the production bundle. `?remote=1` under `npm run dev` forces the WebSocket shim so it can be exercised against a live server instead of the mock.

### The embedded server

New module `src-tauri/src/web_remote/` runs an axum server (axum is already a dependency) inside the desktop process. When enabled it binds `<port>` (default **4377**, configurable) on the interfaces the **bind scope** selects (default `0.0.0.0`, every interface — see below); disabling severs every live socket (`ConnectionRegistry::close_all`, the same mechanism revocation uses) and then flips a graceful-shutdown `watch` that unbinds every listener — turning the master switch off is a security action, so already-connected devices lose control immediately rather than lingering until their next reload (graceful shutdown alone only stops *new* connections; an open WebSocket never closes on its own). A port **or bind-scope** change rebinds through the same stop→start path, so it also drops existing connections (they cannot follow to the new port/interface). Config (`enabled`, `port`, `require_approval`, `bind_scope`) persists in the existing settings storage under `web_remote.config` (`bind_scope` is `#[serde(default)]` → a config written before the field existed loads as `all`), and `restore_on_boot` re-binds on app start if the feature was left enabled — a bind failure (port taken, or `tailscale` scope with no tailnet address) is logged, never fatal.

### Bind scope (interface exposure)

`bind_scope` narrows which interfaces the server listens on, so on a hostile network the port simply isn't open on the untrusted segment:

- **`all`** (default) → `0.0.0.0:<port>` — every interface. Historical behavior; any LAN or tailnet peer with the port can reach it.
- **`tailscale`** → the tailnet address(es) `endpoints::tailnet_ips()` reports (the same CGNAT-range + `tailscale status` discovery the endpoint list uses) **plus loopback**, each bound as its own listener sharing one router. The port is reachable over the mesh VPN and locally, but never on the LAN. If **no** tailnet address is present the enable/rebind **fails** with `No Tailscale address found — connect Tailscale or choose a different access scope` and the server stays off — it never silently falls back to `0.0.0.0` (that would re-expose the port the scope exists to hide).
- **`loopback`** → `127.0.0.1:<port>` only — reachable just from this machine (e.g. tunnelled in over SSH).

`bind_addrs(scope, port)` resolves a scope to the concrete `SocketAddr`s; `start_server` binds one listener per address up front (a partial bind failure drops the already-bound listeners, never leaking a half-open server) and serves the same router on all of them behind a shared shutdown signal. `web_remote_enable` rolls `enabled` back to `false` if the bind fails, so the UI never shows "enabled but not running"; `web_remote_set_config` restores the previous scope/port and rebinds to it if a scope/port change can't bind, so a bad change leaves the server on its last-good address rather than off. The scope is surfaced on `WebRemoteStatus.bind_scope` and chosen in Settings → Remote Access ("Who can connect").

Module layout:

- `mod.rs` — managed `WebRemoteState`, config persistence, server lifecycle, the Tauri command surface, the boot-restore + e2e-autostart hooks.
- `server.rs` — the axum router, the connection registry, the WebSocket lifecycle (writer task, keepalive pinger, reader loop), and the shared session-admission gate.
- `auth.rs` — pairing tokens, sessions, WS tickets, the rate limiter, origin checks.
- `dispatch.rs` — invoke dispatch via synthesized `on_message`, plus the channel interceptor/router.
- `events.rs` — the `listen_any` fan-out hub with per-event refcounting.
- `endpoints.rs` — reachable-endpoint enumeration (loopback / LAN / tailnet / MagicDNS), de-noised of Docker/virtual interfaces and grouped for the UI (This device / Local network / Tailscale / Other) with one `recommended` hint.
- `assets.rs` — the authenticated `/api/assets` file route (`convertFileSrc` replacement).
- `proxy.rs` — the auth-gated browser-pane WS + HTTP proxy to loopback `agent-browser` daemons.
- `snapshot.rs` — the authenticated `/api/snapshot` bulk state-bootstrap route (a small versioned client API).

The frontend served is the app's own bundle, resolved through Tauri's `AssetResolver` (assets are embedded in the release binary — there is no `dist/` on disk in production; a debug-only `dist/` fallback keeps `npm run tauri:dev` serviceable). A path with no file extension falls back to `index.html` for client-side routing.

### WS protocol contract

The transport is `GET /ws?ticket=<one-time-ticket>`, upgraded after ticket validation. Both sides (`src-tauri/src/web_remote/{server,dispatch,events}.rs` and `src/remote/{transport,shim}.ts`) match this exactly:

Text frames (JSON):

- C→S `{"t":"invoke","id":<u64>,"cmd":"<name>","args":{...}}`. `Channel` instances serialize (via `@tauri-apps/api`'s `toJSON`) to `"__CHANNEL__:<callbackId>"` strings, minted by the shim's own `transformCallback` registry.
- S→C `{"t":"ok","id":<u64>,"data":<json>}` / `{"t":"err","id":<u64>,"error":<json>}` — id-matched. The server spawns one task per invoke, so responses may return out of order (allowed by design). **Terminal input is the exception:** `write_to_pty` frames are drained through a per-connection serial lane (`is_ordered_invoke`) instead of a per-frame task, so a burst of keystrokes reaches the PTY in the exact order the client sent it — the task-per-invoke concurrency would otherwise let two rapid writes race and scramble the bytes, which the desktop's in-process IPC never does.
- C→S `{"t":"listen","event":"<name>"}` / `{"t":"unlisten","event":"<name>"}`; S→C `{"t":"event","event":"<name>","payload":<json>}`.
- S→C JSON channel body: `{"t":"chan","ch":<callbackId>,"idx":<u64>,"data":<json>}`.

Binary frames (raw channel bodies, e.g. PTY bytes): `[0x01][u32 BE callbackId][u64 BE idx][payload]`. The shim delivers `{index, message}` to the stored callback exactly as the dev mock does — the mock is the reference for what `@tauri-apps/api`'s `Channel` expects.

Keepalive: the server pings every 30s; two unanswered pings closes the socket. On close the shim reconnects with exponential backoff (1s → 16s cap), re-sends its `listen` subscriptions, and re-issues a WS ticket first. In-flight invokes reject on disconnect; the UI's own `get_app_state`-on-mount plus snapshot events self-heal the view.

### Invoke dispatch (Rust ladder, Option 1)

A browser's `invoke` frame is turned into a real `tauri::webview::InvokeRequest` and driven through the main window's webview via `WebviewWindow::on_message` — the same entry point the desktop's own IPC uses (`dispatch::dispatch_invoke`). This reuses argument deserialization, `State`/`AppHandle`/`Window` extraction, ACL resolution, and error formatting for **every one of the app's ~300 commands with zero per-command wiring**. The request reuses the main window's own URL so ACL resolves identically to a desktop-initiated invoke. `on_message` runs on `spawn_blocking` (a sync command runs inline, an async command is handed to Tauri's runtime); a oneshot carries the `InvokeResponse` back to the WS task, which emits the id-matched `ok`/`err` frame. `InvokeResponseBody::Json` → `data`; a panicking command resolves to an `err` frame via the dropped oneshot, never crashing the server.

### Channels and binary PTY frames

A `tauri::ipc::Channel` deserialized inside a synthesized invoke would, by default, post its frames to the *desktop* webview's JS — the wrong client. Tauri exposes a **channel interceptor**, registered on the `Builder` in `lib.rs`, that fires for every channel send with `(callback_id, index, body)`. Before dispatching an invoke, the dispatcher walks the args and rewrites each `__CHANNEL__:<clientId>` marker to a fresh **server-side** channel id, registering a route `server_id → (this WS, clientId)` in `ChannelRouter`. When the command later sends on that channel, the interceptor calls `ChannelRouter::route`, which serializes the body to the owning WS — a `chan` text frame for JSON, the compact `[0x01]…` binary frame for raw bodies — and returns `true` to suppress the desktop delivery.

This is uniform across every channel-taking command (`attach_pty_output`, `attach_agent_chat_output`, …) and needs no knowledge of their signatures, so it is unaffected by concurrent changes to those commands' fan-out internals. A fast-path atomic (`active` route count) lets the desktop-only hot path — local PTY output with no web channels open — skip the router lock entirely and pay only one relaxed load. Routes die with their WS connection (`remove_conn` on socket close).

### Event hub

Browsers subscribe to app events over the WS; `events::EventHub` multiplexes all subscribers onto a single `AppHandle::listen_any` registration **per event name**, reference-counted by subscriber. The first subscriber registers the desktop-side listener; the last to leave (or disconnect) unregisters it. So there is exactly one desktop listener per active event no matter how many browsers watch it, and none once no one is — the desktop's own event bus sees no extra churn. Payloads forward verbatim (re-parsed to a value so they nest rather than double-encode).

### HTTP snapshot bootstrap (`GET /api/snapshot`)

Without help, a freshly-paired (or reconnecting) web client renders blank until a full chain completes: `POST /api/ws-ticket` → `GET /ws` upgrade → React mount → the app's own `get_app_state` invoke over the socket → the reply. `snapshot.rs` collapses the tail of that chain. It exposes one authenticated route, `GET /api/snapshot`, that returns the whole initial state in a single HTTP GET the client fires **in parallel** with the WS handshake, so the UI paints real state as soon as it mounts instead of paying a post-mount round-trip.

The payload is a stable, versioned envelope:

```json
{ "api_version": 1, "app_state": <AppStateSnapshot>, "status": <WebRemoteStatus> }
```

- `app_state` is byte-for-byte what the `get_app_state` command returns — it is produced by the **same** `AppStateStore::snapshot()` call, so there is no second serialization path to keep in sync.
- `status` is the same `WebRemoteStatus` the `web_remote_status` command and the `web-remote-state-changed` event carry (built by the shared `build_status`).
- The response is `Cache-Control: no-store` (a live snapshot must never be served from a cache) and `application/json`.

Admission is the same gate the other authenticated `/api/*` routes use (`server::require_session`): same-origin, an approved non-revoked session by bearer or the HttpOnly cookie. Unauth → 401, cross-origin → 403.

**Seeding is WS-subordinate and stale-safe.** The frontend seam is entirely inside the WebSocket shim — zero app-component changes, so desktop/Tauri and dev-mock paths are byte-identical (they never install the shim, and `fetchSnapshot` is a shim option they never pass). The shim (`src/remote/{snapshot-seed,shim}.ts`) fires the prefetch at install time — concurrently with `transport.connect()` — and answers the app's **first** `get_app_state` from it, but only when the prefetch has already settled and **no** WS-delivered app-state has superseded it. The guard is a monotonic counter (`wsAppStateSeq`), bumped on every `app-state-changed` event the socket delivers: the initial seed is served only while that counter is still `0`, so a WS snapshot that arrived first always wins. The prefetch is never awaited on the hot path (a slow/hung fetch can't delay the first render — the WS invoke covers that), and any failure resolves to `null` and falls back to the WS path silently. On every **reconnect** the shim refetches and pushes the result through the same `app-state-changed` delivery the app already consumes — catching up on state that changed while the socket was down (those events had no subscriber) — again discarded if a fresher WS snapshot lands first. Full-snapshot last-writer-wins makes this safe; the counter guard makes it stale-safe.

**Native-client API seed.** This endpoint is deliberately the first piece of a small, versioned API intended for future **non-web** clients (a native mobile client is the obvious next consumer), not a web-only shortcut. A native client with no WebView and no `@tauri-apps/api` shim can still bootstrap its whole view from one authenticated GET, and `api_version` lets it detect a breaking envelope change (bump it on any incompatible shape change). The web transport happens to be the first consumer; the shape is designed to outlive it.

## Auth model

The trust model is **pairing token → persistent revocable session**, with short-lived tickets keeping the session secret out of WS URLs and logs.

1. **Pairing token** — 32 random bytes, 10-minute TTL, single use, in memory only (`auth::PairingStore`). Minted by the desktop (`web_remote_create_pairing`) and handed to a browser out-of-band as a QR code or a `/#pair=<token>` deep link.
2. **Session** — created when a browser presents a valid pairing token to `POST /api/pair`. Persisted in SQLite (`web_remote_sessions`: id, name, user_agent, `token_hash`, timestamps, `approved`, `revoked`). The session token is SHA-256 hashed at rest; the plaintext is returned to the browser once and never stored. `authenticate` resolves a presented token by constant-time comparing its SHA-256 against **every** active row's hash without early-return, so timing leaks neither which row matched nor whether one did.
3. **WS ticket** — 30-second TTL, single use, in memory (`auth::TicketStore`). A browser trades its session (bearer or cookie) at `POST /api/ws-ticket` for a ticket, then opens `/ws?ticket=…`. Ticket redemption is atomic (remove-then-TTL-check under one lock), so two simultaneous `GET /ws` with the same ticket resolve to exactly one winner. `ws_upgrade` **re-validates** `!revoked && approved` after redeeming the ticket, in case the session was revoked during the ticket's 30s window.

Credentials travel two ways, both funneled through the same `authenticate`: an `Authorization: Bearer <token>` header (the shim's `fetch`), and an `HttpOnly; SameSite=Strict; Path=/` cookie (`cmux_web_session`) for `<img>`/asset GETs that can't set a header. There is no confusion/privilege gap between them.

**Approval mode** (`require_approval`, off by default): a new pairing is inserted `approved = 0` and gets no tickets until the desktop approves it. The desktop's Remote Access pane surfaces pending devices with approve/reject; reject closes any live sockets and deletes the row.

### Account-mode admission (`POST /api/pair-account`)

A second way to mint a `web_remote_sessions` row, alongside the pairing token: a browser on a reachable endpoint (LAN / the user's own Tailscale) signs into the **same Codemux account the desktop is signed into** — no QR dance, no pairing code. This is Stage A of `docs/plans/web-remote-account-mode.md`; the transport is still LAN/Tailscale, only the admission step changes.

**Why the desktop proxies the sign-in.** `api.codemux.org`'s CORS allowlist is fixed (`localhost:1420`, `codemux.org`, `tauri.localhost`, `codemux://`) and does **not** include the dynamic web-remote origin `http://<desktop-ip>:4377`, so the browser cannot call the auth API directly. Instead the browser derives the shared `codemux-api-*` **AuthSecret** client-side (`src/remote/auth-derivation.ts`, byte-identical to `src-tauri/src/auth/derivation.rs` — pinned to the same golden value) and POSTs `{email, auth_secret}` to the desktop's own same-origin `POST /api/pair-account`. The desktop — which already talks to `api.codemux.org` server-side with no CORS constraint — forwards the secret to `/api/auth/desktop/signin`, resolves the presented credentials to a `user.id`, and admits a session **iff** that id equals the desktop's own signed-in user (`load_cached_user`). The raw password only ever exists in the browser; the AuthSecret only ever travels browser → the user's own desktop.

The route (`web_remote::server::pair_account` → `web_remote::account::verify_and_mint`) uses the same origin check + per-IP rate limiter as `/api/pair`, and on success returns `{session_id, session_token, approved}` + the same `Set-Cookie`, so everything downstream (tickets, `/ws`, fan-out, revocation) is unchanged. The minted row is tagged `source = "account"` + `account_user_id` (an additive migration on `web_remote_sessions`); pairing-token rows are `source = "pair"`.

- **Master toggle, default off** (`account_mode_enabled`): `/api/pair-account` returns `403 account_mode_disabled` when off. Account compromise must never reach a device that never opted in. `/api/health` advertises the flag so the browser bootstrap knows whether to offer the account-login screen.
- **Approval defaults ON for account sessions.** Even when `require_approval` is off (it gates the pairing path), an account-minted session starts **pending** — a desktop-side approve click is a cheap circuit-breaker against a compromised account. The explicit opt-out `trust_account_browsers` (default off) admits them immediately.
- **Desktop must be signed in.** Account mode verifies "same account" against `load_cached_user`; if the desktop is signed out, `/api/pair-account` returns `403 account_signed_out` and the Settings pane surfaces the state (`WebRemoteStatus.account_signed_in`).
- **Non-enumerating errors.** Bad credentials → `401 account_auth_failed`; credentials for a *different* account → `403 account_mismatch`. Neither reveals whether an email exists.

On the browser, the pre-app bootstrap (`src/remote/{bootstrap.tsx,account-pair.ts}`) offers "Sign in with your Codemux account" (email + password) when the server advertises account mode, alongside the paste-a-code path, and handles the approval-pending state with the same "waiting for approval" screen the pairing path uses. GitHub-OAuth-in-the-browser is future work (Stage A is email/password only).

### Pairing from the terminal — `codemux remote pair`

When you're away from home you can SSH into the desktop and mint a pairing code from the shell, without opening the GUI. The control socket is same-machine + unauthenticated by design — over SSH you **are** on the machine — so it is the right transport. `codemux remote pair [--name <label>]` sends a `web_remote_pair` control command (`control.rs::dispatch_request`), which calls `web_remote::control_pair`:

- It errors clearly if remote access is off (`Remote access is not enabled — enable it in Settings first`) — the server must be bound for the URL to be reachable.
- Otherwise it mints a one-time token through the **same** shared path the GUI uses (`web_remote::mint_pairing` → `PairingStore::issue_named`; there is no duplicate token logic), and pairs the URL to the endpoint enumeration's single `recommended` pick (MagicDNS → tailnet → local network, falling back to loopback), so the printed URL matches what the Settings pane would show.
- The optional `--name <label>` rides along on the token as a **suggested device name**: the pair handler uses it only as a fallback when the connecting client sends no `device_name` of its own (the bundled web client always derives one), so a device paired from the terminal still shows a friendly name in the desktop list.

The CLI prints a scannable **terminal QR** of the pairing URL (pure-Rust `qrcode` crate, unicode `Dense1x2` renderer — no image deps added), plus the link, raw token, endpoint (kind + secure-context note), and expiry. Scan it with a phone camera or open the link in any browser that can reach the machine. The token is single-use and 10-minute-TTL like any GUI-minted pairing.

**Revocation severs live sockets immediately.** `web_remote_revoke_session` marks the row revoked and calls `ConnectionRegistry::close_session`, which frames a `Close` and trips a per-connection `watch` that unwinds the read loop (releasing its channels + event subscriptions). `web_remote_active_session_hashes` filters `WHERE revoked = 0`, so a revoked token can never re-authenticate.

**Rate limiting**: `POST /api/pair` is capped at 5 attempts per minute per IP, keyed on the real TCP peer (`ConnectInfo<SocketAddr>.ip()`) — not `X-Forwarded-For`, so a spoofed header can't bypass it. A rejected attempt is not recorded, so a backing-off client recovers.

**Origin checks** run on every state-touching route — the `/ws` upgrade, `/api/pair`, `/api/ws-ticket`, `/api/assets`, and both `/proxy/browser/*` routes. A present `Origin` must match the request `Host`; a missing `Origin` is allowed (native clients and same-origin asset GETs don't send one, and CSRF requires a browser that always does). A present-but-mismatched Origin is the cross-site case and is rejected.

## Endpoint enumeration

`web_remote_list_endpoints` (`endpoints::list`) enumerates every URL a browser could use to reach the bound server, so the settings UI can show copy-ready links with an accurate per-endpoint security note. Each entry carries a `kind`, a coarse UI `group`, a `secure` flag, and a single `recommended` hint:

- **loopback** (`127.0.0.1`) — always first, and the one origin a browser treats as a **secure context** over plain HTTP (so clipboard/notifications keep working here). Group: `this_device`.
- **lan** — non-loopback **RFC 1918** private IPv4 on a real local interface. Group: `local_network`. (Marginal addresses — `169.254` link-local IPv4 and non-link-local IPv6 that aren't on the tailnet — are still surfaced, but grouped as `other`.)
- **tailnet** — interface IPs inside the `100.64.0.0/10` CGNAT range, plus every address (IPv4 **and** IPv6) that `tailscale status --json` reports for this node (when the mesh CLI is present). Group: `tailscale`.
- **magicdns** — the node's MagicDNS name, labeled with a hint to enable the mesh's HTTPS serve for a trusted certificate. Group: `tailscale`.

**Virtual interfaces are filtered by name.** Docker bridges (`docker*`, `br-*`), `veth` pairs, libvirt (`virbr*`), Kubernetes CNI plumbing (`cni`/`flannel`/`cali`/`kube`), VirtualBox/VMware host-only nets, ZeroTier (`zt*`), and the Tailscale tunnel (`tailscale*`) are skipped by `is_virtual_iface`, so their `172.x`/`169.254.x`/etc. addresses never masquerade as a reachable endpoint. (The `br-` prefix is hyphenated so a real bridge `br0` survives.) Tailnet addresses come from the CLI rather than the skipped `tailscale0` interface — which is why a Tailscale IPv6 lands under `tailscale`, not `other`.

**Groups drive the UI.** The four groups — `this_device`, `local_network`, `tailscale`, `other` — render as labelled sections in both the "Reachable at" list and the pairing-card device picker; empty groups are dropped and `other` sits behind a default-closed disclosure. Exactly one endpoint is marked `recommended` (the best "reach from anywhere" option: MagicDNS, else a tailnet IP, else a local-network IP; loopback-only leaves nothing marked), surfaced with a "Recommended" chip. The frontend grouping helper (`remote-access-utils.ts`) degrades any unrecognised `group` value into `other`, so a future backend group can't render blank.

Only loopback reports `secure: true`. LAN/tailnet are plain HTTP from the server's point of view — it can't know whether a mesh proxy is terminating TLS in front of it — so they report `secure: false` and the UI surfaces the consequence. Everything degrades gracefully: no interfaces, no `tailscale` binary, or a malformed status blob just yields fewer entries, never an error.

## Settings & device management UX

Settings → Remote Access (`src/components/settings/remote-access-section.tsx`) is the desktop control surface:

- **Master toggle** with a plain-language exposure warning (turning it on lets other devices drive this one).
- **Access scope** ("Who can connect") — a segmented control over `all` / `tailscale` / `loopback` with a one-line explanation for each, wired through `web_remote_set_config({ bindScope })`. Changing it rebinds (the same drop-connections stop→start path a port change uses); switching to `tailscale` with no tailnet address is rejected with a toast and the control snaps back to the last-good scope.
- **Port field** with validation; a port change while running rebinds the listener.
- **Grouped endpoint list** — copy buttons under group headers (This device / Local network / Tailscale, with an `other` disclosure), a per-endpoint security note (secure loopback vs plain-HTTP LAN vs mesh HTTPS), and a **Recommended** marker on the best from-anywhere endpoint, from `web_remote_list_endpoints`.
- **Pairing generator** — a link plus a client-side-rendered **QR code** (`use-qr-svg.ts`) encoding the `/#pair=<token>` URL, with a live countdown to the token's 10-minute expiry.
- **Paired/connected devices list** — name, platform (derived from the user agent), last-seen relative time, and a live-connection dot, with per-device **revoke** and **revoke-all**.
- **Approval-mode toggle** and the **pending-approval flow** — newly pending devices raise a desktop notification + badge and show approve/reject controls.
- **Account access subsection** — the `account_mode_enabled` master toggle (with a note that the desktop must stay signed in, and a warning when it isn't), the `trust_account_browsers` approval opt-out, and account-minted devices tagged **Account** vs **Paired** in the devices list.

Everything live-updates: every server/session state change emits `web-remote-state-changed` on the global bus, which both the desktop pane and any paired web client listen to. Clipboard copy is secure-context aware (it degrades where the browser blocks the Clipboard API on an insecure origin).

Once paired, the web client shows a persistent "Remote — connected to `<host>`" indicator (`src/remote/status-banner.ts`) that degrades to a pulsing amber "Reconnecting…" on a socket drop and to an offline state on revocation (then reloads back to the pairing screen).

## Multi-client semantics

**Mirror mode (v1).** All clients — the desktop window and every paired browser — see the same `AppStateSnapshot`, including `active_workspace_id` and focus. Switching a workspace on the phone switches it on the desktop. Per-client (non-mirrored) views are explicitly out of scope for v1 (a deep state-model refactor; deferred).

**Stream fan-out.** The live streams that used to target a single desktop channel now fan out to N subscribers so the desktop and one or more browsers can watch the same terminal or agent chat at once:

- **Terminal** (`src-tauri/src/terminal/mod.rs`): `SessionRuntime` replaced its single `output_channel` with a subscriber list keyed by generation. `attach_pty_output` returns its generation and replays the `pending_output` ring **only to the new channel**; `detach_pty_output` takes a required generation arg and removes only that subscriber (never clobbering a newer attach). Flow control pauses the PTY reader **only when ALL subscribers request pause** (a per-generation pause set), and an attach clears only its own generation's pause — so a paused-then-detached browser can't park the reader forever, backed by the `FLOW_MAX_PARK` backstop. `dropped_chunks` counts only when there are zero subscribers.
- **Agent chat** (`src-tauri/src/commands/agent_chat.rs`): `AgentChatChannelRegistry` became list-per-thread; attach pushes, detach removes by generation with empty-bucket cleanup, and `forward_event` iterates all subscribers of a thread. No replay is needed — SQLite history covers a late joiner.

The frontend threads the generation token from `attachPtyOutput` through to `detachPtyOutput` on cleanup (`TerminalPane.tsx`). Desktop single-client behavior is byte-identical to before fan-out.

**The desktop window owns scrollback serialization.** Web clients never ack `serialize-terminal-buffers` and never write scrollback files — `use-scrollback-serializer.ts` no-ops when `isRemoteClient()` is true. There is exactly one serialization owner regardless of how many browsers are attached.

## Web fallbacks & parity

The desktop UI assumes affordances a plain browser lacks. Each is bridged behind `isRemoteClient()` (`src/components/remote/is-remote-client.ts`), the single source of truth set by the shim before mount, so desktop behavior is never touched:

- **File/folder picking → in-app path browser.** `pickFolder`/`pickFiles` (`src/lib/file-dialog.ts`) route to a path-browser modal (`src/components/remote/remote-path-picker.tsx`) backed by the existing directory-listing commands, returning the same `absolute path | null` / `path[]` shape the native dialog does. This unlocks **opening new projects and creating workspaces from the browser** — a first-class goal, not an afterthought.
- **Notifications → Web Notifications with toast fallback.** The backend emits a global `notification` event alongside every native desktop notification. On the web client, `use-web-notifications.ts` raises a real OS notification via the Web Notifications API when permission is granted and the tab is hidden, and an in-app toast otherwise. Permission is requested lazily on the first event, never on mount. The desktop ignores the event (the OS notification already fired).
- **`convertFileSrc` → `/api/assets`.** The shim maps a local file path to `GET /api/assets?path=<absolute>` on the server origin; `assets.rs` streams the file back with a guessed MIME type so `<img src>` and editor previews resolve exactly as the desktop `asset:` protocol does. Auth-gated (bearer or cookie + same-origin); directories and non-regular files collapse to a bare 404 (no path leak, no directory listing).
- **Browser-pane proxy.** A remote client can't reach the loopback `agent-browser` daemons (ports 9223–9299), so `proxy.rs` bridges them through the authenticated origin: `GET /proxy/browser/:port/ws` upgrades and pipes the screencast socket frame-for-frame (binary-safe, backpressure-aware), and `ANY /proxy/browser/:port/api/*rest` forwards the daemon's HTTP endpoints. The port is validated strictly against the agent-browser range before any connection, keeping the proxy from becoming a general-purpose loopback forwarder. The HTTP forwarder rebuilds a **minimal** request (request line + `Host` + body headers only, single `write_all`) to respect the daemon's single-segment ~1.4 KB read constraint and to never leak the session's cookies/bearer to the daemon; it rejects any path segment containing an ASCII control character so a percent-decoded `%0d%0a` can't smuggle a header or a second request line into the daemon.
- **Hidden window chrome.** The custom title-bar window controls and drag regions (`window-chrome.tsx`, `title-bar.tsx`) render nothing on the web — there is no OS window to minimize/maximize/close. The shim answers the window/webview plugin calls as no-ops so nothing crashes.
- **Update defer + web-triggered desktop update.** The web client has no updater plugin, so the desktop's `useUpdateChecker` hook is the single updater. When a desktop update is ready it publishes availability to the server (`web_remote_publish_update_available`), which rides `web-remote-state-changed` (and the `web_remote_status` snapshot, so late joiners see it) to paired browsers. A browser can ask the desktop to run its normal download-and-restart flow (`web_remote_request_update` → the `web-remote-update-requested` event the desktop updater listens for; the desktop confirmation UX still applies). Conversely, updates never restart the app on their own — download and restart are explicit user actions — and while any remote device is attached (`connected_sessions > 0`) the desktop's update toast additionally surfaces a "remote devices are connected" hint before the user restarts. The PTY daemon keeps agents alive across a restart; the web client reconnects via the shim's backoff loop.

## Security model and constraints

- **Default off.** Nothing binds until the user enables it; disabling tears the listener down.
- **A paired session has desktop-level control by design.** It *is* the desktop (decision 5 in the plan). The `/api/assets` route is arbitrary-file-read on purpose — the same capability the desktop `asset:` protocol already has. The security boundary is pairing + revocation + the network layer, **not** per-command ACLs. Per-command scopes for paired devices are future work.
- **Plain-HTTP endpoints are not secure contexts.** Only loopback qualifies. On a LAN/tailnet plain-HTTP origin a browser blocks the Clipboard API and can restrict notifications; the settings UI surfaces this per endpoint. For trusted HTTPS the recommended path is the user's **mesh VPN serve feature** (which fronts the plain-HTTP server with a valid certificate) or the user's own reverse proxy — TLS is delegated, not embedded, in v1.
- **No PTY bytes through any cloud service.** Browser ⇄ desktop traffic is direct. There is no relay in v1.
- **Account-mode admission is opt-in and desktop-verified.** With account mode on (default off), a browser can obtain a session by proving it owns the desktop's own Codemux account (`POST /api/pair-account`, above) — but the raw password never leaves the browser (only the derived AuthSecret does, to the user's own desktop), the desktop verifies the resolved `user.id` equals its own signed-in user, and account sessions default to pending approval. GitHub-OAuth-in-the-browser remains future work; Stage A is email/password only. Pairing stays the offline/LAN path and inherits the desktop's account for everything downstream.
- **localStorage session trade-off.** The paired session token lives in the browser's `localStorage`, so pairing survives a refresh without re-scanning a QR. On a shared machine that is a documented trade-off; revoke-from-desktop is the mitigation and severs the socket instantly.
- **DoS hygiene (accepted v1 trade-offs):** the per-connection outbound queue is unbounded and each invoke spawns a task, but both are reachable only by an authed full-control session and the PTY path is flow-controlled; axum caps inbound WS messages at 64 MiB by default. The pairing rate-limiter's per-IP map is not aged-out, but entries are tiny and the population is a LAN/tailnet device set.

## Dev/test affordances

All three are compiled only into debug builds and, even there, dormant unless an env var is set — no release path is weakened:

- **`web_remote::e2e_autostart`** (`#[cfg(debug_assertions)]` + `CODEMUX_WEB_REMOTE_E2E=1`): on boot, enables the server on `CODEMUX_WEB_REMOTE_PORT` (default 4377) with approval off, mints a one-time pairing token, and writes the full pairing URL to the file named by `CODEMUX_WEB_REMOTE_E2E_PAIRING_FILE` (created `0600` so only the launching user can read the token). `restore_on_boot` yields to this hook under the same gate so they never race on the bind. Lets an automated harness drive the served web client without a human clicking inside the native window.
- **`auth::seed_dev_offline_login`** (`#[cfg(debug_assertions)]` + `CODEMUX_DEV_OFFLINE_LOGIN=1`): seeds a cached offline auth user via the normal `save_auth` path so a harness can boot the app without a real account or network. No-ops when a real session is already stored, so it never clobbers a genuine login.
- **`?remote=1` under `npm run dev`**: forces `main.tsx` to install the real WebSocket shim (against a running server) instead of the in-process dev mock, so the production transport can be exercised in a plain browser during development.

## Verification

- **Rust unit/integration** (`cargo test`, `src-tauri`): the `web_remote` suite covers the pairing lifecycle (single-use, TTL, unknown/expired rejection), ticket single-use + binding + expiry, the rate limiter, `authenticate` round-trip + revocation + pending-approval, the origin check matrix, the connection registry (targeted `close_session` frame + signal), the session-admission gate (bearer + cookie, missing/unknown/pending/cross-origin), the channel router (marker rewrite incl. nested, JSON `chan` frame + binary frame shape, per-connection route teardown), the event fan-out, the endpoint enumeration (tailnet/LAN range detection, loopback-first-and-secure, IPv6 bracketing), the asset route (regular-file MIME + length, directory/missing → 404, MIME guessing), the snapshot route (`api_version`, the `{api_version, app_state, status}` envelope shape built from the real `AppStateStore::snapshot`, and its admission: unauth → 401, cross-origin → 403, approved session admitted), the proxy path guards (port range, control-char rejection), the **account-mode admission** (`web_remote::account` — mints on same-user, rejects on user mismatch, 403 when the master toggle is off, pending-vs-approved per the `trust_account_browsers` opt-out, clear error when the desktop is signed out; the `api.codemux.org` signin is mocked via `mockito`, never hit for real) and its error → HTTP status mapping (`server::account_pair_error_response`), the **account config** serde defaults (account mode off, approval on; legacy config loads without the fields) and status snake-case wire contract, the **bind-scope** address resolution (`all`→0.0.0.0, `loopback`→127.0.0.1 only, `tailscale`→tailnet+loopback, no-tailnet→clear error with no silent 0.0.0.0 fallback, unknown→all), the **config bind-scope** serde default (legacy config without the field loads as `all`; snake-case wire contract), the **control-pair** command (errors when disabled and mints no token; when enabled mints a URL embedding a single-use token that pairs via the same `PairingStore` path and carries the `--name` label as a fallback), the pairing suggested-name round-trip, and the CLI's `render_qr` / `print_pairing` formatter. The terminal and agent-chat suites cover the fan-out invariants (two subscribers interleave, detach(A) leaves B streaming, pause(A alone) doesn't stall B, replay reaches only the attacher, registry detach removes only the matching subscriber).
- **Frontend** (`npm run test`): the **account AuthSecret derivation** (`src/remote/auth-derivation.test.ts` — golden-value pins byte-identical to the Rust/Vexis `codemux-api-*` suite, plus determinism, email normalization, and no-password-leak), the **account login flow** (`src/remote/account-pair.test.ts` — the login-screen method decision `resolveAuthMethods`/`initialAuthMode`, the `/api/health` probe, the non-enumerating error-code → copy mapping, and `pairAccount` round-trip including the pending-approval `approved=false` path), the **settings account subsection** (`remote-access-section.test.tsx` — toggle wiring, the signed-out warning, and Account-vs-Paired device tagging), shim contract tests against a fake WS (invoke round-trip, channel ordering, listen refcounting), the transport (reconnect/ticket flow), the snapshot seeding (`fetchSnapshot` envelope validation + failure → `null`; the first `get_app_state` served from the seed with no wire frame; the seed discarded once a newer WS snapshot arrived; clean fall-back when the prefetch fails; reconnect refetch pushed via `app-state-changed`, and discarded if a fresher WS event wins the race), the path-browser utils, the web-notifications delivery matrix, the remote-access settings utils + section (including the bind-scope segmented control defaulting to `all` and switching to `tailscale only`), the `web_remote_set_config` wrapper carrying `bindScope`, and the update-defer decision.
- **Live end-to-end** (Stage 4): the real desktop app (`npm run tauri:dev`) with the server enabled, driven through the Codemux browser pane — pair via link + QR, watch a live terminal (replay + live bytes + input), run an agent-chat turn, open an existing project, create a new workspace via the web path-browser, second-client mirroring (desktop + web at once), revoke mid-session (socket drops), and reconnect behavior.

## Relationship to remote hosts

Web remote access is a **UI transport**: a second frontend for the same backend. Remote hosts (`docs/features/remote-hosts.md`) is a **compute transport**: the same frontend driving a backend on another machine over SSH. They are orthogonal and compose cleanly:

- A paired **web client** issues the same invokes the desktop does, so it transparently sees and drives whatever remote hosts the desktop has configured — pushing a workspace to a host, watching a host-side agent, adopting a sibling workspace — with no web-specific host code.
- The two never share a byte path: web traffic is browser ⇄ desktop (direct), host traffic is desktop ⇄ host (SSH). A future relay/account tier for web access is a strictly additive phase and does not touch the remote-hosts SSH stack.

The clean mental model: remote hosts move *where the work runs*; web remote access moves *where you watch and steer it from*.

## Important Touch Points

- `src-tauri/src/web_remote/` — the embedded server: `mod.rs` (state/lifecycle/commands/boot hooks; `bind_addrs` bind-scope resolution; `mint_pairing` shared token path; `control_pair` for the CLI; `account_mode_enabled` + `trust_account_browsers` config + `account_signed_in` status), `server.rs` (router, `bind(addr)`, connection registry, WS lifecycle, session gate, pair handler's suggested-name fallback, the `pair_account` route + `account_pair_error_response` mapping, `/api/health` account-mode advertisement), `account.rs` (account-mode `verify_and_mint`: same-account verification against `load_cached_user` + session minting), `auth.rs` (pairing/session/ticket/rate-limit/origin; `PairingStore::issue_named`/`consume_named` carry the CLI `--name` label), `dispatch.rs` (invoke via `on_message` + channel router), `events.rs` (`listen_any` hub), `endpoints.rs` (endpoint enumeration + `tailnet_ips()` for the tailscale bind scope), `assets.rs` (`/api/assets`), `proxy.rs` (browser-pane proxy), `snapshot.rs` (`/api/snapshot` bulk state bootstrap).
- `src-tauri/src/auth/derivation.rs` — the shared `codemux-api-*` AuthSecret derivation account mode mirrors client-side; `AuthSecret::from_web_remote_derived` wraps a browser-derived secret arriving over `/api/pair-account`. `src-tauri/src/auth/{mod,api}.rs` — `load_cached_user` (the desktop's signed-in user) + `login_email_api` (the server-side `/api/auth/desktop/signin` account mode reuses).
- `src-tauri/src/database.rs` — `web_remote_sessions` gains `source` + `account_user_id` (additive migration); `web_remote_insert_account_session`.
- `src-tauri/src/control.rs` — the `web_remote_pair` control-socket command (same-machine, drives `web_remote::control_pair`); `src-tauri/src/cli.rs` — the `codemux remote pair [--name <label>]` subcommand + `render_qr` terminal QR (`qrcode` dep in `Cargo.toml`).
- `src-tauri/src/lib.rs` — registers `WebRemoteState`, wires the channel interceptor to `channel_router().route`, calls `restore_on_boot` + `e2e_autostart` in `setup`, and lists the twelve `web_remote_*` commands in `generate_handler!`.
- `src-tauri/src/database.rs` — the `web_remote_sessions` table + its accessors (insert/get/list/active-hashes/touch/approve/revoke/delete).
- `src-tauri/src/terminal/mod.rs`, `src-tauri/src/commands/agent_chat.rs` — the multi-subscriber stream fan-out.
- `src-tauri/src/notifications.rs` — emits the global `notification` event the web bridge consumes.
- `src/main.tsx` — three-way runtime shim selection (real / dev mock / web shim; `?remote=1` dev override).
- `src/remote/` — `bootstrap.tsx` (pairing + account-login screen + connect loop), `account-pair.ts` (account sign-in POST + login-method decision + error-code mapping), `auth-derivation.ts` (client-side `codemux-api-*` AuthSecret derivation via `@noble/hashes`, golden-pinned), `shim.ts` (installs `__TAURI_INTERNALS__`; seeds the first `get_app_state` + reconnect re-seed), `transport.ts` (WS + reconnect + ticket flow), `snapshot-seed.ts` (the `/api/snapshot` prefetch feeding the seed), `session.ts` (localStorage session), `status-banner.ts` (remote indicator), `web-remote-events.ts` (`web-remote-state-changed` helper), `bootstrap-entry.ts`.
- `src/components/remote/` — `is-remote-client.ts` (the `isRemoteClient()` source of truth), `remote-path-picker.tsx` + `-store.ts` + `path-utils.ts` (web file/folder picker).
- `src/components/settings/remote-access-section.tsx` (+ `remote-access-utils.ts`, `use-qr-svg.ts`) — the Remote Access settings pane.
- `src/hooks/use-web-notifications.ts` — Web Notifications / toast bridge.
- `src/hooks/use-update-checker.ts` — desktop updater as the single updater; publishes availability + defers restart while remote devices attach.
- `src/hooks/use-scrollback-serializer.ts` — no-ops on the web client (desktop owns serialization).
- `src/lib/file-dialog.ts` — routes to the web path picker when remote.
- `src/components/layout/{window-chrome,title-bar}.tsx`, `src/components/browser/BrowserPane.tsx` — remote-gated chrome + browser-pane proxy wiring.
- `docs/plans/web-remote-access.md` — the locked design contract, WS protocol reference, and remaining work.

## Notes

- The dev mock (`src/dev/tauri-mock.ts`) is the reference for the `Channel` `{index, message}` delivery shape; keep the production shim's channel dispatch aligned with it.
- Keep the WS protocol frame shapes in `dispatch.rs`/`events.rs` and `transport.ts`/`shim.ts` in lockstep — they are two ends of one contract.
- Current-truth lives here; active next steps (relay/account tier, per-client views, per-command ACL scopes, browser-pane screencast confirmation on a real display, updater UX against a published release) live in `docs/plans/web-remote-access.md`.
