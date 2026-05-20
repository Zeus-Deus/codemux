# Codemux Status

- Purpose: Canonical reality snapshot for the repo.
- Audience: Anyone deciding what is actually true today.
- Authority: Current implementation truth.
- Update when: Behavior, constraints, or known gaps change.
- Read next: `docs/core/PLAN.md`, `docs/core/TESTING.md`

## Current Headline

Codemux is past Linux MVP and shipping cross-platform binaries. The workspace shell, terminal management, git integration, presets, settings sync, and most ADE features are real and daily-drivable on both Linux and Windows. Latest released version is `v0.5.2`.

The headline addition since the `v0.5.0` reindex is **Automations** — named prompt + agent + RFC 5545 recurrence schedules that fire on a user-chosen host, create a fresh worktree per fire, run the agent headlessly, and record a real terminal status. Automations are account-synced across devices and surfaced in a first-class left-sidebar view; their eight `automation_*` tools bring the MCP server inventory to **52 tools**. The agent-chat surface (Step 6–12: chat pane, multi-provider picker, skills sync, attachments, mode pills, slash commands, plan proposals, MCP host runtime, …) remains merged to `main` and Beta-gated; the **persistent PTY daemon** is the default spawn path (every shell survives app close, the env-var escape hatch is the only off-switch), the **SSH workspace-push** action that was deferred in step 2d has landed (push a worktree to a user-owned host with Claude conversation sync), and the right sidebar + Settings panel have been redesigned to a **refined-minimal aesthetic** to match the rest of the app. The MCP server inventory grew from 31 to 44 tools across the Phase 1 / 1.5 / 1.6 vexis-agent integration steps before Automations took it to 52.

OpenFlow and the browser pane are still being hardened. OpenFlow is intentionally disabled on Windows until the bash-wrapper rewrite lands.

The repo structure is clean and domain-split:

- `src/` is the React + Tailwind + shadcn UI and Tauri IPC layer
- `src-tauri/` is the Rust app/runtime layer
- `sidecar/claude-agent/` is the Bun-compiled TypeScript subprocess that hosts the Claude Agent SDK

## Solid — Daily-Drivable Features

### Workspace & terminals
- Workspace shell, sidebar, workspace sections with color coding and drag-drop
- Multi-session terminals with xterm.js, WebGL renderer + Unicode 11 widths, kitty protocol, low-latency pane input
- Hidden-pane terminal pause to eliminate cross-workspace typing lag
- Tab bar with terminal, browser, editor, and diff tab types
- Pane splits, resize, drag-swap, close
- **Terminal pane persistence across workspace switch**: xterm.js `Terminal` instance + PTY-output channel survive component unmount via the module-level cache in `src/components/terminal/terminal-cache.ts`. Workspace switches reparent the wrapper into a hidden parking node instead of disposing, so alt-screen TUIs (Claude Code, lazygit, btop, vim) keep rendering correctly on return. Disposal is driven by AppState diffs in `useTerminalCacheGc` so close-pane / close-tab / close-workspace / PTY-exit all reach `disposeTerminal`.
- **Session persistence**: terminal scrollback save/restore across restarts (Windows-only backend backstop in `scrollback::flush_cache_to_disk`), adapter-based resume for CLI tools (Claude Code `--resume`/`--continue` via hook-captured session IDs)
- **Persistent PTY daemon** (`pty_daemon::server` + `client` + `supervisor` + `manifest`): every shell spawn now routes through a detached `codemux pty-daemon` subprocess so agents survive app close. On relaunch the supervisor adopts the running daemon and reattaches live sessions. **Default-on**, no setting; `CODEMUX_DISABLE_PTY_DAEMON=1` is the only escape hatch. Graceful fallback to the in-process portable-pty path on every error site, plus a 3-failures-in-60s crash circuit breaker that disables the daemon path for the rest of the process lifetime. Unix only; Windows still uses the in-process path until the named-pipe IPC is wired.

