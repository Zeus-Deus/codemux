# Codemux Testing

- Purpose: Canonical verification strategy and testing policy.
- Audience: Anyone implementing or validating changes.
- Authority: Testing layers, commands, and manual-validation guidance.
- Update when: Verification commands, tooling, or testing philosophy change.
- Read next: `docs/core/STATUS.md`, relevant feature docs

## Default Commands

- `npm run verify`: full default verification pass
- `cargo check --manifest-path src-tauri/Cargo.toml`: Rust compile check
- `cargo test --manifest-path src-tauri/Cargo.toml`: Rust backend and state tests
- `npm run check`: TypeScript type checks (`tsc --noEmit`)
- `npm run test`: frontend tests

Default to `npm run verify` after meaningful work. Use the narrower commands when iterating on one layer.

`npm run verify` is exactly `cargo check && cargo test && npm run check && npm run test` — it does **not** cover the Claude sidecar. The sidecar is a separate Bun package with its own suite (`sidecar/claude-agent/test/*.test.ts`: session, permissions, MCP bridge, respond-to-request, ping, real-tools). Run it directly with `bun test` from `sidecar/claude-agent/` whenever you touch `sidecar/claude-agent/src/` — the session-lifecycle, permission-mode, and stale-resume recovery behavior documented in `docs/features/agent-chat.md` is enforced there and nowhere else.

`npm run verify` also does not run the shell installer suite. Run `bash scripts/install-sh.test.sh` whenever `scripts/install.sh` or its artifact/distro-selection contract changes; the suite is network-free and performs no installation.

## Visual Verification (UI work)

When iterating on UI, verify visually against the real React UI running in a browser pane:

1. `npm run dev` — boots Vite at `http://localhost:1420` with the dev-only Tauri mock auto-installed.
2. `codemux browser open http://localhost:1420` — the real Codemux UI loads with seed data.
3. `codemux browser screenshot` — capture visual proof.

The mock lives in `src/dev/` and only loads when no real Tauri runtime is detected (dual-guarded in `main.tsx`), so it never ships in production and stays dormant under `npm run tauri:dev`. It is a fixture/in-memory backend — not a real one — so terminals, git, agents, and other Rust-backed behavior are not exercised this way; use `npm run tauri:dev` (desktop window, not browser-pane-visible) for real-IPC testing. See `docs/features/dev-mock-runtime.md`.

## Testing Layers

- Rust domain tests for workspaces, pane trees, terminal lifecycle, persistence, notifications, memory, indexing, providers, and workflow state
- Bun tests in `sidecar/claude-agent/test/` for the Claude Agent SDK sidecar (not run by `npm run verify`)
- frontend interaction tests for important workspace, pane, and browser flows
- focused end-to-end coverage later for a few critical workflows rather than every UI detail

## End-to-End Harnesses (manual, off by default)

- `scripts/e2e/*.sh` — live Docker-backed harnesses run manually from the repo root:
  - `daemon-worktree-setup-e2e.sh` — clean containerized host for headless `worktree_create` provisioning (issue #78): drives the real authed HTTP `tools/call` surface and asserts worktree creation + setup-script run + gitignored-include copy on the container's filesystem. Requires docker, python3, and a debug `codemux-remote` build (`CMX_WT_E2E_BIN` overrides the binary).
  - `opencode-sync-e2e.sh` / `opencode-real-session-e2e.sh` — OpenCode conversation sync across workspace push/pull against a Docker SSH host (issue #16).
- `scripts/e2e/remote-bootstrap/run.sh` — Docker/systemd Ubuntu + Fedora harness for the fresh-host claim: installer → mocked-API login → `codemux connect` → persistent user unit → local and cross-container health, plus status/off/reconnect/reinstall idempotency. It never calls the production API; see the adjacent `README.md` for prerequisites and invocation.
- Env-gated Rust integration tests skip by default and run live when pointed at a host: `CODEMUX_E2E_SSH_HOST` gates `src-tauri/tests/codemux_ssh_roundtrip.rs`, `opencode_sync_roundtrip.rs`, and `opencode_real_roundtrip.rs` (the OpenCode pair also needs `CMX_OC_E2E_*` variables — see the script headers).

## Manual Validation Rules

- Implemented is not the same as verified.
- A roadmap checkbox is not release proof.
- Browser and remote-runtime changes need especially careful manual validation because they cross process and transport boundaries.

## High-Value Manual Workflows Right Now

- app startup and fallback launch behavior
- workspace creation, switching, closing, and pane operations
- mixed terminal and browser layouts
- browser toolbar and automation command flows
- memory, handoff, and indexing workflows
- Agent Chat workflow approval, progress, cancellation, and transcript stability
