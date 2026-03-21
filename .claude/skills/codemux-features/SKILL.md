---
name: codemux-features
description: Use when implementing new ADE features for Codemux — diff viewer, git worktree workspaces, port detection, terminal presets, setup/teardown scripts, MCP server, IDE integration, command palette, session persistence, browser DevTools, or any feature inspired by competing ADEs like Superset, cmux, Conductor, or Emdash. Also use when asked to add a feature from another ADE or make Codemux competitive with other tools.
---

# Codemux ADE Feature Implementation Guide

Patterns and priorities for implementing features that make Codemux a complete Agentic Development Environment. This file tells you WHAT to build and HOW to approach it.

For visual and CSS standards, read the `/codemux-ui` skill.
For project architecture and current state, read `WORKFLOW.md` and `docs/INDEX.md` first.
For OpenFlow orchestration patterns, read `docs/features/openflow.md` and `docs/plans/openflow.md`.

## Ground Rules

1. **Check before building.** Before implementing any feature, verify what already exists in the codebase. Search for related types, commands, components, and stores. Codemux has partial implementations of several features that should be extended, not duplicated.

2. **Work in branches.** Create a feature branch for any significant new feature. Name it `feature/<feature-name>`.

3. **Reference open source competitors.** Superset and cmux are open source. When implementing a feature they have, you MAY clone their repo to `/tmp` for reference:
   - `gh repo clone superset-sh/superset /tmp/superset-ref` (Electron + TypeScript)
   - `gh repo clone manaflow-ai/cmux /tmp/cmux-ref` (Swift + libghostty)
   Inspect their implementation for ideas, then adapt to Codemux's Tauri + Rust + Svelte architecture. Do not copy code directly — understand the approach and reimplement it.

4. **Backend-first.** Codemux's architecture is backend-state-driven. Implement features in Rust first (state, commands, persistence), then expose via Tauri commands, then build the Svelte UI. The frontend is a view of backend truth.

5. **Run verification.** After implementing, run `npm run verify`. For Rust changes, also run `cargo test --manifest-path src-tauri/Cargo.toml`.

---

## Feature Priority Order

Implement in this order. Each feature builds on the previous ones.

### Priority 1: Git Diff Viewer

**Why first:** This is the single biggest missing feature. Every competing ADE (Superset, Conductor, Emdash) has this. Without it, users must leave Codemux to review what an agent changed.

**What exists:** A `diff` artifact type is scaffolded in the type system. No UI, no git operations, no diff rendering. The `git_branch` field on workspaces exists but is always `None`.

**What to build:**

Backend (Rust):
- Add git operations module (`src-tauri/src/git.rs` or similar)
- Implement: `git_status(path)` → list of changed files with status (modified/added/deleted/untracked)
- Implement: `git_diff(path, file)` → unified diff string for a file
- Implement: `git_diff_staged(path, file)` → staged diff
- Implement: `git_stage(path, files)`, `git_unstage(path, files)`
- Implement: `git_commit(path, message)`, `git_push(path)`
- Implement: `git_branch(path)` → current branch name (fix the always-None field)
- Expose all as Tauri commands
- Watch the workspace directory for file changes and emit events when git status changes

Frontend (Svelte):
- New pane type: `kind: 'diff'` in the pane tree
- New component: `src/components/panes/DiffPane.svelte`
- File list sidebar within the diff pane showing changed files grouped by: against base, staged, unstaged
- Diff rendering with syntax highlighting: green lines for additions (`--ui-success`), red for deletions (`--ui-danger`), monospace font matching terminal
- Two view modes: split (side-by-side) and unified (inline)
- Stage/unstage buttons per file
- Commit message input + commit button
- Push button with ahead/behind indicator

Open the diff pane via: keybind, pane header action button, or sidebar action on workspace row.

**Reference:** Superset's diff viewer has focus mode (one file at a time with prev/next navigation), split vs unified toggle, and sections for "against base", "commits", "staged", "unstaged". Inspect `apps/desktop/src` in the Superset repo for their implementation.

---

### Priority 2: Git Worktree-Based Workspaces

