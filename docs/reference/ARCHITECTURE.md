# Codemux Architecture

- Purpose: Explain the repository and runtime boundaries at a glance.
- Audience: Contributors cleaning up or extending Codemux.
- Authority: Canonical architecture map for repo structure and cross-layer entry points.
- Update when: Major boundaries, command surfaces, or browser/runtime ownership change.
- Read next: `docs/core/PROJECT.md`, `docs/reference/CONTROL.md`, `docs/features/browser.md`

## Top-Level Split

Codemux is one Tauri desktop app repo, not a separate web frontend plus network backend.

- `src/`: React + Tailwind + shadcn UI and Tauri IPC callers
- `src-tauri/`: Rust domain/runtime, Tauri command surface, CLI, socket control, PTY and browser runtime integration
- `sidecar/`: Bun-compiled TypeScript subprocesses Codemux spawns — currently just `sidecar/claude-agent/`, which hosts Anthropic's Claude Agent SDK and speaks JSON-RPC to the Rust side (it has its own Bun test suite, not covered by `npm run verify`)
- `iroh-wasm/`: a separate Rust crate compiled to WebAssembly (`npm run build:iroh-wasm`) that gives the hosted browser client an iroh QUIC endpoint — the relay/from-anywhere transport in `docs/features/web-remote-access.md`
- `scripts/`: build helpers and the manual `scripts/e2e/*.sh` Docker harnesses
- `docs/`: canonical project docs

## Frontend Layer

The frontend is React + Tailwind v4 + shadcn (preset b3kIbNYVW). State management is zustand. All Tauri IPC goes through typed wrappers in `src/tauri/`.

