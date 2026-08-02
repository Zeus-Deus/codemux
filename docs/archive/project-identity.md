# Project Identity (first-class projects + workspace kind)

- Purpose: Preserve the completed implementation record for first-class project identity and explicit `main | worktree` workspace kind.
- Audience: Anyone investigating the original design, migration, or rollout.
- Authority: Historical implementation record. Current behavior lives in `docs/features/workspaces-sync.md` and `docs/features/workspaces-overview.md`.
- Update when: Rarely; only to correct historical inaccuracies.
- Read next: `docs/features/workspaces-sync.md`, `docs/features/workspaces-overview.md`
- Status: SHIPPED in `v0.7.4` — Phases 1, 2, and the cloud round-trip landed. The optional first-class `projects` table was not part of this initiative.

> Historical snapshot: present-tense "Today", "remaining", and "local-only" statements below describe the implementation sequence at the time. The feature docs above supersede them.

## Goal

Today "project" is an implicit, path-derived concept: the displayed name is `basename(project_root)`, and there is no stable identity that survives a repo being cloned at different paths on different devices/hosts, nor any explicit `main`-vs-`worktree` distinction (it's inferred from `worktree_path == null`). This caused agent-/host-created workspaces to appear anonymous and unrelated to their siblings. We want the Superset guarantees — stable project identity, `main|worktree` typing, "a project can never be worktrees-without-a-root", and cross-host convergence of the same repo — implemented the clean way for our stack.

## Design decision: deterministic identity (the key deviation from Superset)

Superset mints a random UUID once per project on a host and **replicates it** to every device via cloud Postgres, reconciling independent clones by canonical git remote (`findByGitHubRemote` returns candidates, host picks). That requires a replicated projects table and a reconcile step.

Codemux instead computes identity **deterministically**:

```
canonical_key = canonical_remote(repo)   // e.g. https://github.com/owner/name, lowercased, no .git
              ?? absolute(project_root)   // local-only repos with no remote
project_uid   = UUIDv5(CODEMUX_PROJECT_NAMESPACE, canonical_key)
```

Every checkout of the same remote, on any host or device, computes the **same** `project_uid` with zero coordination — so cross-host/device convergence falls out for free and does not depend on the (external) cloud API. Local-only repos (no remote) get a device-stable uid from their path; they can't converge across devices until a remote is added, which is acceptable and documented. This is strictly simpler than Superset's replicated-UUID + reconcile and is a better fit for a REST + SSH stack.

`workspace_kind`:
- `main` — the repo root checkout (`.git` is a directory; equivalently `worktree_path` is null / path == git root).
- `worktree` — a per-branch worktree (`.git` is a file pointing at the parent repo).

## Active Priorities

### Phase 1 — host/sync path + kind + sweep (THIS change; fully in-repo + testable)

1. **Shared identity helper** — new `src-tauri/src/project_identity.rs`: `canonical_remote(&str) -> Option<String>` (github/gitlab/bitbucket ssh+https → canonical https, lowercased, `.git`/trailing-slash stripped), `project_uid(canonical_key: &str) -> String` (UUIDv5; add `uuid` `v5` feature), `derive_kind(path: &Path) -> WorkspaceKind` (`.git` dir vs file). Pure, unit-tested.
2. **Daemon registry** — `remote/workspace.rs`: add `project_uid`, `project_name`, `kind`, `repo_remote` columns; port the desktop's idempotent `ALTER TABLE … ADD COLUMN` migration loop into `create_schema` (the daemon DB currently has NO upgrade path — highest-risk item). `WorkspaceStore::create` stamps all four (uid from canonical remote else project_root; name from basename; kind from `.git`; remote from `git remote get-url origin` when resolvable). Add `sweep_normalize_main()` and call it once from `run_serve_async` after `open` (idempotent; promotes the root checkout of each project_uid to `main`, demotes duplicates).
3. **Wire + poller** — `Workspace` already serializes verbatim, so the four fields appear on the `codemux-remote workspace list` envelope automatically. Add them to `RemoteWorkspace` (`hosts_inventory.rs`) and thread through `reconcile_host_inventory` into `workspaces_sync`. This also populates `project_remote` for remote-discovered rows (currently hardcoded null — fixes a documented v1 gap).
4. **Local SQLite** — `database.rs`: add `project_uid`, `project_name`, `workspace_kind` to `workspaces_sync` via the ALTER loop (append-only, read at the highest positional indices to avoid the `row_to_workspace_sync` off-by-one footgun). Extend `WorkspaceSyncRecord` + every CRUD SELECT/INSERT/UPDATE + the remote-discovered helpers. These are **local-only columns for now** (like `origin_uid`) — `upsert_workspace_sync_from_server` must NOT clobber them, so they survive cloud pulls until Phase 2 makes the cloud authoritative.
5. **Command + UI** — `WorkspaceSyncView` (Rust + `src/tauri/commands.ts`) exposes the three fields. The overview groups remote rows by `project_uid` (falling back to the existing name grouping) and renders a `main`/`worktree` badge. `detect_same_branch_project_conflict` upgrades from `(basename(project_path), git_branch)` to `(project_uid, git_branch)` when uid is present.

### Phase 2 — desktop-local first-class uid (DONE) + cloud round-trip (remaining)

6. ~~Add `project_uid` + `workspace_kind` to `WorkspaceSnapshot`, stamp at create, thread through `reconcile_from_snapshot`.~~ **Done** — stamped in `set_workspace_project_root` (the single choke point every create path calls; git remote computed outside the lock), threaded into the local `workspaces_sync` columns, and the pull-conflict guard now matches exactly on `project_uid`. Local UI grouping uses `project_uid` as its `projectKey`.
7. **Remaining — external `codemux-api` repo** (cannot be edited/tested from here): add optional `project_uid`/`project_name`/`workspace_kind` columns to `codemux_workspaces` + validation + round-trip tests, mirror in `preload.ts`. Today the local columns are not synced to the cloud, so a row pulled on another device has `project_uid = null` there and falls back to path/name grouping. With deterministic uid this is an optimization (a pulled row could recompute uid from the synced `project_remote`), not a correctness requirement.

### Phase 2 cloud — DONE (deployed + verified on the VPS)

Shipped: `codemux_workspaces` gained `project_uid` / `project_name` / `workspace_kind` (additive `ADD COLUMN IF NOT EXISTS`, mirroring `git_head_sha`), threaded through the GET/POST/PATCH handlers + `parseWorkspaceBody` validation (`workspaceKind ∈ {main,worktree}`), mirrored in `preload.ts`, with 4 round-trip tests. Deployed via `docker compose up -d --build api`. **303 server tests pass; the 44 production rows were untouched; container healthy; no errors in logs.** Desktop side: `ServerWorkspace` + `WorkspaceUpsertBody` carry the fields; `push_insert`/`push_update` send them; `upsert_workspace_sync_from_server` now persists them (server-authoritative on pull). Timestamped `.bak-<ts>` copies of the 3 server source files were left in `~/codemux-api/api/src/` as a code-rollback safety net (harmless to the build/test globs); the pre-change DB snapshot is `backups/codemux_20260529_144518.sql.gz`.

The original concrete plan (kept for reference):

#### Concrete plan (codemux-api, VPS-verified)

Verified live on the VPS (`~/codemux-api`, `ssh work@78.47.192.173`): `codemux_workspaces` exists with 44 rows; daily 3am backups healthy; `git_head_sha` was added via `ALTER TABLE … ADD COLUMN IF NOT EXISTS` — this change mirrors it exactly. **100% additive, no data loss, no destructive ops.**

**Safety first**: run `~/codemux-api/backup.sh` (manual snapshot) before deploying. Additive nullable columns mean the 44 rows are untouched; old desktop clients ignore unknown GET fields and simply don't send the new ones on POST (→ null). Rollback = revert code + rebuild; the 3 nullable columns can stay harmlessly.

**Server changes — `~/codemux-api/api/src/index.ts`** (mirror in `api/src/tests/preload.ts`):
1. DDL: in the startup CREATE block (after the `git_head_sha` ALTER, ~line 260) append:
   `ALTER TABLE codemux_workspaces ADD COLUMN IF NOT EXISTS project_uid TEXT;` + `project_name TEXT;` + `workspace_kind TEXT;`
2. `WorkspaceFields` + `parseWorkspaceBody` (~2447): add `optionalString` for `projectUid` (≤200), `projectName` (≤500), `workspaceKind` (≤20; optionally assert ∈ {`main`,`worktree`}).
3. `rowToWorkspace` (~2423): add `projectUid`/`projectName`/`workspaceKind` = `row.* ?? null`.
4. GET SELECT (~2515), POST INSERT cols+VALUES+RETURNING+params (~2570), PATCH SET+RETURNING+params (~2626): add the 3 columns to each.
5. `api/src/tests/workspaces.test.ts`: add POST→GET and PATCH round-trip assertions for the 3 fields (mirror the `gitHeadSha` tests).

**Desktop changes — this repo** (`src-tauri/src/workspaces_sync.rs` + `database.rs`):
6. `ServerWorkspace` wire type + `WorkspaceUpsertBody`: add `projectUid`/`projectName`/`workspaceKind` (serde camelCase; `skip_serializing_if` on the body).
7. `push_insert`/`push_update`: include the 3.
8. `pull` → `upsert_workspace_sync_from_server`: add the 3 params and SET them — **flip `project_uid`/`workspace_kind` from "leave untouched" to server-authoritative on pull** (so a row pulled on another device gets the uid). Update the `workspaces-sync.md` "local-only" note accordingly.

**Deploy order**: server first (`docker compose up -d --build api`; it applies the DDL on startup), run `docker compose exec api bun test` (must stay green), confirm GET still serves the 44 rows; *then* ship the desktop change. Server is backward-compatible with the current desktop in between.

### Phase 3 — first-class projects table (optional)

8. A `projects` table (local + cloud) keyed by `project_uid` for project-level metadata (name override, color, image, settings), replacing the path-keyed `ui_state`/`recent_projects` scheme so per-project UI state survives path changes and syncs across devices.

## Open Questions / trade-offs

- **Deterministic uid changes if a repo's remote is added or changed later.** Rare; the project re-keys (old worktrees regroup on next reconcile). Acceptable; documented. Superset has the same effect via `linkRepoCloneUrl`.
- **Local-only repos can't converge cross-device** until they gain a remote. Matches Superset (`repoCloneUrl = null` ⇒ UUID-only identity).
- **Cloud is out of repo.** Phase 1 keeps the new fields local-only/derived so nothing depends on the server; Phase 2 graduates them. End-to-end testing here covers daemon → poller → sync → view → UI, plus unit tests; live cloud round-trip is verified in the `codemux-api` repo.

## Likely Touch Points

- `src-tauri/src/project_identity.rs` (new), `src-tauri/Cargo.toml` (`uuid` `v5`)
- `src-tauri/src/remote/workspace.rs`, `src-tauri/src/bin/codemux_remote.rs`
- `src-tauri/src/hosts_inventory.rs`, `src-tauri/src/database.rs`
- `src-tauri/src/workspaces_sync.rs`, `src-tauri/src/commands/workspaces_sync.rs`
- `src/tauri/commands.ts`, `src/components/workspaces-overview/*`
- `docs/features/workspaces-sync.md`, `docs/features/workspaces-overview.md`

## Already Landed

- **Pre-work** (commit `7b3d4f4`): source-side `project_root` stamp in the daemon `WorkspaceStore::create` + poller path fallback + UI title fallback + same-project grouping by name.
- **Phase 1 — first-class project identity + kind + sweep (complete):**
  - `src-tauri/src/project_identity.rs` — `canonical_remote`, deterministic `project_uid` (UUIDv5, `uuid` `v5` feature), `derive_kind`. Unit-tested.
  - Daemon `remote/workspace.rs` — `project_uid`/`project_name`/`kind`/`repo_remote` columns + idempotent ALTER migration + create-time stamping (uid from canonical remote else project_root; kind from `.git`) + `sweep_backfill_identity`, called from `run_serve_async`.
  - Wire + poller — `RemoteWorkspace` parses the new fields; `reconcile_host_inventory` threads `project_uid`/`workspace_kind` into `workspaces_sync` and finally populates `project_remote` (previously hardcoded null).
  - Local SQLite — `workspaces_sync.project_uid`/`workspace_kind` columns (additive ALTER, local-only; `upsert_workspace_sync_from_server` leaves them untouched). `WorkspaceSyncView` (Rust + TS) exposes them.
  - UI — overview groups by stable `projectKey` (`project_uid` preferred) and renders a `main`/`worktree` badge. Conflict guard upgraded to exact `project_uid` match with basename fallback.
  - Tests: Rust unit (identity, daemon create/sweep/migration, poller threading) + a real-binary e2e (`tests/codemux_remote_inventory.rs::cli_to_reconcile_propagates_project_identity`: real git repo → compiled `codemux-remote workspace list` → parse → reconcile). Frontend: `projectKey`/grouping/kind-badge tests. All green.
- **Phase 2 (local side) — complete:** `WorkspaceSnapshot.project_uid`/`workspace_kind` (Rust + TS), stamped in `set_workspace_project_root` (git remote computed outside the lock), threaded through `reconcile_from_snapshot` → `insert/update_workspace_sync` into local-only `workspaces_sync` columns, pull-conflict guard upgraded to exact `project_uid` match, local UI `projectKey` uses `project_uid`. Tests: `set_workspace_project_root_stamps_project_identity`, `reconcile_threads_project_identity_from_snapshot`. Remaining Phase 2 = the cloud round-trip (external repo).

## Notes

- Migration safety is the top risk: the daemon DB lives on user machines and currently only does `CREATE TABLE IF NOT EXISTS`. Port the desktop's idempotent ALTER-loop first.
- Wire is forward/backward compatible: `Workspace` serializes new fields, `RemoteWorkspace` ignores unknown fields, so mixed daemon/desktop versions don't break.
