# Keyboard Shortcuts

- Purpose: Complete reference of all keyboard shortcuts in Codemux.
- Audience: Users and contributors.
- Authority: Canonical shortcut reference.
- Update when: Shortcuts are added, removed, or rebound.
- Read next: `docs/reference/FEATURES.md`

## Global Shortcuts

These work anywhere in the app. All require **Ctrl** (Linux) or **Cmd** (macOS).

Defined in `src/hooks/use-keyboard-shortcuts.ts` and `src/components/terminal/TerminalPane.tsx`.

| Shortcut | Action | Notes |
|----------|--------|-------|
| Ctrl+K | Command palette | Fuzzy search for any action |
| Ctrl+Shift+P | Command palette (alt) | Alternative binding |
| Ctrl+] | Next workspace | Cycles through sidebar workspace list |
| Ctrl+[ | Previous workspace | |
| Ctrl+T | New terminal tab | Standard workspaces only (not OpenFlow) |
| Ctrl+W | Close active tab | Only when workspace has more than one tab |
| Ctrl+1 through Ctrl+9 | Jump to tab by position | Tab 1 = leftmost |
| Ctrl+Shift+B | New browser tab | |
| Ctrl+B | Toggle File Tree panel | Opens right panel to Files tab |
| Ctrl+P | Find file by name | File name search overlay |
| Ctrl+Shift+E | Open workspace in editor | Opens in first detected editor |
| Ctrl+Shift+F | Search in files | Keyword search across workspace files |
| Ctrl+Shift+G | Toggle Changes panel | Opens right panel to Changes tab |
| Ctrl+Shift+J | Next pane | |
| Ctrl+Shift+K | Previous pane | |
| Ctrl+L | Next pane | Vim-style alternative |
| Ctrl+H | Previous pane | Vim-style alternative |
| Ctrl+Alt+ArrowLeft | Shrink active pane | 5% per keypress |
| Ctrl+Alt+ArrowRight | Expand active pane | 5% per keypress |
| Ctrl+Alt+ArrowUp | Shrink active pane (vertical) | 5% per keypress |
| Ctrl+Alt+ArrowDown | Expand active pane (vertical) | 5% per keypress |
| Ctrl+Alt+H | Shrink active pane | Vim-style alternative to arrow keys |
| Ctrl+Alt+L | Expand active pane | |
| Ctrl+Alt+K | Shrink active pane (vertical) | |
| Ctrl+Alt+J | Expand active pane (vertical) | |

## Terminal Shortcuts

These work inside terminal panes. Handled by xterm.js `customKeyEventHandler` in `src/components/terminal/TerminalPane.tsx`.

| Shortcut | Action | Notes |
|----------|--------|-------|
| Shift+Enter | Newline in agent input | Only when kitty keyboard protocol is active (e.g., OpenCode). Sends CSI 13;2u. When kitty is inactive, Shift+Enter is equivalent to Enter. |
| Ctrl+Backspace | Backward kill word | Sends Ctrl+W (0x17) to PTY |
| Ctrl+Shift+C | Copy selected text | Copies terminal selection to system clipboard |
| Ctrl+Shift+V | Paste from clipboard | Pastes system clipboard into terminal |

## Component Shortcuts

These work in specific UI contexts.

| Shortcut | Action | Context | Source |
|----------|--------|---------|--------|
| Enter | Navigate to URL | Browser address bar focused | Browser pane (TODO) |
| Enter / Space | Activate workspace | Workspace row focused | `sidebar-workspace-row.tsx` |
| Enter / Space | Activate pane | Pane header focused | `PaneNode.tsx` |
| Enter | Activate tab | Tab focused via keyboard | `tab-bar.tsx` |

## Known Conflicts

| Keys | Conflict | Resolution |
|------|----------|------------|
| Ctrl+H | Pane cycling vs. browser history-back | Ctrl+H is intercepted at window level before reaching the browser. Some users may expect browser history behavior. |
| Ctrl+L | Pane cycling vs. browser address bar focus | Ctrl+L is intercepted at window level. Browser address bar must be clicked directly. |
| Ctrl+W | Close tab vs. close window (some WMs) | Only fires when workspace has >1 tab. When only one tab exists, the event is not prevented and may reach the window manager. |

## Important Touch Points

- `src/hooks/use-keyboard-shortcuts.ts` — global shortcuts (Ctrl+Shift+D, Ctrl+T, Ctrl+W, etc.)
- `src/components/terminal/TerminalPane.tsx` — `customKeyEventHandler()` (terminal shortcuts)
- `src/components/layout/tab-bar.tsx` — tab keyboard navigation
- `src/components/layout/PaneNode.tsx` — pane split/close buttons
