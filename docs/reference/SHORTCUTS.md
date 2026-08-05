# Keyboard Shortcuts

- Purpose: Complete reference of all keyboard shortcuts in Codemux.
- Audience: Users and contributors.
- Authority: Canonical shortcut reference.
- Update when: Shortcuts are added, removed, or rebound.
- Read next: `docs/reference/FEATURES.md`

## Global Shortcuts

These work anywhere in the app. Most are Ctrl-based, but not all — the workspace jumps use **Alt**, and `Escape` / `F5` are bare keys.

Registered in `src/lib/keybind-registry.ts` and dispatched from `src/hooks/use-keyboard-shortcuts.ts`; `src/lib/app-shortcuts.ts` holds the interception list that keeps these keys from reaching xterm.js.

| Shortcut | Action | Notes |
|----------|--------|-------|
| Ctrl+K | Command palette | Fuzzy search for any action |
| Ctrl+B | Toggle sidebar | Collapse the left sidebar to an icon rail / expand it (see `docs/features/sidebar.md`) |
| Ctrl+Shift+B | Toggle right panel | Show/hide the right panel (Files, Changes, Review, Orchestration) for the active workspace |
| Ctrl+, | Open settings | Opens the settings panel |
| Ctrl+Shift+? | Keyboard shortcuts | Opens Settings to the keyboard-shortcuts page (press Ctrl+Shift+/) |
| Ctrl+O | Open project | Opens a folder picker to add an existing project |
| Ctrl+Shift+P | Find file by name | File name search overlay (via fd/find) |
| Ctrl+Shift+F | Search in files | Keyword search across workspace files (via rg/grep) |
| Ctrl+N | New agent | Same as the sidebar **+**: new home chat draft, or the New Workspace dialog |
| Ctrl+Shift+N | New workspace in current project | Quick-create a workspace/agent in the active workspace's project |
| Ctrl+] | Next workspace | Cycles through sidebar workspace list |
| Ctrl+[ | Previous workspace | |
| Alt+1 through Alt+9 | Jump to workspace by position | Nth visible inbox card counting from the top — cards are newest-first, so Alt+1 is the **newest** workspace (filter-scoped; settled and snoozed rows excluded); holding Alt shows index badges on the cards |
| Ctrl+Shift+G | Run dev command | Runs the project's configured dev server command |
| Ctrl+T | New terminal tab | Active workspace |
| Ctrl+W | Close active tab | Only when workspace has more than one tab |
| Ctrl+1 through Ctrl+9 | Jump to tab by position | Tab 1 = leftmost |
| Ctrl+Shift+D | Split active pane right | Horizontal split |
| Ctrl+Shift+E | Split active pane down | Vertical split |
| Ctrl+Shift+W | Close active pane | |
| Escape | Close overlay | `closeOverlay` — dismisses the topmost open overlay |
| Ctrl+R | (blocked) | `blockReload` — swallowed so the WebView can't reload the app out from under live sessions |
| Ctrl+Shift+R | (blocked) | `blockHardReload` — same rationale |
| F5 | (blocked) | `blockF5Reload` — same rationale |

The registry defines 41 actions in total; the table above covers the app-global ones. All are rebindable in **Settings → Keyboard shortcuts** (rendered by `src/components/settings/keybind-editor.tsx`, the only consumer of `KEYBIND_REGISTRY` in settings).

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
| Ctrl+1 – Ctrl+9 (Cmd on macOS) | Jump to Nth model row | Agent Chat model picker popover open | `MultiProviderModelPicker.tsx` — capture-phase listener scoped to the open popover, so it overrides the global tab jump without rebinding it |
| Enter | Navigate to URL | Browser address bar focused | `BrowserToolbar.tsx` |
| Enter / Space | Activate workspace | Sidebar inbox card or settled row focused | `sidebar-inbox-card.tsx`, `sidebar-inbox.tsx` |
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
- `src/components/layout/tab-bar.tsx` — tab keyboard navigation
- `src/components/layout/sidebar-inbox-jump.ts` — visual-order targets for the Alt+1..9 workspace jumps
- `src/components/settings/keybind-editor.tsx` — shortcut display + rebinding UI in settings
