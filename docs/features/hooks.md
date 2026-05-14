# Agent Hooks

- Purpose: Describe the agent hook integration that powers real-time agent status tracking across Claude Code, Codex, Gemini, OpenCode, and Pi.
- Audience: Anyone working on agent lifecycle, status indicators, session resume, or the HTTP hook server.
- Authority: Canonical feature doc for the hooks subsystem.
- Update when: Hook script, hook server, per-agent registration, or status event handling changes.
- Read next: `docs/features/session-persistence.md`, `AGENTS.md`

## What This Feature Is

Codemux tracks agent status in real time (amber = working, green = idle, red = needs attention) by registering lifecycle hooks with each supported agent CLI. When an agent fires a lifecycle event, it runs a Codemux-provided script that notifies a local HTTP server, which updates the sidebar and tab bar status dots.

The same hook path also captures the Claude Code session ID for adapter-based resume — without the hook, Codemux would have no way to see the session ID because Claude Code is a TUI (no stdout output of session metadata).

The per-agent event vocabularies and config formats were verified directly against the installed CLI binaries/packages (Codex 0.130 `HookEventNameWire` enum, Gemini CLI 0.42 bundle, `@opencode-ai/plugin` + `@opencode-ai/sdk` type defs, `@mariozechner/pi-coding-agent` `ExtensionAPI`) rather than assumed.

## Current Model

**Four moving pieces**:

1. **HTTP hook server** (`src-tauri/src/hooks.rs`) — a localhost TCP listener on a random port, bound at app startup via `start_hook_server(app)`. The port is stored in a `OnceLock<u16>` and injected into every PTY session as `CODEMUX_HOOK_PORT`. The server parses query-string GET requests of shape `/hook?sessionId={codemux_session}&eventType={event}&agentSessionId={agent_session}` and routes them through `map_event_type()` to state updates that emit `app-state-changed` events back to the React frontend.

2. **Hook notification script** (`~/.codemux/hooks/notify.sh`, written by `ensure_hook_script()`) — a POSIX shell script that takes the event type as its `$1` arg, reads the agent's JSON blob on stdin, extracts `session_id` with `jq` if available, and makes a one-shot `curl` call to the hook server. It degrades gracefully: if `jq` is missing, the session ID capture is skipped but the event is still reported. If `CODEMUX_HOOK_PORT` or `CODEMUX_SESSION_ID` are unset (e.g. because the user is running the agent outside Codemux), the script exits 0 silently. A second script, `gemini-notify.sh`, exists for Gemini because its CLI blocks on the hook until it receives valid JSON on stdout.

3. **Per-agent registration** — at startup `lib.rs` calls one registration function per agent. Each writes into that agent's own config location (see table below). Codex/Gemini/OpenCode/Pi registration is gated on the agent's config directory already existing, so Codemux never creates config for a tool the user hasn't installed. Only the hook section is touched; all other user settings are preserved, and prior Codemux entries are replaced in place so repeated startups don't accumulate duplicates.

4. **Canonical event mapping** (`map_event_type()`) — the single source of truth that normalizes every agent's event vocabulary into the four `PaneStatus` states. The per-agent registration just decides which event names get wired up; this function decides what each one means.

## Per-Agent Registration

