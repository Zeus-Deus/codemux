# Terminal System

- Purpose: Describe the PTY-based terminal subsystem powering all terminal panes.
- Audience: Anyone working on terminal rendering, shell integration, or PTY management.
- Authority: Canonical feature doc for the terminal layer.
- Update when: PTY behavior, rendering, shell integration, or session lifecycle change.
- Read next: `docs/features/presets.md`, `docs/reference/SHORTCUTS.md`

## What This Feature Is

The terminal system provides multi-session PTY terminals rendered with xterm.js. Every terminal tab is a real pseudoterminal spawned by the Rust backend, with data streamed to the React frontend via Tauri channels.

## Current Model

The Rust layer uses `portable-pty` to spawn shells. Each terminal session has a master PTY handle, a read thread, and a write path. The frontend renders with xterm.js's **WebGL renderer** (`@xterm/addon-webgl`), which offloads glyph rasterization to the GPU — the recommended renderer for the fast, long-running agent output Codemux streams. The addon is loaded after `term.open()` (it needs the opened terminal's DOM element) and registers an `onContextLoss` handler that disposes it so xterm falls back to its DOM renderer if the GPU drops the canvas context; construction is wrapped in try/catch so an environment without WebGL2 support also falls back to the DOM renderer instead of breaking the pane. Data flows: PTY read thread -> Tauri channel -> xterm.js write. User input flows: xterm.js onData -> Tauri command `write_to_pty` -> PTY master write. PTY output delivery clones the Tauri channel under the session lock and sends outside that lock, so active keystrokes do not block behind IPC delivery for busy terminals.

### Terminal Pane Lifecycle (per-mount) + throttled write pump

The live terminal render path is `src/components/terminal/TerminalPane.tsx`. Each pane constructs its own xterm.js `Terminal` (WebGL renderer, DOM fallback) on mount and disposes it — including the WebGL addon — on unmount. Because `src/components/layout/workspace-main.tsx` only renders the active workspace and `PaneContainer` only renders the active surface (tab), switching workspaces or tabs **unmounts** the previous pane tree and **mounts** the new one — terminals are rebuilt from scratch on every switch (a fresh xterm + scrollback restore + PTY reattach).

To keep that switch from freezing the UI, every byte destined for xterm — disk-scrollback restore, the PTY reattach replay, and steady live output — is funneled through a single throttled write pump (`src/components/terminal/terminal-write-pump.ts`). The pump drains a bounded byte budget per macrotask and yields between batches, so a multi-MB `attach_pty_output` replay (the backend replays the whole `pending_output` ring on every attach) fills in over a few frames instead of blocking the click + visibly "pouring in". Historical bytes are enqueued **before** the live channel attaches, so the single FIFO drain preserves xterm's stateful parse order (alt-screen / cursor state across the history→live boundary).

On unmount the outgoing pane serializes its buffer (`@xterm/addon-serialize`) and caches it for restore — **except** for alternate-screen buffers (the long-running TUI agents: Claude Code, lazygit, vim, btop), whose serialized content is garbled and is never restored anyway. Serializing those is skipped (`buildScrollbackPayload` returns empty `data` when `alternate_buffer`), which removes the dominant synchronous cost from switching away from an agent pane. The screen is reconstructed on return by the PTY reattach replay (`attach_pty_output` replays the full `pending_output` byte history, which re-enters alt-screen and redraws — this is why the backend retains the full ring rather than only post-detach bytes; trimming it would blank an idle agent on return).

