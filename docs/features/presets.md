# Terminal Presets

- Purpose: Describe the preset system for quick-launching terminal commands and agents.
- Audience: Anyone working on preset management, the preset bar, or agent launch flows.
- Authority: Canonical feature doc for terminal presets.
- Update when: Preset behavior, storage, builtin set, or launch modes change.
- Read next: `docs/features/setup-teardown.md`

## What This Feature Is

Terminal presets are saved command templates that launch with one click from the preset bar or settings. They power the quick-launch buttons for coding agents (Claude Code, Codex, OpenCode, Gemini, Antigravity, Copilot, Cursor Agent, Amp, Grok, Droid, Mastracode) and custom user workflows.

## Current Model

Presets are stored in SQLite via the database layer. Fourteen builtin presets ship by default and are synced on every load. Versioned preset-store migrations update obsolete untouched builtin defaults while preserving customized commands; schema v2 replaces the legacy persisted Codex command `codex --full-auto` with the current interactive YOLO command. Users can create (via the **New preset** button in Settings > Presets or the preset-bar gear menu), edit, delete, pin, and reorder custom presets through Settings > Presets.

A preset is either a **raw command** preset (a list of literal shell command lines, the original model) or a **structured "agent launcher"** preset (pick an agent + model + reasoning + a prompt). Both kinds are the same `TerminalPreset` under the hood and launch through the identical `apply_preset` pipeline; the structured kind additionally persists a `launch_config` so the editor can round-trip the pickers. The editor's **Type** selector switches a preset between the two modes.

Every builtin agent preset launches in its CLI's skip-permissions / auto-approve ("YOLO") mode so the agent runs without per-action confirmation prompts. The exact flag is per-CLI and was verified against each tool's own `--help` (see the table below).

In **GUI chrome** (Agent Chat GUI on — the default — with a real non-OpenFlow workspace) the preset-bar row does not render: it is replaced by the title bar's `+` agent-launcher popover (GUI chat presets / CLI agents, Shift = split) plus an inline ember chat favorite. The preset data model, storage, and `apply_preset` launch pipeline are unchanged — the launcher reads the same live preset snapshot and fires the same commands. Draft (lazy-creation) workspaces keep the preset bar. See `docs/features/gui-chrome.md`.

## Structured Agent Launchers

The structured editor lets a user build a named launcher — e.g. a **"Git Pull"** preset that launches Claude with Opus + High reasoning and a canned prompt — and it deliberately **reuses the exact model-selection solution the New Workspace dialog uses** (`LaunchModelPicker` + `LaunchReasoningPicker`), not a bespoke one:

