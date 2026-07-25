# Web Remote Access — Implementation Plan (locked spec)

- Purpose: Track the remaining work on web remote access and preserve the locked design contract. Stages 1–4 have landed; current-truth lives in `docs/features/web-remote-access.md`.
- Audience: Anyone changing this area. The design decisions + WS protocol contract below are the canonical wire spec both ends must match.
- Authority: Locked design contract + remaining-work tracker. Current behavior is canonical in `docs/features/web-remote-access.md`.
- Update when: A contract changes, a deferred item is picked up, or an open question is resolved.
- Read next: `docs/features/web-remote-access.md`, `docs/features/remote-hosts.md`, `docs/features/dev-mock-runtime.md`, `docs/features/terminal.md`, `docs/features/agent-chat.md`
- Status: ACTIVE — v1 LAN/pairing base shipped in `v0.14.0`. Deferred: per-client (non-mirrored) views, per-command ACL scopes, live browser screencast, and updater follow-ups.

## Goal

The desktop app grows an embedded, default-off HTTP+WebSocket server. A browser on another machine (laptop, phone) loads the same UI bundle and drives the same running instance — same projects, sessions, terminals, agent chats. Work continues when the browser disconnects (PTY daemon already guarantees this). Reachability v1: LAN + the user's own tailnet (mesh VPN detected, not embedded), with trusted HTTPS via the mesh's serve feature. A relay/account tier is a later, strictly additive phase.

This is a **UI transport** (second frontend for the same backend). The existing remote-hosts system is a **compute transport** (same frontend, remote backend). They compose: a web client automatically sees and drives remote hosts because all of that logic lives behind the invoke surface.

## Design decisions (locked)

1. **The seam is `window.__TAURI_INTERNALS__`.** The dev mock (`src/dev/tauri-mock.ts`) proves the entire UI runs in a plain browser through a 5-member internals object + `window.__TAURI_EVENT_PLUGIN_INTERNALS__`. The web client installs a *real* shim backed by a WebSocket instead of fixtures. Zero changes to app components.
2. **Server lives in the desktop app process** (module `src-tauri/src/web_remote/`), axum. It serves the frontend bundle via Tauri's `AssetResolver` (assets are embedded in the binary in production — there is no `dist/` on disk) and exposes `/api/*` + `/ws`.
3. **Default off. Nothing binds until the user enables it.** When enabled, bind `<port>` (default port 4377, configurable) on the interfaces the **bind scope** selects — `all` (`0.0.0.0`, default) / `tailscale` (tailnet + loopback) / `loopback` (127.0.0.1 only). Disable tears the listener(s) down. `tailscale` with no tailnet address fails the enable rather than falling back to `0.0.0.0`.
4. **Auth: one-time pairing token → persistent revocable session.** Sessions in SQLite, token hashed (SHA-256) at rest, constant-time compare. WS connects via short-lived single-use tickets so session tokens never appear in WS URLs. Optional approval mode: new pairings sit pending until approved on the desktop.
5. **Full command surface for authenticated sessions.** A paired device has desktop-level control by design (it IS the desktop). Security boundary is pairing + revocation + network layer, not per-command ACLs (future work).
6. **Multi-client is mirror-mode v1.** All clients see the same `AppStateSnapshot` including `active_workspace_id`/focus. Per-client views are explicitly out of scope for v1.
7. **Desktop window is the scrollback-serialization owner.** Web clients never ack `serialize-terminal-buffers` and never write scrollback files.
8. **TLS is delegated to the mesh serve feature or the user's own proxy in v1.** The settings UI surfaces the secure-context consequence (clipboard etc. degrade on plain HTTP).
9. **No PTY bytes through any cloud service, ever.** Browser ⇄ desktop traffic is direct.

## WS protocol contract (both sides MUST match this exactly)

Endpoint: `GET /ws?ticket=<one-time-ticket>` → upgrade after ticket validation.

Text frames (JSON):

- C→S `{"t":"invoke","id":<u64>,"cmd":"<name>","args":{...}}` — args are JSON-serialized by the shim; `Channel` instances serialize (via `@tauri-apps/api` `toJSON`) to `"__CHANNEL__:<callbackId>"` strings; the shim's own `transformCallback` registry issues the ids.
- S→C `{"t":"ok","id":<u64>,"data":<json>}` and `{"t":"err","id":<u64>,"error":<json>}` — id-matched, out-of-order responses allowed (server spawns a task per invoke).
- C→S `{"t":"listen","event":"<name>"}` / `{"t":"unlisten","event":"<name>"}`; S→C `{"t":"event","event":"<name>","payload":<json>}`. Server implements this with `app.listen_any(name)` registered on first subscriber, unregistered on last.
- S→C channel data (JSON bodies): `{"t":"chan","ch":<callbackId>,"idx":<u64>,"data":<json>}`.

