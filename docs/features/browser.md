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
- stream stability across concurrent worktrees: daemons are PID-tracked (not port-tracked), the workspace-id map is the single canonical key, `close()` is atomic (remove entry → kill PID → await exit → verify port free), `allocate_port()` runs a symmetric `TcpListener::bind` probe on every OS, and `BrowserPane.tsx` reactively reconnects when `stream_url` changes. Replaces the older flow where the manager could lose track of which port belonged to which workspace, leak daemons, and leave panes stuck at the right URL with no frames. See `docs/archive/browser-stream-fix.md` for the full landed plan.

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
- lower interaction fidelity than a real embedded browser because the pane is screenshot-driven
- toolbar behavior, back and forward, reload, and arbitrary text entry still need focused validation
- browser console capture is not yet a full live log stream from the displayed pane

## Important Touch Points

- `src-tauri/src/agent_browser.rs` — `AgentBrowserManager`, stealth flags, stream port allocation (PID-tracked daemons keyed by `workspace_id`, symmetric bind probe on every OS, atomic teardown), viewport argv builder (`resolve_viewport_params`, `format_dpr`)
- `src-tauri/src/browser_viewport.rs` — preset table (`PRESETS`, `RESET_SPEC`) and `parse_spec` for preset / `WxH` / `reset` parsing
- `src-tauri/src/commands/browser.rs` — Tauri commands for pane creation, URL navigation, automation
- `src-tauri/src/cli.rs` — `BrowserCommand::Viewport` and `BrowserCommand::ViewportPresets` CLI subcommands
- `src-tauri/src/mcp_server.rs` — `browser_viewport` and `browser_viewport_presets` MCP tools
- `src/components/browser/BrowserPane.tsx` — screenshot rendering, toolbar, address bar, reactive `stream_url` reconnect on URL change, auto-syncs viewport on pane resize via legacy `width`/`height` payload (which the new socket handler accepts unchanged)
- `src/components/browser/InspectorPanel.tsx` — browser inspector/DevTools panel
- `docs/reference/BROWSER-AGENT-COMMANDS.md` — CLI and socket command reference
- `docs/archive/browser-stream-fix.md` — landed cross-platform stream-stability plan (PID tracking, single canonical key, atomic teardown, symmetric bind probe, reactive frontend reconnect)
