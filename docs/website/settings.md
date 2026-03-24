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

## Shortcuts

A read-only reference of all keyboard shortcuts grouped by category. See [Keyboard Shortcuts](keyboard-shortcuts.md) for the full list.

## Notifications

- **Sound** — Toggle notification sounds on/off
- Desktop notifications use the system notification daemon (notify-rust)
