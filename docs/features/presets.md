# Terminal Presets

- Purpose: Describe the preset system for quick-launching terminal commands and agents.
- Audience: Anyone working on preset management, the preset bar, or agent launch flows.
- Authority: Canonical feature doc for terminal presets.
- Update when: Preset behavior, storage, builtin set, or launch modes change.
- Read next: `docs/features/setup-teardown.md`

## What This Feature Is

Terminal presets are saved command templates that launch with one click from the preset bar or settings. They power the quick-launch buttons for coding agents (Claude Code, Codex, OpenCode, Gemini) and custom user workflows.

## Current Model

Presets are stored in SQLite via the database layer. Five builtin presets ship by default and are synced on every load. Users can create, edit, delete, pin, and reorder custom presets through Settings > Presets.

## What Works Today

- quick-launch bar below the tab bar with pinned preset buttons
- toggle bar visibility (Settings > Presets or command palette)
- builtin presets: Claude Code, Codex, OpenCode, Gemini, Shell
- custom preset creation with name, description, commands, working directory, icon
- launch mode: new tab or split pane
- auto-run on workspace creation or new tab
- pin/unpin presets to control bar visibility
- preset editor in Settings > Presets (full CRUD)
- default preset selection

## Builtin Presets

| Name | Command | Pinned |
|------|---------|--------|
| Claude Code | `claude --dangerously-skip-permissions` | yes |
| Codex | `codex --full-auto` | yes |
| OpenCode | `opencode` | yes |
| Gemini | `gemini --yolo` | yes |
| Shell | (empty — opens default shell) | no |

## Current Constraints

- presets are local-only (not synced across devices)
- no preset import/export
- no conditional presets (e.g. only show if tool is installed)
- builtin presets cannot be deleted, only unpinned

## Important Touch Points

- `src-tauri/src/presets.rs` — `TerminalPreset`, `PresetStore`, builtin definitions, SQLite persistence
- `src-tauri/src/commands/presets.rs` — Tauri commands: `create_preset`, `update_preset`, `delete_preset`, `apply_preset`, `set_preset_pinned`, `set_preset_bar_visible`, `get_preset_store`
- `src/components/layout/preset-bar.tsx` — quick-launch bar UI
- `src/components/settings/preset-editor.tsx` — settings CRUD panel
