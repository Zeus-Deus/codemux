---
title: Projects
description: Open, create, and manage projects with onboarding wizards and package manager detection.
---

# Projects

Projects in Codemux represent git repositories. Each project groups its workspaces in the sidebar and shares configuration like setup scripts and run commands.

## Opening an Existing Project

1. Click **Add repository** in the sidebar
2. Select **Open project**
3. Pick a folder from the file dialog
4. Codemux checks if it's a git repo — if not, it offers to initialize one
5. The project appears in the sidebar and the **onboarding wizard** opens

Recent projects appear in the project picker dropdown for quick access.

## Creating a New Project

1. Click **Add repository** in the sidebar
2. Select **New project**
3. Choose one of two options:

### Empty Repository

- Select a parent directory and enter a name
- Codemux creates the directory, initializes a git repo, and opens the onboarding wizard

### Clone from URL

- Enter a git URL (HTTPS or SSH)
- Repository name is auto-derived from the URL (editable)
- Codemux clones the repo and opens the onboarding wizard

## Project Onboarding Wizard

When you open or create a project, a two-step wizard helps you set up your first workspace.

### Step 1: Task and Branch

- **Task description** — Describe what you're working on (e.g., "Fix the login page CSS")
- **Branch name** — Auto-generated from your task via AI, or edit manually
- **Base branch** — Defaults to main/master, changeable in advanced options
- **Existing worktrees** — If the project has worktrees from outside Codemux, a banner lets you import them all with one click

### Step 2: Setup Scripts

Codemux scans your project and auto-detects setup commands based on files it finds:

| Detected File | Setup Command | Enabled by Default |
|---------------|---------------|--------------------|
| `bun.lock` | `bun install` | Yes |
| `pnpm-lock.yaml` | `pnpm install` | Yes |
| `yarn.lock` | `yarn install` | Yes |
| `package-lock.json` | `npm ci` | Yes |
| `package.json` (no lockfile) | `npm install` | Yes |
| `Cargo.toml` | `cargo build` | Yes |
| `go.mod` | `go mod download` | Yes |
| `poetry.lock` | `poetry install` | Yes |
| `uv.lock` | `uv sync` | Yes |
| `requirements.txt` | `pip install -r requirements.txt` | Yes |
| `Gemfile` | `bundle install` | Yes |
| `composer.json` | `composer install` | Yes |
| `.env.example` / `.env.sample` / `.env.template` | `cp .env.* .env` | Yes |
| `.gitmodules` | `git submodule update --init --recursive` | Yes |
| `docker-compose.yml` / `compose.yml` | `docker compose up -d` | No |

You can toggle individual commands on/off, or switch to **custom mode** to write your own commands (one per line). Teardown commands (run when the workspace closes) are configurable in an optional collapsible section.

The wizard also lets you select which **agent** to auto-launch in the new workspace.

### What Happens After the Wizard

1. Codemux creates a git worktree for your branch
2. Setup scripts run in the background
3. The selected agent launches with your task description as its initial prompt
4. You're ready to work

## Closing a Project

Right-click a project header in the sidebar and select **Close Project**.

A confirmation dialog shows:

- The project name and number of workspaces
- Warning: "This will close N workspace(s) and kill all active terminals"
- Reassurance: "Your files and git history will remain on disk"

Closing a project removes all its workspaces from the sidebar but does not delete any files from disk. Worktree directories are kept intact.

## Project Context Menu

Right-click a project header in the sidebar for these options:

| Action | Description |
|--------|-------------|
| Open in File Manager | Opens the project directory in your system file manager |
| Copy Path | Copies the full project path to the clipboard |
| Change Color | Pick a color for the project's sidebar avatar (12 colors + default) |
| Close Project | Close all workspaces and remove from sidebar |

## Project Sidebar Display

Each project in the sidebar shows:

- A **letter avatar** with the first letter of the project name (optionally color-coded)
- The **project name**
- A **workspace count** in parentheses
- A **collapse toggle** to show/hide workspaces
- A **new workspace button** (`+`) to create a workspace in that project
