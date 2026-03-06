# Codemux

Codemux is a Linux-first AI coding workspace inspired by `cmux`.

It combines:

- terminal panes
- browser panes
- split layouts
- workspaces and sidebar navigation
- attention/notification signaling for agents
- browser automation primitives
- local CLI/socket control for external tools

Long term, it is also meant to host:

- codebase indexing
- portable agent memory
- `OpenFlow`, a multi-agent orchestration system

Portable agent memory has now started landing as a local-first project memory layer.

## Current Status

This repo is now beyond the initial prototype stage.

Implemented so far:

- multi-session terminal backend
- workspace and pane model
- split pane rendering and resizing
- browser pane MVP
- browser automation MVP
- notification system
- local control socket and CLI commands
- local project memory and handoff packet generation
- persisted layout state
- backend and frontend test foundations

## Run

Install dependencies:

```bash
npm install
```

Check the frontend:

```bash
npm run check
```

Run frontend tests:

```bash
npm run test
```

Check the Rust backend:

```bash
cargo check --manifest-path src-tauri/Cargo.toml
```

Run Rust tests:

```bash
cargo test --manifest-path src-tauri/Cargo.toml
```

Run the app:

```bash
npm run tauri dev
```

## CLI Control

When the app is running, Codemux exposes a local control socket.

Examples:

```bash
cargo run --manifest-path src-tauri/Cargo.toml -- status
cargo run --manifest-path src-tauri/Cargo.toml -- notify "Agent needs input"
cargo run --manifest-path src-tauri/Cargo.toml -- json get_app_state
```

## Project Memory Workflow

Codemux stores shared project memory in:

```text
.codemux/project-memory.json
```

Use it to carry context across:

- different tools
- different agent sessions in the same tool
- fresh sessions when prior context has become too large

Recommended workflow:

```bash
cargo run --manifest-path src-tauri/Cargo.toml -- memory set --brief "What this project is" --goal "What we are doing now" --focus "What I want the next tool to work on"
cargo run --manifest-path src-tauri/Cargo.toml -- memory add decision "We are keeping Tauri + Svelte + Rust"
cargo run --manifest-path src-tauri/Cargo.toml -- memory add next "Implement indexing after memory"
cargo run --manifest-path src-tauri/Cargo.toml -- handoff
```

You can also manage memory in the Codemux sidebar UI.

The important idea is:

- do not restore full raw chats by default
- store structured memory instead
- generate compact handoff prompts for the next tool/session

## Codebase Indexing Workflow

Codemux now supports a local-first lexical index stored in:

```text
.codemux/index.json
```

Use it like this:

```bash
cargo run --manifest-path src-tauri/Cargo.toml -- index build
cargo run --manifest-path src-tauri/Cargo.toml -- index status
cargo run --manifest-path src-tauri/Cargo.toml -- index search "browser automation"
```

What it does today:

- indexes supported text/code files
- chunks files into search-sized blocks
- extracts lightweight symbols
- supports fast lexical search
- refreshes incrementally through file watching while the app is running

What it does not do yet:

- embeddings
- semantic vector search
- advanced symbol graph intelligence

Protocol details are in `docs/CONTROL.md`.

## Project Memory Files

These are the most important top-level docs:

- `PROJECT.md` - product vision and architecture intent
- `PLAN.md` - phased implementation roadmap and progress
- `TESTING.md` - testing strategy and what kinds of tests matter
- `docs/CONTROL.md` - local control protocol and CLI usage
- `.codemux/project-memory.json` - per-project shared memory store

## Codebase Map

Frontend:

- `src/App.svelte` - main workspace shell and sidebar UI
- `src/components/panes/TerminalPane.svelte` - terminal pane host
- `src/components/panes/BrowserPane.svelte` - browser pane and automation bridge
- `src/components/panes/PaneNode.svelte` - recursive split-tree renderer
- `src/stores/appState.ts` - frontend app-state API bindings to Tauri commands
- `src/stores/theme.ts` - theme state and Omarchy fallback handling
- `src/lib/paneTree.ts` - reusable pane-tree helpers
- `src/lib/paneTree.test.ts` - first frontend unit tests

Backend:

- `src-tauri/src/lib.rs` - app bootstrap and Tauri command registration
- `src-tauri/src/main.rs` - binary entrypoint, app vs CLI mode
- `src-tauri/src/commands.rs` - Tauri commands and browser automation command model
- `src-tauri/src/control.rs` - Unix socket control server and JSON protocol
- `src-tauri/src/cli.rs` - local CLI interface for Codemux control
- `src-tauri/src/state/mod.rs` - state module boundary
- `src-tauri/src/state/state_impl.rs` - app-state store and domain logic
- `src-tauri/src/terminal/mod.rs` - PTY/session manager
- `src-tauri/src/config/mod.rs` - theme loading and Omarchy integration
- `src-tauri/src/memory.rs` - portable project memory and handoff generation

## Testing Strategy

Codemux uses layered testing:

- Rust tests for domain logic and state transitions
- Vitest frontend tests for meaningful UI/helper logic
- later, a small number of high-value end-to-end tests

The goal is not fake coverage. The goal is protection around the parts that are easy to break and expensive to debug manually.

## Current Platform Notes

- Linux is the first-class target right now
- Omarchy support exists but falls back cleanly when unavailable
- some browser features are currently Linux-leaning, such as screenshot capture using `grim`

## What Is Next

The next major implementation area should be one of:

1. `Phase 10` OpenFlow core runtime
2. `Phase 11` OpenFlow runtime MVP

Recommended next step: finish Phase 10 and then begin Phase 11.

Why:

- Codemux now has the core prerequisites OpenFlow needs
- the next critical step is to turn the OpenFlow idea into an explicit engine contract
- once the design spec is locked, the runtime can be implemented cleanly

Current OpenFlow design spec is now available from the backend and documented in `PROJECT.md`.
