# Session Persistence

- Purpose: Describe the terminal scrollback persistence and session adapter system.
- Audience: Anyone working on terminal restore, session adapters, or troubleshooting resume behavior.
- Authority: Canonical feature doc for session persistence.
- Update when: Scrollback behavior, adapter format, close sequence, or restore UI change.
- Read next: `docs/features/terminal.md`, `docs/features/presets.md`

## What This Feature Is

Session persistence saves terminal scrollback content when Codemux closes and restores it when reopened. For CLI tools that support it (like Claude Code), an adapter system can detect session metadata and offer a "Resume" button to continue where the user left off.

## Current Model

### Scrollback Serialization

On close, the Rust backend emits `serialize-terminal-buffers` to the frontend. Each mounted `TerminalPane` uses `@xterm/addon-serialize` to capture its buffer content. The serialized data is sent back to the backend via `save_terminal_scrollback` and written to `~/.local/share/codemux/scrollback/{workspace_id}/{pane_id}.dat` (Linux) or `%APPDATA%\codemux\scrollback\{workspace_id}\{pane_id}.dat` (Windows) with a JSON metadata sidecar.

The backend waits for the frontend to ack with a `scrollback-serialization-complete` event, then closes regardless. The timeout differs per platform:

- **Linux/macOS**: 3 seconds. Tauri IPC is fast enough that the happy path completes well under budget.
- **Windows**: 10 seconds. Slower Tauri IPC + slower xterm serialization for many panes can exceed 3s, which silently truncates serialization and leaves panes "fresh" on the next launch. 10s gives enough headroom without making clean closes feel sluggish.

### `ScrollbackCache` Backend Backstop (Windows)

Inactive tabs and workspaces never re-mount their `TerminalPane`, so the frontend serialization dance only sees panes that are currently visible. The backend keeps an in-memory `ScrollbackCache` for sessions that were unmounted during the session (tab/workspace switches), and on Linux that cache is normally drained by the frontend's `flushScrollbackCache` call before the close timeout fires.

On Windows, slower IPC means the cache flush can race the timeout. As a safety net, the close handler runs `scrollback::flush_cache_to_disk(&cache)` after the timeout regardless of whether the frontend completed. The function is idempotent — if the frontend already drained the cache, this returns 0 and is a no-op. The backstop is `#[cfg(windows)]`-gated because Linux IPC is fast enough that the frontend reliably completes the serialization dance well within budget; on Linux the backstop would never have anything to flush in practice, but the cost of running it is so small it could be enabled there too if a similar race ever surfaces.

The cache also runs `refresh_stale_scrollback_metadata()` on close to update sidecar files for sessions that were never re-serialized by the frontend (inactive workspaces).

On open, each pane checks for saved scrollback. If found, the content is written to xterm before the new PTY attaches, followed by a `── session restored ──` separator.

### Session Adapter System

Adapters are defined in `~/.config/codemux/session-adapters.toml`. Each adapter specifies:

- `detect_pattern` — regex matched against the original command that spawned the terminal
- `capture` — patterns to extract from terminal output (e.g. session IDs)
- `resume_args` — what to append to the preset command for resume
- `resume_label` — button text shown in the restored pane
- `validate` — optional shell command to check if resume is possible

When a PTY spawns with a known command (from a preset), the adapter system attaches a lightweight output scanner to the PTY read loop. It scans the first 200 lines or 60 seconds of output, capturing metadata via regex.

On restore, if adapter metadata exists, the frontend checks if resume is valid and shows a small inline widget with a Resume button.

The Resume action waits for the PTY writer to become available before sending the command, so restored sessions are safe to resume immediately on startup.

### Adapter TOML Format

```toml
[adapters.claude-code]
detect_pattern = "claude"

capture = [
  { key = "session_id", pattern = 'Session ID:\s*([a-f0-9-]+)' }
]

resume_args = "--resume {session_id}"
resume_label = "Resume Claude Code session"
validate = "claude sessions list 2>/dev/null | grep -q {session_id}"
```

**Fields:**

| Field | Required | Description |
|-------|----------|-------------|
| `detect_pattern` | yes | Regex matched against the spawn command |
| `capture` | no | Array of `{ key, pattern }` — regex with one capture group |
| `resume_args` | no | Args appended to preset command. `{key}` placeholders filled from captures |
| `resume_label` | no | Button label in the restore UI |
| `validate` | no | Shell command to test if resume is possible. `{key}` placeholders supported |
| `fallback_resume_args` | no | Used when `resume_args` has unresolved placeholders (missing captures) |

### Hook-Based Session ID Capture (Claude Code)

Claude Code is a TUI app — it doesn't print session IDs to stdout. Instead, Codemux captures the session ID via Claude Code's hook system. When a hook fires (e.g. `UserPromptSubmit`), Claude Code passes JSON on stdin that includes `session_id`. Codemux's `notify.sh` extracts this and forwards it to the hook server.

The built-in adapter config:

```toml
[adapters.claude-code]
detect_pattern = "claude"
resume_args = "--resume {claude_session_id}"
resume_label = "Resume Claude Code session"
fallback_resume_args = "--continue"
```

