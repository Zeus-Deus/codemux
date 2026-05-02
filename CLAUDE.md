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
