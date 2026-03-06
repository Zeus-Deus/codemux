# Codemux Workflow Guide

This file is for future coding sessions so work can continue consistently without re-explaining the whole process.

## Read These First

When starting a new session on Codemux, read these files in this order:

1. `PROJECT.md`
2. `PLAN.md`
3. `README.md`
4. `TESTING.md`
5. `docs/CONTROL.md`

## Working Rules

- Follow `PLAN.md` in order unless there is a good reason not to.
- Mark completed checklist items in `PLAN.md` as soon as the work is actually done.
- Update `README.md` when major new user-facing features or workflows land.
- Update `PROJECT.md` when product direction or architecture decisions change.
- Update `docs/CONTROL.md` when CLI/socket protocol changes.
- Keep `TESTING.md` aligned with the real testing strategy.

## Implementation Style

- Prefer finishing a phase cleanly before jumping ahead.
- Keep the project Linux-first, but avoid locking architecture to Linux only.
- Keep OpenFlow modular so Codemux is the flagship host, but the engine can be embedded elsewhere later.
- Keep memory and indexing local-first.
- Avoid over-engineering before a solid working implementation exists.

## Verification Expectations

After meaningful changes, run the relevant checks:

- `cargo check --manifest-path src-tauri/Cargo.toml`
- `cargo test --manifest-path src-tauri/Cargo.toml`
- `npm run check`
- `npm run test`

If something new changes how the app is used, update the docs.

## Current Process Pattern

For each new phase:

1. inspect current files and architecture
2. implement the phase
3. verify with checks/tests
4. mark finished items in `PLAN.md`
5. update docs if needed

## Important Context

- `PROJECT.md` explains what Codemux and OpenFlow are supposed to become.
- `PLAN.md` explains the exact build order.
- `WORKFLOW.md` explains how to continue work consistently across future sessions.

## Handoff Pattern

For future tool sessions, prefer this order:

1. read `WORKFLOW.md`
2. read `PROJECT.md`
3. read `PLAN.md`
4. use `.codemux/project-memory.json` or `codemux handoff`
5. avoid replaying full raw chat logs unless absolutely necessary

The goal is to continue with structured memory plus workflow guidance, not giant transcript reuse.