- If the hook captured a session ID → `--resume <exact-uuid>` (works with multiple panes in the same worktree)
- If no session ID was captured (jq missing, hooks didn't fire) → falls back to `--continue` (resumes most recent session in CWD)

### Fallback Resume Args

Adapters can define `fallback_resume_args` as a safety net. If `resume_args` has unresolved `{key}` placeholders (because the capture failed or the hook didn't fire), the fallback is used instead.

This enables a graceful degradation chain:
1. Exact session resume via captured ID
2. Directory-based resume via `--continue`
3. Fresh shell if adapter check fails entirely

### Adding a Custom Adapter

1. Edit `~/.config/codemux/session-adapters.toml`
2. Add a new `[adapters.your-tool]` section
3. Set `detect_pattern` to match your tool's command
4. Set `resume_args` to the flag your tool uses to resume
5. Optionally add `capture` patterns if your tool prints a session ID to stdout
6. Restart Codemux

**Example with output capture:**

```toml
[adapters.my-agent]
detect_pattern = "my-agent"
capture = [
  { key = "run_id", pattern = 'Run started: ([A-Z0-9]+)' }
]
resume_args = "--continue {run_id}"
resume_label = "Continue my-agent run"
```

**Example with hook-based capture (TUI tools):**

If your TUI tool exposes session IDs through a hook or callback system (similar to Claude Code), you can capture the ID via `notify.sh` and use `{claude_session_id}` as the capture key. The hook script extracts `session_id` from the JSON stdin that Claude Code passes to hooks.

For other TUI tools that expose session IDs through different channels, you can modify the hook script to capture from whatever source your tool provides (file, env var, API) and forward it to the hook server via the `agentSessionId` query parameter.

### Alternate Screen Buffer (TUI Apps)

Full-screen TUI applications (vim, htop, Claude Code, lazygit, etc.) use the terminal's alternate screen buffer. Serialized alternate-buffer content is garbled and unusable.

Codemux detects this automatically: if the terminal was in alternate buffer mode when serialized, the scrollback data is **not restored**. The pane gets a clean fresh shell. If an adapter matched, Codemux auto-resumes the restored session after the shell is ready.

## What Works Today

- scrollback saved to disk on close, restored on open (including inactive tabs and workspaces)
- alternate screen buffer detection — TUI panes get clean fresh shells
- `── session restored ──` separator between old and new output
- session adapter config in TOML with user-editable file
- output scanner captures metadata from PTY output (first 200 lines / 60s)
- auto-resume for restored adapter-backed sessions after PTY readiness
- adapters work with or without output capture (Claude Code uses `--continue`)
- `validate` command checks if resume is actually possible
- original_command tracked per terminal session
- adapter captures enriched automatically by the Rust backend at save time
- settings: enabled toggle, scrollback lines cap, max disk usage
- orphan cleanup on startup (removes scrollback for deleted workspaces)
- disk limit enforcement on startup
- graceful degradation: any failure in restore = fresh shell

## Current Constraints

- only plain text serialization (xterm-addon-serialize) — no image content
- adapter output scanning is regex-based, not structural
- capture patterns only match within single lines
- resume constructs the full command as `{original_preset_command} {resume_args}` — no support for command rewriting
- no per-adapter timeout configuration (global 200 lines / 60s)

## Important Touch Points

- `src-tauri/src/scrollback.rs` — scrollback file I/O, cleanup, disk limits, `ScrollbackCache`, `flush_cache_to_disk`, `refresh_stale_scrollback_metadata`
- `src-tauri/src/session_adapters.rs` — adapter config loading, output scanner, resume commands
- `src-tauri/src/lib.rs` — close handler (serialize-terminal-buffers event), startup cleanup, `SCROLLBACK_TIMEOUT_SECS` (3s on Unix, 10s on Windows), Windows-only backend backstop
- `src-tauri/src/terminal/mod.rs` — scanner wiring in PTY read loop
- `src-tauri/src/settings_sync.rs` — `SessionRestoreSettings` struct (defaults: `enabled=true`, `scrollback_lines=10000`, `max_total_mb=100`)
- `src-tauri/src/state/state_impl.rs` — `original_command` field on `TerminalSessionSnapshot`
- `src-tauri/src/commands/presets.rs` — sets `original_command` when applying presets
- `src/components/terminal/TerminalPane.tsx` — serialize addon and scrollback restore
- `src/hooks/use-scrollback-serializer.ts` — global serialization coordinator
- `src/tauri/commands.ts` — scrollback and adapter command wrappers
- `~/.config/codemux/session-adapters.toml` — user-editable adapter config
- `~/.local/share/codemux/scrollback/` (Linux) or `%APPDATA%\codemux\scrollback\` (Windows) — scrollback storage on disk

## Troubleshooting

**Scrollback not restoring:**
- Check Settings > Session Restore > enabled is on
- Verify files exist at `~/.local/share/codemux/scrollback/`
- Check Codemux logs for serialization errors

**Resume button not showing:**
- Verify `~/.config/codemux/session-adapters.toml` has a matching adapter
- Check that the tool prints its session ID in the first 200 lines of output
- Run the `validate` command manually to see if it succeeds

**How to reset:**
- Delete `~/.local/share/codemux/scrollback/` to clear all saved scrollback
- Delete `~/.config/codemux/session-adapters.toml` to regenerate default adapters
