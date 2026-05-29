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
- `created_at`, `updated_at`, `deleted_at` — standard sync timestamps

What does NOT sync (per-device runtime state):

- `cwd`, `worktree_path`, `pane_statuses`, terminal sessions, surface tree
- Git deltas (ahead/behind/changed_files) — read from the local git tree on each device
- `notification_count`, `notifications_muted` — per-device user state

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
- `workspaces_adopt_synced(server_id)` → `AdoptOutcome` — host-backed cross-device adoption: creates a local workspace shell with `host_id` set, links the sync row, then drives the existing `workspace_pull_back_impl` to rsync the worktree from the host. Idempotent (re-running on an already-adopted row returns the existing local workspace id). Clone-fallback path for `host_server_id IS NULL` rows is deferred to a follow-up.

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

- Server: full GET/POST/PATCH/DELETE at `/api/workspaces`, deployed and live at `https://api.codemux.org/api/workspaces`. 36 endpoint tests pass (`gitHeadSha` round-trip coverage added in Phase 4c); full server suite at 299/299.
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
   - Reconcile: per inventory row, INSERT a sibling-only sync row (workspace_id NULL, `host_server_id = host.server_id`, `origin_uid = workspace.id` (UUID), dirty=1) or UPDATE in place if `(host_server_id, origin_uid)` already exists. Disappeared rows get soft-deleted.
   - **`project_path` derivation.** The sync row's `project_path` is set to the host workspace's `project_root` when present, else falls back to the workspace's own `path`. For a non-worktree checkout the workspace `path`'s basename *is* the project name, so the fallback recovers it; worktrees keep `project_root` (whose basename is the project, not the branch). See `hosts_inventory.rs::reconcile_host_inventory`. **This is now a backstop, not the primary fix** — see the source-side stamp below.

#### Source-side project identity (where it's actually fixed)

The blank-project-name bug for agent-/host-created workspaces is fixed at the *source*, mirroring how every desktop create path already behaves (`commands/workspace.rs` resolves `find_git_root(cwd).unwrap_or(cwd)`). The headless daemon's `WorkspaceStore::create` (`src-tauri/src/remote/workspace.rs`) now derives `project_root` when the caller (the `workspace_create` MCP tool) omits it: the git root of the workspace's `path`, or the path itself when it isn't a repo. So a workspace an agent registers on a host always carries a project identity at the moment of record creation — `codemux-remote workspace list` reports it, the cloud row gets it, and every consumer benefits.

Important: the daemon `workspace_create` tool is **registration-only** — it does not create a worktree (or any files). An agent building a new project does the git work itself (`git clone`/`git init` via the terminal tools) into a **regular folder**, then registers that path. Worktrees are only for additional branches; the daemon never puts a root/main checkout inside `~/.codemux/worktrees/`. The earlier symptom was purely the missing `project_root` metadata, not a misplaced checkout.

#### First-class project identity (Phase 1)

Beyond `project_root`, the daemon now stamps a stable, deterministic **`project_uid`** (`UUIDv5(canonical git remote ?? project_root)`), a **`project_name`**, a **`kind`** (`main` | `worktree`), and the canonical **`repo_remote`** at create time, and an idempotent boot sweep backfills them onto pre-existing rows. These ride the `workspace list` wire envelope; the poller threads `project_uid` + `workspace_kind` into new local-only `workspaces_sync` columns and finally populates `project_remote` for remote-discovered rows. The overview groups by `project_uid` and labels `kind`. Full design + phasing in `docs/plans/project-identity.md`. The cloud schema does not carry these yet (Phase 2, external `codemux-api` repo), so — like `origin_uid` — `upsert_workspace_sync_from_server` leaves the local columns untouched on pull; deterministic uids mean cross-device grouping still converges via the synced `project_remote`.
3. **Cloud push** (existing). The 30 s `workspaces_sync::push` tick walks every dirty row including these sibling-only ones, POSTs to `/api/workspaces`, and stamps the assigned `server_id`. Other devices pull and render them under the host's bucket. Same code path as a manual push — no new API surface.

### Identity contract for remote-discovered rows

| Column | Value | Purpose |
|---|---|---|
| `workspace_id` | `NULL` | Marker for "lives elsewhere, not adopted here." Sibling-row rendering. |
| `host_server_id` | host's `server_id` | Bucket the row under the right host in the overview. |
| `origin_uid` | remote daemon's UUID | Dedupes repeated polls — re-discovering the same UUID UPDATEs in place instead of inserting again. **Local-only column**: not in the cloud schema. |
| `server_id` | assigned by first `push()` | Cross-device identity. After this exists, other devices see the row via cloud pull. |
| `dirty` | `1` on insert / on field change | Triggers the next push tick. |

The `origin_uid` column is additive (`ALTER TABLE workspaces_sync ADD COLUMN origin_uid TEXT`) and only ever set by the inventory reconcile path. Local-pushed workspaces and cloud-pulled sibling rows leave it null.

### Pull-conflict guard

The Pull-to-this-device dialog now refuses to clobber a workspace the user's already working on. `AdoptionPreview.same_branch_project_exists_at` is set to the local workspace_id when **another local workspace** on this device matches `(basename(project_path), git_branch)` with the previewed remote row. The dialog renders `SameBranchProjectBlock` with an "Open the existing workspace" CTA and hides the Pull button.

