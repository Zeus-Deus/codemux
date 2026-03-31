# Features

- Purpose: Complete inventory of user-facing features in Codemux.
- Audience: Website docs, contributors, product reference.
- Authority: Canonical feature list.
- Update when: Features are added, removed, or significantly changed.
- Read next: `docs/reference/SHORTCUTS.md`, `docs/reference/ARCHITECTURE.md`

## Workspaces

- Create workspaces with preset pane layouts (1, 2, 4, 6, or 8 terminal slots, or shell+browser)
- Create workspaces at a specific directory path
- Create OpenFlow workspaces for multi-agent orchestration runs
- Switch workspaces via sidebar click or Ctrl+]/[
- Rename workspaces by double-clicking the active workspace name in the sidebar
- Close workspaces with all child sessions
- Workspace rows show title, git branch, and compact working directory path
- Notification count badges per workspace in the sidebar
- Window focus indicator (green dot) in sidebar header

## Tabs

- Multiple tabs per workspace (Terminal, Browser, Changes)
- Terminal tabs each get their own independent pane surface with split support
- Browser tabs open a full-pane embedded browser
- Changes/Diff tabs (placeholder — viewer coming soon)
- Add tab via "+" dropdown or keyboard shortcuts (Ctrl+T, Ctrl+Shift+B, Ctrl+Shift+D)
- Close tabs with X button or Ctrl+W (cannot close the last tab)
- Switch tabs by clicking or Ctrl+1 through Ctrl+9
- Tab state persists when switching between workspaces

## Terminals

- xterm.js with WebGL rendering (canvas/DOM fallback)
- Kitty keyboard protocol support for enhanced key reporting in agent tools
- Custom key handlers: Ctrl+Backspace (kill word), Ctrl+Shift+C/V (copy/paste)
- Terminal theme syncs with Omarchy color palette (foreground, cursor, selection, 16 ANSI colors)
- Terminal background uses fixed neutral shell palette
- PTY resize auto-syncs when pane resizes
- Terminal lifecycle status overlay (starting, ready, exited, failed with exit code)
- Shell font family configurable via Omarchy shell appearance

## Panes

- Split panes horizontally or vertically from pane header buttons
- Resize splits by dragging the handle between panes
- Resize active pane via keyboard (Ctrl+Alt+arrow keys or Ctrl+Alt+H/J/K/L)
- Cycle between panes with Ctrl+Shift+J/K or Ctrl+H/L
- Drag pane headers to swap panes (visual drop target highlighting)
- Close panes from header X button
- Add browser pane alongside a terminal pane from header "+" button
- Active pane highlighted with accent border glow

## Browser

- Embedded browser with URL address bar
- Navigate by typing URL (auto-prefixes http:// for bare domains)
- Home button (resets to about:blank) and refresh button
- Open current URL in system browser via external link button
- Screenshot-based rendering with 1-second refresh polling
- Click-to-interact on rendered viewport (coordinates mapped from display to actual viewport)
- Loading spinner and error banner display
- Agent-driven browser mode for automated testing

## OpenFlow Orchestration

- Dedicated OpenFlow workspace type with agent configuration panel
- Select agent roles: orchestrator, planner, builder, reviewer, tester, debugger, researcher
- Choose CLI tool (opencode, claude), model, provider, and thinking mode per agent
- Run creation with title and goal specification
- Real-time orchestration view with agent node graph and status indicators
- Communication panel with full message log between agents
- User message injection into running orchestration
- Message delivery tracking (shows which injections have been processed)
- System marker filtering (hides internal protocol messages)
- Auto-follow scroll with "jump to latest" button
- Phase badges showing current orchestration state
- Pause, cancel, and retry controls for runs
- Run status sidebar section with pulsing activity indicators

## Notifications

- Workspace-scoped alert notifications with severity levels (info, attention)
- Sidebar notification section with unread badge counts
- Expandable alert list with message preview (2-line clamp) and timestamps
- Mark all read button
- Desktop notifications via system notification daemon (notify-rust)
- Desktop notification triggers window focus, raise, and Hyprland window manager integration
- Notification sound toggle in sidebar footer
- Global toast notices for errors and status messages (bottom-right)

## Project Memory

- Project brief, current goal, and current focus fields
- Constraints list (one per line)
- Tabbed memory drawer in sidebar (Brief, Goal, Notes)
- Entry types: pinned context, decisions, next steps, session summaries
- Memory stats showing counts per category
- Handoff packet generator with suggested prompt for next session
- Persisted to `.codemux/memory.json` in project root

## Theming

- Omarchy theme integration — accent, success, danger, attention colors from system theme
- Fixed neutral dark shell palette (sidebar, headers, borders stay constant across themes)
- Terminal colors fully theme-reactive (text, cursor, ANSI palette change with theme)
- Sans-serif font for shell chrome, monospace for terminal content and code paths
- Shell font family customization via backend config
- Fallback Tokyonight-inspired theme when Omarchy unavailable

## Persistence

- Full workspace layout persists across restarts (tabs, pane trees, surfaces, titles)
- Terminal sessions are respawned on restart (fresh shell, layout preserved)
- Browser sessions restored with URL history
- Notification state persisted
- Notification sound preference persisted
- Project memory persisted independently per project root
- Debounced disk writes (500ms quiet period) to prevent write amplification

## File Editor

- Built-in code editor using CodeMirror 6 with syntax highlighting
- Open files from file tree or search results as editor tabs
- Language support for 20+ languages (JS, TS, Rust, Python, Go, etc.)
- Markdown rendered preview mode
- Dirty state tracking with modified indicator on tabs
- Custom dark theme matching Codemux shell
- File tree with `.gitignore` awareness and common directory exclusion
- 2 MB file size limit, UTF-8 only, binary file detection

## AI Merge Resolver

- AI-powered merge conflict resolution on temporary branches
- Safety model: never touches real branches without explicit user approval
- Temporary branch creation (`bot/merge-*`), resolution, diff review, approve/reject
- Configurable CLI tool and model for the resolver agent
- Entry points in Changes panel and PR panel
- Full state machine: idle → creating_branch → resolving → review → applying

## MCP Server

- JSON-RPC 2.0 MCP server over stdio transport (26 tools)
- Three-tier browser automation: DOM selectors, CDP coordinates, OS-level input
- Workspace, pane, notification, and git tools for agent self-orchestration
- Auto-configuration for Claude Code and Claude Desktop
- Launched via `codemux mcp`

## CLI / Socket Control

- Unix socket server at `$XDG_RUNTIME_DIR/codemux.sock`
- JSON request/response protocol with command routing
- Single-instance enforcement (checks for existing socket before starting)
- External tool integration (opencode, claude-cli can send commands via socket)

## Important Touch Points

- `src/App.tsx` — App root, state init, keyboard shortcuts
- `src/components/layout/` — App shell (AppSidebar, WorkspaceMain, TabBar, PaneNode, RightPanel)
- `src/components/terminal/TerminalPane.tsx` — xterm.js terminal with PTY connection
- `src/components/ui/` — shadcn primitives (sidebar, tabs, resizable, etc.)
- `src/stores/app-store.ts` — zustand global state from Tauri backend
- `src/tauri/commands.ts` — typed Tauri invoke wrappers (80+ commands)
- `src/hooks/use-keyboard-shortcuts.ts` — global keyboard shortcuts
- `src-tauri/src/state/state_impl.rs` — Backend state management
- `src-tauri/src/commands/workspace.rs` — Tauri command handlers
- `src-tauri/src/control.rs` — Unix socket control server
