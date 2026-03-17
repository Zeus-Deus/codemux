# Codemux Workflow Guide

This is the first file a new coding session should read.

## Start Here

1. `docs/INDEX.md`
2. `docs/core/PROJECT.md`
3. `docs/core/STATUS.md`
4. `docs/core/PLAN.md`
5. `docs/core/TESTING.md`
6. feature/reference docs relevant to the active area
7. `AGENTS.md` for Codemux-specific agent operating rules

If the docs system feels stale or messy, also read `docs/reference/DOCS_REINDEX.md` before making doc changes.

## Which Doc Owns What

- `docs/core/PROJECT.md`: durable product intent and architecture direction
- `docs/core/STATUS.md`: current repo reality
- `docs/core/PLAN.md`: roadmap and build order
- `docs/core/TESTING.md`: verification policy
- `docs/features/*`: current subsystem capability and constraints
- `docs/plans/*`: active implementation plans and next steps
- `docs/reference/*`: stable command and protocol references
- `docs/archive/*`: superseded design notes worth keeping

## Handoff Discipline

- Update the canonical doc that actually changed, not every doc that mentions it.
- Update `docs/core/STATUS.md` when implementation reality changes.
- Update `docs/core/PLAN.md` or a file in `docs/plans/` when the intended next build steps change.
- Update feature docs when a subsystem meaningfully changes behavior or constraints.
- Use `.codemux/project-memory.json` or `codemux handoff` for compact session memory when it helps the next agent.
- Record concise facts, not transcript dumps.
- If the docs have drifted badly, run the cleanup process in `docs/reference/DOCS_REINDEX.md` instead of patching the mess piecemeal.

## Working Rules

- Prefer the docs hub over scattered root notes.
- Use `STATUS` for truth and `PLAN` for ordering; do not mix them.
- Keep core docs short and durable.
- Move temporary debugging journals into `docs/plans/` or `docs/archive/`.
- Start new feature docs from `docs/templates/FEATURE_TEMPLATE.md`.
- Start new plan docs from `docs/templates/PLAN_TEMPLATE.md`.

## Verification

- Default to `npm run verify` after meaningful changes.
- Use `cargo check --manifest-path src-tauri/Cargo.toml`, `cargo test --manifest-path src-tauri/Cargo.toml`, `npm run check`, and `npm run test` when iterating on one layer.

## Docs Rule

Treat the files under `docs/` as the single source of truth. If a path is not in this workflow or `docs/INDEX.md`, do not assume it is still part of the maintained docs system.
