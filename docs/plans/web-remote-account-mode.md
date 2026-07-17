# Web Remote — Account Mode & Headless Server Mode (design)

- Purpose: Stage a concrete implementation design for two large future capabilities of web remote access — (1) **account-based reach-from-anywhere** ("sign in with your Codemux account, no Tailscale, drive a brand-new device from any browser") and (2) **headless server mode** (run the full web-remote backend with no GUI, started over SSH).
- Audience: Anyone picking up the relay/account tier or a headless-serve mode. Read after you already know today's web-remote reality.
- Authority: Active design/plan. **Design 1 Stages A, B, and C have landed on `main`** (default-off; the hosted `app.codemux.org` service + `api.codemux.org` device registry are gated human deploys, not yet live) — account-mode admission's current truth lives in `docs/features/web-remote-access.md` § "Account-mode admission", the from-anywhere iroh/hosted tier in § "From-anywhere tier", and its deploy runbook in `docs/plans/app-codemux-org-hosting.md`. **Design 2 (headless server mode) is still design-only.** The locked v1 contract is in `docs/plans/web-remote-access.md`.
- Update when: A stage lands (move it to the feature doc), an open question resolves, or a connectivity/identity decision is made.
- Read next: `docs/features/web-remote-access.md`, `docs/plans/web-remote-access.md`, `docs/features/auth.md`, `docs/plans/mcp-on-remote.md` (the `Identity::Cloud` passthrough + relay layering this reuses), `docs/features/remote-hosts.md`, `docs/features/workspaces-sync.md` (the existing cloud-API sync shape both designs copy).

## Goal

