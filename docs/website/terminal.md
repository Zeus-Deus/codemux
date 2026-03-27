---
title: Terminal
description: Terminal panes with tabs, splits, canvas rendering, and PTY integration.
---

# Terminal

The terminal is the primary interface in Codemux. It uses xterm.js with a canvas renderer, backed by real PTY sessions managed in Rust.

## Panes and Splits

Each tab contains a pane tree. Panes can be split horizontally or vertically, resized by dragging the divider, and swapped by dragging pane headers.

- **Split right**: `Ctrl+Shift+D` — creates a new pane to the right
- **Close pane**: `Ctrl+Shift+W` — removes the active pane
- **Cycle panes**: `Ctrl+Shift+J` (next) / `Ctrl+Shift+K` (previous)
- **Resize pane**: `Ctrl+Alt+Arrow` keys (5% per press)

Vim-style alternatives: `Ctrl+L` / `Ctrl+H` for pane cycling, `Ctrl+Alt+H/L/K/J` for resizing.

## Tabs

Each workspace can have multiple tabs. Each tab has its own independent pane tree.

| Shortcut | Action |
|----------|--------|
| Ctrl+T | New terminal tab |
| Ctrl+W | Close active tab |
| Ctrl+1 through Ctrl+9 | Jump to tab by position |
| Ctrl+Shift+B | New browser tab |

Tabs can be reordered by dragging. The last tab cannot be closed.

## Preset Bar

The preset bar sits above the pane area and provides one-click agent launching:

- **Pinned presets** — Pin your most-used agent configurations (Claude Code, Codex, OpenCode, Gemini) for instant access
- **Run button** — Shows "Run" when a dev command is configured, or "Set Run" to configure one. Press `Ctrl+Shift+G` to run from anywhere.
- **Settings** — Gear icon opens preset management. Toggle bar visibility from the dropdown.

Custom presets support multiple commands, working directory overrides, and launch modes (new tab or split pane).

## Smart Headers

Terminal pane headers display contextual information:

- **Agent name** — When running a recognized agent (Claude, Codex, OpenCode, Gemini), the header shows the agent name instead of the shell path
- **CWD** — When running a plain shell, the header shows the current working directory

## Canvas Renderer

Codemux uses the xterm.js canvas renderer by default with WebGL as a fallback. This avoids the WebGL context freeze that can occur when many terminals are open simultaneously.

## Terminal Features

- **Kitty keyboard protocol** — Enhanced key reporting for tools like OpenCode
- **Ctrl+Backspace** — Backward kill word (sends Ctrl+W to PTY)
- **Ctrl+Shift+C** / **Ctrl+Shift+V** — Copy/paste
- **Auto-resize** — PTY dimensions sync when panes resize
- **Status overlay** — Shows terminal state (starting, ready, exited with code)

## Theming

Terminal colors sync with the system theme (Omarchy integration). The 16 ANSI colors, foreground, cursor, and selection colors are all theme-reactive. When no Omarchy theme is detected, a Tokyonight-inspired fallback is used.

## Persistence

Pane layout and tab structure persist across restarts. Terminal scrollback is lost on restart — sessions respawn with a fresh shell in the same layout.