**Why second:** This enables the core ADE workflow — each agent works in isolation on its own branch without merge conflicts. Diff viewer becomes more powerful when each workspace is a clean worktree.

**What exists:** Workspaces are logical UI containers with a `cwd` field. No git worktree creation, listing, or management. Zero worktree references in the codebase.

**What to build:**

Backend (Rust):
- Add worktree operations to the git module
- Implement: `create_worktree(repo_path, branch_name)` → creates worktree, returns path
- Implement: `list_worktrees(repo_path)` → list existing worktrees
- Implement: `remove_worktree(path)` → clean up worktree on workspace delete
- Implement: `import_worktrees(repo_path)` → discover existing worktrees on disk
- When creating a workspace of type "branch" or "worktree", create a git worktree and set the workspace `cwd` to it
- Track worktree metadata on the workspace: branch name, ahead/behind counts, base branch

Frontend (Svelte):
- Extend NewWorkspaceLauncher to offer: "New branch", "Existing branch", "Pull request" options (like Superset)
- Show branch name and ahead/behind in WorkspaceRow
- On workspace delete, offer to also remove the worktree
- Import existing worktrees from disk

**Reference:** Superset's entire workspace model is worktree-based. Their sidebar shows branch + ahead/behind status. Clone their repo to see how they handle worktree lifecycle.

---

### Priority 3: Port Detection and Management

**Why third:** Connects the terminal-to-browser workflow. Agent starts a dev server → port detected → click opens in browser pane. Critical for web development workflows.

**What exists:** Internal port allocation for OpenFlow (3900-4199) and browser CDP (9222+). No user-facing port scanning.

**What to build:**

Backend (Rust):
- Add port scanning module
- Scan for listening TCP ports owned by processes in each workspace's process tree
- Periodic scan (every 2-5 seconds) or on-demand
- Track: port number, PID, process name, workspace association
- Expose as Tauri command and emit events on port changes
- Add ability to kill a process by port

Frontend (Svelte):
- Ports section in sidebar (per workspace), or in the pane header area
- Each port shows: port number, process name, status indicator
- Click a port → open `localhost:PORT` in browser pane
- Kill button per port
- Support static port config via `.codemux/ports.json` (like Superset)

**Reference:** Superset discovers ports from workspace processes and groups them in the sidebar. They also support `.superset/ports.json` for static config. On Linux, port scanning can use `/proc/net/tcp` or `ss` for efficiency.

---

### Priority 4: Terminal Presets and Quick-Launch

**Why fourth:** Reduces friction from "open app" to "agent is working". New users should be able to start an agent with one click.

**What exists:** Layout presets (single, pair, quad, etc.) and workspace template kinds. No saved commands or auto-run.

**What to build:**

Backend (Rust):
- Preset data structure: name, description, commands (list of strings), working directory (relative), launch mode (split panes vs new tabs)
- Store presets in config (`~/.config/codemux/presets.json` or within app state)
- Built-in quick-add templates: claude (`claude --dangerously-skip-permissions`), codex, opencode, gemini (`gemini --yolo`), aider
- Default preset option: auto-apply when creating workspaces
- On workspace creation with a preset, spawn terminal(s) and write the command(s) to PTY stdin

Frontend (Svelte):
- Preset manager in settings or sidebar
- Quick-launch bar above terminal tabs (pinned presets for one-click access)
- Extend NewWorkspaceLauncher: after choosing layout, optionally pick a preset to auto-run
- Right-click preset: run in current terminal, current tab, new tab

**Reference:** Superset's presets system includes quick-add templates for popular agents, a preset bar for one-click access, and split/tab launch modes. Check their settings UI.

---

### Priority 5: Setup and Teardown Scripts

**Why fifth:** Makes worktree-based workflows practical. Each new worktree needs dependency installation, env file copying, etc.

**What exists:** Nothing. Workspace lifecycle is hardcoded in Rust.

**What to build:**

