# One-Command Remote Bootstrap (`install.sh` + `codemux connect`)

- Purpose: Track the work that makes remote control setup a two-command story on a fresh
  Linux box or VPS: `curl -fsSL https://get.codemux.org/install.sh | sh` → `codemux connect`.
- Audience: Anyone touching the CLI bootstrap surface (`login`/`connect`), the installer
  script, the endpoint enumeration, or the hosted-tier deploy.
- Authority: Active work plan only, not current truth.
- Update when: A phase lands, a gated deploy happens, or the command surface changes.
- Read next: `docs/features/web-remote-access.md`, `docs/plans/web-remote-account-mode.md`,
  `docs/plans/app-codemux-org-hosting.md`, `docs/features/auth.md`
- Status: MOSTLY LANDED — installer, CLI auth/connect, persistent Linux service, docs, and Ubuntu/Fedora Docker coverage are in-repo; release publication and hosted infrastructure are still gated.

## Goal

A user with a brand-new Linux machine or headless VPS gets from zero to
"reachable from any browser via app.codemux.org" with one install command and one
setup command — no manual release downloads, no GUI requirement, no pairing-token
chicken-and-egg, no manual systemd wiring. The prior-art comparable tools ship this
as a single `npx`-style command; our equivalent is a static installer script on our
own domain plus a `codemux connect` subcommand, which a single Rust binary makes
strictly simpler (no runtime prerequisite, no pinned-runtime reinstall step).

## Active Priorities

1. Gated deploys (human-confirmed, production VPS): serve `install.sh` at
   `get.codemux.org`; apply the API repo's device-registry patch + CORS origin;
   stand up `app.codemux.org` per its runbook; DNS records; then the live smoke test.
2. Release + AUR/website follow-through so real installs fetch a published binary
   containing `login`/`connect` (the code landed after `v0.15.6`).
3. Decide and implement OAuth-friendly device authorization for accounts without a
   password; `login --token` is the current escape hatch.
4. Decide whether macOS launchd and Windows Task Scheduler services belong in the
   first cross-platform bootstrap or remain documented manual `codemux serve` paths.

## Open Questions

- Device-code sign-in: `codemux login` covers email/password; OAuth-only accounts
  (no Better Auth password) currently need `login --token <bearer>`. A proper
  RFC 8628 device-authorization grant needs new API-repo surface — worth it for the
  VPS story, decide alongside the gated API deploy.
- `codemux connect` on macOS/Windows: config-only today (manual `codemux serve`
  fallback message). launchd/Task Scheduler units are follow-up work.
- Public-endpoint exposure posture: `public` endpoints are plain HTTP; whether to
  push harder toward relay-only (loopback bind + iroh) as the recommended VPS shape
  once the hosted tier is live.
- Headless dependency weight: the binary links WebKitGTK even for `serve`; a
  serve-only build profile would shrink VPS installs but adds a release artifact.

## Likely Touch Points

- `scripts/install.sh` (+ `scripts/install-sh.test.sh`) — the installer
- `src-tauri/src/auth/cli_login.rs`, `src-tauri/src/cli.rs` — `login`/`logout`/`whoami`
- `src-tauri/src/web_remote/{mod,serve,registration}.rs`, `src-tauri/src/control.rs` —
  connect bootstrap, relay control command, registration name field
- `src-tauri/src/web_remote/endpoints.rs`, `src/components/settings/remote-access-utils.ts` —
  `public` endpoint kind
- `src/components/settings/remote-access-section.tsx` — relay subsection
- `docs/features/web-remote-access.md`, `docs/features/auth.md`, `docs/reference/CONTROL.md`

## Already Landed

- Endpoint enumeration surfaces globally routable addresses (`kind: public`,
  group `public_internet`); recommendation order MagicDNS → tailnet → LAN → public,
  so `codemux serve` / `remote pair` on a public-IP VPS prints a usable pairing URL.
- `codemux login` / `logout` / `whoami` — standalone headless account sign-in
  (email + client-side AuthSecret derivation; no-echo prompt; `CODEMUX_PASSWORD`
  escape hatch for automation), persisting the same cached auth the GUI writes; a
  running GUI/serve instance picks it up on the next registration tick.
- Settings → Remote Access "From anywhere (relay)" subsection: relay-mode toggle
  wired to `relayModeEnabled`, device-registration readout, signed-out warning.
- `scripts/install.sh` — distro-aware installer (deb/rpm/AppImage-extract paths,
  dependency install, `CODEMUX_ARTIFACT`/`CODEMUX_INSTALL_VERSION` overrides, dry-run).
- `codemux connect` / `connect status` / `connect off` — the one-command bootstrap
  (running-instance vs. `codemux.service` unit branch, linger, rollback), plus the
  `web_remote_set_relay` control command, `login --token`, and the registration
  `name` field surfaced in Settings.
- Network-free installer unit coverage in `scripts/install-sh.test.sh` and a
  Docker/systemd Ubuntu + Fedora end-to-end harness in
  `scripts/e2e/remote-bootstrap/` covering install → login → connect → reachable
  service, idempotent reconnect/reinstall, status, and disconnect.
- Canonical docs for CLI sign-in, one-command bootstrap, and the command sampler in
  `docs/features/auth.md`, `docs/features/web-remote-access.md`, and
  `docs/reference/CONTROL.md`.

## Notes

- Every VPS/DNS change in priority 3 is a gated, confirm-first human deploy — same
  discipline as `docs/plans/app-codemux-org-hosting.md`.
- The e2e harness must never hit the real API: mock `api.codemux.org` inside the
  compose network (the CLI honors `CODEMUX_API_URL` in tests).
- Keep this plan about the bootstrap surface; hosted-tier design stays in
  `docs/plans/web-remote-account-mode.md`.
