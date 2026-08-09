# Hosting `app.codemux.org` — the from-anywhere web client (runbook)

- Purpose: A step-by-step deploy runbook for serving the hosted **from-anywhere** web client at `app.codemux.org`. This is the browser entry point for the account/relay tier: sign in, pick one of your desktops, and drive it from any network over an E2E-encrypted iroh relay.
- Audience: Whoever operates the Codemux control-plane VPS / DNS. This is a **gated, human-run deploy** — nothing here is automated and nothing in this repo deploys it.
- Authority: Active operational runbook for the hosted client. The transport/handshake design it rests on lives in `docs/plans/web-remote-account-mode.md` (Design 1, Stage C); the shipped LAN/mesh contract is in `docs/plans/web-remote-access.md`.
- Update when: The build flags, the static-serve topology, the DNS target, or the API-side CORS/registry dependency changes.
- Read next: `docs/plans/web-remote-account-mode.md`, `docs/features/web-remote-access.md`, and — for the control-plane half — the API repo's `DEPLOY-DEVICE-REGISTRY.md`.
- Status: ACTIVE — code landed on `main`, and the gated human deploy has since happened: `get.codemux.org/install.sh` serves the real installer, `api.codemux.org/health` returns `{"status":"ok"}` with `/api/devices` returning `401` unauthenticated, and `app.codemux.org` serves the hosted client (probed 2026-08-08). An end-to-end from-anywhere round trip against the deployed service has not been re-verified here. The runbook below stays as the record of how the deploy is performed and re-performed.

> **Do not deploy from this document automatically.** Every step below is run by a human with VPS + DNS access. The VPS is never touched by tooling in this repo.

---

## 1. What `app.codemux.org` actually is

`app.codemux.org` serves **static files only** — the same Codemux web client bundle you already ship for the LAN/pairing path, built in *hosted-relay mode*. It is **not** in the data path and never sees plaintext:

- **Sign-in, device discovery, and connect-grants** go to the control plane at `api.codemux.org` (Better Auth session + the `/api/devices*` registry). See the API repo's `DEVICE-REGISTRY.patch` / `DEPLOY-DEVICE-REGISTRY.md`.
- **The actual session** (terminals, agent chat, every `/ws` frame) rides an **iroh** QUIC bi-stream straight from the browser to the user's desktop — hole-punched when possible, otherwise forwarded as **ciphertext** through free public relays. The relay operator, and the `app.codemux.org` static host, only ever move opaque bytes.

So the static host is a dumb, cacheable CDN origin. All trust and all data flow through `api.codemux.org` and the desktop, not through this hostname.

### End-to-end flow

```
browser @ app.codemux.org  (static bundle, hosted-relay mode)
     │  1. sign in            → POST api.codemux.org/api/auth/sign-in/email   (AuthSecret; raw password never leaves the page)
     │  2. list devices       → GET  api.codemux.org/api/devices
     │  3. pick a device
     │  4. mint connect-grant → POST api.codemux.org/api/devices/:id/connect  {browserNonce}
     │  5. dial node_id over iroh (public relays; ciphertext only)
     ▼
desktop (relay mode ON, signed into the SAME account)
        6. hello-account {grant, nonce}  →  verify at api.codemux.org/api/devices/grant/verify
        7. same-account + same-node + nonce checks  →  welcome
        8. the entire existing web client now runs over the iroh pipe, unchanged
```

The desktop must have **Settings → Remote Access → relay mode ON** and be **signed into the same Codemux account**; it self-registers into the device registry so it appears in step 2.

---

## 2. Build the frontend in hosted-relay mode

Two artifacts must be built, in this order, on a machine with the Rust toolchain (the wasm build needs `rustup` + the `wasm32-unknown-unknown` std) and Node.

### 2a. Build the wasm relay client (required)

```bash
npm run build:iroh-wasm        # → scripts/build-iroh-wasm.sh → public/iroh-wasm/
```

This compiles the browser iroh client and stages `public/iroh-wasm/{iroh_wasm.js, iroh_wasm_bg.wasm}` (~2.7 MB raw, ~744 KB brotli). It is **gitignored and never committed**; you build it at deploy time. `vite build` then copies it into `dist/iroh-wasm/`, served at `/iroh-wasm/…`.

If this artifact is **absent**, the site still builds and still loads — a user can sign in and list devices — but the connect step fails with:

> "Relay client unavailable — build it with scripts/build-iroh-wasm.sh, then redeploy."