Backend (Rust):
- Read `.codemux/config.json` from workspace directory (or project root)
- Structure: `{ "setup": ["npm install", "cp .env.example .env"], "teardown": ["docker-compose down"] }`
- Run setup commands sequentially after workspace/worktree creation
- Run teardown commands before workspace/worktree deletion
- Expose environment variables: `CODEMUX_ROOT_PATH`, `CODEMUX_WORKSPACE_NAME`, `CODEMUX_WORKSPACE_ID`
- If teardown fails, offer force-delete option
- Support user overrides in `~/.config/codemux/projects/<id>/config.json`

Frontend (Svelte):
- Show setup/teardown progress in the workspace during creation
- Error handling with "force delete" option on teardown failure
- Notify user when setup completes

**Reference:** Superset has a thorough setup/teardown system with priority ordering (user override > worktree-specific > project default) and local config extensions. See their `.superset/config.json` docs.

---

### Priority 6: IDE Integration (Open in Editor)

**Why sixth:** Quick escape hatch. Sometimes users need their full editor. One keybind opens the workspace directory in their preferred editor.

**What exists:** Nothing. Folder picker exists but no "open in editor" action.

**What to build:**

Backend (Rust):
- Detect installed editors: check for `code`, `cursor`, `nvim`, `vim`, `zed`, `idea` in PATH
- Command to open workspace cwd in a given editor: spawn `code /path/to/workspace` etc.
- Config for preferred editor

Frontend (Svelte):
- "Open in..." button on workspace row or pane header
- Dropdown with detected editors if multiple found
- Keybind: something like Ctrl+Shift+E to open current workspace in preferred editor
- If only one editor detected, skip the dropdown and open directly

This is simple to implement and high value. On Linux, `xdg-open` won't help here — need explicit editor binary detection.

---

### Priority 7: Keybind Command Palette

**Why seventh:** Major UX upgrade for keyboard-first users. Ties all features together with fast access.

**What exists:** Global keybinds for pane navigation exist. No command palette overlay.

**What to build:**

Frontend (Svelte):
- New component: `src/components/CommandPalette.svelte`
- Trigger: Ctrl+K (or Ctrl+Shift+P)
- Fuzzy-match search over a list of actions
- Actions include: switch workspace (by name), focus pane (by number), create workspace, split pane, open browser pane, open diff pane, start OpenFlow run, open in IDE, switch theme, toggle sidebar sections, open port in browser, run preset
- Keyboard navigation: arrow keys to move, Enter to execute, Escape to close
- Visual: centered overlay, `var(--ui-layer-2)` background, strong border, shadow, search input at top, filtered results below
- Show keybind hints next to actions that have dedicated shortcuts

Backend: minimal — the palette is a frontend-only component that calls existing Tauri commands.

---

### Priority 8: MCP Server

**Why eighth:** Enables agents to programmatically control Codemux. An agent running in a terminal can create workspaces, spawn other agents, open browser panes, and manage tasks.

**What exists:** Unix socket API (`codemux.sock`) with JSON protocol. This is a solid foundation but is not MCP-compatible.

**What to build:**

Backend (Rust):
- Implement MCP (Model Context Protocol) server that agents can connect to
- Expose tools: workspace CRUD, pane management, browser control, notification, port info, memory access
- Bridge to existing socket API handlers — reuse the same Rust helper implementations
- Support both local socket transport and HTTP transport
- Authentication: local socket is trusted (same as current), HTTP needs token auth

Configuration:
- Generate `.mcp.json` in workspace directories so agents auto-discover the Codemux MCP server
- Support `claude mcp add codemux` style registration

**Reference:** Superset's MCP server exposes tasks, workspaces, devices, and AI session management. Their agent can start sub-agents in other workspaces. Check their MCP docs and `apps/api/src/trpc/routers/` for implementation structure.

---

### Priority 9: Session Persistence (Scrollback)

**What exists:** Layout, workspace structure, and terminal metadata persist. PTYs respawn on restart. But terminal scrollback is lost and agent sessions are stripped.

**What to build:**

Backend (Rust):
- On app close or periodic interval, dump terminal scrollback to disk per session
- Store in `~/.config/codemux/scrollback/<session-id>.bin` or similar
- On terminal reconnect after restart, write saved scrollback to the terminal before attaching live PTY output
- Cap scrollback storage (e.g., last 10,000 lines per session)
- For dead sessions (agent finished), show scrollback as read-only with a "Restart" button

