# Features

- Purpose: Complete inventory of user-facing features in Codemux.
- Audience: Website docs, contributors, product reference.
- Authority: Canonical feature list.
- Update when: Features are added, removed, or significantly changed.
- Read next: `docs/reference/SHORTCUTS.md`, `docs/reference/ARCHITECTURE.md`

## Workspaces

- Create workspaces with preset pane layouts (1, 2, 4, 6, or 8 terminal slots, or shell+browser)
- Create workspaces at a specific directory path
- Lazy workspace creation (Beta-gated): sidebar `+` and boot-into-Home open a client-side chat draft; the draft is promoted on first message send
- Switch workspaces via sidebar click or Ctrl+]/[
- Rename workspaces by double-clicking the active workspace name in the sidebar
- Archive local workspaces non-destructively with all child sessions closed; attach-in-place and host-bound workspaces use non-destructive Close instead
- Auto-transition to main workspace after merge+delete
- Sidebar workspace inbox — the expanded sidebar is one flat list of workspace cards (repo eyebrow · work title + issue chip · blocker line when the agent needs you · mono meta line with branch/↑ahead/+−/PR chip/remote/notifications) with agent state on the right (working indicator / pulsing "Needs you" / "Done · review" / idle time)
- Project filter dropdown (All projects + per-repo rows with active-workspace counts, pinned add-repo button) above the inbox filters both active cards and settled rows; search box opens the command palette
- Settle/un-settle — hover a card for "✓ Settle" to sweep it into a persisted one-line "Settled" section (violet merge icon for merged PRs); hover a settled row to Un-settle. Purely visual: nothing is archived or deleted
- Work-based card naming — while an agent is live and an issue is linked, the card titles itself after the issue (+ `#n` chip) and keeps the branch on the meta line
- Show git stats toggle (Settings → Appearance → Sidebar) to hide the ↑ahead and +/− numbers on cards
- "Checkout default branch" workspace action
- Derivative-branch picker with icons, recency, and worktree tab
- Notification count badges per workspace in the sidebar
- Window focus indicator (green dot) in sidebar header
- Per-project avatar in the sidebar project header: paste an image URL, data URL, or a website/domain (websites resolve through a favicon service); re-saving or re-opening the picker cache-busts the derived favicon so a changed site icon refreshes instead of serving stale bytes

## Tabs

- Multiple tabs per workspace (`terminal`, `browser`, full `diff`, and `editor`)
- Terminal tabs each get their own independent pane surface with split support
- Browser tabs open a full-pane embedded browser
- Diff tabs with unified and split views, syntax highlighting, and section filtering
- Editor tabs with CodeMirror 6, language detection, and markdown preview
- Add tab via "+" dropdown or keyboard shortcuts (Ctrl+T new terminal tab, Ctrl+Shift+D split pane right)
- Close tabs with X button or Ctrl+W (cannot close the last tab)
- Switch tabs by clicking or Ctrl+1 through Ctrl+9
- Tab state persists when switching between workspaces

## Presets & Launchers

- Quick-launch preset bar for coding agents (Claude Code, Codex, OpenCode, Gemini, Antigravity, Copilot, Cursor Agent, Amp, Grok, Droid, Mastracode, Pi, Shell, Chat Agent) — each agent preset launches in its CLI's skip-permissions / YOLO mode
- Two preset kinds: **raw command** presets (literal shell command lines) and structured **"agent launcher"** presets (pick an agent + model + reasoning + a prompt using the same capability-driven pickers as the New Workspace dialog)
- Create custom presets from the UI via a **New preset** button (Settings → Presets header, or the preset-bar gear menu); edit, delete, pin/unpin, and reorder presets
- Auto-run a preset on workspace creation or new tab; agent-context injection for supported agents
- Preset failures surface as toasts

## Terminals

- xterm.js with WebGL rendering, gated by a hardware-GL probe (DOM renderer on software-rendered WebGL and Linux WebKitGTK; DOM fallback also on context loss / missing WebGL2)
- Kitty keyboard protocol support for enhanced key reporting in agent tools
- Custom key handlers: Ctrl+Backspace (kill word), Ctrl+Shift+C/V (copy/paste)
- Terminal theme syncs with Omarchy color palette (foreground, cursor, selection, 16 ANSI colors)
- Terminal background uses fixed neutral shell palette
- PTY resize auto-syncs when pane resizes
- Terminal lifecycle status overlay (starting, ready, exited, failed with exit code)
- Shell font family configurable via Omarchy shell appearance
- Live working-directory hint in the pane header, shown only when the session has moved off the workspace root (`Terminal · src-tauri`); OSC 7 from shell integration is the primary source with a Linux `/proc` poll fallback, deep paths elide to a 2-segment tail, `$HOME` contracts to `~`, and the hover tooltip carries the full path

## Panes

- Split panes horizontally or vertically from pane header buttons
- Resize splits by dragging the handle between panes
- Cycle between panes via command palette (Focus Next Pane / Focus Previous Pane)
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
- Native-feel manual interaction: drag-to-select text, hover effects, double/triple-click selection, right/middle-click, a live cursor mirroring the remote page, and a host-clipboard bridge (Ctrl/Cmd+C/X/V)
- Loading spinner and error banner display
- Agent-driven browser mode for automated testing
- Viewport presets via `codemux browser viewport <mobile|tablet|desktop|WxH|reset>` — real CDP-driven viewport resize so CSS media queries fire and screenshots capture at the simulated dimensions
- Per-workspace browser sessions stable across concurrent worktrees (PID-tracked daemons, single canonical key, atomic teardown)
- Browser inspector / DevTools panel

## Agent Chat (default interface — opt-out back to CLI via Settings → Interface)

- In-app chat pane that talks to CLI coding agents (Claude, Codex, OpenCode) through a conversational UX instead of a raw terminal
- Three providers behind one unified picker:
  - Claude via Claude Agent SDK (Bun-compiled sidecar)
  - Codex via `codex app-server` JSON-RPC
  - OpenCode via Rust-direct HTTP against a managed `opencode serve` child (federates 100+ upstream providers behind one rail entry)
- Streaming transcript with messages, tool approvals (per-tool body rendering), plan proposals (ExitPlanMode), AskUserQuestion panels, thinking indicator
- Agent-produced images from supported tool-result blocks render as safe inline thumbnails with a lightbox; malformed, non-image, and unsafe-URL blocks remain visible as raw fallback content
- Mode pills: Ask / Allow always / Plan / Debug, with Shift+Tab cycling
- Attachments via `+` and `@`: files, folders, GitHub issues + PRs, images via paste / drop / picker
- Slash command popup with cross-provider parsing, GUI-local `/model` and `/default`, Claude `/workflow`, skills, and live provider-native Claude command discovery
- Cross-provider skill system (watcher, conflicts, disable, refined compat)
- Server-side skills sync across devices (plaintext name/content, encrypted at rest; no client-held key, so OAuth/SSO users sync with no password prompt)
- Permission settings page with per-tool body rendering and AllowAlways rule persistence
- Session history selector + transcript persistence + replay on session resume
- Cross-provider subagent card, docked live-activity bar, and read-only child-transcript drill-in
- Claude workflow orchestration card + conditional right-panel phase/agent drill-in
- Follow-up queueing with "Send now" steering, dead-run detection, and one-click interrupted-run continuation
- Context Row under the composer plus detached background-browser status/peek/promotion in GUI mode
- Run checkpoints (opt-in via Settings → Agent): background working-tree snapshot at session start, restore button in the pane header rolls the workspace back to the pre-run state
- Permission-mode mid-session restart
- Favorites on the model picker with `localStorage` persistence
- Debug-mode banner with explicit exit dialog
- Plain-quit on interface toggle flip (no auto-restart) to keep user data intact

## Notifications

- Workspace-scoped alert notifications with severity levels (info, attention)
- Sidebar notification section with unread badge counts
- Expandable alert list with message preview (2-line clamp) and timestamps
- Mark all read button
- Desktop notifications via system notification daemon (notify-rust)
- Desktop notification triggers window focus, raise, and Hyprland window manager integration
- Notification sound toggle in sidebar footer
- **Per-worktree mute** (right-click → "Mute notifications"): silences agent-completion notifications for one workspace; muted state shows a bell-off icon in the sidebar row
- Agent status indicators (red/amber/green dots) light up for Claude, Codex, Gemini, OpenCode, and Pi sessions
- Global toast notices for errors and status messages (bottom-right)

## Resource Monitor

- CPU-chip icon in the title bar opens a popover showing CPU + memory usage for Codemux itself and every live terminal process tree
- Per-terminal usage summed across the full process subtree, grouped Project → Workspace → Session (collapsible, sortable by Memory / CPU / Name)
- App self-usage split into main / web view / other buckets
- Host RAM-share readout with a severity-colored progress bar; amber/red dots flag elevated usage
- Memory reported as PSS (proportional set size) on Linux so shared WebKit/Chromium/node pages aren't overcounted; RSS/working-set on macOS/Windows
- Click a workspace or session row to activate it
- Toggle via Settings → Appearance (`appearance.show_resource_monitor`, on by default)

## Project Memory

- Project brief, current goal, current focus, and constraints fields
- Entry types: pinned context, decisions, next steps, session summaries
- CLI access: `codemux memory show`, `codemux memory set`, `codemux memory add`
- Handoff packet generator: `codemux handoff`
- Persisted to `.codemux/project-memory.json` in project root
- No frontend drawer/panel UI yet (backend and CLI only)

## Theming

- Omarchy theme integration — accent, success, danger, attention colors from system theme
- Fixed neutral dark shell palette (sidebar, headers, borders stay constant across themes)
- Terminal colors fully theme-reactive (text, cursor, ANSI palette change with theme)
- Sans-serif font for shell chrome, monospace for terminal content and code paths
- Shell font family customization via backend config
- Fallback Tokyonight-inspired theme when Omarchy unavailable
- Color palette variant (cool / warm) and spacing density (comfortable / compact) in Settings → Appearance
- Configurable sidebar working indicator — glyph (braille / ring / blink / sweep / typing) and color (amber / white / ember / green / sky / violet; no red, reserved for needs-input)

## Persistence

- Full workspace layout persists across restarts (tabs, pane trees, surfaces, titles)
- Terminal scrollback saved to disk on close and restored on open (including inactive tabs)
- Adapter-based session resume for CLI tools (Claude Code `--resume`/`--continue` via hook-captured session IDs)
- Alternate screen buffer detection — TUI panes get clean shells instead of garbled content
- Browser sessions restored with URL history
- Notification state persisted
- Notification sound preference persisted
- Done-review checkmarks survive a restart — only `Review` pane statuses whose pane still exists are persisted; working/permission states are deliberately dropped (they would describe processes that died with the app) along with entries for panes that no longer exist
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

## Changes Panel

- Right sidebar panel showing git diff for current workspace
- Stage/unstage individual files or all changes
- Commit with message editor
- Push to remote
- AI commit message generation
- Opens diff tabs with unified and split views

## PR Integration

- Right-sidebar Review panel with PR header, CI checks, and read-only review threads
- PR creation with title, body, base branch, and draft toggle
- Incoming PRs list with fork checkout into worktree workspaces
- PR badge display in sidebar workspace rows
- Review submission, merge controls, deployments, and conflict-resolution entry points are intentionally absent from the current resting layout; their backend commands remain registered for a possible re-wire

## GitHub Issues

- Link issues to workspaces from creation dialog
- Issue picker with search
- Auto-branch naming from issue title and number
- Prompt auto-injection of issue context for agents
- Sidebar display with detail popover
- CLI: `codemux issue list/view/link`

## AI Merge Resolver

- Backend-complete AI merge conflict resolution on temporary branches, **currently unreachable from the UI** because both launch buttons were removed by the refined-minimal panel pass
- Safety model: never touches real branches without explicit user approval
- Temporary branch creation (`bot/resolve-<branch>-into-<base>-<n>`), resolution, diff review, approve/reject
- Configurable CLI tool and model for the resolver agent
- Full state machine and five registered Tauri commands remain intact; `src/stores/ai-merge-store.ts` currently has zero importers

## MCP Server (Codemux as server)

- JSON-RPC 2.0 MCP server over stdio transport (**55 tools**)
- Three-tier browser automation: DOM selectors, CDP coordinates, OS-level input
- Workspace, pane, notification, and git tools for agent self-orchestration
- Browser viewport presets (`browser_viewport`, `browser_viewport_presets`)
- Phase 1 vexis-agent tools: `terminal_write`, `terminal_read`, `workspace_open`, `app_status`, `port_list`
- Phase 1.5 vexis-agent tools: `worktree_create`, `preset_apply`, `preset_list`
- Phase 1.6 vexis-agent lifecycle + issue tools: `workspace_close`, `pane_close`, `issue_list`, `issue_get`, `issue_link_workspace`
- Automation tools: `automation_list`, `automation_get`, `automation_create`, `automation_update`, `automation_delete`, `automation_pause`, `automation_resume`, `automation_runs`
- Workspace archive tools: `workspace_archive`, `workspace_unarchive`, `workspace_archive_list`
- Auto-configuration for Claude Code and Claude Desktop
- Launched via `codemux mcp`

## MCP Host (Codemux as host)

- Runs user-installed MCP servers as first-class infrastructure
- Discovers configs across Codemux / Claude / Cursor paths
- Spawns each server once, dedupes identical configs
- Exposes tools to the Claude SDK via an in-process facade with dynamic `setMcpServers` refresh
- Settings panel and composer `+` popup surface enable/disable + status badges + tool list modal + 50-tool cap warning
- Codex MCP support planned via HTTP gateway (Step 11)

## Auth & Sync

- GitHub OAuth, email/password with email verification
- AES-256-GCM encrypted token storage with machine-bound key
- Auth derivation: Argon2id → HKDF-SHA256 → `AuthSecret` (server) via `derive_auth_secret`; the sibling `EncryptionKey` (client-only) is now a Vexis-only protocol canary since skills moved server-side in PR #112
- Cross-product byte-identical with Vexis (pinned in CI)
- Server-side skills sync across devices (plaintext name/content, encrypted at rest; no client-held key, so OAuth/SSO users sync with no password prompt)
- Settings → Account → Sync surface with live status + relative-time + Export/Import controls (two-state ready/sign-in dashboard)
- Per-user synced settings with server sync, offline cache, and dirty flag

## Command Palette

- Ctrl+K opens fuzzy-search overlay with all major actions
- Action groups: Workspaces, Panes, Tabs, Search, View, Navigation
- Dynamic workspace switching entries
- Keybind hints shown inline, reflecting user customizations
- Quick access to split, close, search, settings, and MCP regeneration

## Workspace Creation

- Multi-step creation dialog with task description, branch selection, and agent preset
- Model + reasoning (+ Claude context-window) selection before launch, via a model pill next to the agent picker — appears for any preset launching a modeled CLI (`claude`/`codex`/`opencode`/`gemini`); "Default" emits no flag
- AI-generated branch names from task description
- GitHub issue linking with auto-branch naming and prompt context injection
- PR linking with branch auto-fill
- File attachments appended to agent prompt — staged as chips with per-type badges (image thumbnails, "Pasted image" labels for clipboard pastes)
- Project onboarding flow with package manager detection and setup script configuration
- Orphan worktree detection and import
- First-class non-git folder mode with git-only controls hidden and an opt-in bare `git init` action
- Agent Chat first-send scope row for location/checkout/branch, with deferred worktree creation and automatic worktree naming

## Workspace Archive

- Primary sidebar lifecycle action for local workspaces: archives the row without deleting files, worktree, or branch
- Undo toast plus Settings → Archive restore list, grouped by project with stale-entry hints
- Protected repo roots can be archived but never deleted; dirty worktree deletion requires an explicit force escalation
- Archive Project processes worktrees before the root and records entries only after each close succeeds
- Archive state is device-local; attach-in-place and host-bound workspaces use Close because a lossless archive cannot preserve their binding

## Setup & Teardown Scripts

- Run commands automatically on workspace create/delete, plus a "run" command for dev servers (`Ctrl+Shift+G` opens a dedicated "Workspace Run" terminal tab)
- Setup commands run after worktree creation (background, non-blocking); teardown runs before deletion
- Config precedence: `.codemux/config.json` in the workspace dir → repo root → Settings → Projects UI
- Essential for worktree workflows where each worktree needs its own dependency install, env files, or service setup

## IDE Integration

- Auto-detect 19 installed editors across four families: **VS Code family** (VS Code, Cursor, VSCodium), **Modern editors** (Zed, Windsurf, Trae, Fleet, Lapce), **JetBrains** (IntelliJ IDEA, PyCharm, PhpStorm, WebStorm, GoLand, RubyMine, CLion, Rider, DataGrip, Android Studio), and **Other** (Sublime Text)
- Title bar launcher button with default editor and dropdown partitioned into labelled sections (section headers only render when more than one family is detected)
- Workspace context menu "Open in Editor" entry (same family grouping)
- Default editor preference synced across devices
- On Windows, detection falls back to `%LOCALAPPDATA%\Programs` / `%ProgramFiles%` paths when `PATH` lookup misses (VS Code / Cursor / VSCodium / Zed / Windsurf / Trae / Lapce); JetBrains stays PATH-only via Toolbox shims

## File Tree

- Right sidebar panel showing workspace directory structure
- Lazy-loaded directory expansion with caching
- File type icons and size display
- Gitignored items shown with reduced opacity
- Hidden files toggle (persisted in settings)
- Click to open files in built-in editor

## Search

- File name search (Ctrl+Shift+P) via fd/find with fuzzy matching
- Content search (Ctrl+Shift+F) via rg/grep across workspace files
- Results displayed in overlay with file path, line number, and match preview
- Click to open result in built-in editor or jump to terminal

## Code Indexing

- Local full-text search index at `.codemux/index.json`
- 40-line file chunking with symbol extraction
- Automatic file watching with debounced rebuild
- CLI access: `codemux index build`, `codemux index status`, `codemux index search`
- 512 KB per-file limit, 50 MB total index cap
- Supports 20+ file extensions (rs, ts, tsx, js, jsx, py, go, java, etc.)

## Port Detection

- Automatically detects listening TCP ports that dev servers open and lists them in the sidebar with port number, process name, and optional label
- Open a detected port in the browser pane, or kill the owning process
- Linux scan via `/proc/net/tcp{,6}`; Windows via `netstat -ano` + `tasklist` with a kernel/service process-name filter
- **Docker-published container ports** for open codemux worktrees surface under a dedicated **Docker** group labeled by container name — recovers ports the Linux `/proc` scan can't attribute because `docker-proxy` runs as root; kill is hidden for these rows
- Static port labels via `.codemux/config.json`; system ports (22/80/443/5432/3306/6379/27017) and Codemux-internal ranges are filtered out
- macOS port detection is not yet implemented

## Settings Panel

- Centralized configuration overlay (Ctrl+,)
- 19 sections (`ALL_SECTION_IDS` in `settings-view.tsx`): Interface, Account, Appearance, Editor, Terminal, Presets, Projects, Archive, Git, Agent, Permissions, Skills, MCP, Hosts (labeled "Devices"), Remote Access, Browser, Shortcuts, Notifications, Session Restore. Sync is a subsection inside Account, not a section of its own
- Keyboard shortcut editor with conflict detection and search
- Server-synced settings with offline cache
- Workspace-level project config (setup/teardown scripts, worktree includes)
- Interface section with the Agent Chat GUI master toggle, default on (controls `enable_agent_chat` and `enable_lazy_workspace_creation` together; off = classic CLI interface)

## Remote Hosts + Workspace Push

- Settings → Hosts pane with SSH probe + bootstrap-install consent modal
- Full `codemux-remote` server binary (bundled per-target into the laptop app) installs to `~/.local/bin/` on the host; upload uses an `ssh-cat` pipeline to dodge OpenSSH 9+'s broken scp tilde-expansion
- SSH transport: probe → bootstrap → tunnel → tunnel supervisor with auto-reconnect
- `WorkspaceSnapshot.host_id` model + shared `<DevicePicker>` pill component wired into the new-workspace dialog
- **Push workspace to host** action (zero-touch): rsync the worktree, spawn the remote daemon, install + start a systemd user unit, drop a per-workspace `.mcp.json`, register the workspace in the daemon's registry, attach the local UI through an SSH-forwarded socket, and sync the Claude conversation across local/remote ends
- **Background `hosts_upgrade` poller**: ~5 s after every app start, silently re-bootstraps any host whose `codemux-remote` version differs from the bundled binary (defers the restart while host agents are running so an upgrade never kills live work)

## Web Remote Access

- Default-off embedded HTTP+WebSocket server serves the real Codemux UI to another browser and forwards the same Tauri invoke/event/channel contract through a `__TAURI_INTERNALS__` shim
- Pairing-token and opt-in same-account admission, revocable sessions, approval mode, and `all` / `tailscale` / `loopback` bind scopes
- Mirror-mode multi-client terminal and Agent Chat fan-out; web-specific file picker, notifications, asset serving, browser-pane proxy, and update handoff
- `codemux remote enable|disable|pair` controls a running GUI instance from the CLI
- `codemux serve [--scope …] [--port N] [--relay]` boots the full backend headlessly on Tauri `MockRuntime`, prints a pairing QR/link, and runs until SIGINT/SIGTERM
- Account/iroh/hosted-client code has landed default-off; `app.codemux.org` and the `api.codemux.org` device registry still require gated deployment

## Workspaces Overview & Cross-Device Sync

- Full-screen **Workspaces** overlay (sidebar button under Automations) listing every workspace this account tracks — local + every host pushed-to + every sibling device on the same account
- Device-grouped sections (This device / each configured host / removed-host orphans); same-project workspaces cluster under a project header; configured-host buckets stay visible even when empty
- Filters: search (title / branch / project), project, device, status; sort by recently-active / name / branch
- Per-row actions: open, copy branch, rename, push to any host, pull back, delete; live agent-status dots (working / needs-input / ready-to-review) shared with the sidebar
- **Cross-device sync**: every create / rename / push / pull / delete propagates through the shared API on a 30 s loop, scoped per-user; sibling-device workspaces render as dashed cards under the right host bucket
- **Asymmetric auto-publish from `codemux-remote` hosts**: workspaces an agent creates directly on a host surface in every device's overview within ~90 s without an explicit push (60 s SSH inventory poll → cloud push)
- **Sibling-device adoption** ("Pull to this device"): host-backed rsync when both devices share a host, clone-from-git fallback otherwise; one-click **"Pull project"** materialises the protected repo root then recreates each worktree under it
- **Repo-unit sync**: a repo's default-branch root is a protected `repo root` (close/detach only, never deleted-as-worktree); legacy divergent full copies show an amber `standalone copy` chip with a non-destructive "Reconcile copy…" action
- **Safety guardrails**: confirm-before-push (per-host "don't ask again") + 10-second Undo toast on every push / pull / adopt; cross-device HEAD-divergence chip; elapsed-time pill while a push or pull is in flight
- First-run welcome banner + "how it works" popover

## Operate a Remote Workspace In Place ("Open on host")

- "Open on host" action on a host-backed overview row drives a host workspace **in place** — terminal/agent runs on the host in its real directory, output streams live, and **nothing is copied under `~/.codemux/` locally**
- The third remote mode alongside Push (copy up) and Pull (copy down): use a remote machine's workspace from the desktop without ever materialising a local copy
- Survives app close: the host pty-daemon is detached (reused via a per-socket pidfile, or spawned with `setsid` / `nohup`), so closing the app leaves the host process running and reopening re-tunnels and reattaches the live sessions
- Renders an "on host" badge; the only teardown is detach ("Close — leave running on host") — never delete / push / pull
- Requires the workspace's host to be configured locally (an SSH path is needed); local-FS surfaces (file tree, Changes, ports) are empty for an attach-only workspace

## MCP-on-Remote (Headless Codemux Daemon)

- `codemux-remote serve` runs an axum HTTP server on loopback with a bearer-token manifest at `<state-dir>/manifest.json` (mode `0600`)
- 12-tool stdio MCP catalog (smaller than the desktop's 55 — no panes, no browser, no global notifications): `workspace_{create,list,info,update,close}`, `worktree_create`, `terminal_{spawn,write,read,list,close}`, `app_status`
- `codemux-remote mcp` bridges agent CLIs on the host to the daemon over HTTP — drop-in MCP server entry for Claude Code / Codex / Cursor on the remote
- On every `serve` startup, idempotently writes a `codemux` MCP entry into every supported user-level agent config it finds (`~/.claude.json`, `~/.codex/config.toml`, `~/.cursor/mcp.json`)
- `Identity` enum on every dispatch carries `Local` today and reserves `Cloud { user_id, org_id, role }` for a future optional paid-tier relay — purely additive
- Process supervision via systemd user unit (provisioned at host bootstrap) with `Restart=on-failure` and `loginctl enable-linger`
- Unix-only; Windows build is a no-op stub

## Persistent Agents

- Every shell spawn runs inside a detached `codemux pty-daemon` subprocess so agents survive app close
- Supervisor adopts the running daemon on relaunch and reattaches live sessions (alt-screen TUIs and CLI agents resume in-place)
- Default-on, no setting; `CODEMUX_DISABLE_PTY_DAEMON=1` is the only escape hatch
- Graceful fallback to in-process portable-pty at every error site; 3-failures-in-60s crash circuit breaker
- Hook notifications survive an app restart: stale inherited hook ports retry the current port published at `~/.codemux/hooks/active-port`
- Unix only (Linux + macOS); Windows still uses the in-process path until the named-pipe IPC is wired

## Automations

- Scheduled agent runs: a named prompt + agent + RFC 5545 (iCalendar) recurrence that fires on a user-chosen host
- Each fire creates an isolated git worktree and runs the agent headlessly (`claude --print` / `codex exec`)
- Real terminal status per run: `succeeded` / `failed` / `skipped_offline` / `skipped_busy`; same-automation overlap is serialised
- First-class **Automations view** in the left sidebar: list + detail pane, frequency/time/weekday schedule builder (raw RFC 5545 escape hatch), per-automation run history, per-row health dot
- Account-synced across devices via `automations_sync` against the live `/api/automations` endpoints; `automation_runs` stay per-device
- Host routing: the desktop runs `host_id IS NULL` automations; `codemux-remote scheduler` (a systemd user service provisioned at host bootstrap) runs host-targeted ones; a stuck-run reconciler fails crashed runs at startup
- GitHub backbone: a remote host clones/fetches the project's git remote with its own credentials; per-repo `git ls-remote` preflight flags an unreachable repo at setup
- Surface: seven `automations_*` Tauri commands + `automations_check_repo_access`, eight `automation_*` MCP tools, and seven control-socket commands (`automation_list/get/create/update/set_enabled/delete/runs` — the socket has no `automation_pause`/`automation_resume`)

## CLI / Socket Control

- Local IPC transport: Unix socket at `$XDG_RUNTIME_DIR/codemux.sock` (Linux/macOS) or named pipe at `\\.\pipe\codemux-{username}` (Windows)
- JSON request/response protocol with command routing
- Single-instance enforcement (checks for an already-live listener before starting a new one)
- External tool integration (opencode, claude-cli, MCP server can send commands via the local transport)
- Remote-access CLI controls (`codemux remote enable|disable|pair`) and full headless web-remote entrypoint (`codemux serve`)

## Important Touch Points

- `src/App.tsx` — App root, state init, keyboard shortcuts
- `src/components/layout/` — App shell (AppSidebar, WorkspaceMain, TabBar, PaneNode, RightPanel)
- `src/components/terminal/TerminalPane.tsx` — xterm.js terminal with PTY connection
- `src/components/ui/` — shadcn primitives (sidebar, tabs, resizable, etc.)
- `src/stores/app-store.ts` — zustand global state from Tauri backend
- `src/tauri/commands.ts` — typed Tauri invoke wrappers (~320 commands)
- `src/hooks/use-keyboard-shortcuts.ts` — global keyboard shortcuts
- `src-tauri/src/state/state_impl.rs` — Backend state management
- `src-tauri/src/commands/workspace.rs` — Tauri command handlers
- `src-tauri/src/control.rs` — control server (Unix socket on Linux/macOS, named pipe on Windows)
