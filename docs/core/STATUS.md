# Codemux Status

- Purpose: Canonical reality snapshot for the repo.
- Audience: Anyone deciding what is actually true today.
- Authority: Current implementation truth.
- Update when: Behavior, constraints, or known gaps change.
- Read next: `docs/core/PLAN.md`, `docs/core/TESTING.md`

## Current Headline

Codemux is approaching Linux MVP. The workspace shell, terminal management, git integration, and ADE features are real and daily-drivable. OpenFlow and browser pane are still being hardened.

The repo structure is clean and domain-split:

- `src/` is the React + Tailwind + shadcn UI and Tauri IPC layer
- `src-tauri/` is the Rust app/runtime layer
- `src-old/` preserves the previous Svelte frontend for reference

## Solid — Daily-Drivable Features

- Workspace shell, sidebar, workspace sections with color coding and drag-drop
- Multi-session terminals with xterm.js, WebGL rendering, kitty protocol
- Tab bar with terminal/browser tab types
- Pane splits, resize, drag-swap, close
- Git worktree-based workspaces (create from new/existing branch, import orphans)
- Built-in diff viewer / Changes panel (right sidebar, stage/unstage/commit/push)
- File tree panel (right sidebar, lazy-loaded, opens in external editor)
- Search: keyword search (Ctrl+Shift+F via rg) and file name search (Ctrl+P via fd)
- Git sidebar enrichment (branch, ahead/behind, diff stats, PR badge)
- Port detection (auto-scan, sidebar display, open in browser)
- Terminal presets with quick-launch bar (Claude Code, Codex, OpenCode, Gemini)
- IDE integration (detect editors, open workspace, Ctrl+Shift+E)
- Command palette (Ctrl+K, fuzzy search across all actions)
- PR integration (create, view, checks, merge via gh CLI, auth status check)
- Setup/teardown scripts (.codemux/config.json)
- Workspace creation from branch with layout + preset selection
- Notifications with D-Bus, Hyprland focus, attention badges
- Local project memory and lexical indexing
- CLI and socket control
- Global overlay manager (single overlay at a time)
- Neutral dark shell theming with Omarchy accent sync
- Sans-serif shell chrome, monospace terminals

## Partial / Being Hardened

- Browser pane: screenshot-driven, functional but lower fidelity than native
- OpenFlow: orchestration works but large-run reliability and intervention flow still maturing
- Session persistence: layout persists, scrollback lost on restart
- Browser automation depth: basic commands work, missing wait conditions and DOM inspection

## Known Constraints

- Notification click-to-focus on Wayland and mako still needs deeper D-Bus or native handling
- Control socket is local-user only and currently unauthenticated
- Notification sound toggle exists in state, but actual audio playback is not implemented
- The legacy Chromium/CDP runtime still exists in-tree, but the canonical visible browser path is `agent-browser`

## React Frontend Rebuild Status

The frontend was rebuilt from Svelte to React + Tailwind v4 + shadcn (branch `feature/react-frontend-rebuild`). The Rust backend is unchanged.

### Working in React frontend

- App shell: shadcn Sidebar with collapsible workspace sections, tab bar, right panel
- Workspace list from real Tauri backend data (zustand + app-state-changed events)
- Terminal panes with xterm.js + canvas renderer + PTY via Tauri Channel
- Pane splits (horizontal/vertical) with CSS Grid, resize handles, drag-to-swap
- Right panel: flexbox layout with pointer-based resize (320px default, 240-500px range)
- Keyboard shortcuts: Ctrl+Shift+D split, Ctrl+Shift+W close pane, Ctrl+T new tab, Ctrl+W close tab
- Semantic theming: shadcn oklch dark mode + custom --success/--danger/--warning tokens
- Terminal theme reads dynamically from CSS variables via MutationObserver
- New workspace creation from sidebar "+" button
- Tauri bridge: 80+ typed command wrappers, 8 event helpers, all types ported

### TODO — Not yet ported from old Svelte frontend

- Right panel content: Changes panel, File tree, PR panel
- OpenFlow UI (orchestration view, agent config, communication panel)
- Command palette (Ctrl+K)
- Search panels (file search, content search)
- Browser pane rendering
- Workspace drag-and-drop reordering in sidebar
- Context menus (workspace, section, pane)
- Terminal presets bar
- Notification sounds
- Settings panel
- Memory drawer

## Read This With

- `docs/core/PLAN.md` for build order
- `docs/core/TESTING.md` for verification policy
- `docs/features/browser.md` or `docs/features/openflow.md` for subsystem detail
