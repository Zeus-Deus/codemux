# Codemux Architecture

- Purpose: Explain the repository and runtime boundaries at a glance.
- Audience: Contributors cleaning up or extending Codemux.
- Authority: Canonical architecture map for repo structure and cross-layer entry points.
- Update when: Major boundaries, command surfaces, or browser/runtime ownership change.
- Read next: `docs/core/PROJECT.md`, `docs/reference/CONTROL.md`, `docs/features/browser.md`

## Top-Level Split

Codemux is one Tauri desktop app repo, not a separate web frontend plus network backend.

- `src/`: Svelte UI, view models, and Tauri IPC callers
- `src-tauri/`: Rust domain/runtime, Tauri command surface, CLI, socket control, PTY and browser runtime integration
- `docs/`: canonical project docs

## Frontend Layer

The frontend is organized around domain stores and UI surfaces.

- app shell and workspace layout: `src/App.svelte`, `src/components/panes/*`, `src/components/sidebar/*`
- OpenFlow UI: `src/components/openflow/*`
- shared app-state store: `src/stores/core.ts`
- workspace and pane commands: `src/stores/workspace.ts`
- browser commands: `src/stores/browser.ts`
- memory commands: `src/stores/memory.ts`
- OpenFlow runtime and comm-log state: `src/stores/openflow.ts`
- shared frontend types: `src/stores/types.ts`

The frontend talks to Rust through `invoke(...)` plus the `app-state-changed` event stream.

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

## OpenFlow Boundary

OpenFlow is integrated into Codemux, but it still keeps a separate runtime boundary.

- OpenFlow workspace shell lives in the normal workspace system
- run state lives in `OpenFlowRuntimeStore` and `AgentSessionStore`
- orchestration logic and comm-log parsing live under `src-tauri/src/openflow/`
- OpenFlow browser viewing currently mounts `BrowserPane` on the shared default browser session

OpenFlow workspaces are intentionally treated as runtime-oriented surfaces rather than long-term persisted workspace state.