Two gaps motivate this design, both strictly additive to shipped v1 (default-off embedded server, pairing-token → revocable session, LAN + the user's own Tailscale reachability, no cloud in the data path):

1. **Account mode.** A browser that signs into the *same* Codemux account the desktop is signed into becomes an authorized client — from any network, with no pairing-token QR dance and no requirement that the user has set up Tailscale. Pairing stays the offline/LAN path; account sign-in is the from-anywhere path.
2. **Headless server mode.** Run the full web-remote backend — all ~300 commands plus terminal/agent-chat fan-out — on a box with no GUI, started over SSH, so a home server or VPS can be driven from a browser exactly like a desktop.

The two compose: a headless server reachable via account+relay is the full "reach a brand-new device from anywhere" story — no desktop, no LAN, no mesh VPN.

Non-negotiable inherited constraint (locked decision 9): **no plaintext PTY/agent/source bytes through any cloud service we or a third party operate.** This is the single hardest constraint and it drives the entire connectivity recommendation below.

---

# Design 1 — Account-based remote

## 1.1 Identity model

### What exists today

The web-remote trust chain (`src-tauri/src/web_remote/auth.rs`) is: out-of-band **pairing token** (32 bytes, 10-min TTL, single use) → `POST /api/pair` → persistent **session** row in SQLite `web_remote_sessions` (SHA-256 `token_hash` at rest, plaintext returned once) → short-lived **WS ticket** → `/ws`. `authenticate()` constant-time-compares a presented token's hash against every active row. The desktop is separately signed into a Codemux account (`docs/features/auth.md`): a Better Auth bearer token, machine-encrypted at `~/.local/share/codemux/auth-token.enc`, verifiable at `api.codemux.org/api/auth/desktop/verify` which returns `user.id`.

Account (OAuth) sign-in is deliberately desktop-only today; the web client authenticates to a *device*, not an account (decision in `web-remote-access.md` § Security).

### The change: a second way to obtain a session

Account mode does **not** replace the session model — it adds a new way to *mint* a `web_remote_sessions` row. Everything downstream (tickets, WS, fan-out, revocation, origin checks) is untouched. Concretely, a new admission route:

```
POST /api/pair-account
  Authorization: Bearer <the browser's own Codemux account token>
  → desktop resolves the presented token to a user_id
  → iff it equals the desktop's own signed-in user_id: insert a web_remote_sessions row
    (mark source = "account", record account_user_id + device_grant_id)
  → returns {session_id, session_token, approved} + Set-Cookie, exactly like /api/pair
```

Two ways the desktop can verify "same account", in ascending order of what the connectivity stages need:

- **A. Same-account proof (Stage A — LANDED, desktop-proxied sign-in variant).** The obstacle: `api.codemux.org`'s CORS allowlist is fixed and excludes the dynamic web-remote origin `http://<desktop-ip>:4377`, so the browser **cannot** call the auth API directly to obtain or verify a token. So instead of the browser holding its own Better Auth token, the browser collects email + password, derives the `codemux-api-*` **AuthSecret** client-side (identical algorithm to `auth/derivation.rs`; the raw password never leaves the browser), and POSTs `{email, auth_secret}` to the desktop's own same-origin `POST /api/pair-account`. The desktop — which *can* reach the API server-side with no CORS constraint — calls `/api/auth/desktop/signin` with that secret, reads the resolved `user.id`, and compares to `load_cached_user(db).id`. Match → session. Requires the desktop to be online (it almost always is); degrades to pairing token when offline. (A future variant could instead have the browser present a Better-Auth token it obtained through an in-browser OAuth/device-code flow and verify it via `/api/auth/desktop/verify`; email/password was the simplest first UX and needs no new server surface.)
- **B. Control-plane-brokered vouch (Stage B+).** Once a relay/control plane exists, the browser authenticates to the control plane (which already holds its Better Auth session), the desktop authenticates to the control plane as a registered device, and the control plane hands the desktop a signed grant: "this inbound connection is user U, browser device B." The desktop trusts the relay's verdict rather than doing its own token round-trip. **This is exactly the `Identity::Cloud { user_id, org_id, role }` passthrough already reserved in `src-tauri/src/remote/identity.rs` and sketched in `docs/plans/mcp-on-remote.md`** — reuse that enum and its "daemon trusts the relay's verdict, never imports Better Auth itself" discipline.

### What stays as-is

- **Pairing token = offline/LAN path.** No account, no internet required; the air-gapped-LAN and "lend my phone to a colleague on this network" cases keep working.
- **Approval mode composes.** `require_approval` still gates account-minted sessions. **Recommendation: approval defaults ON for account-minted sessions** even though it defaults off for pairing — account compromise (below) is a real threat and a desktop-side approve click is a cheap circuit-breaker. Add an explicit "trust browsers on my account without approval" opt-out.
- **A desktop-side master toggle for account mode, default off**, mirroring the base feature's default-off posture. Account compromise must not reach a device that never opted in.

## 1.2 Connectivity architecture

### The problem

Today reachability = LAN + the user's own Tailscale. From a random coffee-shop network with no mesh VPN, the browser cannot reach a NAT'd home desktop at all — the desktop can't accept inbound, and the browser has no address. Something must let the desktop dial *out* to a rendezvous and the browser reach it *through* that rendezvous (or hole-punch a direct path brokered by it).

### Options considered

| Substrate | Reachability | Who's in the data path | Can the operator read traffic? | Verdict |
|---|---|---|---|---|
| **Cloudflare Tunnel (`cloudflared`) / ngrok** | Excellent, zero user setup | Edge terminates TLS | **Yes — plaintext at the edge** | Rejected. Violates decision 9 for a code tool; third-party dependency + per-tunnel cost. |
| **Self-hosted TCP/WS relay on our VPS, TLS-terminating** | Good | Our relay terminates TLS | **Yes — we could read PTY bytes / source** | Rejected as default. Only acceptable as an explicitly-consented "convenience, we can see your traffic" fallback we recommend against. |
| **Self-hosted relay, blind byte-forwarder + app-layer E2E (Noise inside the relayed WS)** | Good | Relay forwards ciphertext only | No (E2E handshake browser↔desktop) | Viable but this is "reinvent iroh, worse." |
| **Iroh (`n0-computer/iroh`) — pure-Rust QUIC, hole-punching + relay fallback, E2E-encrypted, browser/WASM support** | Excellent, zero user setup | Direct P2P when hole-punch succeeds; our own iroh relay forwards *ciphertext* on fallback | **No — relay sees only encrypted QUIC** | **Primary recommendation.** Rust-native, privacy-preserving, satisfies decision 9. |

### Recommendation

**Primary: an iroh-based E2E substrate with our VPS as discovery + relay-of-last-resort.**

- The desktop becomes an **iroh endpoint** and registers its `NodeId` with the control plane when account mode is enabled.
- The browser connects as an iroh endpoint via iroh's **browser/WASM transport**, reaching the desktop by `NodeId`. Direct hole-punched path when possible; our **self-hosted iroh relay** on the VPS is the fallback for symmetric-NAT cases.
- **Data path is browser↔desktop, E2E-encrypted QUIC.** The relay forwards ciphertext and cannot read or inject frames (mutual NodeId authentication). Decision 9 is satisfied even on the relay fallback path — which the TLS-terminating options cannot claim.
- **The existing `/ws` protocol rides *inside* the iroh stream unchanged.** Iroh replaces only the raw WebSocket *transport*; the `src/remote/transport.ts` ↔ `dispatch.rs`/`events.rs` frame contract (invoke/ok/err/listen/event/chan + binary PTY frames) is byte-identical. This is the whole reason the shim seam is valuable here.

**The control plane is discovery + credential brokering only — never in the data path.** It runs on the infrastructure the project already operates (`api.codemux.org`, the Hetzner VPS, Better Auth + Postgres + Docker + Traefik; see the codemux-api-infrastructure skill). New surface, following the exact pattern of `codemux_workspaces` / hosts-sync (`docs/features/workspaces-sync.md`):

- New Postgres table `codemux_devices` (`user_id` FK → `"user"(id) ON DELETE CASCADE`, `device_id`, `node_id`/`device_pubkey`, `name`, `platform`, `last_seen_at`, timestamps), additive `CREATE TABLE IF NOT EXISTS`, mirrored in `preload.ts`.
- New endpoints, all `authenticateBearer`-gated and `WHERE user_id = $1`-scoped:
  - `POST /api/devices` — desktop registers its `NodeId` + name (authenticated by its own account bearer).
  - `GET /api/devices` — browser lists the account's reachable devices to pick from.
  - `POST /api/devices/:id/connect` — browser requests a short-lived, signed **connection grant** (device `NodeId` + relay hint + the "user U, browser B" vouch of §1.1-B). 404 (not 401) on a device it doesn't own, matching the workspaces-sync anti-enumeration rule.

**Interim (ships without the E2E substrate): Stage A over an already-reachable endpoint.** Before any relay exists, account mode still delivers value as *just the auth improvement*: on the same LAN or the user's own Tailscale, `/api/pair-account` drops the QR dance for account holders. Zero new infra. This is the genuine "simpler interim option" — it needs neither the VPS control plane nor iroh, only the desktop route + a browser account-login screen. If a from-anywhere interim is wanted before iroh is ready, a self-hosted **blind-forwarding** WS relay (ciphertext only, app-layer Noise) is the fallback; a **TLS-terminating** relay should ship, if ever, only behind an explicit "we can see this traffic" consent gate and is not recommended for a code tool.

### Privacy posture summary

- Under the recommendation, **the relay operator (us) cannot read user traffic** — QUIC E2E, relay sees ciphertext. This is the property that lets account mode exist without breaking decision 9.
- The control plane *does* learn metadata: which account owns which devices, connection timing, coarse volume. That is unavoidable for account-scoped discovery and is the same metadata `workspaces_sync` / `hosts_sync` already expose to the VPS.

## 1.3 Security

- **Credential lifetime.** An account-minted `web_remote_sessions` row is the same persistent revocable session as today, but tagged with `account_user_id` + `device_grant_id`. Recommend minting it **short-lived and refreshable while the browser holds a valid Better Auth session** rather than the pairing model's long-lived token, so a stolen session token dies quickly.
- **Proof-of-possession (recommended for the relay tier).** The prior-art comparable tool binds its credentials with DPoP. Mirror it cheaply: have the browser mint a non-extractable WebCrypto keypair and sign each `POST /api/ws-ticket` (and the `/connect` request), so a captured bearer/session token can't be replayed from another device. The existing ticket model already keeps secrets out of URLs; PoP closes the replay gap the relay introduces.
- **Revocation — three levels, all severing live sockets via the existing `ConnectionRegistry::close_session`:**
  1. Desktop revokes the session (existing `web_remote_revoke_session`) — instant socket drop.
  2. Control-plane device de-registration / account sign-out — the browser can no longer broker a `/connect`, and the desktop drops the iroh session bound to that grant.
  3. Better Auth session expiry (30-day) — the browser can no longer prove the account.
- **Device approval reuse.** The pending-approval flow is unchanged; account mode just defaults it ON (§1.1).
- **Composition with origin checks / session gate.** Two things change and must be documented:
  - The UI shell is served from the control-plane origin (e.g. `app.codemux.org`), not `http://<desktop>:4377`. The desktop's same-origin CSRF check (`auth::origin_ok`) exists to stop a malicious site riding an *ambient cookie*. The iroh data channel carries **no ambient cookies** — auth is an explicit in-band bearer/PoP — so the CSRF concern is structurally absent on that transport. Keep `origin_ok` enforced on the HTTP-served `/api/*` endpoints (LAN/Tailscale path) and document that it is moot, not bypassed, on the cookieless iroh channel.
  - The `require_session` admission gate is unchanged: every frame still rides an approved, non-revoked session, whether the transport is a raw WS (LAN) or an iroh stream (relay).
- **Threat model.**
  - *Relay compromise.* Under iroh the relay sees only ciphertext and cannot read or inject `/ws` frames (mutual NodeId auth). Residual: DoS and traffic-analysis (timing/volume). A TLS-terminating relay would instead expose full plaintext — which is precisely why it is not the recommendation.
  - *Account compromise.* An attacker with the account's Better Auth session can broker a connection to a device → full desktop control (a paired session *is* the desktop, decision 5). Mitigations layer: desktop account-mode master toggle (default off) + approval-on-by-default + short-lived PoP-bound sessions + visible connected-device list with revoke. Account compromise alone reaches a device only if that device opted into account mode *and* (if approval on) a human approves.
  - *MITM.* Iroh QUIC is mutually authenticated by NodeId; the browser trusts the NodeId it received over TLS from the control plane inside a signed grant. Forging the path requires compromising `api.codemux.org` (TLS + signed grants) or the desktop's iroh key. No new plaintext-on-the-wire exposure versus today.

## 1.4 Stages, feasibility, effort, risk

| Stage | Ships | Infra | Feasibility | Effort | Risk | Degrades to |
|---|---|---|---|---|---|---|
| **A — Account-verified session issuance ✅ LANDED** | `POST /api/pair-account` on the desktop server; desktop-side account-mode toggle (default off, approval default on) + `trust_account_browsers` opt-out; browser account-login screen in `src/remote/{bootstrap.tsx,account-pair.ts}`; a path that verifies same-user via `/api/auth/desktop/signin` (the desktop-proxied sign-in, since CORS blocks a direct browser call) + `load_cached_user` and mints a `web_remote_sessions` row tagged `source="account"`. Transport still LAN/Tailscale. See `docs/features/web-remote-access.md`. | None (uses existing `api.codemux.org`). | Shipped. | Low–Med | Low | Pairing token (offline). |
| **B — Control-plane device registry + discovery ✅ CODE LANDED (VPS deploy gated)** | `codemux_devices` table + register/list/connect endpoints on the VPS; desktop self-registers `NodeId`/device pubkey when relay mode on (`web_remote/registration.rs`); browser device-picker (`src/remote/device-registry.ts`). Transport still LAN/Tailscale, but the browser now *finds* the device by account. | **VPS deploy (gated, confirm-first).** New table + endpoints + tests, `docker compose up -d --build`, redeploy `preload.ts` parity — see the API repo's `DEPLOY-DEVICE-REGISTRY.md`. Desktop/client code shipped; endpoints not yet deployed. | High — a near-copy of the `codemux_workspaces` endpoints; well-trodden. | Med | Med (schema/deploy discipline; VPS is production). | Stage A (LAN account pairing). |
| **C — E2E connectivity substrate (iroh) ✅ CODE LANDED (relay/host deploy gated)** | Desktop iroh endpoint + `NodeId` registration (`web_remote/iroh.rs`, `iroh = "1"`); browser WASM iroh transport (`src/remote/iroh-transport.ts`, `iroh-wasm-loader.ts`, built via `npm run build:iroh-wasm`) behind the same frame contract; hosted static client `app.codemux.org` (`VITE_CODEMUX_HOSTED`, `src/remote/hosted.ts`) with in-browser GitHub OAuth (`src/remote/hosted-oauth.ts`); iroh's public relays for symmetric-NAT fallback (ciphertext only). Now truly from-anywhere, no user network setup, no plaintext at the relay. | **Gated human deploy of `app.codemux.org` + the control-plane connect/grant-verify endpoints** (`docs/plans/app-codemux-org-hosting.md`). Code shipped; the hosted service is not yet stood up. | Medium — iroh is production-grade but its browser/WASM transport and public-relay fallback are the least-proven pieces here; unverified end-to-end until deployed. | High | Med–High (new crypto/transport surface; relay ops; WASM bundle size in the shim). | Stage B endpoint (LAN/Tailscale) if iroh direct+relay fail; pairing token if the control plane is down. |

**Smallest first stage with user-visible value: Stage A.** "Sign in with your Codemux account on the same network — no QR code" ships with zero infrastructure and no relay, and it is the foundation every later stage builds on.

**Production-infra work is confined to Stages B and C and every VPS deploy is a gated, confirm-first step** (new Postgres tables, new endpoints, and — for C — a new relay service, all on the live `api.codemux.org` box). Follow the codemux-api-infrastructure conventions: additive DDL, `preload.ts` parity, `authenticateBearer` + user-scoping, tests before deploy.

---

# Design 2 — Headless server mode

## 2.1 What's entangled with the GUI, concretely

The shipped web-remote backend is *inside* the Tauri desktop process and its invoke dispatch is webview-coupled by construction:

- **Invoke dispatch routes through the main window's webview.** `dispatch::dispatch_invoke` turns a browser `invoke` frame into a real `tauri::webview::InvokeRequest` and drives it through `WebviewWindow::on_message` — "the same entry point the desktop's own IPC uses" (`web-remote-access.md` § Invoke dispatch). This reuses arg-deserialization, `State`/`AppHandle`/`Window` extraction, ACL resolution, and error formatting for all ~300 commands **with zero per-command wiring** — but it structurally **requires a webview to exist.** This is *the* blocker for a true no-webview backend.
- **`AppHandle` is threaded everywhere.** `app.state::<T>()`, `app.emit(...)`, and the event hub's `AppHandle::listen_any` (`events::EventHub`) all need a live Tauri app. The mcp-on-remote recon counted ~403 Tauri couplings across ~143 files / ~98K LOC of Rust.
- **The channel interceptor** is registered on the `Builder` and fires for every `Channel` send regardless of which window is focused — so PTY/agent-chat fan-out is *not* the blocker; it works as long as the Tauri runtime is up. The blocker is specifically the `on_message` *inbound* dispatch.
- **Genuinely display-bound surfaces**: browser-pane (`agent-browser` daemons need a display), native notifications, tray/menubar, window chrome, the updater plugin. The existing headless daemon already drops `pane_*`/`browser_*` for this reason.

### The existing headless precedent and its ceiling

`src-tauri/src/bin/codemux_remote.rs` (`serve`) is a real headless daemon — axum on loopback, bearer-token manifest, its **own** SQLite workspace registry, its **own** minimal PTY wrapper, and a **hand-written 12-tool catalog** (`workspace_*`, `terminal_*`, `worktree_create`, `app_status`). It proves headless Codemux control is possible — but it is deliberately **not** the full backend. It has no `AppStateStore`, no agent-chat, no browser pane, no ~300-command surface, and no webview. Extending it to full fidelity means re-implementing every command against a shared context — i.e., the deferred `codemux_core` extraction, not a shortcut.

## 2.2 Is a true no-webview full-fidelity backend feasible?

Yes, but only by **re-architecting invoke dispatch off the webview** — you cannot keep `on_message` without a webview, and Tauri exposes no public "invoke a command by name headlessly" API. The re-architecture is the `codemux_core` plan (`docs/plans/mcp-on-remote.md` Steps 1 + 9): lift every command's inner logic out of its `#[tauri::command]` wrapper into a plain function over an explicit `CoreContext` (holding `Arc<AppStateStore>`, `Arc<DatabaseStore>`, `Arc<PtyState>`, … + a `dyn UiNotifier`) plus an `Identity`, then build a **headless command router** that maps `cmd` name → inner fn (no webview). The shipped web-remote server/auth/events/snapshot layers are reusable as-is; only the `on_message` dispatch step is swapped for the router. That work was explicitly sized and **deferred** as a multi-PR effort — and even done, it re-implements the arg-deserialization / State-extraction that `on_message` gives for free today, so it carries real behavioral-drift risk.

## 2.3 The three paths

| Path | What it is | Effort | Risk | Fidelity | Needs a display? |
|---|---|---|---|---|---|
| **1 — True headless (`codemux_core` + headless router)** | Extract all command logic behind `CoreContext`/`Identity`; a webview-free router replaces `on_message`; web-remote server rides on top. | **Very high** (multi-PR; the deferred Steps 1 + 9). | **High** — re-implement + re-verify arg/ACL/State handling for ~300 commands; two dispatch paths to keep from drifting. | Full, and RAM-lean. | No. |
| **2 — GUI-runs-but-hidden, under a virtual display** | Launch the full Tauri app with the main window `visible: false` under Xvfb (or a headless GPU session). The webview still exists (invisibly), so `on_message` dispatch and the entire backend run **unchanged**. A `--serve-web[=port]` flag boots the app, enables web-remote, suppresses tray/updater, shows no window. | **Low–Med** (hidden-window mode + CLI flag + virtual-display launch recipe + server-mode suppression of tray/updater/native-notify → logs). | **Med** — depends on a display server: Xvfb + WebKitGTK works well on Linux servers; macOS/Windows headless webview is harder. RAM cost of a full webview. | **Full** — all 300 commands, terminal + agent-chat fan-out, and Xvfb even gives `agent-browser` a real display so the **browser pane works too**. | Yes (virtual). |
| **3 — Auto-launch path 2 over SSH** | Package Path 2 as the "it just works" SSH flow: the bootstrap provisions Xvfb + a systemd user unit that runs the hidden-window app under the virtual display — mirroring the existing `provision_serve` systemd wiring in `docs/features/remote-hosts.md`. | **Low–Med** on top of Path 2. | **Med** (same display dependency; systemd/Xvfb provisioning). | Full. | Yes (virtual). |

## 2.4 Recommendation

**Ship Path 2/3 (hidden window under a virtual display) as the pragmatic answer; hold Path 1 (`codemux_core`) as the long-horizon target.**

Rationale: Path 2 delivers the **full** web-remote backend — every command, both fan-out streams, and even the browser pane (Xvfb supplies the display `agent-browser` needs) — with essentially **none** of the re-architecture risk, because the webview-coupled `on_message` path is preserved intact. The cost is a webview in RAM plus a virtual display, which is acceptable on a server and far cheaper than a multi-PR extraction. On Linux servers (the target) `xvfb-run` + WebKitGTK is a well-worn recipe; wrap it behind a `--serve-web` flag and an SSH bootstrap that provisions an Xvfb-backed systemd user unit exactly like `provision_serve` does today.

Path 1 remains the "someday, display-free, RAM-lean" goal and is already scoped in `mcp-on-remote.md`. The convergence prize: once `codemux_core` exists, one headless router could serve **both** the MCP tool surface *and* the full web UI, and the `serve` daemon and the desktop backend stop being two implementations — but that is a large, separable effort that should not gate a shippable headless web-remote.

Note the existing `codemux-remote serve` daemon is a precedent for Path 1's *shape*, not a shortcut to full fidelity — reaching 300 commands through it is the `codemux_core` work by another name.

## 2.5 Composition with Design 1

Headless server mode + account mode is the complete "brand-new device from anywhere" story: a headless server (Path 3) registers its `NodeId` with the control plane (Design 1 Stage B/C) and is reachable from any browser via account+iroh, with no GUI, no LAN, no mesh VPN. The headless server is just another web-remote backend behind the same `/ws` contract, so Design 1's transport work applies to it verbatim.

---

## Open Questions

- **Account mode default & approval.** Ship account-mode default-off with approval default-on (recommended)? Or trust-account-browsers by default for frictionless UX? Product call — it trades a compromised-account blast radius against a click.
- **Where the UI shell is served in the relay tier.** Control-plane origin (`app.codemux.org`, recommended) vs desktop-served-over-iroh. Affects origin-check story and CSP.
- **iroh browser/WASM transport maturity.** Needs a spike before committing Stage C: bundle size in the shim, connection-establishment latency, symmetric-NAT relay-fallback reliability. If it's not ready, the blind-forwarding Noise relay is the fallback design.
- **PoP scope.** Is DPoP-style proof-of-possession worth the browser-keypair complexity at Stage A, or defer to the relay tier (Stage C)? Recommend defer, but keep the ticket path shaped for it.
- **Org/team semantics.** Better Auth's `organization` plugin is enabled. Does account mode stay strictly single-user (a browser reaches only devices under its *own* `user_id`), or does it eventually allow org-scoped device sharing? Keep v1 single-user; the `Identity::Cloud { org_id, role }` fields already reserve the shape.
- **Headless on macOS/Windows.** Path 2/3 leans on Xvfb + WebKitGTK (Linux). Is headless-serve a Linux-server-only feature for v1 (likely yes), with Path 1 the only route to display-free macOS/Windows?
- **Device de-dup / naming.** `codemux_devices` cross-device identity and re-registration semantics (a reinstalled desktop, a rotated `NodeId`) — mirror the `origin_uid`/`server_id` discipline from `workspaces-sync.md`.
- **Relay cost/scale.** An iroh relay that only forwards ciphertext on NAT-fallback is cheap (the mcp-on-remote estimate of thousands of idle connections fitting the existing Hetzner box applies), but PTY-heavy sessions that can't hole-punch will push real bytes through it. Budget/QoS is a product decision.

## Likely Touch Points

Account mode (desktop):
- `src-tauri/src/web_remote/auth.rs` — new account-verified admission alongside `authenticate`; session rows tagged `source`/`account_user_id`/`device_grant_id`.
- `src-tauri/src/web_remote/server.rs` — `POST /api/pair-account` route; `require_session` unchanged.
- `src-tauri/src/web_remote/mod.rs` — account-mode config flag + master toggle; device self-registration hook.
- `src-tauri/src/web_remote/endpoints.rs` — add the relay/iroh endpoint kind to enumeration.
- `src-tauri/src/auth/mod.rs` / `auth/api.rs` — reuse `/api/auth/desktop/verify` to resolve a presented token → `user.id`; `load_cached_user`.
- `src-tauri/src/remote/identity.rs` — promote the reserved `Identity::Cloud` onto the web-remote admission path.
- `src/remote/{bootstrap.tsx,transport.ts,session.ts}` — browser account-login screen; iroh transport behind the existing frame contract; device picker.
- `src/components/settings/remote-access-section.tsx` — account-mode toggle, connected-account-devices list, approval controls.

Account mode (control plane — gated VPS deploy, codemux-api repo):
- `api/src/index.ts` — `codemux_devices` DDL (`CREATE TABLE IF NOT EXISTS`) + `POST/GET /api/devices`, `POST /api/devices/:id/connect`; `authenticateBearer` + `WHERE user_id = $1`; 404-not-401 on foreign ids.
- `api/src/tests/preload.ts` + `api/src/tests/devices.test.ts` — DDL parity + auth/isolation/validation tests.
- `docker-compose.yml` — new iroh-relay service (Stage C); Traefik/UDP wiring.

Headless server mode:
- `src-tauri/src/bin/codemux_remote.rs` — precedent + limits (12-tool daemon, no webview, no `AppStateStore`).
- New `--serve-web[=port]` app entry (Path 2): hidden-window boot, enable web-remote, suppress tray/updater/native-notify → logs.
- `src-tauri/src/ssh/bootstrap.rs` — Xvfb + systemd-user-unit provisioning for Path 3, alongside `provision_serve`.
- (Path 1, deferred) `codemux_core` extraction per `docs/plans/mcp-on-remote.md` Steps 1 + 9 — `control.rs`, `state.rs`, `terminal/`, `mcp_server.rs`, `CoreContext`, `UiNotifier`, `Identity`.

## Already Landed

**Design 1, Stage A — Account-verified session issuance.** Shipped: `POST /api/pair-account` on the desktop server (`web_remote::server::pair_account` → `web_remote::account::verify_and_mint`); the desktop-side `account_mode_enabled` master toggle (default off) + `trust_account_browsers` approval opt-out (approval default **on** for account sessions); the browser account-login screen in `src/remote/{bootstrap.tsx,account-pair.ts}`; the client-side `codemux-api-*` AuthSecret derivation (`src/remote/auth-derivation.ts`, golden-pinned byte-identical to the Rust/Vexis peer); same-user verification via `/api/auth/desktop/signin` + `load_cached_user`; `web_remote_sessions` rows tagged `source="account"` + `account_user_id`; the Settings → Remote Access "Account access" subsection (toggle, opt-out, signed-out warning, Account-vs-Paired device tags). Transport stays LAN/Tailscale. Current truth now lives in `docs/features/web-remote-access.md` § "Account-mode admission".

**Design 1, Stages B + C — device registry + iroh from-anywhere transport (CODE LANDED, hosted service not yet deployed).** The desktop-side and client-side code for the whole from-anywhere tier has merged to `main` (all default-off, gated by a relay-mode toggle): the desktop as an iroh QUIC endpoint that self-registers its `NodeId` with the `api.codemux.org` device registry (`src-tauri/src/web_remote/{iroh,registration}.rs`, `iroh = "1"`); the hosted static client at `app.codemux.org` (`VITE_CODEMUX_HOSTED`, `src/remote/hosted.ts`, `hosted-bootstrap.tsx`) that signs in with GitHub OAuth (`src/remote/hosted-oauth.ts`) or email/password, lists the account's devices (`device-registry.ts`), mints a connect-grant, and dials the chosen device over an E2E-encrypted iroh stream (`iroh-transport.ts`, `iroh-connection.ts`, `iroh-wasm-loader.ts`, `iroh-codec.ts`) that carries the unchanged `/ws` frames; iroh's public relays forward ciphertext for symmetric-NAT fallback. **Still gated:** the `app.codemux.org` static host and the `api.codemux.org` device-registry + grant-verify endpoints are human-run deploys (`docs/plans/app-codemux-org-hosting.md` + the API repo's `DEPLOY-DEVICE-REGISTRY.md`), so real cross-network connectivity is unverified until the first live deploy. Current truth for the shipped code lives in `docs/features/web-remote-access.md` § "From-anywhere tier".

The **chosen account-proof design was option A** (desktop-proxied token/secret presentation), because `api.codemux.org`'s fixed CORS allowlist excludes the dynamic web-remote origin — the browser cannot call the auth API directly, so it derives the AuthSecret locally and lets the desktop (which already talks to the API server-side) do the sign-in and same-user check. The raw password never leaves the browser. Better Auth's device-code flow (option B) was rejected for Stage A as clunkier UX with no offsetting benefit at the LAN/Tailscale tier.

Foundation this builds on, already shipped on this branch:
- Web-remote v1: embedded default-off axum server, pairing→session→ticket→`/ws`, multi-client fan-out, endpoint enumeration, `/api/snapshot` (`docs/features/web-remote-access.md`).
- The `codemux-remote serve` headless daemon with its 12-tool surface, manifest auth, and the reserved `Identity::Cloud` enum (`docs/features/remote-hosts.md`, `docs/plans/mcp-on-remote.md`).
- The account system + `api.codemux.org` control plane and its sync-table pattern (`docs/features/auth.md`, `docs/features/workspaces-sync.md`, codemux-api-infrastructure skill).

## Notes

- Keep the `/ws` frame contract (`dispatch.rs`/`events.rs` ↔ `transport.ts`/`shim.ts`) as the stable seam: every connectivity substrate here (raw WS, iroh stream, relayed WS) is a transport swap *under* that contract, never a change *to* it. That invariant is what makes all of this additive.
- Decision 9 ("no plaintext through any cloud service") is the load-bearing constraint. It rules out the easy tunnels and is the sole reason iroh (E2E, relay-sees-ciphertext) is the recommendation over `cloudflared`/ngrok.
- Every VPS change is a gated, confirm-first production deploy — treat Stages B/C infra work as such.
- This file is next-steps only. When a stage lands, move its behavior into `docs/features/web-remote-access.md` and trim it here.
