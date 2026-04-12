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
- Tab bar with terminal, browser, editor, and diff tab types
- Pane splits, resize, drag-swap, close
- Git worktree-based workspaces (create from new/existing branch, import orphans)
- Changes panel in right sidebar (stage/unstage/commit/push, inline per-file diffs, AI commit messages)
- Full-pane diff viewer tab (unified/split layouts, section filters incl. `against_base`, hunk/file navigation, focus mode)
- File tree panel (right sidebar, lazy-loaded, opens in built-in editor or external editor)
- Search: keyword search (Ctrl+Shift+F via rg) and file name search (Ctrl+Shift+P via fd)
- Git sidebar enrichment (branch, ahead/behind, diff stats, PR badge)
- Port detection (auto-scan, sidebar display, open in browser)
- Terminal presets with quick-launch bar (Claude Code, Codex, OpenCode, Gemini, Pi, Shell)
- IDE integration (detect editors, open workspace, Ctrl+Shift+E)
- Command palette (Ctrl+K, fuzzy search across all actions)
- PR integration (create, view, checks, merge via gh CLI, auth status check, incoming PRs list with fork checkout)
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
- MCP server exposing 29 tools via JSON-RPC 2.0 (browser, workspace, pane, git, notification)
- Session persistence: terminal scrollback saved/restored across restarts, adapter-based resume for CLI tools (Claude Code)

## Partial / Being Hardened

- Browser pane: screenshot-driven, functional but lower fidelity than native
- OpenFlow: orchestration works but large-run reliability and intervention flow still maturing
- AI merge resolver: backend and frontend working, needs testing depth and live validation
- Browser automation depth: DOM commands, coordinate commands, and OS-level input work; wait conditions and JS evaluation added in v0.24.0

## Known Constraints

- Notification click-to-focus on Wayland and mako still needs deeper D-Bus or native handling
- Control socket is local-user only and currently unauthenticated
- Notification sound toggle exists in state, but actual audio playback is not implemented
- Browser automation uses `agent-browser` v0.24.0 (pure Rust binary, direct CDP). The legacy Playwright/Node.js path and the unused `BrowserManager` Rust CDP implementation have been removed.
- Feature docs exist for all major subsystems: auth, auto-update, browser, changes panel, code indexing, command palette, diff viewer, execution backends, file editor, file tree, GitHub issues, hooks, IDE integration, MCP server, merge resolver, notifications, observability, OpenFlow, ports, PR integration, presets, project memory, search, session persistence, settings, settings sync, setup-teardown, terminal, workspace creation, worktree setup

## Windows Support Foundation

Windows support foundation has been merged to main (commit `cc9b946`, 19 commits past the `v0.1.19` tag — not yet in a published release). The work was verified end-to-end via a throwaway test tag against the production release pipeline:

- `cfg`-gates cover every Linux-specific code path — the app compiles on `x86_64-pc-windows-msvc` without unsafe `unix` stubs
- Control socket → named pipe (`\\.\pipe\codemux-{username}`) via `tokio::net::windows::named_pipe`
- Port detection via `netstat -ano` parser (cross-platform pure function, unit-tested on Linux CI)
- Agent-browser port reclamation via `netstat -ano` + `taskkill` with exact-port matching
- OpenFlow disabled at the UI + backend level on Windows (bash wrappers not yet ported) — sidebar shows a greyed-out "OpenFlow is not yet available on Windows" tooltip
- `release.yml` builds on `[ubuntu-22.04, windows-latest]` with `fail-fast: false`; tauri-action merges both platforms into a single `latest.json` so existing Linux auto-updates keep working AND Windows clients auto-update the same way
- NSIS installer produced on Windows CI (`--bundles nsis` to skip MSI which needs WiX)
- 547 Rust tests (+38 from the Windows pass) run on both matrix legs of `ci.yml`

Still gated before a real Windows v1 release:
- Windows Authenticode code signing — SmartScreen warning expected on unsigned first-install, deferred behind a cert budget decision
- OpenFlow bash wrapper rewrite — blocks OpenFlow on Windows
- Tier 3 input injection via Win32 `SendInput` — deferred (Tier 1/2 sufficient for MVP)
- Full PTY lifecycle / worktree / agent-spawn integration tests on a live Windows runner

See `docs/plans/windows-support.md` for the complete checklist and status.

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
- Search: file name search (Ctrl+Shift+P) and content search (Ctrl+Shift+F)
- Browser pane with screenshot-driven rendering and toolbar
- Workspace drag-and-drop reordering in sidebar
- Terminal presets bar with quick-launch
- Settings panel with keyboard shortcuts, appearance, and project scripts
- Auth system with GitHub OAuth, email/password, session encryption
- Synced settings (per-user server-synced with offline cache)
- Semantic theming: shadcn oklch dark mode + custom --success/--danger/--warning tokens
- Terminal theme reads dynamically from CSS variables via MutationObserver
- Tauri bridge: 120+ typed command wrappers, 12 event helpers, all types ported

### Remaining Gaps

- Context menus on pane headers (workspace rows, workspace section groups, tabs, changes panel rows, and sidebar ports section already have them)
- Notification sound playback (toggle exists in settings and state, but no actual audio output)
- Memory drawer UI (backend memory system exists, no frontend drawer/panel yet)
- File editor: no LSP integration, no multi-cursor, no rename/delete from editor

## Read This With

- `docs/core/PLAN.md` for build order
- `docs/core/TESTING.md` for verification policy
- `docs/features/*` for subsystem detail (auth, auto-update, browser, changes-panel, code-indexing, command-palette, diff-viewer, execution, file-editor, file-tree, github-issues, hooks, ide-integration, mcp-server, merge-resolver, notifications, observability, openflow, ports, pr-integration, presets, project-memory, search, session-persistence, settings, settings-sync, setup-teardown, terminal, workspace-creation, worktree-setup)
- `docs/plans/windows-support.md` for the active Windows cross-platform work
