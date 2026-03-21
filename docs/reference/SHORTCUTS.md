# Keyboard Shortcuts

- Purpose: Complete reference of all keyboard shortcuts in Codemux.
- Audience: Users and contributors.
- Authority: Canonical shortcut reference.
- Update when: Shortcuts are added, removed, or rebound.
- Read next: `docs/reference/FEATURES.md`

## Global Shortcuts

These work anywhere in the app. All require **Ctrl** (Linux) or **Cmd** (macOS).

Defined in `src/App.svelte` — `handleWindowKeydown()`.

| Shortcut | Action | Notes |
|----------|--------|-------|
| Ctrl+] | Next workspace | Cycles through sidebar workspace list |
| Ctrl+[ | Previous workspace | |
| Ctrl+T | New terminal tab | Standard workspaces only (not OpenFlow) |
| Ctrl+W | Close active tab | Only when workspace has more than one tab |
| Ctrl+1 through Ctrl+9 | Jump to tab by position | Tab 1 = leftmost |
| Ctrl+Shift+B | New browser tab | |
| Ctrl+Shift+D | New diff/changes tab | |
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

These work inside terminal panes. Handled by xterm.js `customKeyEventHandler` in `src/components/panes/TerminalPane.svelte`.

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
| Enter | Navigate to URL | Browser address bar focused | `BrowserPane.svelte` |
| Enter | Save rename | Sidebar workspace rename input | `Sidebar.svelte` |
| Escape | Cancel rename | Sidebar workspace rename input | `Sidebar.svelte` |
| Escape | Close modal | New Workspace Launcher open | `NewWorkspaceLauncher.svelte` |
| Enter | Send message | OpenFlow comm panel input | `CommunicationPanel.svelte` |
| Enter / Space | Activate workspace | Workspace row focused | `WorkspaceRow.svelte` |
| Enter / Space | Activate pane | Pane header focused | `PaneNode.svelte` |
| Enter | Activate tab | Tab focused via keyboard | `TabBar.svelte` |

## Known Conflicts

| Keys | Conflict | Resolution |
|------|----------|------------|
| Ctrl+H | Pane cycling vs. browser history-back | Ctrl+H is intercepted at window level before reaching the browser. Some users may expect browser history behavior. |
| Ctrl+L | Pane cycling vs. browser address bar focus | Ctrl+L is intercepted at window level. Browser address bar must be clicked directly. |
| Ctrl+W | Close tab vs. close window (some WMs) | Only fires when workspace has >1 tab. When only one tab exists, the event is not prevented and may reach the window manager. |

## Important Touch Points

- `src/App.svelte` — `handleWindowKeydown()` (global shortcuts)
- `src/components/panes/TerminalPane.svelte` — `customKeyEventHandler()` (terminal shortcuts)
- `src/components/panes/BrowserPane.svelte` — `handleKeydown()` (address bar)
- `src/components/openflow/CommunicationPanel.svelte` — `handleKeydown()` (message input)
- `src/components/tabs/TabBar.svelte` — tab keyboard navigation