- **Agent** — chosen from the builtin CLI agents (the preset bar's agents). Selecting one sets the preset's base command + icon; agent-family detection (`detectLaunchFamily`) then drives the pickers.
- **Model + Reasoning** — the same capability-driven pickers as launch-at-workspace-creation: live-harvested model lists (Claude via the SDK, Gemini via the API, OpenCode federated), real effort levels, and Claude's 1M context window. Shown only for the families Codemux models — **Claude, Codex, OpenCode, Gemini**; other agents (Copilot, Cursor, …) show no model picker (honest — Codemux has no model data for them).
- **Prompt** — passed to the agent as a quoted positional argument (its first instruction). Optional.

The model/reasoning/context are **not** baked into the command. They ride in `launch_config.model_selection` (a `ModelSelection`) and are spliced into the command at launch by Rust **`apply_model_selection`** — the same mechanism the launch dialog uses, so a preset and a dialog launch produce identical commands and stale flags can never desync from the picks. The prompt is baked into `commands[0]` as a trailing positional. The agent-context injection (`inject_agent_context`) runs on top at launch, so the positional prompt and the appended `--system-prompt "$CODEMUX_AGENT_CONTEXT"` coexist.

On bar-launch, the preset's `model_selection` is passed to `apply_preset` (which already accepts and applies it). Storing the structured `ModelSelection` — rather than a baked `--model` string — is what lets the editor re-open with the model/reasoning pills repopulated.

## What Works Today

- quick-launch bar below the tab bar with pinned preset buttons
- toggle bar visibility (Settings > Presets or command palette)
- builtin presets: Claude Code, Codex, OpenCode, Gemini, Antigravity, Copilot, Cursor Agent, Amp, Grok, Droid, Mastracode, Pi, Shell, Chat Agent
- custom preset creation via the **New preset** button (Settings > Presets header, and the preset-bar gear menu) — opens an unsaved draft (a Claude launcher) that is persisted only on **Create preset**
- structured "agent launcher" presets: pick agent + model + reasoning + prompt using the same capability-driven pickers as the New Workspace dialog (see "Structured Agent Launchers"); the editor's **Type** selector toggles between structured and raw command modes
- custom preset creation with name, description, commands, working directory, icon
- launch mode: new tab or split pane (Shift+click on preset bar button opens in split pane)
- auto-run on workspace creation or new tab
- pin/unpin presets to control bar visibility
- preset editor in Settings > Presets (full CRUD)
- default preset selection
- agent context injection: preset commands for supported tools (Claude, Codex, Pi, Gemini) are automatically wrapped with the host shell's env-var expansion (`$CODEMUX_AGENT_CONTEXT` on Unix, `$env:CODEMUX_AGENT_CONTEXT` on Windows PowerShell) so agents receive Codemux-aware instructions at launch. The Gemini path writes its system prompt to a temp file under `std::env::temp_dir()` and sets `GEMINI_SYSTEM_MD` (Unix uses `printf '%s'` then env-prefix on the same line; Windows uses `Set-Content -NoNewline` then `$env:GEMINI_SYSTEM_MD = '<path>'`)
- Droid YOLO injection: interactive `droid` (Factory) has no skip-permissions CLI flag — full autonomy is only settable via a settings file. `inject_agent_context` rewrites the bare `droid` preset command to first write `{"sessionDefaultSettings":{"autonomyLevel":"high"}}` to a temp file under `std::env::temp_dir()` and then launch `droid --settings <path>` (same cross-platform inline-write split as Gemini). A `droid` command that already carries `--settings` is left untouched.
- preset failures surface as toast notifications: `applyPreset` rejections (e.g. CLI not installed) are routed through the sonner toast wrapper as `toast.error("Preset Name: {error}")` instead of being silently swallowed by `console.error`
- Windows-only line terminator fix: preset commands typed into the terminal use `\r` on every platform so PowerShell actually executes the command on submit (not just Windows — the constant is unconditional)

## Builtin Presets

| Name | Command | YOLO flag rationale | Pinned |
|------|---------|---------------------|--------|
| Claude Code | `claude --dangerously-skip-permissions` | skip all permission prompts | yes |
| Codex | `codex --dangerously-bypass-approvals-and-sandbox` | skip all confirmation prompts and execute commands without sandboxing (verified against `codex --help`; the older `--full-auto` flag is not accepted by the interactive CLI) | yes |
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

- `src-tauri/src/presets.rs` — `TerminalPreset`, `PresetLaunchConfig` (structured source: `agent_command` + `model_selection` + `prompt`), `PresetStore`, builtin definitions, SQLite persistence
- `src-tauri/src/agent_capability.rs` — `ModelSelection` + `apply_model_selection`: the shared model/reasoning/context flag-injection used by both the launch dialog and `apply_preset`
- `src-tauri/src/commands/presets.rs` — Tauri commands: `get_presets`, `create_preset` (takes `icon` + `launch_config`), `update_preset` (takes `launch_config` + `clear_launch_config`), `delete_preset`, `apply_preset` (takes a `model_selection` applied to every command), `set_preset_pinned`, `set_preset_bar_visible`, `reorder_presets`
- `src-tauri/src/agent_context.rs` — `inject_agent_context` per-binary preset-command rewrite (context injection for Claude/Codex/Pi/Gemini; `--settings` autonomy injection for Droid)
- `src/lib/launch-models.ts` — `detectLaunchFamily` / `familyToProviderKind` / `REASONING_FLAG_FAMILIES` / `GEMINI_MODELS`: shared launch-model helpers the editor reuses
- `src/components/overlays/launch-model-picker.tsx` + `launch-reasoning-picker.tsx` — the capability-driven pickers, reused verbatim by the preset editor and the New Workspace dialog
- `src/components/icons/preset-icon.tsx` + `src/assets/preset-icons/*.svg` — official agent logos rendered in the bar/tabs
- `src/components/layout/preset-bar.tsx` — quick-launch bar UI (gear menu has **New Preset** → `requestNewPreset` ui-store action; bar-launch passes the preset's `model_selection` to `applyPreset`). When many presets are pinned the bar overflows into a single horizontally-scrollable row (`overflow-x-auto`, fixed `h-8`, buttons `shrink-0` so they never wrap). A plain mouse wheel pans the bar via `useHorizontalWheelScroll` (`src/lib/wheel.ts`): a **native, non-passive** wheel listener — React's root-registered wheel handlers are passive, so the old `onWheel` + `preventDefault` was a silent no-op — with delta-mode normalization (line/page → px) and **edge chaining**: the event is consumed only when the strip actually moved, so wheeling at either end scrolls the ancestor instead of trapping the gesture. `title-bar-tabs.tsx` uses the same hook. The native chat preset (`kind === "chat_agent"`) is **pinned to the far-left slot** — ahead of Claude Code and every other CLI preset — because clicking it opens the GUI chat pane rather than a terminal, so it should be the easiest to find. It renders outside the drag/reorder `DndContext` (it is not reorderable) and is excluded from `serverPinnedIds` / `localPinnedOrder` so the sortable index math never sees it. This is a pure render-time pin: it does not depend on the builtin's position in the persisted store, so existing users (whose stored `Chat Agent` sits at whatever slot it was seeded/synced into) still get it leftmost. The Beta gate is unchanged — when the agent-chat toggle is off the chat preset is filtered out entirely and CLI order is untouched.
- `src/components/settings/settings-view.tsx` — `PresetEditorSheet` (structured + raw modes; structured reuses `LaunchModelPicker`/`LaunchReasoningPicker` sourced from `provider-capabilities-store` + `gemini-models-store`) and the Presets section with the **New preset** draft + `pendingPresetCreate` create-and-open flow
- `src/stores/ui-store.ts` — `pendingPresetCreate` flag + `requestNewPreset` / `clearPendingPresetCreate` actions bridging the bar's "New Preset" to the settings editor
- `src/dev/tauri-mock.ts` — browser-dev mock for the preset commands + provider capabilities (mutable in-memory store, emits `presets-changed`)
