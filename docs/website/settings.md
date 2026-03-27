---
title: Settings
description: Configure appearance, terminal, git, AI tools, and keyboard shortcuts.
---

# Settings

Open settings from the command palette (`Ctrl+K` > "Open Settings") or the sidebar gear icon.

## Appearance

- **Theme** — Syncs with Omarchy system theme automatically. Falls back to a Tokyonight-inspired dark theme.
- **Accent colors** — Success, danger, warning, and attention colors derived from the system palette.
- Terminal colors are fully theme-reactive (16 ANSI colors + foreground, cursor, selection).

## Editor

- **IDE selector** — Choose which external editor to use when opening files. Codemux auto-detects installed editors (VS Code, Cursor, Zed, etc.).
- Open workspace in editor: `Ctrl+Shift+E`

## Terminal

- **Default shell** — Uses `$SHELL` by default
- **Scrollback** — Terminal scrollback buffer size
- **Cursor style** — Block, underline, or bar
- **Canvas rendering** — Avoids WebGL context limits when many terminals are open

## Git

- **Default base branch** — Used when creating feature branches (defaults to `main`)

### AI Tools

- **AI commit messages** — Enable/disable the sparkle button next to the commit input. Requires Claude CLI.
- **Model override** — Specify a model for commit message generation, or leave empty for the CLI default.

### Merge Conflict Resolver

- **Enable** — Show the "Resolve with AI" button in the Conflicts section
- **CLI tool** — Claude Code, Codex, or OpenCode
- **Model override** — Specify a model or leave empty
- **Strategy** — Smart merge, Keep both, Prefer my branch, or Prefer target

## Presets

Manage terminal presets — saved agent configurations for one-click launch.

- **Create / Edit / Delete** — Add presets with custom names, icons, and commands
- **Multiple commands** — Each preset can run several commands in sequence
- **Launch mode** — New tab or split pane
- **Auto-run** — Optionally run on workspace creation or new tab
- **Pin to preset bar** — Toggle "Show in preset bar" for quick access
- **Working directory** — Override the default CWD per preset

## Projects

Configure per-project settings for workspaces.

- **Run command** — The dev command executed by the Run button (`Ctrl+Shift+G`). Set it here or via `.codemux/config.json`.
- **Setup scripts** — Commands that run automatically when a workspace opens (e.g., `npm install`, `docker compose up -d`)
- **Teardown scripts** — Commands that run when a workspace closes (e.g., `docker compose down`)

Settings configured here are stored in the database. File-based config (`.codemux/config.json`) takes precedence when present.

## Shortcuts

A read-only reference of all keyboard shortcuts grouped by category. See [Keyboard Shortcuts](keyboard-shortcuts.md) for the full list.

## Notifications

- **Sound** — Toggle notification sounds on/off
- Desktop notifications use the system notification daemon (notify-rust)
