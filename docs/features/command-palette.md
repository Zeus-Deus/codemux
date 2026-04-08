# Command Palette

- Purpose: Describe the command palette and its available actions.
- Audience: Anyone adding commands or working on keyboard shortcuts.
- Authority: Canonical feature-level reality doc.
- Update when: Actions, groups, or dispatch behavior changes.
- Read next: `docs/reference/SHORTCUTS.md`, `docs/features/search.md`

## What This Feature Is

The command palette (Ctrl+K) is a fuzzy-search overlay that provides quick access to all major actions in Codemux. It is built on shadcn's Command component with real-time filtering.

## Current Model

Actions are defined as `CommandItem` components organized into `CommandGroup` containers. Each action calls a `run()` helper that closes the palette and executes the action. Keybind hints are resolved dynamically from `useResolvedKeybinds()` so they reflect user customizations.

## What Works Today

### Action Groups

**Workspaces** — Dynamic list of all workspaces for quick switching, plus:
- Create New Workspace
- Run Dev Command (Ctrl+Shift+G)

**Panes**
- Split Pane Right (Ctrl+Shift+D)
- Split Pane Down
- Close Pane (Ctrl+Shift+W)

**Tabs**
- New Terminal Tab (Ctrl+T)
- Close Tab (Ctrl+W)
- Open Browser
- Open Diff Viewer

**Search**
- Find File by Name (Ctrl+Shift+P)
- Search in Files (Ctrl+Shift+F)

**View**
- Toggle Right Panel
- Toggle Sidebar (Ctrl+B)
- Toggle Preset Bar
- Open Settings (Ctrl+,)
- Regenerate MCP Config

**Navigation**
- Focus Next Pane
- Focus Previous Pane

### Behavior

- Fuzzy matching on action labels as user types
- "No results found." message when nothing matches
- Keybind hints shown inline next to each action
- Workspace entries update dynamically as workspaces change
- Actions dispatch Tauri commands or UI store mutations

## Current Constraints

- No user-defined custom commands
- No recently-used action ranking
- No nested submenus within groups

## Important Touch Points

- `src/components/overlays/command-palette.tsx` — palette UI and action definitions
- `src/hooks/use-resolved-keybinds.ts` — keybind hint resolution
- `src/lib/keybind-registry.ts` — keybind action IDs and defaults
- `src/stores/ui-store.ts` — palette open/close state
