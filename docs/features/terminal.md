# Terminal System

- Purpose: Describe the PTY-based terminal subsystem powering all terminal panes.
- Audience: Anyone working on terminal rendering, shell integration, or PTY management.
- Authority: Canonical feature doc for the terminal layer.
- Update when: PTY behavior, rendering, shell integration, or session lifecycle change.
- Read next: `docs/features/presets.md`, `docs/reference/SHORTCUTS.md`

## What This Feature Is

The terminal system provides multi-session PTY terminals rendered with xterm.js and WebGL. Every terminal tab is a real pseudoterminal spawned by the Rust backend, with data streamed to the React frontend via Tauri channels.

## Current Model

The Rust layer uses `portable-pty` to spawn shells. Each terminal session has a master PTY handle, a read thread, and a write path. The frontend uses xterm.js with the WebGL renderer for GPU-accelerated display. Data flows: PTY read thread -> Tauri channel -> xterm.js write. User input flows: xterm.js onData -> Tauri command `write_to_pty` -> PTY master write.

## What Works Today

- multiple concurrent terminal sessions per workspace
- shell detection (respects `$SHELL`, falls back to `/bin/bash`)
- PTY resize on pane/window resize
- xterm.js WebGL renderer with kitty keyboard protocol support
- terminal theme reads dynamically from CSS variables via MutationObserver
- session state tracking (running, exited, error)
- environment injection: `CODEMUX`, `CODEMUX_VERSION`, `CODEMUX_WORKSPACE_ID`, `CODEMUX_BROWSER_CMD`, `BROWSER`, `CODEMUX_AGENT_CONTEXT`
- working directory set to workspace root on creation
- comm log support for OpenFlow agent communication tracking
- ANSI code stripping for log capture

## Current Constraints

- scrollback is lost on app restart (sessions are not persisted)
- no session save/restore (layout persists, terminal content does not)
- no split-pane multiplexing within a single PTY (splits are separate sessions)
- no inline image rendering (sixel/iTerm2 protocols)

## Important Touch Points

- `src-tauri/src/terminal/mod.rs` — PTY spawning, read/write, session management, comm log locks
- `src-tauri/src/commands/workspace.rs` — `create_terminal_session`, `write_to_pty`, `resize_pty`, `attach_pty_output`
- `src/components/terminal/TerminalPane.tsx` — xterm.js rendering, WebGL, input handling
- `src/lib/app-shortcuts.ts` — terminal-specific keyboard shortcuts
- `src-tauri/src/agent_context.rs` — environment variable injection for terminal sessions
