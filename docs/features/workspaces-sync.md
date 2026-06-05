# Workspaces Sync (cross-device workspace registry)

- Purpose: Describe the cross-device workspace registry — the server-backed mirror that lets every device of the same account see every workspace it has authored or pushed to a host. Powers the "lives on another device" affordance in the Workspaces overview.
- Audience: Anyone touching the workspace push/pull pipeline, the Workspaces overview UI, the SQLite schema, or the API server's `codemux_workspaces` table.
- Authority: Canonical feature doc for the cross-device workspace sync layer.
- Update when: The synced field set, the wire protocol, the sync cadence, or the conflict-resolution rules change.
- Read next: `docs/features/workspaces-overview.md` (the UI consumer), `docs/features/remote-hosts.md` (the hosts sync this builds on top of).

## What This Feature Is

When you push a workspace from your laptop to a host, the Workspaces overview on a *different* device of your account should be able to see that workspace listed under that host. This feature is the registry that makes that possible.

Concretely: every time a workspace is created, renamed, pushed-to-host, pulled-back, or deleted, a record of that change makes its way to the shared API server (78.47.192.173). Every device of the same account periodically pulls the full registry and renders it in the overview alongside its own local workspaces.

## Current Model

### The synced field set

What syncs:

- `title` — display name
- `host_server_id` — which host the workspace currently lives on (null = local to the originating device)
- `project_path` — informational only; the originating device's path-on-disk
- `project_remote` — git remote URL; other devices use it to know how to clone the project if/when they adopt
- `git_branch` — current branch
- `git_head_sha` — current HEAD commit SHA on the workspace's branch (Phase 4c). Optional, ≤200 chars. Powers cross-device divergence detection: when two devices have clone-adopted the same workspace and their `git_head_sha` values differ, both rows render an amber `diverged` chip.
- `project_uid` — deterministic `UUIDv5(canonical git remote ?? project_root)` (project-identity Phase 2 cloud). Stable across devices/hosts; the overview groups by it and the pull-conflict guard matches on it. Server-authoritative on pull *when present* — `upsert_workspace_sync_from_server` uses `COALESCE(server, local)` (`v0.7.5`) so a null cloud value never clobbers a locally-derived uid.
- `project_name` — display name for the project (basename-derived at the source). Server-authoritative on pull.
- `workspace_kind` — `main` (repo root checkout) | `worktree` (per-branch worktree). Renders as a badge on sibling rows. Server-authoritative on pull *when present* (same `COALESCE(server, local)` guard as `project_uid`).
- `created_at`, `updated_at`, `deleted_at` — standard sync timestamps

What does NOT sync (per-device runtime state):

- `cwd`, `worktree_path`, `pane_statuses`, terminal sessions, surface tree
- Git deltas (ahead/behind/changed_files) — read from the local git tree on each device
- `notification_count`, `notifications_muted` — per-device user state

Local-only sync columns (threaded device→device through the host-inventory poller but **never sent to the cloud API**, so they add no server schema): `origin_uid`, `origin_path` (see "Identity contract for remote-discovered rows"), and `default_branch` (post-`v0.7.8`). The daemon stamps `default_branch` at creation and backfills legacy rows on boot; the host poller threads it into the `workspaces_sync` row/view/TS so a project-first pull can checkout the repo's real default branch and classify protection against it — see "Project-first pull + protected root" below.

### Storage layers

