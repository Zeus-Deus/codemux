---
title: Keyboard Shortcuts
description: Complete reference of all keyboard shortcuts in Codemux.
---

# Keyboard Shortcuts

All shortcuts use **Ctrl** on Linux. Defined in `src/hooks/use-keyboard-shortcuts.ts` and `src/components/terminal/TerminalPane.tsx`.

## Global

| Shortcut | Action |
|----------|--------|
| Ctrl+K | Command palette |
| Ctrl+Shift+P | Command palette (alternative) |
| Ctrl+] | Next workspace |
| Ctrl+[ | Previous workspace |
| Ctrl+T | New terminal tab |
| Ctrl+W | Close active tab |
| Ctrl+1 through Ctrl+9 | Jump to tab by position |
| Ctrl+Shift+B | New browser tab |
| Ctrl+B | Toggle File Tree panel |
| Ctrl+P | Find file by name |
| Ctrl+Shift+E | Open workspace in editor |
| Ctrl+Shift+F | Search in files |
| Ctrl+Shift+G | Toggle Changes panel |

## Panes

| Shortcut | Action |
|----------|--------|
| Ctrl+Shift+D | Split pane right |
| Ctrl+Shift+W | Close active pane |
| Ctrl+Shift+J | Next pane |
| Ctrl+Shift+K | Previous pane |
| Ctrl+L | Next pane (vim) |
| Ctrl+H | Previous pane (vim) |

## Pane Resizing

| Shortcut | Action |
|----------|--------|
| Ctrl+Alt+ArrowRight | Expand active pane |
| Ctrl+Alt+ArrowLeft | Shrink active pane |
| Ctrl+Alt+ArrowDown | Expand vertically |
| Ctrl+Alt+ArrowUp | Shrink vertically |
| Ctrl+Alt+L | Expand (vim) |
| Ctrl+Alt+H | Shrink (vim) |
| Ctrl+Alt+J | Expand vertically (vim) |
| Ctrl+Alt+K | Shrink vertically (vim) |

Each keypress adjusts size by 5%.

## Terminal

| Shortcut | Action |
|----------|--------|
| Ctrl+Shift+C | Copy selected text |
| Ctrl+Shift+V | Paste from clipboard |
| Ctrl+Backspace | Backward kill word |
| Shift+Enter | Newline in agent input (Kitty protocol) |

## Known Conflicts

| Keys | Conflict |
|------|----------|
| Ctrl+H | Pane cycling intercepts browser history-back |
| Ctrl+L | Pane cycling intercepts browser address bar focus |
| Ctrl+W | Only fires when >1 tab exists; may reach window manager otherwise |