| Agent | Mechanism | Config location | Function | Registered events |
|---|---|---|---|---|
| **Claude Code** | Merge nested-format hook entries (event passed as arg) | `~/.claude/settings.json` | `register_claude_code_hooks()` | `UserPromptSubmit`, `Stop`, `PermissionRequest`, `Notification`, `PostToolUse`, `SessionEnd` |
| **Codex** | Merge nested-format hook entries; **bare** command, event read from stdin JSON (Codex ≥0.129 auto-loads it) | `~/.codex/hooks.json` | `register_codex_hooks()` | `UserPromptSubmit`, `PostToolUse`, `PermissionRequest`, `Stop` |
| **Gemini** | Merge nested-format hook entries (event passed as arg) + install a Gemini-specific hook script | `~/.gemini/settings.json` + `~/.codemux/hooks/gemini-notify.sh` | `register_gemini_hooks()` | `BeforeAgent`, `AfterTool`, `AfterAgent`, `Notification`, `SessionEnd` |
| **OpenCode** | Install a JS plugin (OpenCode has no hook-config file) | `$XDG_CONFIG_HOME/opencode/plugin/codemux-notify.js` | `register_opencode_plugin()` | `session.status`, `session.idle`, `session.error`, `permission.ask` |
| **Pi** | Install a TypeScript extension (auto-discovered, no config file) | `~/.pi/agent/extensions/codemux-notify.ts` | `register_pi_extension()` | `before_agent_start`, `tool_execution_end`, `agent_end`, `session_shutdown` |

Claude Code, Codex, and Gemini share the nested hook shape — `{ "hooks": { "<EventName>": [ { "matcher"?, "hooks": [ {type, command} ] } ] } }` — so they all go through the shared `merge_nested_hooks_file()` helper. Claude and Gemini's hook config can pass the event name as a command arg; Codex registers the **bare** `notify.sh` path (matching the format Codex itself writes) and `notify.sh` recovers the event name from the `hook_event_name` field in the JSON it pipes on stdin.

OpenCode and Pi have no hook-config file. OpenCode loads JS plugins; its plugin watches `session.status` busy/idle transitions and `permission.ask` and shells out to `notify.sh`, pre-normalizing to the canonical `Start` / `Stop` / `PermissionRequest` vocabulary. Pi auto-discovers TypeScript extensions in `~/.pi/agent/extensions/`; its extension subscribes to the lifecycle events above and shells out to `notify.sh` via `node:child_process`, gating on `ctx.hasUI` so non-interactive/subagent Pi sessions don't drive the dot.

Codex, Gemini, OpenCode, and Pi registration is gated on the agent's config directory already existing, so Codemux never creates config for a tool the user hasn't installed. Claude Code is always registered.

## Events Tracked

`map_event_type()` understands every agent's vocabulary. Representative entries:

| Event(s) | Agent(s) | Status effect |
|---|---|---|
| `UserPromptSubmit`, `Start`, `task_started`, `BeforeAgent` | Claude, OpenCode, Codex, Gemini | Dot turns amber (agent working) |
| `PostToolUse`, `AfterTool` | Claude, Codex, Gemini, Pi | Dot turns amber — clears a stuck red pulse after the user answers a question |
| `Stop`, `task_complete`, `agent-turn-complete`, `AfterAgent` | All | Dot turns green (agent idle / ready for review) |
| `SessionEnd` / `session_end` | Claude, Gemini | Status cleared (agent exiting) — note Codex has no `SessionEnd` event |
| `PermissionRequest`, `Notification`, `permission.ask`, `exec_approval_request`, `apply_patch_approval_request`, `request_user_input` | Claude, OpenCode, Codex, Gemini | Dot turns red (needs attention) |

The full mapping lives in `map_event_type()`. The per-agent `*_HOOK_EVENTS` constants (and the OpenCode/Pi templates) decide which of these names each agent actually fires.

Claude Code's `Notification` event is overloaded: it fires both for a genuine permission/approval prompt *and* for a 60-second idle reminder ("Claude is waiting for your input"). Only the permission case should raise the red dot — the idle reminder fires on an agent that has already finished and gone idle (green), and there is no follow-up event to clear a red dot raised from it. `notify.sh` therefore inspects the `Notification` payload's `message` field and forwards the event only when it describes a permission/approval request; the idle reminder is dropped before it reaches the hook server.

## What Works Today

