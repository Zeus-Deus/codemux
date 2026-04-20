# Codemux

## Session Bootstrap

1. Read `WORKFLOW.md` and `docs/INDEX.md` at the start of every session.
2. Read the relevant canonical docs under `docs/core/`, `docs/features/`, `docs/plans/`, and `docs/reference/` before making assumptions.
3. Read `AGENTS.md` for agent operating rules (browser automation, Codemux-specific behavior).

## Docs System

- Treat `docs/` as the single source of truth for project documentation.
- Use `docs/templates/FEATURE_TEMPLATE.md` and `docs/templates/PLAN_TEMPLATE.md` when creating new docs.
- If the docs feel stale, scattered, or contradictory, read `docs/reference/DOCS_REINDEX.md` and follow that cleanup process from code evidence.

## Verification

- Default to `npm run verify` after meaningful changes.
- Use `cargo check --manifest-path src-tauri/Cargo.toml`, `cargo test --manifest-path src-tauri/Cargo.toml`, `npm run check`, and `npm run test` when iterating on one layer.

## Spawning Child Processes

Policy follows the principal — who triggered the spawn?

- **Agent-facing spawns** (MCP tool calls, `agent_browser`, OpenFlow orchestrator children, session-adapter helpers, etc.): call `crate::execution::sanitize_gui_env_std(&mut cmd)` (or `sanitize_gui_env_tokio`) immediately before `.output()` / `.spawn()` / `.status()`. This strips `DISPLAY`/`WAYLAND_DISPLAY`/etc. and sets neutralizers so AI-driven tool calls can't pop windows on the user's real desktop.
- **User-initiated spawns** (setup/teardown scripts from the Run button, worktree-include `git ls-files`, etc.): do NOT call the sanitize helpers. The user clicked a button; they expect full desktop env. `docker compose up`, `notify-send`, `xdg-open`, and GUI launches must work.
- Exceptions where the sanitize rule doesn't apply even for agent paths (display access required): `hyprctl`, `ydotool`, `systemctl`, `loginctl`.
- When adding new display/DBus/compositor env vars that leak to children, append them to `gui_env_keys()` in `src-tauri/src/execution/mod.rs`.
- Keep `build_linux_bwrap_args` in sync — the two code paths must strip the same set.
- Terminal PTY spawns (`spawn_pty_for_session`, `spawn_pty_for_agent`) are driven by the session's `persona` field (see `src-tauri/src/presets.rs`): `Persona::Human` → full env; `Persona::Agent` → stripped env. The PTY paths do not call `sanitize_gui_env_std` — they build an `ExecutionPolicy` via `worktree_session_default_for_persona` and apply `env_unset`/`env_set` from the prepared command.

## UI & Feature Work

- The `/codemux-ui` skill auto-loads for visual and component work. It defines design standards, theming rules, and ADE feature patterns.

## Skills

- `/codemux-ui` auto-loads for visual and component work.
- `/codemux-features` auto-loads for new ADE feature implementation.
- `/codemux-openflow` auto-loads for orchestration runtime work.

## Codemux Environment

This terminal runs inside Codemux. Check: `test -n "$CODEMUX"`

### Browser

**Never** use `xdg-open` or system browsers. Use:
- `codemux browser open <url>` — navigate browser pane
- `codemux browser snapshot --dom` — list interactive elements with selectors
- `codemux browser click "<selector>"` — click an element
- `codemux browser fill "<selector>" "<text>"` — type into input
- `codemux browser screenshot` — capture screenshot

Always get a snapshot before interacting so you know what elements exist.

### Commands

- `codemux browser --help` — browser control
- `codemux memory show/set/add` — project memory
- `codemux index build/search` — code search index
- `codemux capabilities` — JSON listing of all commands
- `codemux --help` — discover all subcommands
