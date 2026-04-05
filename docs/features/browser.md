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

- `src-tauri/src/agent_browser.rs` — `AgentBrowserManager`, stealth flags, stream port allocation, spawning
- `src-tauri/src/commands/browser.rs` — Tauri commands for pane creation, URL navigation, automation
- `src/components/browser/BrowserPane.tsx` — screenshot rendering, toolbar, address bar
- `src/components/browser/InspectorPanel.tsx` — browser inspector/DevTools panel
- `docs/reference/BROWSER-AGENT-COMMANDS.md` — CLI and socket command reference