### Git & GitHub
- Git worktree-based workspaces (create from new/existing branch, import orphans, derivative-branch picker with recency)
- Changes panel in right sidebar (stage/unstage/commit/push, inline per-file diffs, AI commit messages, Open-PR button on toolbar, AI merge resolver entry)
- Full-pane diff viewer tab (unified/split layouts, section filters incl. `against_base`, hunk/file navigation, focus mode)
- Review tab (renamed from PR tab, React Query refactor): PR creation, header, reviews, checks, deployments, merge controls
- Sidebar PR status icon per workspace with stale-clearing on branch switch and DRAFT collapse
- **Default-branch detection** drives the sidebar branch pill: seed from `origin/HEAD`, follow live remote-branch changes, and the derivative-branch picker drops the phantom `origin/<name>` rows so users never pick a remote-only ref by accident
- "Checkout default branch" workspace action
- Git sidebar enrichment (branch, ahead/behind, diff stats, PR badge) with non-blocking activate + visibility-based gate
- Sidebar ahead/behind arrows refresh against fresh remote refs
- Auto-transition to main workspace after merge+delete
- Merge-into-base runs on the blocking pool so it cannot freeze the GUI; uses `update-ref` for worktree compatibility
- Hide stale merged-PR pill on long-lived branches
- Review tab unfreezes on repos with thousands of PRs (paginated fetch)
- All git/gh shell-outs moved off the GTK main thread to keep IPC responsive

### GitHub issues
- Link issues to workspaces, issue picker in creation dialog, sidebar display with detail popover, auto-branch naming from issue, prompt auto-injection of issue context
- CLI: `codemux issue list/view/link`, control socket commands

### Browser
- Screenshot-driven Chromium session backed by `agent-browser` v0.24.0 (pure Rust, direct CDP)
- Browser pane in pane layouts; address bar, refresh, home, external-link
- Per-workspace stream sessions keyed by `workspace_id` (PID-tracked daemons, single canonical key, atomic teardown, symmetric `TcpListener` bind probe, reactive `stream_url` reconnect on the frontend)
- Dynamic stream ports (9223–9299) for concurrent worktrees
- Stealth Chromium flags + realistic user-agent string
- Browser data management in Settings (clear cookies, clear all data, view data size)
- Inspector panel for debugging web content
- **Viewport presets**: `codemux browser viewport <mobile|tablet|desktop|WxH|reset>` resizes the actual viewport via CDP so CSS media queries fire and screenshots capture at the simulated dimensions. MCP exposes `browser_viewport` + `browser_viewport_presets`.
- Browser stream stability fix shipped (commit `7e36420`): unified port keying, hardened daemon lifecycle, dropped dead workspace_id alias lookups. Eliminates the silent stream failure that appeared after multiple concurrent worktrees used the browser.

### Workspace creation
- Multi-step creation dialog with task description, branch selection, agent preset
- AI-generated branch names from task description
- GitHub issue / PR linking with branch auto-fill
- **Paste clipboard images directly into the prompt input** (in addition to the existing file picker)
- File attachments appended to agent prompt
- Project onboarding flow with package manager detection and setup script configuration
- Orphan worktree detection and import
- Derivative-branch picker (icons, recency, worktree tab)
- Lazy workspace creation (Beta-gated): sidebar-plus and boot-into-Home open a client-side chat draft instead of eagerly materialising a workspace; the draft is promoted on first message send

### Search & navigation
- Keyword search (Ctrl+Shift+F via rg) and file name search (Ctrl+Shift+P via fd)
- Command palette (Ctrl+K, fuzzy search across all actions)
- Local lexical indexing (`codemux index build/search`)

### Notifications
- D-Bus desktop notifications via `notify_rust::Notification` (Normal urgency so daemons auto-dismiss)
- Sidebar notification section with unread badge counts
- Workspace alerts with severity levels
- Desktop notification toast + chime when an off-screen agent finishes
- Notification sound playback wired on all three platforms (Linux: `paplay` + freedesktop `complete.oga`; macOS: `afplay` + Glass.aiff; Windows: PowerShell SystemSounds)
- **Per-worktree mute** (`set_workspace_muted` + sidebar context-menu toggle): silences agent-completion notifications for a specific workspace without touching global sound state; muted state surfaces as a sidebar row icon

