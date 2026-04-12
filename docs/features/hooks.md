# Agent Hooks

- Purpose: Describe the Claude Code hook integration that powers agent status tracking.
- Audience: Anyone working on agent lifecycle, status indicators, session resume, or the HTTP hook server.
- Authority: Canonical feature doc for the hooks subsystem.
- Update when: Hook script, hook server, Claude settings integration, or status event handling changes.
- Read next: `docs/features/session-persistence.md`, `AGENTS.md`

## What This Feature Is

Codemux tracks Claude Code agent status in real time (amber = working, green = idle, red = needs attention) by registering hooks with Claude Code's `~/.claude/settings.json`. When Claude Code fires a hook event, it runs a Codemux-provided shell script that notifies a local HTTP server, which updates the sidebar and tab bar status dots.

The same hook path also captures the Claude Code session ID for adapter-based resume — without the hook, Codemux would have no way to see the session ID because Claude Code is a TUI (no stdout output of session metadata).

## Current Model

**Three moving pieces**:

1. **HTTP hook server** (`src-tauri/src/hooks.rs`) — a localhost TCP listener on a random port, bound at app startup via `start_hook_server(app)`. The port is stored in a `OnceLock<u16>` and injected into every PTY session as `CODEMUX_HOOK_PORT`. The server parses query-string GET requests of shape `/hook?sessionId={codemux_session}&eventType={event}&agentSessionId={claude_session}` and routes them to state updates that emit `app-state-changed` events back to the React frontend.

2. **Hook notification script** (`~/.codemux/hooks/notify.sh`, written by `ensure_hook_script()`) — a POSIX shell script that reads the Claude Code JSON blob on stdin, extracts `session_id` with `jq` if available, and makes a one-shot `curl` call to the hook server. It degrades gracefully: if `jq` is missing, the session ID capture is skipped but the event is still reported. If `CODEMUX_HOOK_PORT` or `CODEMUX_SESSION_ID` are unset (e.g. because the user is running Claude Code outside Codemux), the script exits 0 silently.

3. **Claude Code settings registration** (`register_claude_code_hooks()`) — edits `~/.claude/settings.json` on app startup to add three hook entries under the `hooks` key. Only the `hooks` section is touched; all other user settings are preserved. The writer deduplicates entries that already reference `.codemux/hooks/notify.sh` so repeated startups don't fill the file with duplicates.

## Events Tracked

| Event | Trigger | Status effect |
|---|---|---|
| `UserPromptSubmit` | User submitted a prompt to Claude Code | Dot turns amber (agent working) |
| `Stop` | Claude Code finished responding | Dot turns green (agent idle) |
| `PermissionRequest` | Claude Code is asking for tool-use approval | Dot turns red (needs attention) |

The exact event names are defined in `register_claude_code_hooks()` and must match Claude Code's hook event vocabulary.

## What Works Today

- Per-session agent status dots in the sidebar and tab bar, updated within milliseconds of Claude Code firing a hook
- Automatic session ID capture for Claude Code, powering adapter-based resume across restarts (see `docs/features/session-persistence.md`)
- Startup registration is idempotent — running Codemux twice doesn't duplicate entries
- Unregistration on app quit via `unregister_claude_code_hooks()` — leaves the user's `~/.claude/settings.json` clean
- Graceful no-op if the hook environment variables are missing (safe for users who run Claude Code outside Codemux)
- Graceful degradation if `jq` is not installed (status still works; session capture falls back to `--continue`)

## Current Constraints

- **Claude Code only** — Codex, Pi, Gemini, and OpenCode do not have equivalent hook systems in their CLIs, so their status dots rely on PTY-level output parsing (see `docs/features/session-persistence.md` for the per-adapter story). The hook server does receive events for other agents when they're forwarded via `notify.sh`, but there's no built-in registration path for non-Claude CLIs.
- **Shell script, not native** — the hook notifier is `#!/bin/sh` + `curl` + optional `jq`. This is portable across Linux and macOS but doesn't work on native Windows `cmd.exe` without Git for Windows providing `sh`. Codemux's Windows path currently relies on Git Bash being in `PATH`.
- **No audit log** — hook events are processed and then discarded. If a status transition is wrong, there's no post-hoc way to see the raw events that led to it.
- **Tied to `~/.claude/settings.json`** — if Claude Code changes its hook schema format, `register_claude_code_hooks()` needs to match. The writer checks for both nested (`{"hooks": [...]}`) and legacy flat format when cleaning up stale entries.

## Important Touch Points

- `src-tauri/src/hooks.rs`:
  - `hook_port()` — reads the allocated port from the `OnceLock`
  - `start_hook_server(app)` — binds the TCP listener, spawns the accept loop, returns the allocated port
  - `ensure_hook_script()` — writes `~/.codemux/hooks/notify.sh`, `chmod +x` on Unix
  - `register_claude_code_hooks()` — edits `~/.claude/settings.json` on startup
  - `unregister_claude_code_hooks()` — removes the Codemux hook entries on shutdown
  - `build_claude_hooks_json(script_path)` — constructs the three-event hook block
  - `shell_is_foreground()` — Linux-only helper reading `/proc/{pid}/stat` to suppress hook-triggered notifications when the user's shell is backgrounded
- `src-tauri/src/lib.rs` — `run()` calls `start_hook_server` + `register_claude_code_hooks` during `.setup()` and `unregister_claude_code_hooks` on exit
- `src-tauri/src/terminal/mod.rs` — sets `CODEMUX_HOOK_PORT` and `CODEMUX_SESSION_ID` on every PTY child
- `src-tauri/src/session_adapters.rs` — consumes captured Claude session IDs for resume (`agentSessionId` query parameter)
- `AGENTS.md` — user-facing description of the hook integration
