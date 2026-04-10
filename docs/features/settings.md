# Settings Panel

- Purpose: Describe the current capability and constraints of the settings panel.
- Audience: Anyone working on settings, preferences, or configuration UI.
- Authority: Canonical feature-level reality doc.
- Update when: Settings sections, persistence behavior, or configuration scope changes.
- Read next: `docs/features/settings-sync.md`, `docs/reference/SHORTCUTS.md`

## What This Feature Is

The settings panel is a full-screen overlay providing centralized configuration for all Codemux subsystems. It opens with `Ctrl+,` or from the command palette.

## Current Model

Settings are organized into 12 sections. Most user-facing preferences are persisted via the synced settings store (server-synced with offline cache). UI-only state like the active section is managed in the UI store.

## What Works Today

### Personal Settings

- **Account**: sign-in email, display name, dev mode badge, sign out
- **Appearance**: theme preset, font family, border radius
- **Notifications**: notification sounds (agent completion/attention), desktop notification toggle (D-Bus)
- **Shortcuts**: keyboard keybind customization with conflict detection, search, and reset to defaults

### Editor & Workflow Settings

- **Editor**: select default external editor for opening files, detected editor display
- **Terminal**: cursor style (bar/block/underline), font size (10-22px), color theme (app/system)
- **Presets**: terminal command presets, pin/unpin to quick-launch bar, edit/delete custom presets, toggle preset bar visibility
- **Projects**: workspace lifecycle scripts (setup/teardown/run), worktree include patterns, `.codemuxinclude` file support, environment variable references
- **Git**: default base branch, AI commit message generation (model override), AI merge conflict resolver (strategy/CLI/model selection)
- **Agent**: auto-configure MCP for workspaces toggle
- **Browser**: browser profile storage size display, clear cookies/site data, clear all browser data
- **Session Restore**: save/restore terminal scrollback on restart, scrollback line limit (1000-50000), max disk usage (10-500 MB)

## Current Constraints

- No import/export of settings
- Notification sound toggle exists in UI, but actual audio playback is not implemented
- Custom keybinds stored in `~/.claude/keybindings.json`

## Important Touch Points

- `src/components/settings/settings-view.tsx` — main settings panel component
- `src/components/settings/keybind-editor.tsx` — keyboard shortcuts editor
- `src/stores/synced-settings-store.ts` — persisted user settings (server-synced)
- `src/stores/settings-store.ts` — local theme/appearance state
- `src/stores/ui-store.ts` — settings panel open/section state
- `.codemux/config.json` — workspace-level project config