Frontend:
- Minimal changes — the terminal component already handles PTY attachment. Just need to handle the "write historical scrollback first" flow and a restart prompt for dead sessions.

---

### Priority 10: Browser Automation Depth

**What exists:** Basic browser automation: open, snapshot, click, fill, screenshot, console-logs. Functional but thin compared to competitors.

**What to build (incremental):**

Extend the `codemux browser` CLI and socket API:
- **Wait conditions:** `codemux browser wait --selector "#el" --timeout 10000`, `wait --text "Success"`, `wait --url-contains "/dashboard"`
- **DOM inspection:** `get text "h1"`, `get value "#input"`, `get attr "a" href`, `get count ".items"`, `is visible "#el"`, `is enabled "button"`
- **JavaScript eval:** `codemux browser eval "document.title"`
- **Cookie/storage:** `cookies get`, `cookies set`, `storage local get`, `storage local set`
- **Frame navigation:** `frame "iframe[name='checkout']"`, `frame main`
- **Dialog handling:** `dialog accept`, `dialog dismiss`
- **Scroll:** `scroll --dy 500`, `scroll-into-view "#pricing"`

Prioritize wait conditions and DOM inspection first — these are what agents need most for reliable browser testing.

**Reference:** cmux has the most comprehensive browser automation CLI. Clone their repo and inspect `Sources/CMux/Browser/` for the full command set. Their `--snapshot-after` flag on mutating actions is a clever pattern — the agent gets verification in one command instead of two.

---

### Priority 11: Browser DevTools

**What exists:** CDP debug ports are allocated internally but never exposed to the user.

**What to build:**

- Toggle button on browser pane toolbar to show/hide DevTools
- Render DevTools in a split below or beside the browser viewport
- Since the browser uses CDP internally, DevTools can connect to the same debug port
- This could be a separate browser surface pointing to `chrome://inspect` or the CDP debug URL

Lower priority — most agent-driven workflows don't need visual DevTools, but it's useful for debugging.

---

## Open Source Reference Repos

When implementing any feature above, these repos can be cloned to `/tmp` for implementation reference:

| Repo | Stack | Best for referencing |
|------|-------|---------------------|
| `superset-sh/superset` | Electron, TypeScript, React | Diff viewer, worktrees, port detection, presets, setup/teardown, MCP server |
| `manaflow-ai/cmux` | Swift, libghostty | Browser automation depth, notification system, pane/surface hierarchy |
| `nicepkg/emdash` | Electron, TypeScript | Best-of-N comparison, issue tracker integration |

Usage pattern:
```bash
# Clone for reference (do NOT modify)
gh repo clone superset-sh/superset /tmp/superset-ref --depth 1

# Inspect specific feature
find /tmp/superset-ref -name "*.ts" | xargs grep -l "worktree" | head -20
# or
find /tmp/superset-ref -name "*.ts" | xargs grep -l "diff" | head -20

# Clean up when done
rm -rf /tmp/superset-ref
```

Always adapt patterns to Codemux's architecture (Tauri + Rust backend + Svelte frontend), never transplant code from a different framework.

---

## Feature Implementation Checklist

Before considering any feature done:

1. Backend state and commands implemented in Rust
2. Exposed via Tauri commands
3. Frontend UI follows `/codemux-ui` skill standards
4. Socket API support added where applicable (for agent access)
5. CLI command added where applicable (for `codemux <feature>` access)
6. Existing docs updated: `docs/core/STATUS.md`, relevant feature doc, relevant plan doc
7. `npm run verify` passes
8. Manual testing confirms the feature works in a real workflow

---

## Do Not

- Implement features without checking what already exists first
- Copy code from reference repos — understand the approach, reimplement for Codemux
- Add features without socket/CLI access — agents need to control everything programmatically
- Skip the backend — frontend-only features break the backend-state-driven architecture
- Forget to update docs — the docs system is how the next session knows what changed
