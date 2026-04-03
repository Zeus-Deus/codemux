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

## Solid — Daily-Drivable Features

- Workspace shell, sidebar, workspace sections with color coding and drag-drop
- Multi-session terminals with xterm.js, WebGL rendering, kitty protocol
- Tab bar with terminal/browser tab types
- Pane splits, resize, drag-swap, close
- Git worktree-based workspaces (create from new/existing branch, import orphans)
- Built-in diff viewer / Changes panel (right sidebar, stage/unstage/commit/push)
- File tree panel (right sidebar, lazy-loaded, opens in built-in editor or external editor)
- Search: keyword search (Ctrl+Shift+F via rg) and file name search (Ctrl+P via fd)
- Git sidebar enrichment (branch, ahead/behind, diff stats, PR badge)
- Port detection (auto-scan, sidebar display, open in browser)
- Terminal presets with quick-launch bar (Claude Code, Codex, OpenCode, Gemini)
- IDE integration (detect editors, open workspace, Ctrl+Shift+E)
- Command palette (Ctrl+K, fuzzy search across all actions)
- PR integration (create, view, checks, merge via gh CLI, auth status check)
- GitHub issue integration (link issues to workspaces, issue picker in creation dialog, sidebar display with detail popover, auto-branch naming from issue, prompt auto-injection of issue context, CLI `codemux issue list/view/link`, control socket commands)
- Setup/teardown scripts (.codemux/config.json)
- Workspace creation from branch with layout + preset selection
- Notifications with D-Bus, Hyprland focus, attention badges
- Local project memory and lexical indexing
- CLI and socket control
- Global overlay manager (single overlay at a time)
- Auth system: GitHub OAuth, email/password with email verification, encrypted token storage
- Per-user synced settings with server sync, offline cache, and dirty flag
- Neutral dark shell theming with Omarchy accent sync
- Sans-serif shell chrome, monospace terminals
- Built-in file editor with CodeMirror, syntax highlighting, and markdown preview
- MCP server exposing 26 tools via JSON-RPC 2.0 (browser, workspace, pane, git, notification)

## Partial / Being Hardened

- Browser pane: screenshot-driven, functional but lower fidelity than native
- OpenFlow: orchestration works but large-run reliability and intervention flow still maturing
- AI merge resolver: backend and frontend working, needs testing depth and live validation
- Session persistence: layout persists, scrollback lost on restart
- Browser automation depth: basic commands work, missing wait conditions and DOM inspection

## Known Constraints

- Notification click-to-focus on Wayland and mako still needs deeper D-Bus or native handling
- Control socket is local-user only and currently unauthenticated
- Notification sound toggle exists in state, but actual audio playback is not implemented
- The legacy Chromium/CDP runtime still exists in-tree, but the canonical visible browser path is `agent-browser`

## React Frontend Status

The frontend was rebuilt from Svelte to React + Tailwind v4 + shadcn. The Rust backend is unchanged. The port is complete and the old Svelte frontend has been removed.

### Ported and Working

- App shell: shadcn Sidebar with collapsible workspace sections, tab bar, right panel
- Workspace list from real Tauri backend data (zustand + app-state-changed events)
- Terminal panes with xterm.js + WebGL renderer + PTY via Tauri Channel
- Pane splits (horizontal/vertical) with CSS Grid, resize handles, drag-to-swap
- Right panel with Changes panel, File tree, and PR panel tabs
- OpenFlow UI: orchestration view, agent config, communication panel, agent graph
- Command palette (Ctrl+K) with fuzzy search
- Search: file name search (Ctrl+P) and content search (Ctrl+Shift+F)
- Browser pane with screenshot-driven rendering and toolbar
- Workspace drag-and-drop reordering in sidebar
- Terminal presets bar with quick-launch
- Settings panel with keyboard shortcuts, appearance, and project scripts
- Auth system with GitHub OAuth, email/password, session encryption
- Synced settings (per-user server-synced with offline cache)
- Semantic theming: shadcn oklch dark mode + custom --success/--danger/--warning tokens
- Terminal theme reads dynamically from CSS variables via MutationObserver
- Tauri bridge: 80+ typed command wrappers, 8 event helpers, all types ported

### Remaining Gaps

- Context menus on workspace rows, section headers, and pane headers (Radix primitive exists but not wired up everywhere)
- Notification sound playback (toggle exists in settings and state, but no actual audio output)
- Memory drawer UI (backend memory system exists, no frontend drawer/panel yet)
- File editor: no LSP integration, no multi-cursor, no rename/delete from editor

## Read This With

- `docs/core/PLAN.md` for build order
- `docs/core/TESTING.md` for verification policy
- `docs/features/*` for subsystem detail (browser, openflow, mcp-server, file-editor, merge-resolver, changes-panel, pr-integration, search, notifications, auth, settings-sync, setup-teardown)
