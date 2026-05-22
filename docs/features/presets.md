# Terminal Presets

- Purpose: Describe the preset system for quick-launching terminal commands and agents.
- Audience: Anyone working on preset management, the preset bar, or agent launch flows.
- Authority: Canonical feature doc for terminal presets.
- Update when: Preset behavior, storage, builtin set, or launch modes change.
- Read next: `docs/features/setup-teardown.md`

## What This Feature Is

Terminal presets are saved command templates that launch with one click from the preset bar or settings. They power the quick-launch buttons for coding agents (Claude Code, Codex, OpenCode, Gemini, Antigravity, Copilot, Cursor Agent, Amp, Grok, Droid, Mastracode) and custom user workflows.

## Current Model

Presets are stored in SQLite via the database layer. Fourteen builtin presets ship by default and are synced on every load. Users can create, edit, delete, pin, and reorder custom presets through Settings > Presets.

Every builtin agent preset launches in its CLI's skip-permissions / auto-approve ("YOLO") mode so the agent runs without per-action confirmation prompts. The exact flag is per-CLI and was verified against each tool's own `--help` (see the table below).

## What Works Today

- quick-launch bar below the tab bar with pinned preset buttons
- toggle bar visibility (Settings > Presets or command palette)
- builtin presets: Claude Code, Codex, OpenCode, Gemini, Antigravity, Copilot, Cursor Agent, Amp, Grok, Droid, Mastracode, Pi, Shell, Chat Agent
- custom preset creation with name, description, commands, working directory, icon
- launch mode: new tab or split pane (Shift+click on preset bar button opens in split pane)
- auto-run on workspace creation or new tab
- pin/unpin presets to control bar visibility
- preset editor in Settings > Presets (full CRUD)
- default preset selection
- agent context injection: preset commands for supported tools (Claude, Codex, Pi, Gemini) are automatically wrapped with the host shell's env-var expansion (`$CODEMUX_AGENT_CONTEXT` on Unix, `$env:CODEMUX_AGENT_CONTEXT` on Windows PowerShell) so agents receive Codemux-aware instructions at launch. The Gemini path writes its system prompt to a temp file under `std::env::temp_dir()` and sets `GEMINI_SYSTEM_MD` (Unix uses `printf '%s'` then env-prefix on the same line; Windows uses `Set-Content -NoNewline` then `$env:GEMINI_SYSTEM_MD = '<path>'`)
- Droid YOLO injection: interactive `droid` (Factory) has no skip-permissions CLI flag — full autonomy is only settable via a settings file. `inject_agent_context` rewrites the bare `droid` preset command to first write `{"sessionDefaultSettings":{"autonomyLevel":"high"}}` to a temp file under `std::env::temp_dir()` and then launch `droid --settings <path>` (same cross-platform inline-write split as Gemini). A `droid` command that already carries `--settings` is left untouched.
- preset failures surface as toast notifications: `applyPreset` rejections (e.g. CLI not installed) are routed through the sonner toast wrapper as `toast.error("Preset Name: {error}")` instead of being silently swallowed by `console.error`
- Windows-only line terminator fix: preset commands typed into the terminal use `\r` on Windows so PowerShell actually executes the command on submit (Unix uses `\n`)

## Builtin Presets

| Name | Command | YOLO flag rationale | Pinned |
|------|---------|---------------------|--------|
| Claude Code | `claude --dangerously-skip-permissions` | skip all permission prompts | yes |
| Codex | `codex --full-auto` | full-auto mode | yes |
| OpenCode | `opencode` | no CLI bypass flag | yes |
| Gemini | `gemini --yolo` | auto-approve all | yes |
| Antigravity | `agy --dangerously-skip-permissions` | auto-approve all tool permission requests | yes |
| Copilot | `copilot --allow-all` | `--allow-all-tools` + `--allow-all-paths` + `--allow-all-urls` | yes |
| Cursor Agent | `cursor-agent --yolo` | alias for `--force` ("Run Everything") | yes |
| Amp | `amp --dangerously-allow-all` | disable all command confirmation prompts | yes |
| Grok | `grok --always-approve` | auto-approve all tool executions | yes |
| Droid | `droid` (launched as `droid --settings <autonomy file>`) | interactive `droid` has no flag — autonomy is set via a settings file (`autonomyLevel: high`) | yes |
| Mastracode | `mastracode` | YOLO mode is on by default in the interactive TUI | yes |
| Pi | `pi` | no CLI bypass flag | yes |
| Shell | (empty — opens default shell) | n/a | no |
| Chat Agent | (native agent-chat pane, not a CLI) | n/a | yes |

All seven agent CLIs added alongside Gemini were verified in a clean Docker container: each tool was installed and launched with the flag above, confirming the flag parses and the CLI starts in that mode (rather than rejecting it as an unknown argument).

## Current Constraints

- presets are local-only (not synced across devices)
- no preset import/export
- no conditional presets (e.g. only show if tool is installed)
- builtin presets cannot be deleted, only unpinned

## Important Touch Points

- `src-tauri/src/presets.rs` — `TerminalPreset`, `PresetStore`, builtin definitions, SQLite persistence
- `src-tauri/src/commands/presets.rs` — Tauri commands: `create_preset`, `update_preset`, `delete_preset`, `apply_preset`, `set_preset_pinned`, `set_preset_bar_visible`, `get_preset_store`
- `src-tauri/src/agent_context.rs` — `inject_agent_context` per-binary preset-command rewrite (context injection for Claude/Codex/Pi/Gemini; `--settings` autonomy injection for Droid)
- `src/components/icons/preset-icon.tsx` + `src/assets/preset-icons/*.svg` — official agent logos rendered in the bar/tabs
- `src/components/layout/preset-bar.tsx` — quick-launch bar UI. When many presets are pinned the bar overflows into a single horizontally-scrollable row (`overflow-x-auto`, fixed `h-8`, buttons `shrink-0` so they never wrap). An `onWheel` handler maps vertical wheel delta to `scrollLeft` so a plain mouse wheel pans the bar — `overflow-x-auto` alone only responds to horizontal wheel/trackpad input.
- `src/components/settings/preset-editor.tsx` — settings CRUD panel
