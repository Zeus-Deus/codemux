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

Add a `.codemux/config.json` to your project root:

```json
{
  "setup": ["npm install", "npm run build"],
  "teardown": ["docker-compose down"]
}
```

Setup runs automatically when the workspace is created. Teardown runs when it's closed.

## Keyboard Shortcuts

| Shortcut | Action |
|----------|--------|
| Ctrl+] | Next workspace |
| Ctrl+[ | Previous workspace |
