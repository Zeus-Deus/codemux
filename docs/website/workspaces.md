---
title: Workspaces
description: Create, organize, and manage isolated coding workspaces with git worktree support.
---

# Workspaces

Each workspace is an isolated coding context with its own terminal tabs, pane layout, git branch, and browser state. Workspaces appear in the sidebar and persist across restarts.

## Creating Workspaces

Click the `+` button in the sidebar or use the project onboarding wizard to create a new workspace. Codemux uses a **prompt-first** flow — you describe what you want to do, and Codemux sets up the workspace around that.

### Prompt-First Creation

The workspace creation dialog centers on a prompt textarea ("What do you want to do?"). From this single dialog you can:

1. **Describe your task** — Type what you're working on (e.g., "Add dark mode support")
2. **Auto-generated branch name** — Codemux generates a branch name from your prompt using AI (see below)
3. **Pick an agent** — Select which AI agent to launch (Claude Code, Codex, OpenCode, Gemini, or a custom preset)
4. **Choose a project and base branch** — The project picker shows active and recent projects
5. **Attach files or link a PR** — Optional context for the agent

The prompt becomes the agent's initial task when the workspace opens. Press `Ctrl+Enter` to create.

### AI Branch Name Generation

When you type a task description, Codemux auto-generates a git branch name:

- **AI-generated** — Uses Claude CLI to generate a short 2-4 word hyphenated branch name from your prompt (e.g., "add-dark-mode")
- **Random fallback** — If AI is unavailable, generates a random `adjective-noun` pair (e.g., "swift-reef", "bold-gate")
- **Custom override** — You can always edit the branch name manually
- **Deconfliction** — Names are sanitized and checked against existing branches. Conflicts get a `-2`, `-3` suffix

### Other Creation Methods

You can also create workspaces by:

- **From branch** — Select an existing branch from the dropdown; creates a git worktree
- **From PR** — Link a pull request from the `+` menu to set the branch automatically

Each workspace gets its own terminal tabs and pane layouts.

## Empty Workspace State

When a workspace has no panes open, it shows a centered logo with quick action buttons:

| Action | Shortcut |
|--------|----------|
| Open Terminal | `Ctrl+T` |
| Open Browser | `Ctrl+Alt+B` |
| Open in Editor | `Ctrl+Shift+E` |
| Search Files | `Ctrl+P` |

A "Delete workspace" option is also available at the bottom.

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

You can create custom presets with your own commands, icons, and launch modes (split pane or new tab). The agent picker in the workspace creation dialog also uses these presets.

## Workspace Sections

Organize workspaces into named sections in the sidebar. Drag workspaces between sections to reorder. Sections can be color-coded and collapsed.

## Removing Workspaces

Click the `X` on a workspace row or right-click and select "Close Worktree" to open the remove dialog.

The dialog offers two options:

- **Hide** — Removes the workspace from the sidebar but keeps the worktree files on disk. Available for all workspace types.
- **Delete** — Permanently removes the worktree directory and optionally deletes the local branch. Only available for worktree workspaces.

A checkbox lets you toggle "Also delete local branch" (enabled by default for worktree workspaces).

### Unpushed Commit Warnings

If the workspace has uncommitted changes or unpushed commits, a warning banner appears in the remove dialog before you confirm. This prevents accidental data loss.

### Primary vs. Worktree Workspaces

- **Primary workspaces** (the main checkout) can only be hidden, not deleted
- **Worktree workspaces** support both hide and delete

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