Binary frames (raw channel bodies, e.g. PTY bytes): `[0x01][u32 BE callbackId][u64 BE idx][payload]`. The shim delivers `{index, message}` to the stored callback exactly as `src/dev/tauri-mock.ts` does (that mock IS the reference for what `@tauri-apps/api` `Channel` expects).

Keepalive: server pings every 30s; missed pong ×2 closes. On close, the shim reconnects with exponential backoff (1s → 16s cap), re-sends `listen` subscriptions, and re-issues a ticket first (`POST /api/ws-ticket`). In-flight invokes reject on disconnect; the UI's own `get_app_state`-on-mount + snapshot events self-heal state.

## HTTP surface

- `GET /*` — static UI bundle via `AssetResolver` (public; the bundle is not a secret; all state is behind auth).
- `POST /api/pair` `{token, device_name}` → `{session_id, session_token, approved}` + `Set-Cookie` (HttpOnly, SameSite=Strict, for `<img>`/asset GETs). Rate-limited (5/min/IP). One-time tokens: 32 random bytes, 10 min TTL, single use, in-memory.
- `POST /api/ws-ticket` (Bearer or cookie) → `{ticket}` (30s TTL, single-use).
- `GET /api/snapshot` (auth required) → `{api_version, app_state, status}`, `Cache-Control: no-store`, `application/json`. One-shot bulk state bootstrap: `app_state` is the exact value `get_app_state` returns (same `AppStateStore::snapshot()` path), `status` is the `WebRemoteStatus` payload. The web client fetches it **in parallel** with the WS handshake to seed the first render; also the first piece of a versioned API for future native (non-web) clients (bump `api_version` on a breaking envelope change).
- `GET /api/assets?path=<abs>` (auth required) — replacement for `convertFileSrc`; streams a regular file, non-files → 404.
- `GET /proxy/browser/:port/ws` + `ANY /proxy/browser/:port/api/*rest` (auth required) — browser-pane proxy to the loopback agent-browser daemons; port validated against the 9223–9299 range, control chars rejected, minimal request rebuilt for the daemon's single-segment read.
- `GET /api/health` (public, version only).
- Origin checks on WS upgrade and every state-touching `/api/*` + `/proxy/*` route: Origin host must match request Host (same-origin client only); missing Origin allowed (non-browser clients).

## Rust dispatch strategy (Lane A — shipped as Option 1)

The ~300 commands are registered once in `src-tauri/src/lib.rs` (`generate_handler!`). The web dispatcher builds a real `tauri::webview::InvokeRequest` and drives it through the main window's webview via `WebviewWindow::on_message` — reusing arg deserialization, `State`/`AppHandle`/`Window` extraction, ACL resolution, and error formatting for every command with zero per-command wiring. A `Builder`-registered channel **interceptor** + `ChannelRouter` re-route each command's `Channel` frames to the owning browser (JSON → `chan` frame, raw → binary frame); this is uniform across every channel-taking command and needs no knowledge of their signatures.

## Already landed

Stages 1–4 plus an adversarial review pass have shipped on this branch (all verified: `cargo check`/`test`, `npm run check`/`test`/`build`, plus a live end-to-end drive). Canonical behavior is in `docs/features/web-remote-access.md`.

- **Stage 1 — Foundation.**
  - *Rust server* (`src-tauri/src/web_remote/`): `mod.rs` (state/config/lifecycle/commands/boot-restore), `server.rs` (axum router + connection registry + WS lifecycle + session gate), `auth.rs` (pairing/sessions/tickets/rate-limit/origin), `dispatch.rs` (invoke via `on_message` + channel router), `events.rs` (`listen_any` hub), `endpoints.rs` (loopback/LAN/tailnet/MagicDNS enumeration). SQLite `web_remote_sessions` migration + accessors. Twelve `web_remote_*` commands registered in `generate_handler!`; config persists and `restore_on_boot` re-binds on boot.
  - *Frontend shim* (`src/remote/`): `transport.ts` (WS + reconnect + ticket flow), `shim.ts` (installs `__TAURI_INTERNALS__` + `__TAURI_EVENT_PLUGIN_INTERNALS__`), `bootstrap.tsx` (pairing screen + connect loop), `session.ts`, `status-banner.ts`. `main.tsx` selects real-IPC / dev-mock / web-shim three ways; `?remote=1` dev override.
  - *Stream fan-out*: terminal `SessionRuntime` moved to a per-generation subscriber list (targeted replay, ALL-subscribers-pause flow control, generation-scoped detach); `AgentChatChannelRegistry` moved to list-per-thread. Desktop single-client behavior byte-identical.
