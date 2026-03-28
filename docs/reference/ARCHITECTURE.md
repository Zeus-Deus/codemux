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
- `docs/`: canonical project docs

## Frontend Layer

The frontend is React + Tailwind v4 + shadcn (preset b3kIbNYVW). State management is zustand. All Tauri IPC goes through typed wrappers in `src/tauri/`.

- shadcn primitives: `src/components/ui/` (button, tabs, sidebar, resizable, badge, tooltip, etc.)
- app shell layout: `src/components/layout/` (AppSidebar, PaneNode, TabBar, WorkspaceMain, RightPanel)
- terminal integration: `src/components/terminal/TerminalPane.tsx` (xterm.js + PTY via Tauri Channel)
- React hooks: `src/hooks/` (useTauriEvent, useAppStateInit, useKeyboardShortcuts, useThemeColors)
- zustand stores: `src/stores/` (app-store.ts for AppStateSnapshot, ui-store.ts for local UI state)
- Tauri bridge: `src/tauri/commands.ts` (80+ typed invoke wrappers), `src/tauri/events.ts` (8 event helpers), `src/tauri/types.ts` (all shared types)
- CSS variables: `src/globals.css` (oklch color tokens in :root and .dark, custom --success/--danger/--warning)

The frontend talks to Rust through typed wrappers in `src/tauri/commands.ts` plus the `app-state-changed` event stream. Components never import from `@tauri-apps/api` directly.

## Rust Layer

Rust owns the durable app domain and runtime integration.

- Tauri composition root: `src-tauri/src/lib.rs`
- canonical app state: `src-tauri/src/state/`
- PTY and terminal lifecycle: `src-tauri/src/terminal/`
- OpenFlow runtime: `src-tauri/src/openflow/`
- socket control server: `src-tauri/src/control.rs`
- CLI entrypoint: `src-tauri/src/cli.rs`
- browser runtimes: `src-tauri/src/agent_browser.rs`, `src-tauri/src/browser.rs`
- Tauri command modules: `src-tauri/src/commands/`

## Command Surface

The Tauri command layer is split by domain.

- workspace and pane commands: `src-tauri/src/commands/workspace.rs`
- browser commands: `src-tauri/src/commands/browser.rs`
- OpenFlow commands: `src-tauri/src/commands/openflow.rs`
- memory, indexing, observability, and dialogs: `src-tauri/src/commands/mod.rs`

These command names stay stable at the Tauri boundary even when the internal module layout changes.

## Control Surfaces

Codemux exposes three main control paths:

1. frontend `invoke(...)` calls into Tauri commands
2. local socket control in `src-tauri/src/control.rs`
3. CLI wrappers in `src-tauri/src/cli.rs`

Workspace and browser socket commands are routed through the same Rust helper implementations used by the Tauri command layer so they stay behaviorally aligned.

## Browser Architecture

The current canonical browser path is `agent-browser`.

- visible browser pane control uses `agent_browser_*` commands
- CLI browser commands use the same `agent-browser` execution helpers
- socket `browser_automation` uses the `AgentBrowserManager`

`src-tauri/src/browser.rs` still exists as the legacy Chromium/CDP-backed runtime. It is kept as an internal alternate path, but it is not the primary browser path used by the current pane UI.

## Auth & Settings Sync

Codemux authenticates against a Better Auth API server at `api.codemux.org` (override with `CODEMUX_API_URL`). The desktop app stores encrypted tokens locally and sends Bearer tokens for all API calls.

- auth logic: `src-tauri/src/auth.rs` (encryption, token storage, CSRF, machine key)
- auth commands: `src-tauri/src/commands/auth.rs` (OAuth flow, email sign-in/up, session check)
- settings sync: `src-tauri/src/settings_sync.rs` (server fetch/push, offline cache, dirty flag)
- settings commands: `src-tauri/src/commands/settings_sync.rs` (get, update, patch, reset)
- frontend auth: `src/stores/auth-store.ts`, `src/components/auth/login-screen.tsx`
- frontend settings: `src/stores/synced-settings-store.ts`, `src/components/settings/settings-view.tsx`

Per-user settings sync to the server; machine-local settings (sidebar state, window layout, presets) stay in SQLite. The server is the source of truth when reachable; offline cache with dirty flag handles network outages.

## OpenFlow Boundary

OpenFlow is integrated into Codemux, but it still keeps a separate runtime boundary.

- OpenFlow workspace shell lives in the normal workspace system
- run state lives in `OpenFlowRuntimeStore` and `AgentSessionStore`
- orchestration logic and comm-log parsing live under `src-tauri/src/openflow/`
- OpenFlow browser viewing currently mounts `BrowserPane` on the shared default browser session

OpenFlow workspaces are intentionally treated as runtime-oriented surfaces rather than long-term persisted workspace state.