| Layer | Where | What it holds |
|---|---|---|
| Server-side | Postgres `codemux_workspaces` (on the shared API at 78.47.192.173) | One row per workspace per user. Soft-delete via `deleted_at`. Scoped by `user_id`. |
| Local mirror | SQLite `workspaces_sync` table | One row per workspace this device knows about. Has `server_id` (cross-device id), `workspace_id` (this device's local id, null for sibling-only rows), `dirty` flag, soft-delete tombstone. |
| Local runtime | `app_state.workspaces` (the JSON-persisted layout) | The actual `WorkspaceSnapshot` for workspaces this device has open. Contains all the runtime state. |

`workspaces_sync.workspace_id IS NULL` is the marker for "lives on another device — we don't have a local copy."

### Endpoints (shared API)

| Method | Path | Behavior |
|---|---|---|
| `GET` | `/api/workspaces` | Returns every row owned by the authenticated user, including tombstones. Optional `?hostServerId=` filter. |
| `POST` | `/api/workspaces` | Creates a new row. Server assigns a BIGSERIAL id and returns it stringified. |
| `PATCH` | `/api/workspaces/:id` | Updates one of the user's rows. 404 (not 401) on unknown id to avoid leaking which BIGSERIALs exist. |
| `DELETE` | `/api/workspaces/:id` | Soft-delete: stamps `deleted_at`. Tombstone gets returned in subsequent GETs so other devices learn of it; the desktop client purges its local tombstone after the next pull confirms the server agrees. |

All four endpoints go through `authenticateBearer(c)` (Better Auth, Bearer token, SHA-256 hashed in DB). Every query is scoped with `WHERE user_id = $1` against the token-extracted user.

### Sync client (Rust)

`src-tauri/src/workspaces_sync.rs` mirrors `hosts_sync.rs` and `automations_sync.rs` 1:1:

- `pull(token)` GETs `/api/workspaces`, upserts each row into the local table with `dirty=0` (server is authoritative), then sweeps any local rows whose `server_id` the server stopped returning and tombstones them locally.
- `push(token)` walks `dirty=1` rows and, per row, does one of:
  - DELETE if `deleted_at IS NOT NULL && server_id IS NOT NULL`, then clears dirty and purges the local tombstone on next pass.
  - POST if `server_id IS NULL`, stamps the returned id back onto the local row.
  - PATCH otherwise.
- `try_sync_with_app(app)` is the public entrypoint — pull then push, swallowing single-call failures so flaky network doesn't strand the user.

A `SYNC_IN_PROGRESS` `AtomicBool` guards against overlapping syncs (background loop + a future "Sync now" button could otherwise race).

### Reconcile step

The bridge between `app_state.workspaces` (runtime state) and `workspaces_sync` (the sync mirror) lives at `workspaces_sync::reconcile_from_snapshot`:

1. Build a `host_id (local i64) → host_server_id (string)` map from the local hosts table.
2. Index existing `workspaces_sync` rows by their local `workspace_id`.
3. For each live `WorkspaceSnapshot`:
   - If a matching sync row exists and ANY synced field has changed, UPDATE (marks dirty).
   - If no sync row exists, INSERT (marks dirty).
4. For sync rows whose `workspace_id` is no longer in `app_state.workspaces`, soft-delete (marks dirty).
5. Rows with `workspace_id IS NULL` are skipped entirely — they're pulled-only sibling-device entries and have no app_state counterpart by design.

OpenFlow and Home workspaces are excluded — neither needs cross-device persistence.

### Cadence

A background tokio task at `lib.rs` startup runs:

```rust
loop {
  reconcile_from_snapshot(db, &app_state.snapshot());
  workspaces_sync::try_sync_with_app(&app).await;
  tokio::sleep(30s).await;
}
```

So a workspace mutation propagates to other devices within ~30 seconds. A `workspaces_sync_now` Tauri command exists for the rare case a user explicitly wants to force a sync.

### Tauri commands

- `workspaces_sync_list` → `Vec<WorkspaceSyncView>` — what the overview reads
- `workspaces_sync_now` → `Result<(), String>` — force an immediate pull+push pass
- `workspaces_adoption_preview(server_id)` → `AdoptionPreview` — dialog opens with this; surfaces `can_host_adopt`, `can_clone_adopt`, `host_configured`, `host_label`, `suggested_path`, `is_path_in_use`, `already_adopted_workspace_id` so the modal can pick the right variant without race conditions
- `workspaces_adopt_synced(server_id)` → `AdoptOutcome` — host-backed cross-device adoption: creates a local workspace shell with `host_id` set, links the sync row, then drives the existing `workspace_pull_back_impl` to rsync the worktree from the host. Idempotent (re-running on an already-adopted row returns the existing local workspace id). **Failure handling (post-`v0.7.6`):** the command now checks `pull_outcome.ok`, not just the outer `Result` — a failed rsync/SSH pull used to report `Ok(WorkspacePullOutcome { ok: false, .. })`, which the command unwrapped as success, leaving a broken/empty shell linked with `host_id` set that the idempotency guard then refused to retry. On failure it removes the shell, reverts the row to a re-pullable sibling via `unlink_workspace_sync_from_local`, and returns a real `Err`. All three adopt paths (single-dir, worktree-repo-rsync, clone) stamp `project_uid` via `set_workspace_project_identity` from the synced row so an adopted workspace converges with its siblings (see "Adopt-path identity stamping" below). **Serialized (post-`v0.7.8`):** guarded by a per-`server_id` async creation lock — see "Serialized adopts" below.
- `workspaces_adopt_project(project_uid)` → `ProjectAdoptOutcome` (post-`v0.7.8`) — **project-first** adoption: gathers every un-adopted host-backed sync row for the `project_uid`, adopts the `kind == "main"` **root first** (a root failure aborts — worktrees can't attach to a repo that didn't materialise), then adopts each worktree continue-on-error (each reuses the now-present `~/.codemux/projects/<repo>` repo and just `git worktree add`s its branch). Composes `workspaces_adopt_synced`, so it inherits the creation lock + identity-stamp + failure-rollback behavior. Backs the overview's one-click **"Pull project"** button. Legacy registries with no `main` row still pull (the first worktree adopt rsyncs the parent repo itself, just without a standalone protected-root shell).
- `workspaces_reconcile_copy(workspace_id)` → `String` (post-`v0.7.8`) — non-destructively clears a divergent **standalone copy** (the full `.git`-dir copy under `~/.codemux/worktrees/<repo>/<branch>` that gets the amber chip). Confirms the workspace is a divergent copy (`git::is_divergent_copy`), then **refuses** if it has uncommitted changes or unpushed commits (`git::reconcile_copy_blocker`, returns commit/push guidance) — otherwise detaches the workspace card via the state-level `close_workspace` (removes the card, **leaves files on disk**, never deletes). The user then runs "Pull project" to materialise a proper protected root. No work is ever lost.

### UI integration

`src/components/workspaces-overview/use-overview-items.ts` unifies the two data sources into a single `OverviewItem[]`:

- `kind: "local"` — has a `WorkspaceSnapshot`. The full live card. May also have a sync row (which gives `dirty` for the "Pending sync" pill and `host_server_id` for cross-device bucketing).
- `kind: "remote"` — only known via sync; rendered as a dashed sibling-device card with title + project + branch + a "Pull to this device (coming soon)" affordance in the action menu.

Bucketing is by `host_server_id`, so the same host shows up under the same bucket on every device. A bucket for a `host_server_id` that this device doesn't have locally yet renders as "Host not on this device" rather than silently dropping the workspaces.

### Security model

- Every endpoint authenticates the caller via `authenticateBearer(c)` and scopes every query with `WHERE user_id = $1`.
- PATCH/DELETE on a row not owned by the caller returns 404 (not 401), so BIGSERIAL ids can't be enumerated cross-account.
- The local SQLite uses `user_id = 'local'` for the desktop's own rows; the server-side `user_id` is the Better Auth-issued opaque user id and never appears in local-only contexts.
- Tombstones get hard-deleted locally only after the server has confirmed it agrees the row is gone (the `purge_acknowledged_workspace_sync_deletes` step).
- Per-user cap: `MAX_WORKSPACES_PER_USER = 2000`. Tombstones don't count.
- Body size cap: `MAX_WORKSPACE_BODY = 32KB`.
- Input validation: title required + ≤500 chars; project_path ≤1000 chars; project_remote ≤1000 chars; git_branch ≤500 chars; host_server_id ≤100 chars; all optional fields explicitly allow null. SQL-injection-shaped ids are rejected with 400 before reaching Postgres.

## What Works Today

- Server: full GET/POST/PATCH/DELETE at `/api/workspaces`, deployed and live at `https://api.codemux.org/api/workspaces`. Endpoint tests cover `gitHeadSha` (Phase 4c) and the `projectUid` / `projectName` / `workspaceKind` round-trip (Phase 2 cloud, 4 added); full server suite at 303/303.
- Local sync: pull, push, reconcile, soft-delete tombstones, idempotent server-side upsert.
- UI: synced rows render in the Workspaces overview under the correct device bucket. The "Pending sync" pill surfaces dirty state. Sibling-device rows render with a distinct dashed border and a "lives on another device" pill. Phase 4c divergence chip flags HEAD divergence on every affected row.
- Cadence: 30-second background loop. `workspaces_sync_now` for forced sync.
- **Asymmetric auto-publish from `codemux-remote` hosts**: every configured host's workspace registry is polled over SSH every ~60 s and reconciled into `workspaces_sync` as sibling-only rows (workspace_id NULL, dirty=1). The existing push tick then uploads them, so a workspace an agent creates on a host via the MCP `workspace_create` tool surfaces in every dev device's overview within ~90 s — no explicit push from the laptop required. Pull-conflict guard refuses to clobber a local workspace already on the same branch of the same project. Detailed model and design rationale in the **Asymmetric publish model** section below.

## Asymmetric publish model (auto-publish from remote hosts)

The synced field set above is symmetric — every device publishes its own workspaces and pulls everyone else's. That symmetry breaks down for **headless hosts** running `codemux-remote serve`: they have a workspace registry (the daemon's SQLite at `<state_dir>/codemux.db`), but no logged-in Codemux account, no Better Auth token, and no path to call `/api/workspaces` themselves. Workspaces created directly on the host (e.g. by an agent calling the `workspace_create` MCP tool overnight) used to be invisible to every dev device until someone explicitly pushed them — defeating the point of asking the agent to start work in the first place.

