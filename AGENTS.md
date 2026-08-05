# Codemux

## Session Bootstrap

1. Read `WORKFLOW.md` and `docs/INDEX.md` at the start of every session.
2. Read the relevant canonical docs under `docs/core/`, `docs/features/`, `docs/plans/`, and `docs/reference/` before making assumptions.

## Docs System

- Treat `docs/` as the single source of truth for project documentation.
- Use `docs/templates/FEATURE_TEMPLATE.md` and `docs/templates/PLAN_TEMPLATE.md` when creating new docs.
- If the docs feel stale, scattered, or contradictory, read `docs/reference/DOCS_REINDEX.md` and follow that cleanup process from code evidence.

## Verification

- Default to `npm run verify` after meaningful changes.
- Limit Cargo compilation to two jobs (`CARGO_BUILD_JOBS=2`).
- Use `cargo check --manifest-path src-tauri/Cargo.toml`, `cargo test --manifest-path src-tauri/Cargo.toml`, `npm run check`, and `npm run test` when iterating on one layer.

## Visual Verification

### Visual verification (UI work)

When iterating on UI:

1. `npm run dev` — boots Vite with the Tauri mock auto-installed.
2. `codemux browser open http://localhost:1420` — the real Codemux UI loads with seed data.
3. `codemux browser screenshot` — capture visual proof.

The mock lives in `src/dev/` and only loads when no real Tauri runtime is detected. For real-IPC testing, use `npm run tauri:dev` (desktop window, not browser-pane-visible).

## UI & Feature Work

- For visual and component work, read `docs/reference/DESIGN-SYSTEM.md` (color tokens, theming layers, the no-hardcoded-colors rule) plus the relevant `docs/features/*` doc. (The `/codemux-ui` skill was removed in PR #115; its theming rules now live in the design-system reference.)

## Skills

- `/codemux-features` auto-loads for new ADE feature implementation.

## Codemux Environment

This terminal runs inside Codemux. Check: `test -n "$CODEMUX"`

### Browser

**Never** use `xdg-open` or system browsers. Use:
- `codemux browser open <url>` — navigate browser pane
- `codemux browser snapshot --dom` — list interactive elements with selectors
- `codemux browser click "<selector>"` — click an element
- `codemux browser fill "<selector>" "<text>"` — type into input
- `codemux browser screenshot` — capture screenshot
- `codemux browser viewport <mobile|tablet|desktop|...|WxH|reset>` — resize viewport for responsive testing (CSS media queries fire at the new width); `codemux browser viewport-presets` lists available presets

Always get a snapshot before interacting so you know what elements exist.

### Commands

- `codemux browser --help` — browser control
- `codemux memory show/set/add` — project memory
- `codemux index build/search` — code search index
- `codemux capabilities` — JSON listing of all commands
- `codemux --help` — discover all subcommands
