# Browser Capability

- Purpose: Describe the current browser subsystem, what it can do, and what constraints still matter.
- Audience: Anyone working on browser features, automation, or browser-based validation.
- Authority: Canonical browser capability and constraints document.
- Update when: Browser behavior, user expectations, or known limitations change.
- Read next: `docs/plans/browser.md`, `docs/reference/BROWSER-AGENT-COMMANDS.md`, `AGENTS.md`

## Current Model

The browser pane uses a screenshot-driven Chromium session backed by `agent-browser` v0.24.0 (pure Rust, direct CDP). The visible pane and the explicit CLI browser commands share the same browser session and use the same internal execution helpers.

## What Works Today

- create a browser pane inside a workspace
- navigate with the address bar or `codemux browser open`
- keep browser panes inside split layouts next to terminals
- let agents use `snapshot`, `click`, `fill`, `screenshot`, and `console-logs`
- show user-visible browser updates through repeated screenshot refreshes
- stealth Chromium flags to reduce bot detection fingerprinting
- realistic user-agent string derived from installed Chrome/Chromium version
- per-workspace browser sessions with reconnection on pane recreation
- dynamic stream ports (9223-9299) for concurrent workspace browsers
- browser data management in Settings (clear cookies, clear all data, view data size)
- inspector panel for debugging web content
- viewport presets for mobile / tablet / desktop responsive testing via `codemux browser viewport <preset>` (e.g. `mobile`, `tablet`, `desktop-large`, `reset`) or custom `WxH` dimensions — applies real viewport resize + DPR through CDP, so CSS media queries fire and screenshots capture at the simulated dimensions (replaces the older iframe trick)
- native-feel manual interaction in the pane: drag-to-select text (moves carry the held CDP button; pointer capture finishes drags released outside the pane), hover effects (`:hover` styling — mousemoves are forwarded with a coalescing throttle: ~60/s during drags, ~30/s hover, 10/s in inspector mode), double/triple-click word/paragraph selection (chained `clickCount`), right/middle-click forwarding (web-app context menus work; the host context menu is suppressed), and a live cursor that mirrors the remote page (pointer over links, I-beam over text, resize cursors) via a throttled `elementFromPoint` probe
- host-clipboard bridge: the headless browser's clipboard is invisible to the OS, so Ctrl/Cmd+C and +X mirror the page selection (including input/textarea selections) onto the host clipboard while still performing the in-page copy/cut, and Ctrl/Cmd+V inserts host-clipboard text into the focused editable via chunked `execCommand("insertText")` evals (falls back to forwarding the raw keystroke when focus isn't editable, so in-page paste handlers still run)
- calm reconnect UX: a quiet screencast is not treated as a dead stream (CDP only emits frames on visual change) — when no frame has arrived for 15s the pane probes the daemon's HTTP `/api/status` endpoint and only reconnects when the daemon is actually unresponsive; during a real reconnect the last frame stays visible with a small corner pill instead of a full-screen spinner, and after the fast retry budget (15 × 1.5s) the pane drops to a slow 10s retry loop instead of giving up, so it self-heals whenever the daemon comes back
- stream stability across concurrent worktrees: daemons are PID-tracked (not port-tracked), the workspace-id map is the single canonical key, `close()` is atomic (remove entry → kill PID → await exit → verify port free), `allocate_port()` runs a symmetric `TcpListener::bind` probe on every OS, and `BrowserPane.tsx` reactively reconnects when `stream_url` changes. Replaces the older flow where the manager could lose track of which port belonged to which workspace, leak daemons, and leave panes stuck at the right URL with no frames. See `docs/archive/browser-stream-fix.md` for the full landed plan.
- hang-proof external calls (issue #96): every `agent-browser` CLI / process-control invocation runs through hard-timeout wrappers (`output_capture_with_timeout` / `launch_status_with_timeout`) that kill the child — and, on Unix, its whole process group — on overrun, so a wedged daemon can never block `close()` / `start_stream()` / automation forever. The async callers also push the blocking work onto `tokio::task::spawn_blocking` so a slow CLI can't stall the runtime's worker threads. Default ceilings: ~5s process control, ~10s `close`, ~30s DOM actions, ~60s `open`/`wait`. Replaces the unbounded `std::process::Command::output()` calls that hung `cargo test` to the 30-minute job timeout on the Windows CI image.

## Expected Operating Model

- agents control the browser programmatically
- users see the browser pane as live evidence of that work
- this is usable now, but it is not the final browser architecture yet

## Current Internal Boundary

- canonical path: `agent_browser` commands plus `AgentBrowserManager` in `src-tauri/src/agent_browser.rs`
- CLI browser commands delegate to the same `agent-browser` execution path
- the legacy Playwright/Node.js path and the unused `BrowserManager` Rust CDP implementation have been removed (v0.24.0 migration)

## Current Constraints

- not a native embedded Tauri webview
- the pane is screenshot-driven, so some fidelity gaps remain: no native context menu, no in-pane file dialogs, cursor updates are probe-driven (~8/s) rather than instant
- every eval POSTed to the stream daemon's `/api/command` must keep the whole HTTP request inside one TCP segment (~1.4KB) — the daemon reads requests with a single peek and sees an empty body otherwise. Measured: bodies ≤ ~870 bytes always survive, ≥ ~970 always fail. This is why the pane's page scripts are minified and paste text is chunked at 120 chars (`PASTE_CHUNK_SIZE` in `src/components/browser/stream-protocol.ts`)
- toolbar behavior, back and forward, reload, and arbitrary text entry still need focused validation
- browser console capture is not yet a full live log stream from the displayed pane

## Important Touch Points

- `src-tauri/src/agent_browser.rs` — `AgentBrowserManager`, stealth flags, stream port allocation (PID-tracked daemons keyed by `workspace_id`, symmetric bind probe on every OS, atomic teardown), viewport argv builder (`resolve_viewport_params`, `format_dpr`), and the external-process timeout layer (`output_capture_with_timeout`, `launch_status_with_timeout`, `wait_with_timeout`, `kill_timed_out_child`, `action_timeout`) that bounds every CLI/kill call (issue #96)
- `src-tauri/src/browser_viewport.rs` — preset table (`PRESETS`, `RESET_SPEC`) and `parse_spec` for preset / `WxH` / `reset` parsing
- `src-tauri/src/commands/browser.rs` — Tauri commands for pane creation, URL navigation, automation
- `src-tauri/src/cli.rs` — `BrowserCommand::Viewport` and `BrowserCommand::ViewportPresets` CLI subcommands
- `src-tauri/src/mcp_server.rs` — `browser_viewport` and `browser_viewport_presets` MCP tools
- `src/components/browser/BrowserPane.tsx` — screenshot rendering, toolbar, address bar, reactive `stream_url` reconnect on URL change, auto-syncs viewport on pane resize via legacy `width`/`height` payload (which the new socket handler accepts unchanged); pointer-capture input forwarding (drag selection, hover, click-count chaining, right/middle click), cursor probe, host-clipboard bridge, probe-based liveness + corner reconnect pill
- `src/components/browser/stream-protocol.ts` — pure helpers for the stream-input protocol (button mapping, click-count chaining, coordinate mapping, cursor sanitization) and the daemon HTTP endpoints (`/api/status` liveness probe, `/api/command` evals; page scripts minified for the single-segment HTTP constraint). Unit-tested in `stream-protocol.test.ts`
- `src/components/browser/InspectorPanel.tsx` — browser inspector/DevTools panel
- `docs/reference/BROWSER-AGENT-COMMANDS.md` — CLI and socket command reference
- `docs/archive/browser-stream-fix.md` — landed cross-platform stream-stability plan (PID tracking, single canonical key, atomic teardown, symmetric bind probe, reactive frontend reconnect)
