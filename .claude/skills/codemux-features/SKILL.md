---
name: codemux-features
description: Use when implementing new ADE features for Codemux — tasks system, MCP server, settings panel, notification sounds, custom keybinds, session persistence, browser DevTools, or any feature inspired by competing ADEs like Superset, cmux, Conductor, or Emdash. Also use when asked to add a feature from another ADE or make Codemux competitive with other tools.
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
   Inspect their implementation for ideas, then adapt to Codemux's Tauri + Rust + React + Tailwind + shadcn architecture. Do not copy code directly — understand the approach and reimplement it.

4. **Backend-first.** Codemux's architecture is backend-state-driven. Implement features in Rust first (state, commands, persistence), then expose via Tauri commands, then build the React UI. The frontend is a view of backend truth.

5. **Run verification.** After implementing, run `npm run verify`. For Rust changes, also run `cargo test --manifest-path src-tauri/Cargo.toml`.

---

## Feature Priority Order

Implement in this order. Each feature builds on the previous ones.

### Priority 1: Tasks System

**Why first:** Central task management is the biggest missing workflow feature. Users need to go from issue to workspace to agent session in one flow.

**What exists:** GitHub PR integration via `gh` CLI (create, view, checks, merge). Auth status check with graceful degradation (`GhStatus` enum in `src-tauri/src/github.rs`). No task/issue tracking.

**What to build:**

Backend (Rust):
- Central task list linked to GitHub issues / Linear issues
- Fetch issues via `gh issue list` and `gh issue view` (reuse existing `check_gh_status` auth check pattern)
- Linear integration via API (optional, behind config flag)
- Task data structure: id, title, status, source (github/linear/local), workspace_id, assignee
- Click task → auto-create workspace + agent session (reuse `create_worktree_workspace` + `apply_preset`)
- Task status tracking and sync (update issue status from Codemux)
- Expose as Tauri commands and socket API

Frontend (React):
- Tasks section in sidebar (per project)
- Task detail view with linked workspace, issue body, comments
- Quick action: create workspace from task
- Status badge on workspace rows linked to tasks
- Filter by status, assignee, label

---

### Priority 2: MCP Server

**Why second:** Enables agents to programmatically control Codemux. An agent running in a terminal can create workspaces, spawn other agents, open browser panes, and manage tasks.

**What exists:** Unix socket API (`codemux.sock`) with JSON protocol in `src-tauri/src/control.rs`. This is a solid foundation but is not MCP-compatible.

**What to build:**

Backend (Rust):
- Implement MCP (Model Context Protocol) server that agents can connect to
- Expose tools: workspace CRUD, pane management, browser control, notification, port info, git status, memory access
- Bridge to existing socket API handlers — reuse the same Rust helper implementations
- Support local socket + HTTP transport
- Authentication: local socket is trusted (same as current), HTTP needs token auth

Configuration:
- Generate `.mcp.json` in workspace directories so agents auto-discover the Codemux MCP server
- Support `claude mcp add codemux` style registration

**Reference:** Superset's MCP server exposes tasks, workspaces, devices, and AI session management. Check their MCP docs and `apps/api/src/trpc/routers/` for implementation structure.

---

### Priority 3: Settings Panel

**Why third:** Users need a central place to configure Codemux without editing JSON files.

**What exists:** Theme sync (`src/stores/theme.ts`), notification sound toggle (state only, `set_notification_sound_enabled`), preset management (`src-tauri/src/presets.rs`), editor detection (`detect_editors` in `workspace.rs`). No settings UI.

**What to build:**

Frontend (React):
- Full settings screen (left nav + right content, or modal)
- Sections:
  - **Appearance**: shell font, terminal font size
  - **Editor**: default IDE (use existing `detect_editors` results)
  - **Terminal**: scrollback limit, cursor style
  - **Git**: default base branch
  - **Keyboard**: shortcut viewer/editor (read-only initially, editable in Priority 5)
  - **Notifications**: sound toggle, desktop notification toggle

Backend (Rust):
- Persist settings to `~/.config/codemux/settings.json`
- Load on startup, merge with defaults
- Expose as Tauri commands: `get_settings`, `update_settings`

---

### Priority 4: Notification Sounds

**Why fourth:** The sound toggle already exists in state (`set_notification_sound_enabled`), just needs audio playback wired up.

**What exists:** Notification system with D-Bus (`notify-rust`), Hyprland focus (`hyprctl dispatch focuswindow`), attention badges, and a sound enabled flag in app state. No actual audio playback.

**What to build:**

Backend (Rust):
- Play audio when agent finishes or needs attention
- Use `rodio` or similar crate for audio playback
- Ship a small set of bundled notification sounds
- Configurable per notification level (attention vs info)
- Respect the existing sound toggle state

Frontend (React):
- Sound selection in settings panel (Priority 3)
- Volume control (optional)

---

### Priority 5: Custom Keybinds

**Why fifth:** Power users need to rebind shortcuts. The command palette (Ctrl+K) already lists all actions with their current keybinds.

**What exists:** Keyboard shortcuts in `src/hooks/use-keyboard-shortcuts.ts` and `src/components/terminal/TerminalPane.tsx`. No user customization yet.

**What to build:**

Backend (Rust):
- Keybind config: `~/.config/codemux/keybinds.json`
- Default keybinds as fallback
- Import/export keybind config

Frontend (React):
- Keybind editor in settings panel (extends Priority 3)
- Click-to-rebind UI: select action, press new key combo, save
- Conflict detection (warn if key combo already assigned)
- Reset to defaults button
- Display current keybinds in command palette (already done, just needs to read from config)

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

Always adapt patterns to Codemux's architecture (Tauri + Rust backend + React frontend), never transplant code from a different framework.

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
