# Keyboard Shortcuts

- Purpose: Complete reference of all keyboard shortcuts in Codemux.
- Audience: Users and contributors.
- Authority: Canonical shortcut reference.
- Update when: Shortcuts are added, removed, or rebound.
- Read next: `docs/reference/FEATURES.md`

## Global Shortcuts

These work anywhere in the app. All require **Ctrl**.

Defined in `src/hooks/use-keyboard-shortcuts.ts`, `src/lib/app-shortcuts.ts`, and `src/components/layout/app-shell.tsx`.

| Shortcut | Action | Notes |
|----------|--------|-------|
| Ctrl+K | Command palette | Fuzzy search for any action |
| Ctrl+B | Toggle sidebar | Collapse the left sidebar to an icon rail / expand it (see `docs/features/sidebar.md`) |
| Ctrl+Shift+B | Toggle right panel | Show/hide the right panel (Files, Changes, Review) for the active workspace |
| Ctrl+, | Open settings | Opens the settings panel |
| Ctrl+Shift+? | Keyboard shortcuts | Opens Settings to the keyboard-shortcuts page (press Ctrl+Shift+/) |
| Ctrl+O | Open project | Opens a folder picker to add an existing project |
| Ctrl+Shift+P | Find file by name | File name search overlay (via fd/find) |
| Ctrl+Shift+F | Search in files | Keyword search across workspace files (via rg/grep) |
| Ctrl+N | New agent | Same as the sidebar **+**: new home chat draft, or the New Workspace dialog |
| Ctrl+Shift+N | New workspace in current project | Quick-create a workspace/agent in the active workspace's project |
| Ctrl+] | Next workspace | Cycles through sidebar workspace list |
| Ctrl+[ | Previous workspace | |
| Ctrl+Shift+G | Run dev command | Runs the project's configured dev server command |
| Ctrl+T | New terminal tab | Standard workspaces only (not OpenFlow) |
| Ctrl+W | Close active tab | Only when workspace has more than one tab |
| Ctrl+1 through Ctrl+9 | Jump to tab by position | Tab 1 = leftmost |
| Ctrl+Shift+D | Split active pane right | Horizontal split |
| Ctrl+Shift+E | Split active pane down | Vertical split |
| Ctrl+Shift+W | Close active pane | |

All shortcuts are rebindable in **Settings → Keyboard shortcuts** (rendered from `src/lib/keybind-registry.ts`).

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
| Enter | Navigate to URL | Browser address bar focused | `BrowserToolbar.tsx` |
| Enter / Space | Activate workspace | Workspace row focused | `sidebar-workspace-row.tsx` |
| Enter / Space | Activate pane | Pane header focused | `PaneNode.tsx` |
| Enter | Activate tab | Tab focused via keyboard | `tab-bar.tsx` |

## Known Conflicts

| Keys | Conflict | Resolution |
|------|----------|------------|
| Ctrl+W | Close tab vs. close window (some WMs) | Only fires when workspace has >1 tab. When only one tab exists, the event is not prevented and may reach the window manager. |

## Important Touch Points

- `src/lib/keybind-registry.ts` — canonical keybind definitions and categories
- `src/hooks/use-keyboard-shortcuts.ts` — global shortcuts (Ctrl+Shift+D, Ctrl+T, Ctrl+W, Ctrl+Shift+G, etc.)
- `src/lib/app-shortcuts.ts` — shortcut interception list (blocks keys from reaching xterm.js)
- `src/components/terminal/TerminalPane.tsx` — `customKeyEventHandler()` (terminal shortcuts)
- `src/components/layout/app-shell.tsx` — Ctrl+K command palette handler
- `src/components/layout/tab-bar.tsx` — tab keyboard navigation
- `src/components/settings/settings-view.tsx` — shortcut display in settings