- **Stage 2 — Settings & device management UI** (`src/components/settings/remote-access-section.tsx` + `remote-access-utils.ts` + `use-qr-svg.ts`): master toggle + exposure warning, port field, endpoint list with copy + per-endpoint security note, pairing generator with client-side QR + expiry countdown, paired-devices list with revoke/revoke-all, approval-mode toggle + pending-approval flow, live `web-remote-state-changed` updates, secure-context-aware clipboard, and the "Remote — connected to `<host>`" indicator.
- **Stage 3 — Web fallbacks & parity**: web path browser (`src/components/remote/remote-path-picker.tsx`) unlocking remote project/workspace creation; Web Notifications + toast bridge (`use-web-notifications.ts`); `convertFileSrc` → `/api/assets` (`assets.rs`); browser-pane proxy (`proxy.rs`); hidden window chrome; desktop-as-single-updater with defer-while-remote + web-triggered restart (`use-update-checker.ts`, `web_remote_publish_update_available` / `web_remote_request_update`). All gated on `isRemoteClient()`.
- **Stage 4 — End-to-end verification**: live drive through the Codemux browser pane against `npm run tauri:dev` — pair (link + QR), live terminal (replay + input), agent-chat turn, open project, create workspace via the web path-browser, desktop+web mirroring, revoke-mid-session socket drop, reconnect.
- **Adversarial review pass**: hardened the browser-pane proxy against HTTP request/header (CRLF) injection on the percent-decoded catch-all path (`has_control_char` + 400 guard + regression test); deduped three inline `isRemoteClient()` copies to `src/components/remote/is-remote-client.ts`.
- **HTTP snapshot bootstrapping.** Added `GET /api/snapshot` (`src-tauri/src/web_remote/snapshot.rs`) — an authed, versioned `{api_version, app_state, status}` envelope built from the **same** `AppStateStore::snapshot()` the `get_app_state` command uses (no duplicated serialization) with `Cache-Control: no-store`. The web shim (`src/remote/{snapshot-seed,shim}.ts`) prefetches it **concurrently** with the WS ticket + upgrade and answers the app's first `get_app_state` from it, so the UI paints without a post-mount socket round-trip. WS stays the source of truth: the seed is served only while a monotonic `wsAppStateSeq` (bumped on each `app-state-changed` event) is still `0`, so a WS-delivered snapshot always wins and no stale seed can overwrite newer state; the prefetch is never awaited on the hot path and any failure falls back to the WS path silently. On every reconnect the shim refetches and re-applies via the `app-state-changed` path (discarded if a fresher WS event lands first) to catch up on state missed during the outage. Desktop/Tauri and dev-mock paths are byte-identical (the `fetchSnapshot` shim option is only supplied by the web bootstrap). Deliberately shaped as the first piece of a versioned API for future native (non-web) clients. Tests: Rust snapshot route (`api_version` + envelope shape + 401/403/200 admission); TS seeding (`fetchSnapshot` validation/failure, seed-when-pending with no wire frame, discard-when-superseded, clean fall-back, reconnect re-seed + stale-race discard). Verified: `cargo check`/`test`, `npm run check`/`test`/`build` all green.

### Hardening — CLI pairing over SSH + Tailscale-only bind scope (landed)

Two related hardening features shipped on this branch; canonical behavior is in `docs/features/web-remote-access.md` (§ "Bind scope", § "Pairing from the terminal").

- **`codemux remote pair [--name <label>]`.** Mint a web-remote pairing code from a shell — the away-from-home SSH flow, no GUI needed. The control socket is same-machine + unauthenticated by design, so over SSH it is the right transport. A new `web_remote_pair` control command (`control.rs`) drives `web_remote::control_pair`: it errors if remote access is off (`Remote access is not enabled — enable it in Settings first`), else mints a one-time token through the **shared** `mint_pairing` path (no duplicate token logic) and pairs it to the endpoint enumeration's `recommended` pick. The CLI prints a scannable **terminal QR** (pure-Rust `qrcode`, unicode renderer — no image deps) plus link/token/endpoint/expiry. `--name` rides on the token as a fallback device label (`PairingStore::issue_named`/`consume_named`), used by the pair handler only when the client sends no name. Tests: control-pair disabled→error / enabled→usable single-use token that pairs via the existing path; suggested-name round-trip; `render_qr`/`print_pairing`.
- **Bind scope (`bind_scope`).** New persisted config field (`all` default | `tailscale` | `loopback`), `#[serde(default)]` so old configs load as `all`. `bind_addrs(scope, port)` resolves it to concrete addresses; `start_server` binds one listener per address (partial-failure-safe) sharing one router + shutdown signal. `tailscale` binds the tailnet IP(s) from `endpoints::tailnet_ips()` **plus** loopback; with no tailnet address it fails the enable with `No Tailscale address found — …` and keeps the server off (never a silent `0.0.0.0` fallback). `web_remote_enable` rolls `enabled` back on bind failure; `web_remote_set_config` accepts `bind_scope`, validates it, rebinds on change (same drop-connections path as a port change), and restores the last-good scope/port if the new one can't bind. Surfaced on `WebRemoteStatus.bind_scope` and chosen via a Settings segmented control ("Who can connect"). Tests: bind-scope resolution matrix, serde default, status wire contract, settings control + wrapper.

