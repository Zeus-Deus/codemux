# Plugin platform implementation ledger

Requirements: [engineering specification](BUILD-SPEC.md), revision 2, all 14 chapters.
Baseline: desktop 797a834c; website afe06a6 (newer than specification baseline).
Worktree: isolated; original untracked Hermes integration preserved.

## Ordered delivery

1. Contracts, independent host, SDK and author CLI — in progress.
2. Native manager, permission broker and scoped operations — pending.
3. Trusted UI, palette, deck and controlled composer adapter — pending.
4. Settings, transactional lifecycle and recovery — pending.
5. Independently packed examples and release packaging — pending.
6. Reviewed catalog generation and publication — pending.
7. Website pinned catalog and author docs — pending.
8. Native release hardening and acceptance evidence — pending.

No release is authorized by a passing research probe. The acceptance matrix in the
specification is the release gate. Unrun checks are not passing checks.

## Clarifications backed by source requirements

The sample manifest uses `includeFiles`, but chapter 5 explicitly restricts setting
IDs to `[a-z][a-z0-9-]{0,39}`. Use `include-files` in packages and enforce the stated
identifier grammar. Credential IDs follow that same local identifier grammar.

Hermes contribution ownership, scoped registration and host-owned cleanup informed
the implementation. Its renderer loader explicitly has full application authority;
none of that loader's dynamic-import execution path is reused.

## PR 1 local evidence (Linux x86_64)

- Re-ran all three supplied research-probe modes: pass.
- Protocol crate: 7 tests pass (manifest rejection, bounded frames, generation/
  direction validation, quotas, atomic UI batches, callback disposal, tree limits).
- Native child crate: 3 process tests pass, including five distinct hostile JS
  workloads, absent ambient globals, EOF, stale generation and oversized input.
- Built and type-checked the public SDK; packed both author packages.
- Generated a starter outside the checkout; installed only the packed packages;
  build, type check, static package check and .cmxaddon packing passed.
- Production host/SDK stdio integration passed: JSX view, callback, scoped composer
  request, async response, UI update and unmount. The broker response in this test
  is a fixture; it does **not** establish desktop composer correctness.
- Main frontend `npm run check`: passed. No main-renderer integration yet.
- Windows GNU CI added but not run locally (no Windows environment).

Initialization transfers the permitted 5 MiB bundle as bounded `initialize`
source chunks. This resolves the 5 MiB source / 1 MiB frame requirements without
relaxing either limit or granting filesystem access to JS.

PR 1: https://github.com/Zeus-Deus/codemux/pull/390 (draft).
Initial CI: Linux host + SDK passed; Windows GNU host compiled and all native
process tests passed. Windows schema comparison failed solely on checkout CRLF
versus generated LF. Compare with CRLF normalization; SDK native callback test
on Windows remains pending the corrected run.
