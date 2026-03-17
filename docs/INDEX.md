# Codemux Docs Index

- Purpose: Canonical internal docs hub for new sessions and future handoffs.
- Audience: Humans and coding agents continuing work in this repo.
- Authority: Entry point for the internal documentation system.
- Update when: The doc structure, read order, or file ownership changes.
- Read next: `docs/core/PROJECT.md`, `docs/core/STATUS.md`

## Read Order For New Sessions

1. `docs/core/PROJECT.md`
2. `docs/core/STATUS.md`
3. `docs/core/PLAN.md`
4. `docs/core/TESTING.md`
5. relevant feature docs in `docs/features/`
6. `docs/reference/ARCHITECTURE.md` if you need the repo/layer map
7. `docs/reference/CONTROL.md` if touching CLI, socket, browser automation, memory, or indexing
8. `AGENTS.md` for Codemux-specific agent operating rules

If the docs themselves feel stale or scattered, also read `docs/reference/DOCS_REINDEX.md`.

## Canonical Layers

- `docs/core/*`: durable project truth
- `docs/features/*`: current subsystem capability and constraints
- `docs/reference/*`: stable protocol and command references
- `docs/plans/*`: active implementation notes and next steps
- `docs/archive/*`: superseded notes kept for context

## Current Entry Points

- Browser work: `docs/features/browser.md`, `docs/plans/browser.md`
- OpenFlow work: `docs/features/openflow.md`, `docs/plans/openflow.md`
- Repo boundaries: `docs/reference/ARCHITECTURE.md`
- Control and automation work: `docs/reference/CONTROL.md`
- Docs cleanup and recovery work: `docs/reference/DOCS_REINDEX.md`
- Agent behavior rules: `AGENTS.md`

## Update Rules

- Update `docs/core/PROJECT.md` when the product direction or architecture boundaries change.
- Update `docs/core/STATUS.md` when implementation reality changes.
- Update `docs/core/PLAN.md` when build order or major milestones change.
- Update `docs/core/TESTING.md` when the verification strategy changes.
- Update feature docs when subsystem behavior or constraints change.
- Update plan docs when active next steps or working notes change.
- Move stale notes to `docs/archive/` instead of leaving them mixed into canonical docs.
- Start new feature docs from `docs/templates/FEATURE_TEMPLATE.md`.
- Start new plan docs from `docs/templates/PLAN_TEMPLATE.md`.

## Single Source Of Truth

The maintained docs system now lives entirely in:

- `WORKFLOW.md`
- `docs/core/*`
- `docs/features/*`
- `docs/plans/*`
- `docs/reference/*`
- `docs/archive/*`
- `docs/templates/*` as helper starting points for new docs
