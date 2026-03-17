# Browser Capability

- Purpose: Describe the current browser subsystem, what it can do, and what constraints still matter.
- Audience: Anyone working on browser features, automation, or browser-based validation.
- Authority: Canonical browser capability and constraints document.
- Update when: Browser behavior, user expectations, or known limitations change.
- Read next: `docs/plans/browser.md`, `docs/reference/CONTROL.md`, `AGENTS.md`

## Current Model

The browser pane currently uses a screenshot-driven Chromium session backed by `agent-browser`. The visible pane and the explicit CLI browser commands share the same browser session and now use the same internal execution helpers.

## What Works Today

- create a browser pane inside a workspace
- navigate with the address bar or `codemux browser open`
- keep browser panes inside split layouts next to terminals
- let agents use `snapshot`, `click`, `fill`, `screenshot`, and `console-logs`
- show user-visible browser updates through repeated screenshot refreshes
- open an external browser when a separate browser window is the better fallback

## Expected Operating Model

- agents control the browser programmatically
- users see the browser pane as live evidence of that work
- this is usable now, but it is not the final browser architecture yet

## Current Internal Boundary

- canonical path: `agent_browser` commands plus `AgentBrowserManager`
- CLI browser commands delegate to the same `agent-browser` execution path
- a legacy Chromium/CDP runtime still exists in `src-tauri/src/browser.rs`, but it is not the primary pane path

## Current Constraints

- not a native embedded Tauri webview
- lower interaction fidelity than a real embedded browser because the pane is screenshot-driven
- toolbar behavior, back and forward, reload, and arbitrary text entry still need focused validation
- browser console capture is not yet a full live log stream from the displayed pane