Why basename and not full path: across two devices the same repo will almost always be checked out at different absolute paths (`~/projects/foo` here, `/home/deus/projects/foo` there). Full-path comparison would miss the conflict every time. False positives are bounded — the user would need two distinct projects with the same basename AND the same branch on the same device.

### Known limitations (v1)

- **Cross-device race for the same host workspace.** `origin_uid` is local-only. If Device A and Device B both poll Pandora between cloud pulls, both will publish a row for the same physical workspace and the overview will briefly show two entries. They converge on the next pull tick because the second device sees the first device's cloud row (with matching `host_server_id`, `project_remote`, `git_branch`). Single-device users — the common case today — never hit this. Permanent fix is a server-side `origin_uid` column with a unique partial index; tracked but not in scope.
- **`project_remote` not available.** The remote daemon's schema doesn't carry the originating git remote URL, so remote-discovered rows have `project_remote = null`. Other devices can still adopt via the host-backed (rsync) path; the clone fallback isn't available for these rows.
- **Hosts without a `server_id` are skipped silently.** Until the host record itself has synced (`hosts_sync` pushes it and gets a cloud id), the poller can't tag inventory rows with a stable cross-device host identity. The first cycle after the host syncs picks them up.

## Current Constraints

- **Host-backed adoption + clone fallback both work.** The "Pull to this device" menu item is live for every sibling-device row. The dialog renders the right variant automatically:
  - **Host-backed** (row's `host_server_id` resolves to a configured local host): rsyncs from the host via `workspaces_adopt_synced` + the existing `workspace_pull_back_impl`. The adopted workspace replaces the original on that host (single-source-of-truth model).
  - **Clone fallback** (row has `project_remote` but no shared host): `workspaces_adopt_via_clone` runs `git clone --no-checkout` into `~/.codemux/projects/<basename>`, then `git worktree add` at the branch, and registers a fresh local workspace. Crucially this creates a NEW server_id (does NOT link to the original sibling row) — both devices end up with independent copies sharing a git remote. The dialog warns about uncommitted-work loss before the user confirms.
  - **Neither** (no host, no remote): dialog tells the user to push from the other device first.

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
- `src-tauri/src/database.rs` — `workspaces_sync` table (additive `git_head_sha TEXT` migration in Phase 4c, additive `origin_uid TEXT` migration for the asymmetric publish flow) + `WorkspaceSyncRecord` struct + CRUD impls (`insert_workspace_sync`, `update_workspace_sync_by_workspace_id`, `soft_delete_workspace_sync_by_workspace_id`, `list_workspaces_sync`, `list_workspaces_sync_for_sync`, `list_dirty_workspaces_sync`, `mark_workspace_sync_synced`, `upsert_workspace_sync_from_server`, `purge_acknowledged_workspace_sync_deletes`, `link_workspace_sync_to_local`). Remote-discovered helpers: `insert_remote_discovered_workspace_sync`, `update_remote_discovered_workspace_sync`, `find_workspace_sync_by_host_and_origin_uid`, `list_remote_discovered_for_host`, `soft_delete_remote_discovered_workspace_sync_by_id`.
- `src-tauri/src/hosts_inventory.rs` — background poller for the asymmetric publish flow. `spawn(app)` wired from `lib.rs` setup; per-host cycle runs `probe_host` + `fetch_inventory` + `reconcile_host_inventory`. `build_inventory_argv` locks in the SSH flags + `~/.local/bin/codemux-remote` fallback. `PollStats` captures inserted/updated/soft_deleted per cycle.
- `src-tauri/src/bin/codemux_remote.rs::run_workspace_list` — `workspace list` CLI subcommand the desktop invokes over SSH. Reads the daemon's SQLite directly, no HTTP, no running daemon required.
- `src-tauri/src/commands/workspaces_sync.rs::detect_same_branch_project_conflict` — pull-conflict guard; populates `AdoptionPreview.same_branch_project_exists_at`.
- `src-tauri/src/workspaces_sync.rs` — sync module: `pull`, `push`, `try_sync_with_app`, `sync_workspaces`, `reconcile_from_snapshot`, `ServerWorkspace` wire type.
- `src-tauri/src/commands/workspaces_sync.rs` — Tauri command surface: `workspaces_sync_list`, `workspaces_sync_now`, `workspaces_adoption_preview`, `workspaces_adopt_synced`.
- `src-tauri/src/commands/hosts.rs` — `workspace_pull_back_impl` (extracted from the `#[tauri::command]` wrapper so the adoption flow can call the rsync machinery without going back through Tauri IPC).
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
- `~/codemux-api/api/src/tests/workspaces.test.ts` — 33 tests covering auth, validation, isolation, product boundary, soft-delete, per-user cap.

## Notes

- The pattern is intentionally a near-copy of `hosts_sync` and `automations_sync`. If the wire protocol or sync semantics ever need to change, change them all three together.
- The cross-device identity is `server_id` (stringified BIGSERIAL). Local `workspace_id` strings (`workspace-42` style) are device-local and meaningless cross-device — never use one as a cross-device key.
- The 30s cadence is a magic number; tune by changing the `tokio::time::sleep(30s)` call in `lib.rs`. Lower bound is ~5s before the API would start to feel busy under many devices.
