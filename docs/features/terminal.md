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

### Terminal Pane Persistence

The xterm.js `Terminal` instance, its addons, the persistent wrapper `<div>`, and the PTY-output `Channel` all live in a module-level cache (`src/components/terminal/terminal-cache.ts`) keyed by `sessionId`. The cache lifetime equals the PTY session lifetime; `TerminalPane.tsx` is a thin DOM-attach wrapper that reparents the cached wrapper between its layout container and a body-level `#codemux-terminal-parking` node on mount/unmount.

This is the load-bearing invariant: bytes flowing in from a long-running alt-screen TUI (Claude Code's "Simmering…", lazygit, vim, btop) keep being processed by the same xterm even while the React component is unmounted, so its mode flags / cursor / cell grid stay in sync with the PTY producer. Workspace switches no longer cause garbled or misaligned rendering on return.

Disposal is driven by `useTerminalCacheGc` in `src/hooks/use-terminal-cache-gc.ts`: it diffs `AppState.terminal_sessions` and calls `disposeTerminal(sid)` for any session that disappears, covering close-pane / close-tab / close-workspace / PTY-exit. Because the channel stays attached for the session's lifetime, the Rust-side `pending_output` buffer (`src-tauri/src/terminal/mod.rs`) is effectively a cold-start replay window only; the `dropped_chunks` counter on `SessionRuntime` is a regression signal — non-zero means the channel was somehow detached when the buffer overflowed.

## What Works Today

- multiple concurrent terminal sessions per workspace
- shell detection:
  - **Unix**: respects `$SHELL` and falls back to `/bin/bash`
  - **Windows**: prefers `pwsh.exe` (PowerShell 7+) when on `PATH`, falls back to `powershell.exe` (Windows PowerShell 5.1, pre-installed on every supported Windows version), then `%COMSPEC%`, then literal `"cmd.exe"`. PowerShell wins because the Windows preset wrappers emit PowerShell `$env:VAR` syntax for context injection — see `agent_context.rs` and `docs/features/presets.md`
- PTY resize on pane/window resize
- xterm.js WebGL renderer with kitty keyboard protocol support
- terminal theme reads dynamically from CSS variables via MutationObserver
- session state tracking (running, exited, error)
- environment injection: `CODEMUX`, `CODEMUX_VERSION`, `CODEMUX_WORKSPACE_ID`, `CODEMUX_BROWSER_CMD`, `BROWSER`, `CODEMUX_AGENT_CONTEXT`
- working directory set to workspace root on creation
- comm log support for OpenFlow agent communication tracking
- ANSI code stripping for log capture
- session close kills the PTY child and its entire process group via a single `killpg(pid, SIGKILL)` through the central `terminate_pty_session` helper, so closing a pane/tab/workspace also tears down any Claude CLI, MCP server, or rust-analyzer the shell spawned. `portable-pty`'s Unix spawn path calls `setsid()`, so the shell is a process-group leader and `killpg` reaches its children. A previous version did SIGTERM → 200ms grace → SIGKILL; that grace window is exactly the adversarial case for PID recycling (the shell handles SIGTERM and exits in ~50ms, the kernel reuses the PID for an unrelated process, our SIGKILL lands on the wrong process group), so the current code goes straight to SIGKILL and collapses the race to microseconds. `impl Drop for SessionRuntime` is a safety net that kills the tree with a warning if the normal close path is ever skipped.

## Windows-Specific Notes

- **`portable-pty` is pinned to a fork** (`Zeus-Deus/portable-pty`, branch `codemux-0.8.1-no-window`). Upstream `portable-pty 0.8.1` omits both `CREATE_NO_WINDOW` and `STARTF_USESHOWWINDOW + SW_HIDE` from `dwCreationFlags` in `psuedocon.rs`, which makes every PTY spawn flash a visible `cmd.exe` console window on the taskbar before being attached to a pseudoconsole. The fork ORs the flags in. Pinning instead of upgrading to 0.9.x avoids the `0.9.0` upstream regression #6783 (reader returns garbage output on Windows).
- **`spawn_pty_for_agent` PATH joining** uses the cross-platform `prepend_shim_to_path()` helper (`;` separator on Windows, `:` on Unix). A previous version hardcoded `:` and broke shim lookup on Windows.
- **Preset command line terminator** is `\r` on Windows so PowerShell actually executes the typed-in command on submit (Unix uses `\n`).

## Current Constraints

- no split-pane multiplexing within a single PTY (splits are separate sessions)
- no inline image rendering (sixel/iTerm2 protocols)
- scrollback persistence is plain text only (no image content from sixel/iTerm2)

Note: terminal scrollback is saved and restored across app restarts. See `docs/features/session-persistence.md` for the full persistence and adapter system.

## Important Touch Points

- `src-tauri/src/terminal/mod.rs` — PTY spawning, read/write, session management, comm log locks, dropped-chunk observability counter
- `src-tauri/src/commands/workspace.rs` — `create_terminal_session`, `write_to_pty`, `resize_pty`, `attach_pty_output`
- `src/components/terminal/TerminalPane.tsx` — DOM-attach wrapper, ResizeObserver, focus, status overlay, custom key handler
- `src/components/terminal/terminal-cache.ts` — module-level Terminal cache, parking node, attach/detach/dispose API
- `src/hooks/use-terminal-cache-gc.ts` — disposes cache entries when sessions disappear from AppState
- `src/lib/app-shortcuts.ts` — terminal-specific keyboard shortcuts
- `src-tauri/src/agent_context.rs` — environment variable injection for terminal sessions