### Auth & sync
- GitHub OAuth, email/password with email verification, encrypted token storage (AES-256-GCM, machine-bound key)
- **Auth module split**: `auth/{mod,api,derivation}.rs` with the `AuthSecret` typed boundary on the API helpers (compile-time guard against raw-password leaks)
- **Zero-knowledge auth derivation** (Step 10): `derive_login_credentials(password, email)` produces both the server-visible `AuthSecret` (sent to Better Auth in place of the raw password) and a client-only `EncryptionKey` (32 raw bytes, never leaves the device). Argon2id (m=64MiB, t=3, p=4) with email-bound salt, fanned out via HKDF-SHA256 to two domain-separated secrets. Cross-product byte-identical with Vexis via the shared `codemux-api-*` HKDF labels — pinned in CI.
- **End-to-end-encrypted skills sync** (Step 10, Stages 1-6): cross-device sync of user-authored skills under `~/.codemux/skills/`, `~/.claude/skills/`, `~/.codex/skills/`, `~/.opencode/skills/`. XChaCha20-Poly1305 per blob, machine-bound key persistence at `~/.local/share/codemux/sync-key.enc` (AES-GCM under `/etc/machine-id`). Push triggered by file watcher (1.5s frontend debounce on top of the watcher's 300ms), 5-min periodic when window is visible, or manual "Sync now" button. Last-write-wins by `updated_at`. Settings → Account → Sync surfaces live status + relative-time + Export/Import/Forgot-password controls. Multi-step reset dialog enforces export-or-explicit-skip before the destructive wipe. Production-deployed and end-to-end smoked. See `docs/features/skills-sync.md`.
- Per-user synced settings with server sync, offline cache, and dirty flag

### Presets & launchers
- Terminal presets with quick-launch bar (Claude Code, Codex, OpenCode, Gemini, Pi, Shell)
- Pin/unpin to control bar visibility
- Auto-run on workspace creation or new tab
- Partial-materialise recovery for preset DnD; new-tab preset launch
- Preset failures surface as 8-second sonner toasts (was silently `.catch(console.error)`)
- Agent context injection for Claude / Codex / Pi / Gemini presets (uses `$VAR` on POSIX, `$env:VAR` on Windows)

### File tooling
- File tree panel (right sidebar, lazy-loaded, `.gitignore`-aware, opens in built-in editor or external editor)
- Built-in file editor with CodeMirror 6, syntax highlighting for 20+ languages, markdown preview (image loading in markdown and as standalone files)
- IDE integration (detect editors, open workspace, Ctrl+Shift+E). On Windows, `find_editors()` uses the `which::which()` Rust crate plus `%LOCALAPPDATA%\Programs` / `%ProgramFiles%` fallbacks for VS Code / Cursor / VSCodium / Zed; JetBrains stays PATH-only.
- Port detection (auto-scan, sidebar display, open in browser). Windows path filters system processes (`svchost.exe`, `System`, `lsass.exe`, etc.) so the sidebar doesn't surface 16+ kernel-owned ports.

### Theming
- Neutral dark shell theming with Omarchy accent sync
- Sans-serif chrome (DM Sans), monospace terminals
- Terminal colors fully theme-reactive via CSS variables + MutationObserver
- Fallback Tokyonight-inspired theme when Omarchy unavailable
- **Refined-minimal sidebar redesign**: slim Changes panel + ADE-native right sidebar, consolidated title-bar menu into the sidebar footer, aligned preset-bar icons + tab-bar drop indicator, sidebar workspace rows redesigned with new hover/active states
- **Refined-minimal Settings panel**: every section now uses shared primitives, back-button hit area widened, panel layout matches the sidebar aesthetic

### Agent Chat (Beta — opt-in via Settings → Beta Features)
- Full multi-provider chat pane with streaming, approvals, mode pills (Ask / Allow always / Plan / Debug), and permission-mode restart
- **Three providers** behind one unified picker: **Claude** (Claude Agent SDK via Bun-compiled `claude-agent` sidecar), **Codex** (`codex app-server` JSON-RPC), **OpenCode** (federated — Rust-direct HTTP against a managed `opencode serve` child, 100+ upstream providers funneled through one rail entry). See `docs/features/multi-provider-chat.md`.
- Unified provider+model picker (2-column popover: provider rail + searchable model list); favorites with `localStorage` persistence
- Codex finally GUI-selectable (was hidden behind a stale `ENABLE_PROVIDER_PICKER` flag pre-Step 12)
- Session history selector + draft surface chrome polish + plan proposals + AskUserQuestion panel + thinking indicator
- **Attachments via `+` and `@`**: files, folders, GitHub issues + PRs, images via paste / drop / + picker. Inline chips, send-time injection, expand, caps, gif guard, chip tooltips
- Slash command popup + Shift+Tab mode cycling
- Cross-provider **skill system** (watcher, conflicts, disable, refined compat)
- Per-tool body rendering in approval blocks (read/write/edit/grep/etc.)
- **MCP host runtime**: Codemux discovers user-installed MCP servers across Codemux / Claude / Cursor paths, spawns each child once, dedupes identical configs, exposes tools to the Claude SDK via an in-process facade with dynamic `setMcpServers` refresh. Settings panel and `+` popup surface enable/disable + status badges + tool list modal + 50-tool cap warning. Codex MCP support planned for Step 11 via HTTP gateway.
- Permissions settings page with per-tool body rendering
- Wired Debug mode pill with marker cleanup flow
- Plain-quit on Beta toggle off (no auto-restart)
- See `docs/features/agent-chat.md` for the canonical feature breakdown

### Infrastructure
- Global overlay manager (single overlay at a time)
- MCP server exposing **52 tools** via JSON-RPC 2.0 (browser tier 1/2/3 + info + viewport, workspace incl. open/close, pane incl. close, git, notification, terminal read/write, app_status, port_list, worktree_create, preset_apply/list, issue_list/get/link_workspace, automation list/get/create/update/delete/pause/resume/runs — Phase 1/1.5/1.6 vexis-agent integration tools and the eight `automation_*` tools all merged)
- CLI and socket control (Unix socket on Linux/macOS, named pipe on Windows). Control-endpoint errors now surface instead of being swallowed.
- Per-workspace display isolation (X11/Wayland sandboxing for agent-spawned GUI apps — opt-in for human persona, default-on for agent persona)
- Local project memory (`codemux memory show/set/add`, `codemux handoff`)
- Auto-update via Tauri updater (Linux AppImage + Windows NSIS, signed with the same Ed25519 key, shared `latest.json`)
- Onboarding skip affordance + re-trap fix
- Dev builds isolated from installed release (separate data dirs)
- **Remote hosts + workspace push**: Settings → Hosts with real `hosts_test_connection` probe + `hosts_bootstrap_install` install flow, slim `codemux-remote` server binary (`[[bin]] codemux_remote.rs`), and SSH transport (`ssh::probe`/`bootstrap`/`tunnel`/`tunnel_supervisor`/`push`/`registry`) so a workspace can be pushed to a user-owned SSH host. Push synchronizes the worktree, spawns the remote daemon, attaches the local UI through an SSH-forwarded socket, and **syncs the Claude conversation** across local/remote ends. `WorkspaceSnapshot.host_id` + shared `<DevicePicker>` pill wires the host selection into the new-workspace dialog.

### Automations
- **Scheduled agent runs**: a named prompt + agent + RFC 5545 recurrence that fires on a user-chosen host. Each fire creates an isolated git worktree, runs the agent headlessly (`claude --print` / `codex exec`), and records a real `succeeded` / `failed` / `skipped_offline` / `skipped_busy` terminal status. Same-automation overlap is serialised; a per-minute `fire_key` keeps a double tick idempotent.
- **Automations view**: a first-class destination opened from the left sidebar (under "New agent", above the project list) — list + detail pane for create / edit / pause / resume / delete, a frequency/time/weekday schedule builder with a raw RFC 5545 escape hatch, per-automation run history, and a per-row health dot driven by the last run.
- **Account sync**: `automations_sync` replicates the registry through the live `/api/automations` endpoints with the same dirty-flag / tombstone model as `hosts_sync`, so every signed-in device sees the same list; `automation_runs` stay per-device.
- **Host routing**: the desktop scheduler runs only `host_id IS NULL` automations; `codemux-remote scheduler` — a systemd user service provisioned at host bootstrap — runs host-targeted ones on an always-on machine. A stuck-run reconciler fails crashed runs at scheduler startup so a dead run can't pin its automation in `skipped_busy`.
- **GitHub backbone**: a remote host obtains the project repo by cloning / fetching its git remote with the host's own credentials (no token injected); a per-repo `git ls-remote` preflight flags an unreachable repo at setup, not at the first fire.
- Surface: seven `automations_*` Tauri commands + `automations_check_repo_access`, and eight `automation_*` MCP / control-socket tools. See `docs/features/automations.md`.

### Performance
- High-frequency app-state emits coalesced into 16 ms windows
- `transition-all` scoped to actually-transitioning properties
- Markdown view + workspace-tied components no longer re-render on every backend tick
- Workspace-switch mount-time IPC roundtrips cut; IPC thread unblocked
- Editor file read + language module import parallelised
- Worktree-include listener no longer re-attaches every backend tick
- `ensure-draft-when-empty` effect uses a primitive fingerprint
- Chat transcript rows + file-tree nodes memoised to skip per-token re-renders

## Partial / Being Hardened

- **OpenFlow**: orchestration works but large-run reliability and intervention flow still maturing. Backend-driven orchestration loop (5s active / 15s blocked). Disabled on Windows until the bash-wrapper rewrite lands.
- **Browser pane**: screenshot-driven, functional but lower fidelity than a native embedded webview
- **AI merge resolver**: backend and frontend working, recent fixes (close stdin + kill child on timeout, skip-permissions flags, blocking-pool offload), needs more depth of live validation
- **Browser automation depth**: DOM commands, coordinate commands, OS-level input, wait conditions, JS evaluation, CSS style inspection — all working; toolbar back/forward/reload still need focused validation

## Known Constraints

- Notification click-to-focus on Wayland and mako still needs deeper D-Bus or native handling
- Control socket is local-user only and currently unauthenticated
- Agent Chat is **off by default**; opt in via Settings → Beta Features
- Memory drawer UI is still backend + CLI only (no frontend drawer/panel yet)
- File editor: no LSP integration, no multi-cursor, no rename/delete from editor
- Context menus on pane headers are not yet implemented (workspace rows, section groups, tabs, changes panel rows, and sidebar ports section already have them)
- Browser automation uses `agent-browser` v0.24.0 (pure Rust binary, direct CDP). The legacy Playwright/Node.js path and the unused `BrowserManager` Rust CDP implementation are gone.
- Feature docs exist for all major subsystems (see `docs/INDEX.md`)

## Windows Support

Windows support shipped in `v0.1.20` and `v0.1.21` and has been hardened progressively through every subsequent release. Latest published Windows binaries (NSIS `.exe` installer + auto-update via the shared `latest.json`) ship on `v0.5.2`; in-app auto-update on Windows was fixed in `v0.5.1`.

What's in place:

- `cfg`-gates cover every Linux-specific code path — the app compiles on `x86_64-pc-windows-msvc` without unsafe `unix` stubs
- Control socket → named pipe (`\\.\pipe\codemux-{username}`) via `tokio::net::windows::named_pipe`. Client now retries on `ERROR_PIPE_BUSY`.
- Port detection via `netstat -ano` parser (cross-platform pure function, unit-tested on Linux CI) with a Windows system-process name filter
- Agent-browser port reclamation via `netstat -ano` + `taskkill` with exact-port matching; auto-detect installed Chromium (Edge / Chrome / Brave / Chromium)
- `portable-pty` fork pinned to `Zeus-Deus/portable-pty@codemux-0.8.1-no-window` (`STARTF_USESHOWWINDOW + SW_HIDE` so PTY spawns don't flash a `cmd.exe` console window)
- **PowerShell is the default Windows shell** (`pwsh` → `powershell` → `COMSPEC` → literal `"cmd.exe"`). Agent context injection uses PowerShell `$env:VAR` syntax; preset commands terminate with `\r`. Gemini path writes its system-prompt temp file via PowerShell `Set-Content -NoNewline`.
- Editor detection uses `which::which()` + `%LOCALAPPDATA%\Programs` / `%ProgramFiles%` fallbacks for VS Code, Cursor, VSCodium, Zed; JetBrains stays PATH-only
- `<WindowChrome />` extracted so login, empty-state, settings, and new-project screens have minimize/maximize/close buttons (Codemux runs with `decorations: false`)
- Scrollback flush waits 10s on Windows (3s elsewhere); Windows-only `scrollback::flush_cache_to_disk` backend backstop catches anything the frontend can't persist before timeout
- OpenFlow disabled at the UI + backend level on Windows (bash wrappers not yet ported)
- `release.yml` builds on `[ubuntu-22.04, windows-latest]` with `fail-fast: false`; tauri-action merges both platforms into a single `latest.json`
- NSIS installer produced on Windows CI (`--bundles nsis` to skip MSI which needs WiX)
- Claude Code hooks register and execute on Windows
- Tier-3 OS-level input injection via Win32 `SendInput`
- Path normalization + four latent windows portability issues fixed

Still gated before a polished Windows v1:

- Windows Authenticode code signing (SmartScreen friction expected on unsigned first-install; deferred behind a cert budget decision)
- OpenFlow bash wrapper rewrite (blocks OpenFlow on Windows)
- Full PTY lifecycle / worktree / agent-spawn integration tests on a live Windows runner

See `docs/plans/windows-support.md` for the complete checklist.

## React Frontend Status

The frontend is React + Tailwind v4 + shadcn + Vite. The Rust backend is unchanged. The old Svelte frontend has been removed.

### Working

- App shell: shadcn Sidebar with collapsible workspace sections, tab bar, right panel
- Workspace list from real Tauri backend data (zustand + app-state-changed events, coalesced into 16ms windows)
- Terminal panes with xterm.js WebGL + DOM fallback + PTY via Tauri Channel, persistent across workspace switch
- Pane splits (horizontal/vertical) with CSS Grid, resize handles, drag-to-swap
- Right panel with Changes panel, File tree, and Review (PR) panel tabs
- OpenFlow UI: orchestration view, agent config, communication panel, agent graph
- Agent Chat UI: chat pane, composer (with `+` popup, `@` mention popup, slash command popup, image paste/drop), transcript, mode pill, model picker, session selector, attachment chips, plan proposal block, AskUserQuestion panel, thinking indicator, permission request block, tool-call card with per-tool body rendering, debug-mode banner + exit dialog
- Settings panel (15+ sections including Beta Features, Sync, Skills, MCP, Permissions)
- Command palette (Ctrl+K) with fuzzy search
- Search: file name search (Ctrl+Shift+P) and content search (Ctrl+Shift+F)
- Browser pane with screenshot-driven rendering and toolbar (reactive `stream_url` reconnect)
- Workspace drag-and-drop reordering in sidebar
- Terminal presets bar with quick-launch
- Auth system with GitHub OAuth, email/password, encrypted token storage
- Synced settings (per-user server-synced with offline cache)
- Skills sync UI (Settings → Account → Sync)
- Semantic theming: shadcn oklch dark mode + custom --success/--danger/--warning tokens
- Tauri bridge: 120+ typed command wrappers, 12+ event helpers, all types ported

### Remaining Gaps

- Context menus on pane headers (workspace rows, workspace section groups, tabs, changes panel rows, and sidebar ports section already have them)
- Memory drawer UI (backend memory system exists, CLI works, no frontend drawer/panel yet)
- File editor: no LSP integration, no multi-cursor, no rename/delete from editor

## Read This With

- `docs/core/PLAN.md` for build order
- `docs/core/TESTING.md` for verification policy
- `docs/features/*` for subsystem detail
- `docs/plans/windows-support.md` for the cross-platform checklist
- `docs/plans/openflow.md` for the active OpenFlow hardening work
