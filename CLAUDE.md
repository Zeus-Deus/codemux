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

## Browser Automation

- **Never** use `xdg-open` or `open` — always use `codemux browser open <url>`.
- Check `$CODEMUX_WORKSPACE_ID` to detect if running inside Codemux.
- Get a snapshot before interacting so you know what elements exist.