- Per-session agent status dots in the sidebar and tab bar for Claude Code, Codex, Gemini, OpenCode, and Pi, updated within milliseconds of the agent firing a lifecycle event
- The working → needs-input → working transition: answering a question (or resolving a permission prompt) clears the red pulse instead of leaving it stuck
- The red "needs attention" pulse for Claude (`PermissionRequest`/`Notification`), Codex (`PermissionRequest`), Gemini (`Notification`), and OpenCode (`permission.ask`)
- Automatic session ID capture for Claude Code, powering adapter-based resume across restarts (see `docs/features/session-persistence.md`)
- Startup registration is idempotent — running Codemux twice doesn't duplicate entries
- Codex/Gemini/OpenCode/Pi registration only touches a tool's config if that tool is already installed
- Graceful no-op if the hook environment variables are missing (safe for users who run an agent outside Codemux)
- Graceful degradation if `jq` is not installed (status still works for Claude/Gemini via the arg; Codex/Pi need `jq` to read the event from stdin JSON)

## Current Constraints

- **Pi has no needs-input event** — Pi's `ExtensionAPI` exposes no distinct permission/approval lifecycle event, so Pi gets the amber/green working cadence but not the red pulse.
- **Shell script, not native** — the hook notifiers are `#!/bin/sh` + `curl` + optional `jq`. Portable across Linux and macOS but not native Windows `cmd.exe`. The main `notify.sh` has a PowerShell variant (`notify.ps1`); the Codex/Gemini/OpenCode/Pi paths are Unix-only for now (their registration functions are no-ops on Windows).
- **No audit log** — hook events are processed and then discarded. If a status transition is wrong, there's no post-hoc way to see the raw events that led to it.
- **Tied to each agent's config schema** — if an agent changes its hook schema or event vocabulary, the matching registration function / template needs to follow. The event names are verified against the installed CLIs (see "What This Feature Is"), so a CLI upgrade that renames events would need a re-verification pass.

## Important Touch Points

- `src-tauri/src/hooks.rs`:
  - `hook_port()` — reads the allocated port from the `OnceLock`
  - `start_hook_server(app)` — binds the TCP listener, spawns the accept loop, returns the allocated port
  - `map_event_type()` — canonical event-name → `PaneStatus` mapping for all agents
  - `ensure_hook_script()` / `write_hook_file()` — write hook artifacts into `~/.codemux/hooks/`, `chmod +x` on Unix
  - `merge_nested_hooks_file()` — shared read/merge/write for the nested hook-config format (Claude, Codex, Gemini)
  - `register_claude_code_hooks()` — edits `~/.claude/settings.json`
  - `register_codex_hooks()` — edits `~/.codex/hooks.json` (only if `~/.codex` exists)
  - `register_gemini_hooks()` — installs `gemini-notify.sh` + edits `~/.gemini/settings.json` (only if `~/.gemini` exists; Unix-only)
  - `register_opencode_plugin()` — installs the OpenCode JS plugin (only if OpenCode's config dir exists; Unix-only)
  - `register_pi_extension()` — installs the Pi TypeScript extension into `~/.pi/agent/extensions/` (only if `~/.pi` exists; Unix-only)
  - `unregister_claude_code_hooks()` — removes the Codemux hook entries from `~/.claude/settings.json`
  - `build_*_hooks_json()` / `build_opencode_plugin()` / `build_pi_extension()` — construct the on-disk hook artifacts (used by tests)
  - `shell_is_foreground()` — Linux-only helper reading `/proc/{pid}/stat` to suppress hook-triggered notifications when the user's shell is backgrounded
- `src-tauri/src/lib.rs` — `run()` calls `start_hook_server` + the five `register_*` functions during `.setup()`
- `src-tauri/src/terminal/mod.rs` — sets `CODEMUX_HOOK_PORT` and `CODEMUX_SESSION_ID` on every PTY child
- `src-tauri/src/session_adapters.rs` — consumes captured Claude session IDs for resume (`agentSessionId` query parameter)
- `AGENTS.md` — user-facing description of the hook integration