- shadcn primitives: `src/components/ui/` (button, tabs, sidebar, resizable, badge, tooltip, etc.)
- app shell layout: `src/components/layout/` (AppSidebar, PaneNode, TabBar, WorkspaceMain, RightPanel)
- terminal integration: `src/components/terminal/TerminalPane.tsx` (the live terminal component — per-mount xterm lifecycle: constructs its own xterm.js Terminal + addons on mount, disposes on unmount; only the active workspace/surface is rendered, so switches unmount/remount) + `src/components/terminal/terminal-write-pump.ts` (ordered, byte-budgeted throttle that drains scrollback restore + the `attach_pty_output` reattach replay + live output across macrotasks so switching doesn't freeze the renderer). The module-level persistent cache `src/components/terminal/terminal-cache.ts` (and `src/hooks/use-terminal-cache-gc.ts` / `use-terminal-theme-sync.ts`) is **disabled / not wired** — shipped in `14735bf`, rolled back in `2baa42f`; retained behind its file banner for a possible future flag-gated revival.
- React hooks: `src/hooks/` (useTauriEvent, useAppStateInit, useKeyboardShortcuts, useThemeColors)
- zustand stores: `src/stores/` (app-store.ts for AppStateSnapshot, ui-store.ts for local UI state)
- Tauri bridge: `src/tauri/commands.ts` (~320 typed invoke wrappers), `src/tauri/events.ts` (18 event helpers), `src/tauri/types.ts` (all shared types)
- CSS variables: `src/globals.css` (oklch color tokens in :root and .dark, custom --success/--danger/--warning)

The frontend talks to Rust through typed wrappers in `src/tauri/commands.ts` plus the `app-state-changed` event stream. Components never import from `@tauri-apps/api` directly.

## Rust Layer

Rust owns the durable app domain and runtime integration.

- Tauri composition root: `src-tauri/src/lib.rs`
- canonical app state: `src-tauri/src/state/` (the in-memory `AppStateSnapshot` plus its `layout.json` load/save)
- local persistence: `src-tauri/src/database.rs` (the `codemux.db` SQLite store and its replay-idempotent migration ledger) — see `docs/features/persistence-layer.md`
- PTY and terminal lifecycle: `src-tauri/src/terminal/`
- agent-chat providers: `src-tauri/src/agent_provider/` (the `AgentProvider` trait plus per-provider adapters for Codex and Claude; the Claude adapter drives the sidecar binary under `sidecar/claude-agent/`)
- JSON-RPC stdio helper: `src-tauri/src/json_rpc_child/` (shared framing + routing for long-lived subprocesses both provider adapters use)
- control server: `src-tauri/src/control.rs` (Unix socket on Linux/macOS via `tokio::net::UnixListener`, named pipe on Windows via `tokio::net::windows::named_pipe::ServerOptions`)
- CLI entrypoint: `src-tauri/src/cli.rs`
- browser runtime: `src-tauri/src/agent_browser.rs` — spawns and supervises the **external** `agent-browser` binary (v0.24.0, pure Rust, direct CDP). It is not a Cargo dependency; Codemux shells out to it.
- remote compute transport: `src-tauri/src/ssh/` (push/pull, tunnels, bootstrap) + `src-tauri/src/remote/` (the headless daemon + 12-tool MCP catalog) + `src-tauri/src/pty_daemon/` (the persistent PTY daemon) + the second binary `src-tauri/src/bin/codemux_remote.rs`
- remote UI transport: `src-tauri/src/web_remote/` (embedded axum HTTP+WS server, channel router, event hub, iroh endpoint + device registration)
- scheduled runs: `src-tauri/src/automations/`
- MCP: `src-tauri/src/mcp/` (Codemux as MCP host) and `src-tauri/src/mcp_server.rs` (Codemux as MCP server, 55 tools)
- skills: `src-tauri/src/skills/` + `src-tauri/src/skills_sync/`
- auth: `src-tauri/src/auth/`; child-process environment hygiene: `src-tauri/src/execution/`; config: `src-tauri/src/config/`
- Tauri command modules: `src-tauri/src/commands/`

## Command Surface

The Tauri command layer is split by domain.

`src-tauri/src/commands/` holds 24 domain modules: `agent_chat`, `ai`, `auth`, `automations`,
`branch_name`, `browser`, `database`, `files`, `gemini`, `git`, `github`, `hosts`, `mcp`, `opencode`,
`package_detect`, `permissions`, `presets`, `project_files`, `settings_sync`, `skills`,
`skills_sync`, `update`, `workspace`, `workspaces_sync`. Memory, indexing,
observability, and dialog commands live directly in `commands/mod.rs`.

These command names stay stable at the Tauri boundary even when the internal module layout changes.

## Control Surfaces

Codemux exposes three main control paths:

1. frontend `invoke(...)` calls into Tauri commands
2. local IPC (Unix socket on Linux/macOS, named pipe on Windows) in `src-tauri/src/control.rs`
3. CLI wrappers in `src-tauri/src/cli.rs`

The transport is platform-specific (`unix_transport` vs `windows_transport` modules inside `control.rs`) but exposes a single generic `bind` / `accept` / `connect` / `sync_liveness_probe` API, and `handle_client` is generic over `AsyncRead + AsyncWrite + Unpin + Send` so all accept loops stay identical across platforms. Workspace and browser socket commands are routed through the same Rust helper implementations used by the Tauri command layer so they stay behaviorally aligned.

## Browser Architecture

The canonical browser path is `agent-browser` v0.24.0 (pure Rust, direct CDP).

- visible browser pane control uses `agent_browser_*` commands
- CLI browser commands use the same `agent-browser` execution helpers
- socket `browser_automation` uses the `AgentBrowserManager`

The legacy Playwright/Node.js path and the unused `BrowserManager` Rust CDP implementation have been removed.

## Auth & Settings Sync

Codemux authenticates against a Better Auth API server at `api.codemux.org` (override with `CODEMUX_API_URL`). The desktop app stores encrypted tokens locally and sends Bearer tokens for all API calls.

- auth logic: `src-tauri/src/auth/` — `mod.rs` (token storage, CSRF, machine key), `api.rs` (Better Auth HTTP client), `derivation.rs` (Argon2id + HKDF credential derivation; retained as the Vexis cross-product protocol canary after skills sync moved server-side in PR #112)
- auth commands: `src-tauri/src/commands/auth.rs` (OAuth flow, email sign-in/up, session check)
- settings sync: `src-tauri/src/settings_sync.rs` (server fetch/push, offline cache, dirty flag)
- settings commands: `src-tauri/src/commands/settings_sync.rs` (get, update, patch, reset)
- frontend auth: `src/stores/auth-store.ts`, `src/components/auth/login-screen.tsx`
- frontend settings: `src/stores/synced-settings-store.ts`, `src/components/settings/settings-view.tsx`

Per-user settings sync to the server; machine-local settings (sidebar state, window layout, presets) stay in SQLite. The server is the source of truth when reachable; offline cache with dirty flag handles network outages.

## Workflow Orchestration Boundary

Workflow orchestration is an Agent Chat capability, not a separate workspace runtime. Claude `Workflow` tool events are reduced into the normal persisted transcript, rendered by `src/components/workflow/`, and surfaced through the conditional Orchestration panel. See `docs/features/workflow-orchestration.md`.