> **Historical note — persistent xterm cache (disabled).** A module-level cache (`src/components/terminal/terminal-cache.ts`) that kept xterm instances alive across switches and reparented a wrapper `<div>` into a `#codemux-terminal-parking` node was shipped in `14735bf` and **rolled back** in `2baa42f` for input-lag / malformed-render regressions. The file is retained but **not wired into the live app** (see its top-of-file banner) as the basis for a possible future flag-gated revival — the "keep instances alive" approach is the true-instant fix but must be live-verified first. Its flow-control / backpressure and throttled-replay code is therefore **inert in production**; `terminal-write-pump.ts` is the live equivalent of the throttle, and the live `TerminalPane` is now the path that actually engages PTY producer back-pressure (the cache's HIGH/LOW watermark *constants and approach* were reused there — see "PTY producer back-pressure" above). `useTerminalCacheGc` and `useTerminalThemeSync` (wired in `App.tsx`) operate on the empty cache map and are likewise no-ops today.

## What Works Today

- multiple concurrent terminal sessions per workspace
- shell detection:
  - **Unix**: respects `$SHELL` and falls back to `/bin/bash`
  - **Windows**: prefers `pwsh.exe` (PowerShell 7+) when on `PATH`, falls back to `powershell.exe` (Windows PowerShell 5.1, pre-installed on every supported Windows version), then `%COMSPEC%`, then literal `"cmd.exe"`. PowerShell wins because the Windows preset wrappers emit PowerShell `$env:VAR` syntax for context injection — see `agent_context.rs` and `docs/features/presets.md`
- PTY resize on pane/window resize
- xterm.js WebGL renderer (GPU glyph rendering) with automatic DOM-renderer fallback on GPU context loss or missing WebGL2 support, plus kitty keyboard protocol support
- terminal theme reads dynamically from CSS variables via MutationObserver
- session state tracking (running, exited, error)
- environment injection: `CODEMUX`, `CODEMUX_VERSION`, `CODEMUX_WORKSPACE_ID`, `CODEMUX_BROWSER_CMD`, `BROWSER`, `CODEMUX_AGENT_CONTEXT`
- working directory set to workspace root on creation
- comm log support for OpenFlow agent communication tracking
- ANSI code stripping for log capture
- **throttled write pump (live path)**: every byte destined for xterm goes through `terminal-write-pump.ts`, which drains a bounded byte budget per macrotask and yields between batches. This keeps a fast producer (`yes`, `cat huge-file`, a verbose build, a runaway agent) and — more importantly — the multi-MB `attach_pty_output` reattach replay on every workspace switch from pegging the renderer's main thread. It is the consumer-side throttle that prevents the switch freeze.
- **PTY producer back-pressure (live path — engaged in production)**: the throttled write pump alone only slows the *consumer*; a fast producer (`yes`, a verbose build, a runaway agent) can still outrun xterm's ~5–35 MB/s ingest and balloon the renderer queue (and, while detached, overflow the 256 MiB `pending_output` ring into dropped scrollback). The live `TerminalPane` now watches the pump's queued-byte depth and signals the producer: when queued bytes cross a HIGH watermark (16 MiB) it calls `pause_pty_output`; when they drain below LOW (4 MiB) it calls `resume_pty_output` (hysteresis avoids thrashing — watermark logic + callbacks live in `terminal-write-pump.ts`, wired in `TerminalPane.tsx`). Pausing stops the read loop draining the PTY master fd, so the child blocks on `write()` once the kernel buffer fills — real back-pressure to the producer.
  - **Both spawn paths honor it.** Daemon-backed sessions route the pause/resume to the daemon's `SetFlowPaused` (v2 wire protocol) and its read loop's `flow_paused` gate. In-process (non-daemon) sessions now honor a per-session `flow_paused: Arc<AtomicBool>` that `batched_reader_loop` polls — **previously a deliberate no-op, fixed for issue #73** so the fallback path (Windows, daemon-disabled, daemon failure) gets real back-pressure too.
  - **Self-heal (both paths).** A paused PTY always resumes even if the matching resume is lost: the in-process reader clears the flag on `attach_pty_output` (a reattaching pane starts live) and `detach_pty_output` (a backgrounded agent isn't left blocked), plus a 10 s `FLOW_MAX_PARK` backstop inside the read loop; the renderer also resumes explicitly on unmount. The daemon mirrors this (clears on `Attach`/`Close` + its own max-park). The throttled write pump remains the consumer-side complement — back-pressure does not replace it.
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

- `src/components/terminal/TerminalPane.tsx` — **the live terminal component**: per-mount xterm lifecycle, WebGL renderer load (after `term.open()`, with `onContextLoss` → DOM fallback and a try/catch guard for no-WebGL2 environments; disposed on unmount), scrollback restore + reattach via the throttled write pump, ResizeObserver, focus, status overlay, custom key handler, alt-screen serialize skip, **PTY back-pressure wiring** (pump HIGH/LOW watermark callbacks → `pause_pty_output`/`resume_pty_output`, resume-on-unmount)
- `src/components/terminal/terminal-write-pump.ts` — **the live freeze fix + back-pressure source**: ordered, byte-budgeted write queue that throttles scrollback restore + reattach replay + live output across macrotasks; also tracks total queued bytes and fires HIGH/LOW watermark callbacks (16 MiB / 4 MiB, with hysteresis) so the live path can signal the producer; unit-tested in `terminal-write-pump.test.ts`
- `src-tauri/src/terminal/mod.rs` — PTY spawning, read/write, session management, comm log locks, `attach_pty_output` (replays the full `pending_output` ring on attach — the reattach replay the write pump throttles; clears `flow_paused` as a self-heal), dropped-chunk observability counter, `pause_pty_output` / `resume_pty_output` flow-control commands, and the **in-process `flow_paused` gate** in `batched_reader_loop` (`set_pty_flow_paused` flips a per-session `Arc<AtomicBool>` the reader polls, with a `FLOW_MAX_PARK` backstop). In-process flow-control tests: `test_in_process_flow_control_pauses_and_resumes_reader`, `test_firehose_backpressure_bounds_backend_with_consumer_attached`
- `src-tauri/src/pty_daemon/{protocol,client,server}.rs` — `SetFlowPaused` wire request + per-session `flow_paused` flag gating the daemon read loop (with `Attach`/`Close`/max-park fail-safes); invoked by the live `TerminalPane` path for daemon-backed sessions (the in-process path uses `mod.rs`'s own `flow_paused` gate)
- `src-tauri/src/commands/workspace.rs` — `create_terminal_session`, `write_to_pty`, `resize_pty`, `attach_pty_output`
- `src/components/terminal/terminal-cache.ts` — **DISABLED / not wired** (see file banner): module-level persistent Terminal cache, parking node, attach/detach/dispose API, `pumpWrites` + HIGH/LOW watermark pause/resume. Retained for a possible future flag-gated revival; inert in production. Its watermark constants/approach were reused on the live `TerminalPane`/`terminal-write-pump.ts` path (do NOT resurrect this file — see issue #73 pitfalls).
- `src/hooks/use-terminal-cache-gc.ts`, `src/hooks/use-terminal-theme-sync.ts` — operate on the disabled cache's empty map; no-ops today
- `src/lib/app-shortcuts.ts` — terminal-specific keyboard shortcuts
- `src-tauri/src/agent_context.rs` — environment variable injection for terminal sessions