So building the wasm is mandatory for a working hosted deploy, but a missing wasm can never break the main build (CI stays green without it).

### 2b. Build the SPA with the hosted flag

```bash
VITE_CODEMUX_HOSTED=true npm run build      # → dist/
```

- `VITE_CODEMUX_HOSTED=true` forces the hosted bootstrap branch (`isHostedOrigin()` in `src/remote/hosted.ts`). The client *also* auto-detects the hosted path from the `app.codemux.org` hostname or a `?hosted` query param, so the flag is belt-and-suspenders — but set it so a preview served from any other hostname still behaves as hosted.
- `VITE_CODEMUX_API_BASE` (optional) overrides the control-plane base URL; it **defaults to `https://api.codemux.org`** (`apiBaseUrl()` in `src/remote/hosted.ts`). Leave it unset for production.

The output is a plain SPA in `dist/` (hashed JS/CSS + `dist/iroh-wasm/`). Nothing in it is host-specific beyond the API base.

---

## 3. Static-serve `dist/`

`dist/` is a single-page app. Serve it as static files with:

- **SPA fallback** — serve `index.html` for any path that isn't a real file, so a deep-link / refresh doesn't 404.
- **Correct wasm MIME** — `application/wasm` for `*.wasm` (most servers already do this; verify).
- **Compression** — serve the wasm and JS with **brotli** (or gzip): the wasm is ~744 KB brotli vs 2.7 MB raw. This is the single biggest transfer on first load.
- **Caching** — `Cache-Control: public, max-age=31536000, immutable` for the content-hashed assets under `assets/` and `iroh-wasm/`; `Cache-Control: no-cache` (or a short max-age) for `index.html` so a redeploy is picked up promptly.
- **(If you set a CSP)** — the page makes `fetch` calls to `https://api.codemux.org` and opens WebSocket/WebTransport connections to public iroh relays. A restrictive `connect-src` must allow `https://api.codemux.org` and the relay hosts (`wss:`/`https:`), or connect will be blocked. If in doubt, ship without a custom CSP first.

Pick **one** of the two topologies below.

### Option A — Traefik on the existing VPS

The control-plane VPS already runs Traefik in front of `api.codemux.org`. Add a sibling router for `app.codemux.org` pointing at a tiny static-file container (e.g. `nginx:alpine` or `caddy` serving `/dist`):

- Router rule: `Host(\`app.codemux.org\`)`.
- TLS: reuse the existing Let's Encrypt certresolver.
- Middleware: enable Traefik's `compress` middleware on this router.
- Mount the built `dist/` into the container's web root; redeploy the container on each new build (or bind-mount a directory you `rsync` `dist/` into).

This keeps everything on one box and one TLS setup. The static container shares nothing with the API container beyond the Traefik network — it has no DB, no secrets, no data-path role.

### Option B — Dedicated static host / CDN

Deploy `dist/` to any static host (Cloudflare Pages, Netlify, S3 + CloudFront, etc.):

- Point the host at `dist/` as the publish directory.
- Enable **SPA fallback to `index.html`**, **brotli**, and confirm the platform serves `.wasm` as `application/wasm`.
- Use the platform's managed TLS cert for `app.codemux.org`.

A CDN is the better default for the static bundle (global edge caching of the multi-MB wasm), and it keeps the VPS free of the frontend entirely.

---

## 4. DNS

Point `app.codemux.org` at whichever host you chose:

- **Option A (VPS):** an `A` record `app` → the VPS IPv4 (and an `AAAA` if you serve IPv6). TLS is issued by Traefik on first request.
- **Option B (CDN):** a `CNAME` `app` → the static host's target, then complete the platform's domain-verification + certificate step.

That is the only new DNS record. `api.codemux.org` is unchanged.

---

## 5. Control-plane (API) dependencies — gated, run first

The hosted client is inert until the control plane is ready. Two API-side changes are prerequisites and are covered by the API repo:

1. **Deploy the device registry + grant-verify endpoints.** Apply `DEVICE-REGISTRY.patch` and follow `DEPLOY-DEVICE-REGISTRY.md` in the API repo. This lands `POST /api/devices`, `GET /api/devices`, `POST /api/devices/:id/connect`, and `POST /api/devices/grant/verify`, plus the `codemux_devices` table and the `DEVICE_GRANT_HMAC_SECRET`.

