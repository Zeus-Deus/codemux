# Docs Reindex Playbook

- Purpose: Tell future agents how to audit the real codebase and clean the docs system back into shape when it drifts.
- Audience: Anyone doing docs cleanup, reindexing, or recovery after messy sessions.
- Authority: Stable docs-maintenance playbook. This file should change rarely.
- Update when: The docs architecture or cleanup method itself changes.
- Read next: `WORKFLOW.md`, `docs/INDEX.md`

## When To Use This

Use this file when:

- docs feel stale, duplicated, scattered, or too long
- feature docs no longer match implementation
- several sessions landed code without keeping docs clean
- you want one fresh agent session to re-audit the repo and normalize the docs again

## Non-Negotiable Rules

- Do not trust old docs by themselves. Re-check the codebase.
- Rebuild docs from actual repo evidence, not from memory or old prompts.
- Prefer one clean canonical doc over several overlapping docs.
- Keep `WORKFLOW.md` short and stable.
- Keep `docs/core/*` durable, `docs/features/*` factual, `docs/plans/*` active, `docs/reference/*` stable, and `docs/archive/*` historical.
- When creating new docs, start from `docs/templates/FEATURE_TEMPLATE.md` or `docs/templates/PLAN_TEMPLATE.md`.

## Reindex Procedure

1. Read the docs system first:
   - `WORKFLOW.md`
   - `docs/INDEX.md`
   - `docs/core/PROJECT.md`
   - `docs/core/STATUS.md`
   - `docs/core/PLAN.md`
   - `docs/core/TESTING.md`
   - `AGENTS.md` if browser or agent workflows matter
2. Audit the real repo:
   - inspect the current repo structure
   - inspect actual frontend and backend entry points
   - inspect the active feature areas you expect to document
   - inspect existing docs to see what is stale, duplicated, or missing
3. Compare docs against code:
   - what is true now but undocumented
   - what is documented but no longer true
   - what is duplicated across several files
   - what belongs in features vs plans vs archive
4. Normalize the docs:
   - update `docs/core/STATUS.md` from code reality
   - update affected `docs/features/*` files from code reality
   - update affected `docs/plans/*` files so they reflect real next steps
   - move stale notes into `docs/archive/` or delete them if they are no longer useful
   - remove duplicate docs when a clean canonical replacement exists
5. Re-link the system:
   - keep `WORKFLOW.md` pointing to the right read order
   - keep `docs/INDEX.md` pointing to the right canonical docs
   - remove stale references to deleted or superseded files
6. Verify:
   - re-scan for old file paths or duplicate doc names
   - make sure the docs structure still reads cleanly for a brand-new session

## What Good Looks Like

After a reindex pass:

- a new agent can read `WORKFLOW.md` and `docs/INDEX.md` and know where to go next
- `docs/core/STATUS.md` matches repo reality closely enough to trust
- feature docs explain current behavior without turning into giant work logs
- plan docs explain next steps without pretending to be the source of truth
- old clutter is archived or removed instead of piling up

## Minimum Recovery Pass

If time is limited, prioritize this order:

1. `docs/core/STATUS.md`
2. the feature docs for the active subsystem
3. the matching plan docs
4. `docs/INDEX.md`
5. `WORKFLOW.md`

## Suggested Prompt

If the docs drift again, a good recovery prompt is:

`Read WORKFLOW.md and docs/reference/DOCS_REINDEX.md. Audit the current codebase and clean/re-index the docs system so the markdown matches repo reality again. Update docs from code evidence, remove stale duplicates, and keep the docs structure clean.`
