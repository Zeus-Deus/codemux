# Codemux Workflow Guide

This file is for future coding sessions so work can continue consistently without re-explaining the whole process.

## Read These First

When starting a new session on Codemux, read these files in this order:

1. `PROJECT.md`
2. `PLAN.md`
3. `STATUS.md`
4. `README.md`
5. `TESTING.md`
6. `docs/CONTROL.md`
7. `AGENTS.md` (for agent integration docs)

## Working Rules

- Follow `PLAN.md` in order unless there is a good reason not to.
- Mark completed checklist items in `PLAN.md` only after the implementation exists and has been verified enough to claim the box honestly.
- Update `README.md` when major new user-facing features or workflows land.
- Update `PROJECT.md` when product direction or architecture decisions change.
- Update `STATUS.md` when repo reality changes, especially if implementation, testing, and release-readiness are not aligned.
- Update `docs/CONTROL.md` when CLI/socket protocol changes.
- Update feature-specific docs in `docs/` when a subsystem changes meaningfully, such as browser work in `docs/BROWSER_PLAN.md`.
- Keep `TESTING.md` aligned with the real testing strategy.

## Completion And Handoff Discipline

When meaningful work is finished, leave the repo in a state that helps the next agent start fast.

- Do not end a substantial task with code changes only; also update the relevant docs and handoff memory.
- If implementation reality changed, update `STATUS.md` in the same session.
- If roadmap expectations changed, update `PLAN.md` or the relevant feature doc in `docs/`.
- If the next agent would benefit from the current result, update `.codemux/project-memory.json` or generate a fresh `codemux handoff` summary.
- Record concise facts, not raw transcript dumps: what changed, what now works, what is still broken, and what should happen next.
- If nothing important changed, do not churn docs or memory just to touch them.

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

Default to `npm run verify` when you want the full standard verification pass.

If something new changes how the app is used, update the docs.

If something new changes what the next tool session needs to know, update the project memory/handoff too.

## Current Process Pattern

For each new phase:

1. inspect current files and architecture
2. implement the phase
3. verify with checks/tests
4. mark finished items in `PLAN.md`
5. update docs and handoff memory if needed

## Important Context

- `PROJECT.md` explains what Codemux and OpenFlow are supposed to become.
- `PLAN.md` explains the exact build order.
- `STATUS.md` explains what is actually implemented, partial, and manually validated right now.
- `WORKFLOW.md` explains how to continue work consistently across future sessions.

## Handoff Pattern

For future tool sessions, prefer this order:

1. read `WORKFLOW.md`
2. read `PROJECT.md`
3. read `PLAN.md`
4. read `STATUS.md`
5. read feature-specific docs relevant to the active area, for example browser work in `docs/BROWSER_PLAN.md`
6. use `.codemux/project-memory.json` or `codemux handoff`
7. avoid replaying full raw chat logs unless absolutely necessary

The goal is to continue with structured memory plus workflow guidance, not giant transcript reuse.

## Dynamic Memory Rule

Treat workflow docs plus `.codemux/project-memory.json` as living project memory.

- Future agents should proactively update them when they complete meaningful work, discover a blocker, or change the likely next step.
- This should happen without waiting for the user to ask, as long as the update is factual and clearly useful.
- This is not fully automatic background sync; it works because each new agent reads `WORKFLOW.md` first and follows the same discipline.