### Headless server mode — `codemux serve` (landed, issue #176 Phases 1–3)

The headless server mode from the deferred-work list has shipped; canonical behavior is in `docs/features/web-remote-access.md` § "Headless server mode".

- **`codemux serve [--scope all|tailscale|loopback] [--port N] [--relay]`** (`src-tauri/src/web_remote/serve.rs`: `ServeOptions` + `run_serve`). Runs Codemux as a web-remote server with no GUI — the SSH-in-with-no-display flow. Boots the full backend headless on `MockRuntime` (`codemux_lib::build_headless_app()` — the whole backend is generic over `tauri::Runtime`, with display-coupled pieces gated on `AppMode::ServeHeadless`), enables the server through the **shared** `control_enable` path (bind-scope validation, rollback, config persistence, `web-remote-state-changed` all identical to the GUI/CLI), and — with `--relay` — flips `relay_mode_enabled` + starts the iroh endpoint via the same `web_remote_set_config` path. Scope defaults to `all`, or the persisted scope when remote access was already configured (`restore_on_boot`'s `already_running` handled). Prints the bound port/scope/endpoints banner + a pairing QR via the shared `print_pairing`/`render_qr`, then blocks on SIGINT/SIGTERM and tears down with `RunEvent::Exit` parity. `main` dispatches it on the main thread via `CliOutcome::RunServe` (not `block_on(maybe_run_cli())`) since it is a long-lived foreground process. Mutual exclusion with the GUI is symmetric (both refuse when the other holds the control endpoint). Debug builds warn when no web bundle (embedded or source-tree `dist/`) is available. Tests: `tests/serve_headless_dispatch.rs` (boot + dispatch with no display), `tests/serve_web_remote_roundtrip.rs` (pair → ws-ticket → `/ws` invoke → `ok` frame over real HTTP/WS, GUI-free).

## Remaining work (deferred, not blocking v1)

- **Relay / account tier (post-v1).** Optional cloud relay so a browser can drive the desktop without being on the same LAN/tailnet. Strictly additive — no PTY bytes through the relay (decision 9). Protocol substrate candidates researched (Rust-native E2E QUIC w/ hole punching vs a managed tunnel); decision deferred. Account (OAuth) sign-in stays desktop-only; the relay forwards a *verified device*, not an account login.
- **Per-client (non-mirrored) views.** v1 is mirror-mode (decision 6): all clients share `active_workspace_id`/focus. Independent per-client focus/view state is a deep state-model refactor, explicitly deferred.
- **Per-command ACL scopes for paired devices.** v1 is all-or-nothing by design (decision 5) — a paired device has full desktop control. Scoped/read-only pairings are future work.
- **Browser-pane screencast confirmation on a real display.** The browser-pane proxy is wired and unit-tested, but the end-to-end screencast + input round-trip through the proxy still wants a confirmation pass against a real `agent-browser` daemon on a real display (the Stage-4 drive exercised terminals + chat, not the browser pane live).
- **Updater UX against a published release.** The defer-while-remote + web-triggered-restart flow is implemented and unit-tested, but the full "web client asks the desktop to update, desktop downloads a *published* release + restarts, web client reconnects via backoff" loop still wants a live pass once there is a release to update *to*. Residual risk (a new build fails to boot while the user is remote) is documented; an optional boot watchdog is future work.

## Update-while-remote policy (shipped)

Remote sessions active → auto-update restart is deferred by default; the web client can explicitly trigger "update and restart" (PTY daemon keeps agents alive across the restart; web client reconnects via backoff). Residual risk (new build fails to boot while abroad) is documented; an optional watchdog is future work. See "Updater UX against a published release" above for the remaining live pass.

## Likely touch points

See `docs/features/web-remote-access.md` § "Important Touch Points" for the full map. Reference evidence for future work: `src/dev/tauri-mock.ts` (shim/channel contract), `src-tauri/src/web_remote/*` (server + auth + dispatch + proxy), `src-tauri/src/remote/{server,auth,manifest}.rs` (the remote-hosts daemon's axum + bearer patterns, and the reserved `Identity::Cloud` variant the relay tier would reuse).
