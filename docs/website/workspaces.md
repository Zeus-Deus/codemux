---
title: Workspaces
description: Create, organize, and manage isolated coding workspaces with git worktree support.
---

# Workspaces

Each workspace is an isolated coding context with its own terminal tabs, pane layout, git branch, and browser state. Workspaces appear in the sidebar and persist across restarts.

## Creating Workspaces

Click the `+` button in the sidebar to create a new workspace. You can:

- **New workspace** — Opens at a directory you choose
- **From branch** — Creates a git worktree for the branch so work is isolated from your main checkout
- **From PR** — Opens the PR's branch in a worktree

Each workspace gets its own terminal tabs and pane layouts.

## Layout Presets

When creating a workspace, choose a layout preset:

| Preset | Description |
|--------|-------------|
| Single | 1 terminal pane |
| Pair | 2 panes side by side |
| Quad | 4 panes in a grid |
| Six | 6 panes (3x2) |
| Eight | 8 panes (4x2) |
| Shell + Browser | Terminal on the left, browser on the right |

## Agent Presets

The preset bar at the top of each workspace lets you launch AI agents with one click:

- **Claude Code** — `claude --dangerously-skip-permissions`
- **Codex** — `codex --full-auto`
- **OpenCode** — `opencode`
- **Gemini** — `gemini --yolo`

You can create custom presets with your own commands, icons, and launch modes (split pane or new tab).

## Workspace Sections

Organize workspaces into named sections in the sidebar. Drag workspaces between sections to reorder. Sections can be color-coded and collapsed.

## Context Menu

Right-click a workspace in the sidebar to:

- Rename the workspace
- Open in external editor
- Move to a different section
- Close the workspace

## Setup & Teardown Scripts

Configure commands that run automatically when workspaces open or close. Two configuration methods:

### File-based (`.codemux/config.json`)

Add a `.codemux/config.json` to your project root:

```json
{
  "setup": ["npm install", "npm run build"],
  "teardown": ["docker compose down"],
  "run": "npm run dev"
}
```

- `setup` — Runs when the workspace opens
- `teardown` — Runs when the workspace closes
- `run` — The dev command for the Run button (`Ctrl+Shift+G`)

### Database-based (Settings)

Configure scripts in Settings > Projects. Database settings apply when no `.codemux/config.json` exists. File config takes precedence.

### Docker Compose Support

Scripts automatically receive these environment variables:

- `COMPOSE_PROJECT_NAME` — Derived from the git root directory name, so Docker Compose uses consistent project names across worktrees
- `CODEMUX_ROOT_PATH` — Full path to the git root
- `CODEMUX_WORKSPACE_PATH` — Full path to the workspace directory

### Setup Banner

When a project has worktree workspaces but no setup scripts configured, a banner appears in the sidebar suggesting you configure automation. Dismiss it per-project or click "Configure" to open Settings.

### Project Detection

Codemux detects the project root for each workspace by walking up from the workspace directory to find the git root. This works correctly for worktree workspaces — the project root is the main repository, not the worktree directory.

## Merging Without PRs

For solo developers or personal projects where code review isn't needed, Codemux supports merging feature branches directly into main from the Changes panel. The "Merge into [base]" button uses a temporary resolver branch so main is never modified until the merge is verified clean — same safety guarantees as the AI conflict resolver. See [Merge Resolver](../website/merge-resolver.md#merge-into-main) for details.

## Keyboard Shortcuts

| Shortcut | Action |
|----------|--------|
| Ctrl+] | Next workspace |
| Ctrl+[ | Previous workspace |
