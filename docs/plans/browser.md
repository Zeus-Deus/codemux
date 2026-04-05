# Browser Work Plan

- Purpose: Track active browser implementation work and near-term next steps.
- Audience: Anyone actively changing browser behavior or browser automation.
- Authority: Active browser work plan, not current truth.
- Update when: Browser priorities, unresolved questions, or likely touch points change.
- Read next: `docs/features/browser.md`, `docs/core/STATUS.md`

## Active Priorities

1. Finish a focused manual pass for back, forward, reload, address-bar flow, and text entry on real sites.
2. Harden resize, focus, redraw, and lifecycle behavior inside split layouts.
3. Improve interaction fidelity where the current screenshot-driven pane blocks real workflows.
4. Expose a more useful console log and error stream.
5. Integrate the current browser runtime more cleanly into OpenFlow verification flows.

## Open Questions

- keep the screenshot-driven `agent-browser` model as the long-lived browser surface, or later replace only the display layer with a more native embedded surface
- decide how much rich manual user interaction must be first-class versus agent-driven only

## Likely Touch Points

- `src/components/browser/BrowserPane.tsx`
- `src/components/browser/BrowserToolbar.tsx`
- `src/components/browser/InspectorPanel.tsx`
- `src/stores/app-store.ts`
- `src-tauri/src/agent_browser.rs`
- `src-tauri/src/cli.rs`
- `src-tauri/src/commands/browser.rs`
- `docs/reference/CONTROL.md`
- `AGENTS.md`

## Already Landed

- `agent-browser` v0.24.0 (pure Rust, direct CDP) — Playwright/Node.js path removed
- stealth Chromium flags and realistic user-agent spoofing
- per-workspace browser sessions with dynamic stream ports (9223-9299)
- browser reconnection on pane recreation
- CLI browser commands sharing the same session
- browser data management in Settings (clear cookies, clear all data)
- inspector panel for web content debugging