The fix is a deliberately asymmetric model:

| Role | Behaviour | Why |
|---|---|---|
| Codemux **app** (laptop/desktop) | Manual push/pull stays exactly as it was. Nothing about the workspace lifecycle changes. | Preserves the "close laptop, continue in cloud" flow + user agency over what leaves their dev device. |
| Codemux **remote** daemon (server) | Every dev device polls the host's inventory and republishes new workspaces to the cloud on its behalf. | Servers are always-on, SSH-reachable, and exist to be used as hosts — broadcasting their registry is the unsurprising default. Dev devices remain in charge of polling so the daemon never needs an auth credential. |

### Mechanics

1. **CLI exposed by the host.** `codemux-remote workspace list [--state-dir <path>]` reads the daemon's SQLite registry directly (no running daemon required) and prints `{"host_id": "<gethostname>", "workspaces": [<Workspace>...]}` on stdout. Implementation in `src-tauri/src/bin/codemux_remote.rs::run_workspace_list`. Stable contract — the desktop's parser depends on the shape.
2. **Desktop poller.** `src-tauri/src/hosts_inventory.rs::spawn` starts a background task ~12 s after app setup and runs forever on a 60 s cadence. For each configured host with a `server_id`:
   - Probe via `ssh::probe::probe_host` (re-uses the same `~/.local/bin/codemux-remote` PATH fallback the test-connection flow uses).
   - SSH and run `codemux-remote workspace list` (also with the PATH fallback — see `build_inventory_argv`).
   - Parse the JSON envelope.
   - Reconcile: per inventory row, INSERT a sibling-only sync row (workspace_id NULL, `host_server_id = host.server_id`, `origin_uid = workspace.id` (UUID), dirty=1) or UPDATE in place if `(host_server_id, origin_uid)` already exists. Disappeared rows get soft-deleted. **Post-`v0.7.6`:** if a row was already soft-deleted (e.g. adopted-then-closed, which tombstones the row by `workspace_id`), the next poll used to miss the tombstone — `find_workspace_sync_by_host_and_origin_uid` filtered `deleted_at IS NULL`, so the workspace looked absent and got re-INSERTed, churning the cloud row (other devices saw it vanish-then-reappear). The reconcile now looks the tombstone up explicitly (`find_remote_discovered_tombstone`) and **undeletes** it on reappear (`undelete_remote_discovered_workspace_sync`, clearing the stale `workspace_id` so it can't recreate the orphan-conflict class) instead of inserting a fresh row, and dedupes duplicate ids within a single inventory envelope (`seen_uids` guard).
   - **`project_path` derivation.** The sync row's `project_path` is set to the host workspace's `project_root` when present, else falls back to the workspace's own `path`. For a non-worktree checkout the workspace `path`'s basename *is* the project name, so the fallback recovers it; worktrees keep `project_root` (whose basename is the project, not the branch). See `hosts_inventory.rs::reconcile_host_inventory`. **This is now a backstop, not the primary fix** — see the source-side stamp below.

#### Source-side project identity (where it's actually fixed)

The blank-project-name bug for agent-/host-created workspaces is fixed at the *source*, mirroring how every desktop create path already behaves (`commands/workspace.rs` resolves `find_git_root(cwd).unwrap_or(cwd)`). The headless daemon's `WorkspaceStore::create` (`src-tauri/src/remote/workspace.rs`) now derives `project_root` when the caller (the `workspace_create` MCP tool) omits it: the git root of the workspace's `path`, or the path itself when it isn't a repo. So a workspace an agent registers on a host always carries a project identity at the moment of record creation — `codemux-remote workspace list` reports it, the cloud row gets it, and every consumer benefits.

Important: the daemon `workspace_create` tool is **registration-only** — it does not create a worktree (or any files). An agent building a new project does the git work itself (`git clone`/`git init` via the terminal tools) into a **regular folder**, then registers that path. Worktrees are only for additional branches; the daemon never puts a root/main checkout inside `~/.codemux/worktrees/`. The earlier symptom was purely the missing `project_root` metadata, not a misplaced checkout.

#### First-class project identity (Phase 1)

Beyond `project_root`, the daemon now stamps a stable, deterministic **`project_uid`** (`UUIDv5(canonical git remote ?? project_root)`), a **`project_name`**, a **`kind`** (`main` | `worktree`), and the canonical **`repo_remote`** at create time, and an idempotent boot sweep backfills them onto pre-existing rows. These ride the `workspace list` wire envelope; the poller threads `project_uid` + `workspace_kind` into `workspaces_sync` columns and finally populates `project_remote` for remote-discovered rows. The overview groups by `project_uid` and labels `kind`. Full design + phasing in `docs/plans/project-identity.md`.

The cloud schema now carries these (Phase 2 cloud round-trip — deployed + verified on the VPS): `codemux_workspaces` gained `project_uid` / `project_name` / `workspace_kind` via additive `ADD COLUMN IF NOT EXISTS` (mirroring `git_head_sha`), threaded through the GET/POST/PATCH handlers + `parseWorkspaceBody` validation (`workspaceKind ∈ {main,worktree}`). The desktop `ServerWorkspace` + `WorkspaceUpsertBody` carry the fields, `push_insert`/`push_update` send them, and `upsert_workspace_sync_from_server` now persists them — so unlike the local-only `origin_uid`, `project_uid`/`project_name`/`workspace_kind` are **server-authoritative on pull**: a row pulled on another device arrives already carrying its uid. (Since `v0.7.5`, `project_uid`/`workspace_kind` resolve via `COALESCE(server, local)` so a *null* cloud value never clobbers a locally-derived identity — server-authoritative only when the server value is present.) Deterministic uids mean grouping also converges via the synced `project_remote` for any older/null-uid rows.
3. **Cloud push** (existing). The 30 s `workspaces_sync::push` tick walks every dirty row including these sibling-only ones, POSTs to `/api/workspaces`, and stamps the assigned `server_id`. Other devices pull and render them under the host's bucket. Same code path as a manual push — no new API surface.

### Identity contract for remote-discovered rows

| Column | Value | Purpose |
|---|---|---|
| `workspace_id` | `NULL` | Marker for "lives elsewhere, not adopted here." Sibling-row rendering. |
| `host_server_id` | host's `server_id` | Bucket the row under the right host in the overview. |
| `origin_uid` | remote daemon's UUID | Dedupes repeated polls — re-discovering the same UUID UPDATEs in place instead of inserting again. **Local-only column**: not in the cloud schema. |
| `origin_path` | remote daemon's real on-host `path` | The workspace's actual path on the host (`v0.7.5`), distinct from `project_path` (which collapses to the parent repo for a worktree). The pull rsyncs from this so an agent-created workspace that lives outside `~/.codemux/worktrees/` pulls with its files. **Local-only column.** |
| `server_id` | assigned by first `push()` | Cross-device identity. After this exists, other devices see the row via cloud pull. |
| `dirty` | `1` on insert / on field change | Triggers the next push tick. |

The `origin_uid` and `origin_path` columns are both additive (`ALTER TABLE workspaces_sync ADD COLUMN …`) and only ever set by the inventory reconcile path. Local-pushed workspaces and cloud-pulled sibling rows leave them null, and `upsert_workspace_sync_from_server` keeps them out of its SET list so a cloud pull never wipes them.

### Pull-conflict guard

The Pull-to-this-device dialog now refuses to clobber a workspace the user's already working on. `AdoptionPreview.same_branch_project_exists_at` is set to the local workspace_id when **another local workspace** on this device matches `(basename(project_path), git_branch)` with the previewed remote row. The dialog renders `SameBranchProjectBlock` with an "Open the existing workspace" CTA and hides the Pull button.

Why basename and not full path: across two devices the same repo will almost always be checked out at different absolute paths (`~/projects/foo` here, `/home/deus/projects/foo` there). Full-path comparison would miss the conflict every time. False positives are bounded — the user would need two distinct projects with the same basename AND the same branch on the same device.

**Stale-link fix (post-`v0.7.6`):** `detect_same_branch_project_conflict` used to treat *any* `workspaces_sync` row with a non-null `workspace_id` as "this device already has it" — even when that workspace had been closed/deleted and only a stale sync row remained. Pulling the same branch back from a host then showed a phantom "you already have this branch open" and disabled Pull. The overview (`use-overview-items`) and the adopt idempotency path already intersect against the live `app_state` workspaces; this guard didn't. It now takes a `live_workspace_ids: &HashSet<String>` and skips rows whose linked workspace is no longer live, so all three surfaces agree on what counts as locally present.

### Known limitations (v1)

- **Cross-device race for the same host workspace.** `origin_uid` is local-only. If Device A and Device B both poll Pandora between cloud pulls, both publish a row for the same physical workspace and the overview briefly shows two entries. **Mitigated (post-`v0.7.8`)** by `dedupe_sibling_rows` (see "Client-side duplicate collapse on pull") which collapses the duplicates client-side and tombstones the extras on the next push. Single-device users — the common case today — never hit this anyway. The permanent fix is still a server-side `origin_uid` unique partial index; tracked in `codemux-api`, not yet in scope.
- **`project_remote` only when the host project has a resolvable remote.** Project-identity Phase 1 made the daemon stamp the canonical `repo_remote` at create time and the poller thread it into `project_remote` for remote-discovered rows (previously hardcoded null). So a host workspace whose project has a git remote now carries `project_remote` — the clone-from-git fallback works for it. A host project with no remote (or one the daemon couldn't resolve) still leaves `project_remote = null` and is adoptable only via the host-backed (rsync) path.
- **Hosts without a `server_id` are skipped silently.** Until the host record itself has synced (`hosts_sync` pushes it and gets a cloud id), the poller can't tag inventory rows with a stable cross-device host identity. The first cycle after the host syncs picks them up.

## Robustness hardening (post-`v0.7.8`)

A second multi-device pass closed a cluster of duplicate-row / race / collision bugs in the adopt + inventory pipeline.

### Project-first pull + protected root

Pulling a remote project used to land the repo root as a *deletable* `main` worktree under `~/.codemux/worktrees`, not a protected repo root — the daemon recorded root rows with a **null branch**, so the desktop fell back to the literal `"main"` and `is_protected_repo_root` fail-opened to "deletable"; for local-only repos (no remote) there was no reliable default-branch signal at all. The fix makes the repository (root + default branch) a first-class, synced concept:

- **Git.** `find_default_branch` is now `pub`; `resolve_default_branch` (origin/HEAD → main/master → current branch, covering local-only repos whose default is non-standard) and `ensure_origin_head` (no-op without an origin remote) are new. On root adoption Codemux ensures `origin/HEAD` + checks out the default branch, then classifies protection against the repo's **actual on-disk branch** rather than a possibly-null synced branch.
- **`default_branch` column.** Threaded end-to-end as a **local-only** column (daemon registry → host poller → `workspaces_sync` table/record/view → TS). Not sent to the cloud, so no server-schema change. The daemon stamps it at creation and backfills legacy rows on boot.
- **`workspaces_adopt_project`** materialises the protected root first (at `~/.codemux/projects/<repo>`) then recreates each worktree as a real linked worktree under it (continue-on-error per worktree). The overview's **"Pull project"** action calls it.
- **UI.** Sidebar delete respects `protected` (a root is never destructively deletable); the overview floats the protected root to the top of its project cluster and adds the "Pull project" action.

### Serialized adopts (creation lock)

A per-row async lock (`acquire_adopt_lock(server_id)` in `commands/workspaces_sync.rs`, mirroring Superset's `workspaceCreateLocks`) serialises concurrent adopts of the same row — a double-clicked "Pull", or the host-inventory poller racing a manual pull — so they can't both slip past the `workspace_id` idempotency guard and create duplicate local shells / clobber the same landing path. Guards `workspaces_adopt_synced` and `workspaces_adopt_via_clone`; `workspaces_adopt_project` is covered transitively (it composes the former).

### Client-side duplicate collapse on pull

`dedupe_sibling_rows` (run at the tail of `reconcile_from_snapshot`) converges the cross-device race where two devices poll the same host before either's push lands and the cloud ends up with two rows for one physical workspace. It groups **un-adopted** siblings (`workspace_id IS NULL`) by `(host, origin_uid)` — or `(host, project_uid, branch, kind)` since `origin_uid` is local-only and absent on cloud-pulled rows — keeps the canonical row (one with a `server_id`, lowest id) and tombstones the rest so the next push removes the cloud duplicate. **Adopted rows are never touched.** The permanent fix is a server-side unique partial index on `codemux_workspaces(origin_uid)` (TODO in `codemux-api`); this is the client-side stopgap.

### One repo root per project (daemon-side)

The daemon backfilled project identity but had no equivalent of Superset's `ensureMainWorkspace` sweep, so an agent could register a project's root more than once (or re-register after a move), leaving two `kind=main` rows for one `project_uid` — which surfaced as two "repo root" cards on every device. Now:

- `WorkspaceStore::collapse_main_for_uid(project_uid)` keeps the earliest-created `main` row and deletes the rest (**registry rows only; files on disk untouched**); idempotent, returns the survivor.
- `WorkspaceStore::create` collapses after inserting a `main` row, so re-registering an existing root returns the canonical root instead of spawning a duplicate. A genuinely distinct root has a different `project_uid` (derived from remote/root path), so unrelated projects are never merged.
- `normalize_main_workspaces()` is a boot-time sweep that collapses every project with >1 main row; wired next to `sweep_backfill_identity` on `serve` startup.

### Collision-safe host paths (uid-keyed)

Push/pull/spawn used to land everything at `~/.codemux/worktrees/<basename>/<branch>` (and `projects/<basename>`), so two **different** repos sharing a basename (`acme/api` vs `widgets/api`) collided on the host — one overwriting the other. The host layout is now keyed on the deterministic `project_uid`:

- `workspace_paths`: `conventional_remote_path_keyed` / `conventional_remote_root_path_keyed` + `project_dir_component` yield `<basename>-<short-uid>` (collision-safe *and* readable) when a uid is known, and reproduce the **exact** legacy basename layout when it isn't (the migration safety net).
- Push landing (`commands/hosts.rs`) and the remote agent cwd (`terminal/daemon_backed.rs`) key on the workspace's `project_uid`, so they always agree for a workspace pushed by this build.
- Pull-back keeps the recorded `origin_path` as authoritative and **adds** the uid-keyed path as a fallback candidate *before* the legacy basename one, so both new and already-pushed workspaces resolve. Migration note: a workspace desktop-pushed before this change still pulls back fine (basename fallback); a live agent re-spawn on such a workspace targets the new path and may need a re-push — agent-created workspaces are unaffected (they use the recorded `origin_path`, not the convention).
- **Local adoption landings are uid-keyed too** (issue #65). When a sibling workspace is adopted onto *this* device, the local landing path is now keyed on the row's `project_uid` so adopting two **different** same-basename projects no longer collides locally. Every adoption surface routes through shared helpers in `commands/workspaces_sync.rs` — `adopt_root_landing` (→ `projects/<basename>-<short-uid>`) and `adopt_worktree_landing` (→ `worktrees/<basename>-<short-uid>/<branch>`) — so the **preview's `suggested_path`, the host-backed single-dir landing (root + worktree), `workspaces_adopt_via_clone`, and `adopt_worktree_via_repo_rsync`'s `local_project_path` all agree** (a disagreement would let re-pull duplicate). Each helper carries a **basename read-fallback** (`choose_landing`): if a copy adopted *before* the re-key already lives at the bare-basename path, it's reused instead of re-landing at a new uid-keyed dir, so the "already adopted? reuse `projects/<name>/.git`" idempotency check stays intact. A recreated worktree inherits the uid suffix automatically because `git_create_worktree` derives the worktree dir from the (now uid-keyed) repo dir's basename — so a `worktree`-kind (or clone) row's worktree dir tracks the **repo-root** landing decision, not an independent `worktrees/` check; the preview predicts that route via `adopt_worktree_landing_via_repo` so it still matches when a pre-re-key root occupies the bare `projects/<name>` path.
- **Claude JSONL session continuity follows the actual path** (issue #65). The best-effort `.jsonl` history sync on push and pull-back (`commands/hosts.rs`) used to recompute the bare-basename remote path, so a uid-keyed pushed workspace's conversation history silently never synced (the encoded `~/.claude/projects/<encoded-cwd>/` dir didn't match). It now derives the remote cwd from the workspace's **actual** location via `resolve_remote_cwd`: push reuses `remote_path_str` (the exact uid-keyed/root-aware landing it just pushed to); pull-back uses the candidate source that actually resolved (the recorded `origin_path` or the uid-keyed path). `~/`-relative paths expand against the remote `$HOME`; an absolute `origin_path` passes through unchanged.

### Non-destructive "Reconcile copy"

`workspaces_reconcile_copy` (see the Tauri-command list above) clears the divergent-copy chip by detaching the card — files always stay on disk, and the action refuses while uncommitted/unpushed work exists. This is the one-click reconcile follow-up `docs/plans/repo-unit-sync.md` left open.

## Current Constraints

- **Host-backed adoption + clone fallback both work.** The "Pull to this device" menu item is live for every sibling-device row. The dialog renders the right variant automatically:
  - **Host-backed** (row's `host_server_id` resolves to a configured local host): rsyncs from the host via `workspaces_adopt_synced` + the existing `workspace_pull_back_impl`. As of `v0.7.5` the rsync sources from the row's `origin_path` (the daemon's real on-host path) when present, falling back to the conventional `~/.codemux/worktrees/<project>/<branch>` path only for desktop-pushed workspaces — so an agent-created workspace that lives outside the conventional layout pulls with its files instead of as an empty shell. The adopted workspace replaces the original on that host (single-source-of-truth model).
    - **Repo-root landing** (repo-unit sync — `docs/plans/repo-unit-sync.md`): a `main`-kind row is the repo ROOT. `workspaces_adopt_synced` now lands it at `~/.codemux/projects/<repo>` via `create_synced_root_shell` (a protected, non-worktree root), NOT in the worktrees tree where a root checkout was previously materialised as a divergent full copy. The rsync SOURCE is resolved independently inside `workspace_pull_back_impl`, so only the local landing + classification changed. Symmetrically, `workspace_push_to_host` pushes a repo root to `~/.codemux/projects/<repo>` on the host (via `conventional_remote_root_path`), and the pull is root-aware with a fallback to the legacy `worktrees/` path so roots pushed before the change still pull. Null/legacy rows keep the single-dir worktree landing.
  - **Host-backed, worktree kind** (`v0.7.5`): when the row's `workspace_kind == "worktree"`, `workspaces_adopt_synced` delegates to `adopt_worktree_via_repo_rsync` — it rsyncs the **parent repo** (incl. `.git`, from `project_path`) into `~/.codemux/projects/<repo>`, then `git_recreate_worktree_for_adopted_repo` runs `git worktree prune` (drops the host's stale worktree admin entries pointing at non-existent local paths) + `git worktree add` for the branch into the canonical `~/.codemux/worktrees/<repo>/<branch>`. This rebuilds the worktree's cross-machine-broken `.git` gitfile locally and works for local-only repos (no remote) because rsync carries the objects + branch refs — no clone needed. Adopting a second worktree of the same project reuses the already-rsynced repo. **Post-`v0.7.6`:** `adopt_worktree_via_repo_rsync` creates its local shell **before** the fallible `set_workspace_host_id` + link mutations with no rollback, so a failure used to leave an orphan shell and a retry duplicated it; it now rolls the shell back on error, mirroring the single-dir adopt path.
  - **Clone fallback** (row has `project_remote` but no shared host): `workspaces_adopt_via_clone` runs `git clone --no-checkout` into `~/.codemux/projects/<basename>`, then `git worktree add` at the branch, and registers a fresh local workspace. Crucially this creates a NEW server_id (does NOT link to the original sibling row) — both devices end up with independent copies sharing a git remote. The dialog warns about uncommitted-work loss before the user confirms. **Post-`v0.7.6`:** the clone-adopt path now stamps `project_uid` from the freshly-cloned repo's canonical remote (deterministic UUIDv5 → matches siblings); it previously carried `project_uid = None` and never converged with its siblings in the overview.
  - **Neither** (no host, no remote): dialog tells the user to push from the other device first.

#### Adopt-path identity stamping (post-`v0.7.6`)

The adoption shell is created with `project_uid: None`. Worse, `reconcile_from_snapshot` is a plain `SET` (not `COALESCE`), so the very next reconcile pushed that `None` straight back over the row, **wiping the daemon-derived uid** and breaking cross-device grouping/dedup. All three adopt paths now stamp identity from the synced row via `app_state.set_workspace_project_identity(workspace_id, project_uid, workspace_kind)` immediately after creating the shell (host-backed single-dir + worktree-repo-rsync stamp from the synced row's `project_uid`; clone-adopt derives it deterministically from the cloned repo's canonical remote). This also converges correctly for local-only repos, where the deterministic `UUIDv5(project_root)` matches the daemon's.

## Safety guardrails (Phase 4)

Every push, pull, and adopt action now has two safety nets that make accidents recoverable:

- **Confirm-before-push.** Right-clicking a workspace → `Move to host → <device>` no longer fires the rsync immediately. Instead, `ConfirmPushDialog` opens with the workspace title, the destination, and a 3-bullet explanation of what's about to happen ("Copies files to <host>", "Live editing location moves", "You can pull it back anytime"). Power users who want the old single-tap behaviour can tick "Don't ask again for <host>" — the choice persists per-device in localStorage. The dialog is at `src/components/overlays/confirm-push-dialog.tsx`.
- **Undo for 10 seconds** on every successful push, pull, and adoption. The success toast carries an `Undo` button that fires the reverse action (push ↔ pull). Double-click-guarded so the reverse only runs once. Reverse-action failure surfaces as a follow-up error toast. Implementation: `fireUndoable` in `src/lib/toast.ts`.

Wired at three call sites:
- `sidebar-workspace-row.tsx` — sidebar context menu's push and pull-back items both go through the undo path.
- `pull-to-device-dialog.tsx` — cross-device adoption shows Undo = push back to the source host, so a wrong "Pull to this device" click is recoverable.
- **No "Sync now" UI button.** The command exists but no UI affordance triggers it yet — the 30s loop covers steady-state use. Easy add when wanted.
- **No optimistic UI.** Local mutations show up in the overview immediately (because app_state is reactive), but the cross-device propagation has the 30s latency. Acceptable for v1; could be tightened later by hooking sync triggers into each mutation site.
- **Last-write-wins, no conflict UI.** If two devices edit the same workspace simultaneously, the later push overwrites. Same model as hosts and automations.
- **Hosts must sync first.** A workspace with `host_server_id="42"` is meaningful on Device B only after Device B has pulled the matching host row. The hosts sync runs on mutation; it usually races faster than workspaces, but there's a window where a workspace lands in the "Host not on this device" bucket until the host catches up.

## Important Touch Points

### Local (Codemux desktop)
- `src-tauri/src/database.rs` — `workspaces_sync` table (additive `git_head_sha TEXT` migration in Phase 4c, additive `origin_uid TEXT` migration for the asymmetric publish flow, additive `project_uid TEXT` + `workspace_kind TEXT` migrations for first-class project identity) + `WorkspaceSyncRecord` struct + CRUD impls (`insert_workspace_sync`, `update_workspace_sync_by_workspace_id`, `soft_delete_workspace_sync_by_workspace_id`, `list_workspaces_sync`, `list_workspaces_sync_for_sync`, `list_dirty_workspaces_sync`, `mark_workspace_sync_synced`, `upsert_workspace_sync_from_server`, `purge_acknowledged_workspace_sync_deletes`, `link_workspace_sync_to_local`, `unlink_workspace_sync_from_local` — post-`v0.7.6`, reverts an adopted row to a re-pullable sibling on pull failure). Remote-discovered helpers: `insert_remote_discovered_workspace_sync`, `update_remote_discovered_workspace_sync`, `find_workspace_sync_by_host_and_origin_uid` (now `AND deleted_at IS NULL`), `find_remote_discovered_tombstone` + `undelete_remote_discovered_workspace_sync` (post-`v0.7.6`, the undelete-on-reappear path), `list_remote_discovered_for_host`, `soft_delete_remote_discovered_workspace_sync_by_id`.
- `src-tauri/src/state/state_impl.rs` — `set_workspace_project_identity(workspace_id, project_uid, workspace_kind)` (post-`v0.7.6`): stamps identity onto an adopted shell so the next reconcile doesn't push a `None` over the daemon-derived uid.
- `src-tauri/src/commands/workspace.rs` — `close_workspace` / `close_workspace_with_worktree` capture `host_id` and call `ssh::forget_workspace_client` + `ssh::shutdown_supervisor` on close (post-`v0.7.6` tunnel/daemon teardown; Unix-only, idempotent, no-op for local workspaces).
- `src-tauri/src/hosts_inventory.rs` — background poller for the asymmetric publish flow. `spawn(app)` wired from `lib.rs` setup; per-host cycle runs `probe_host` + `fetch_inventory` + `reconcile_host_inventory`. `build_inventory_argv` locks in the SSH flags + `~/.local/bin/codemux-remote` fallback. `PollStats` captures inserted/updated/soft_deleted per cycle.
- `src-tauri/src/bin/codemux_remote.rs::run_workspace_list` — `workspace list` CLI subcommand the desktop invokes over SSH. Reads the daemon's SQLite directly, no HTTP, no running daemon required.
- `src-tauri/src/commands/workspaces_sync.rs::detect_same_branch_project_conflict` — pull-conflict guard; populates `AdoptionPreview.same_branch_project_exists_at`.
- `src-tauri/src/workspaces_sync.rs` — sync module: `pull`, `push`, `try_sync_with_app`, `sync_workspaces`, `reconcile_from_snapshot`, `ServerWorkspace` wire type.
- `src-tauri/src/commands/workspaces_sync.rs` — Tauri command surface: `workspaces_sync_list`, `workspaces_sync_now`, `workspaces_adoption_preview`, `workspaces_adopt_synced`, `workspaces_adopt_via_clone`, `workspaces_adopt_project` (project-first pull, post-`v0.7.8`), `workspaces_reconcile_copy` (post-`v0.7.8`). Adopt serialization: `adopt_locks()` + `acquire_adopt_lock(server_id)` per-row async lock. Uid-keyed local adoption landings (issue #65): `adopt_root_landing` / `adopt_worktree_landing` / `adopt_worktree_landing_via_repo` + the `choose_landing` basename read-fallback, shared by the preview + all three adopt paths so they agree and re-pull stays idempotent.
- `src-tauri/src/workspaces_sync.rs::dedupe_sibling_rows` — client-side cross-device duplicate collapse (post-`v0.7.8`), run at the tail of `reconcile_from_snapshot`.
- `src-tauri/src/remote/workspace.rs` — `collapse_main_for_uid` / `normalize_main_workspaces` daemon-side one-root-per-project (post-`v0.7.8`); `WorkspaceStore::create` collapses after inserting a `main` row.
- `src-tauri/src/workspace_paths.rs` — `project_dir_component` + `conventional_remote_path_keyed` / `conventional_remote_root_path_keyed` uid-keyed host paths (post-`v0.7.8`, collision-safe).
- `src-tauri/src/git.rs` — `resolve_default_branch`, `ensure_origin_head`, `reconcile_copy_blocker` (post-`v0.7.8`); `find_default_branch` promoted to `pub`.
- `src-tauri/src/commands/hosts.rs` — `workspace_pull_back_impl` (extracted from the `#[tauri::command]` wrapper so the adoption flow can call the rsync machinery without going back through Tauri IPC); `resolve_remote_cwd` (issue #65) — the Claude JSONL push/pull-back sync derives its remote encoded dir from the workspace's actual on-host path (`remote_path_str` on push, the resolved pull candidate on pull-back) instead of recomputing the basename, so history syncs for uid-keyed workspaces.
- `src-tauri/src/state/state_impl.rs` — `create_synced_workspace_shell` helper that adoption uses to create a workspace pre-stamped with `host_id` and the target `worktree_path` before rsync runs.
- `src/components/workspaces-overview/pull-to-device-dialog.tsx` — adoption dialog with the four-variant body (host-backed form, host-not-configured, path-in-use, already-adopted) and the "What this does" disclosure that pre-expands on first pull.
- `src-tauri/src/lib.rs` — registers the 30s background sync loop in the Tauri setup block, plus the two commands in the invoke handler.
- `src/tauri/commands.ts` — `WorkspaceSyncView` TS type + `workspacesSyncList` + `workspacesSyncNow` bindings.
- `src/stores/workspaces-sync-store.ts` — Zustand store + `useWorkspacesSync` hook. Polls every 5s while a subscriber is mounted.
- `src/components/workspaces-overview/use-overview-items.ts` — merges local + synced rows into the unified `OverviewItem[]`.
- `src/components/workspaces-overview/workspaces-overview-section.tsx` — uses the unified list; buckets by `host_server_id`.
- `src/components/workspaces-overview/workspace-overview-row.tsx` — two render branches: `LocalRow` and `RemoteRow`.

### Server (codemux-api on the VPS)
- `~/codemux-api/api/src/index.ts` — `codemux_workspaces` table DDL (under "Codemux workspaces table" comment block) + four endpoints (GET/POST/PATCH/DELETE `/api/workspaces`).
- `~/codemux-api/api/src/tests/preload.ts` — mirror of the DDL so prod-to-test schema parity holds.
- `~/codemux-api/api/src/tests/workspaces.test.ts` — tests covering auth, validation, isolation, product boundary, soft-delete, per-user cap, the `gitHeadSha` round-trip, and the `projectUid` / `projectName` / `workspaceKind` round-trip (POST→GET + PATCH).

## Notes

- The pattern is intentionally a near-copy of `hosts_sync` and `automations_sync`. If the wire protocol or sync semantics ever need to change, change them all three together.
- The cross-device identity is `server_id` (stringified BIGSERIAL). Local `workspace_id` strings (`workspace-42` style) are device-local and meaningless cross-device — never use one as a cross-device key.
- The 30s cadence is a magic number; tune by changing the `tokio::time::sleep(30s)` call in `lib.rs`. Lower bound is ~5s before the API would start to feel busy under many devices.