2. **Allow the hosted origin through CORS.** The browser at `https://app.codemux.org` makes **credentialed cross-origin** calls to `api.codemux.org`. Add the origin to the API's trusted-origins allowlist:

   ```
   BETTER_AUTH_TRUSTED_ORIGINS=…,https://app.codemux.org
   ```

   (comma-separated; see `api/src/index.ts` CORS + `api/src/auth.ts`). Without this, sign-in and the `/api/devices*` calls fail the CORS preflight and the picker never loads.

3. **(Optional) Relay-of-last-resort hint.** If you operate/point at a specific iroh relay for symmetric-NAT cases, set `DEVICE_RELAY_URL_HINT` on the API; `/api/devices/:id/connect` surfaces it as `relayUrlHint`. Leave unset to use the built-in public relays.

---

## 5b. Security headers (REQUIRED)

The static host **must** send a Content-Security-Policy and the standard
security headers. Without a CSP, an XSS on `app.codemux.org` could read the
in-memory account bearer and drive any of the account's desktops — a
credential-theft path *around* the (otherwise end-to-end) encryption. The
encryption itself is unaffected; this closes the operational gap.

The deployed nginx config (`~/app-codemux/default.conf` on the host) is:

```nginx
server {
    listen 80;
    server_name app.codemux.org;
    root /usr/share/nginx/html;
    index index.html;

    # Security headers. nginx add_header does NOT merge across levels — a
    # location with its own add_header drops these — so the cache locations
    # below use `expires` (not add_header) to keep inheriting them.
    add_header Content-Security-Policy "default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data: https:; font-src 'self' data:; connect-src 'self' https: wss:; worker-src 'self' blob:; child-src 'self' blob:; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self' https://api.codemux.org; manifest-src 'self'" always;
    add_header Strict-Transport-Security "max-age=31536000" always;
    add_header X-Frame-Options "DENY" always;
    add_header X-Content-Type-Options "nosniff" always;
    add_header Referrer-Policy "no-referrer" always;

    gzip on;
    gzip_comp_level 6;
    gzip_min_length 1024;
    gzip_types application/javascript text/css application/json image/svg+xml application/wasm;

    location ~ \.wasm$ { default_type application/wasm; expires 5m; try_files $uri =404; }
    location /assets/   { expires 1y; try_files $uri =404; }
    location /iroh-wasm/ { expires 5m; try_files $uri =404; }
    location = /index.html { expires -1; }
    location /          { try_files $uri $uri/ /index.html; }
}
```

Notes:
- `script-src 'self' 'wasm-unsafe-eval'` — the bundle ships no inline scripts,
  so `'self'` is enough for JS; `'wasm-unsafe-eval'` is required for the iroh
  WASM transport to instantiate.
- `style-src 'unsafe-inline'` — the built `index.html` has one inline `<style>`
  (splash/reset); style injection is far lower-risk than script injection.
- `connect-src 'self' https: wss:` — permissive on the iroh relay side (the
  relay host set is not fully enumerable and cannot be smoke-tested without a
  live device); the real XSS defense is the tight `script-src`.
- `Strict-Transport-Security` has **no** `includeSubDomains` on purpose, so it
  never forces HSTS onto sibling `*.codemux.org` services.
- After editing, `docker exec app-codemux nginx -t && docker exec app-codemux nginx -s reload`.

---

## 6. Smoke test after deploy

1. On a desktop: sign into the Codemux account, **Settings → Remote Access → enable**, then turn on **relay mode**. Confirm it shows a `node_id` and "device registered".
2. In a browser on a **different network**, open `https://app.codemux.org`.
3. Sign in with the same account → the desktop appears in the picker.
4. Select it → "Connecting" → the full app loads. If the desktop has approval on (default), approve the browser on the desktop first.
5. Open a terminal and type — bytes flow over iroh, not through `app.codemux.org`.

---

## 7. What is still unverified until this is live

- **Real cross-network connectivity + hole-punching / relay fallback** has only been exercised against a mocked verify endpoint and unit/integration tests. First real end-to-end connect happens after this deploy + DNS + the API registry deploy.
- **Throughput / latency** of the terminal + agent-chat stream over a relayed iroh path (vs the LAN WebSocket) is unmeasured; validate on a genuinely NAT'd second network.
- **wasm load cost** on real mobile/edge links — confirm brotli is actually negotiated and the immutable cache headers land.

Record the results of the §6 smoke test and the throughput check here once run.
