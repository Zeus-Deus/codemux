# Remote workspace pull/adoption fix (passpage empty-worktree investigation)

> **ARCHIVED.** This plan's work has landed; it is kept as the implementation
> record and reasoning trail, not as current truth. For how this behaves today
> read the relevant `docs/features/*` doc (see `docs/INDEX.md`).

- Purpose: Track the fix for agent-created workspaces on a `codemux-remote` host that pull as EMPTY worktrees on the desktop, plus the supporting tool/skill gaps surfaced by the same investigation.
- Audience: Anyone touching the remote daemon tool surface, the hosts-inventory poller, workspaces-sync, or the SSH pull/adoption path.
- Authority: Active work plan. Behavior that lands moves to `docs/features/remote-hosts.md` / `docs/features/workspaces-sync.md`.
- Read next: `docs/plans/mcp-on-remote.md`, `docs/plans/project-identity.md`, `docs/features/workspaces-sync.md`.

## Trigger / symptom

A user runs an agent on a home server (`pandora`) via `codemux-remote mcp`. The agent built a real project at `/home/deus/projects/passpage` (`git init`, commits, branches) and a real, populated worktree at `/home/deus/projects/passpage-ui-polish` (branch `ui-polish-v1`). Both are correctly registered in the daemon. On the **desktop**, the two workspaces appear under project `passpage` as "remote" rows, but pulling/opening them shows **empty file trees**.

## Verified facts (read-only inspection)

- Server is healthy: real repo + populated linked worktree, both registered with correct `path`, `kind`, `project_uid`, `project_name`; `repo_remote = null` (local-only repo). Daemon binary is `0.7.4` and its `workspace list` envelope emits all identity fields correctly.
- The worktree on the host is a **sibling** (`~/projects/passpage-ui-polish`), created by a manual `git worktree add` — NOT under `~/.codemux/worktrees/` (the agent improvised because the headless MCP had no `worktree_create`).
- Desktop: local `~/.codemux/worktrees/passpage/{main,ui-polish-v1}` exist but are **empty, no `.git`**. The desktop `workspaces_sync` rows have empty `project_uid`/`workspace_kind`, and the `main` row has empty `project_path`.

## Root causes

1. **Pull-back rsyncs from an ASSUMED path, not the real one.** `workspace_pull_back_impl` (`commands/hosts.rs`) computes the remote source as `crate::ssh::conventional_remote_path(project_name, branch)` = `~/.codemux/worktrees/<project>/<branch>`. That is correct only for workspaces the **desktop pushed**. An agent-created workspace lives at its real registered `path` (`/home/deus/projects/passpage-ui-polish`), which does not exist under `~/.codemux/worktrees/` → `pull_workspace_back` returns `RemoteNotFound` (or an empty copy) → empty local worktree. The daemon's true `path` is never threaded to the pull.

2. **The real remote path is dropped in sync.** `reconcile_host_inventory` maps `project_path = project_root ?? path`. For a worktree, `project_root` is the parent repo, so the worktree's own on-disk `path` is lost — the sync row cannot tell the pull where the files actually are.

3. **Linked-worktree `.git` is unusable cross-machine.** A worktree's `.git` is a gitfile pointing at `<parent>/.git/worktrees/<name>`. Even if files rsync, that pointer references a path that doesn't exist on the desktop → broken git view. For a **local-only** repo (no remote) this can't be repaired by cloning.

4. **`upsert_workspace_sync_from_server` clobbers local identity.** It is server-authoritative for `project_uid`/`workspace_kind` and will overwrite a correctly-derived local value with a `null` from the cloud. Latent footgun independent of the above.

5. **Headless MCP had no `worktree_create`** (FIXED — see below). The Vexis skill told the agent to call `mcp__codemux__worktree_create`, absent on the remote surface, forcing the `git worktree add` + `workspace_create` improvisation.

## Fixes

### #3 — `worktree_create` on the headless daemon — DONE (this branch)

`src-tauri/src/remote/git.rs` (new): `create_worktree(home, repo, branch, new_branch, base)` runs `git worktree add` under `~/.codemux/worktrees/<repo>/<branch>` (desktop layout), idempotent reuse, real-repo unit tests. Wired as the `worktree_create` MCP tool in `remote/tools/mod.rs` (catalog + dispatch + dispatch-level tests). Registers the worktree with `project_root = parent repo` so it inherits the shared `project_uid` and `kind = worktree`.

### #1 — pull the REAL remote path + stop clobbering identity — DONE (this branch)

- New local-only `origin_path` column on `workspaces_sync` (additive ALTER), threaded through `WorkspaceSyncRecord`, `row_to_workspace_sync` (index 16), and the remote-discovered insert/update CRUD.
- `reconcile_host_inventory` records `origin_path = ws.path` (the daemon's REAL on-host path), distinct from `project_path` (which collapses to the parent repo for a worktree).
- `workspace_pull_back_impl` now rsyncs from `origin_path` when present, falling back to the conventional path only for desktop-pushed workspaces (backward compatible). This fixes the empty pull for agent-created main/standard workspaces end-to-end.
- `upsert_workspace_sync_from_server` uses `COALESCE(server, local)` for `project_uid`/`workspace_kind` so a null from the cloud never clobbers a locally-derived identity. `origin_path`/`origin_uid` are not in its SET list, so they survive cloud pulls untouched.
- Adoption already surfaces `RemoteNotFound`/`RsyncFailed` as `ok:false`.
- Tests: `reconcile_records_real_origin_path_distinct_from_project_path`, `reconcile_updates_origin_path_when_host_path_changes`, plus all 107 existing sync tests still green.

### (superseded) #1 — original plan

- Thread the daemon's actual `Workspace.path` through `RemoteWorkspace` → a new `origin_path` (or reuse) column on `workspaces_sync` → the adopted app_state workspace → `workspace_pull_back_impl`, which rsyncs from that path instead of the reconstructed conventional path. Backward compatible: a pushed workspace's real path already IS the conventional path.
- `upsert_workspace_sync_from_server`: `COALESCE(server, local)` for `project_uid`/`workspace_kind` so a null from the cloud never wipes a known-good local value.
- Adoption already surfaces `RemoteNotFound`/`RsyncFailed` as `ok:false`; ensure the adoption command propagates that as a visible error and does not leave a silent empty shell linked as "adopted".

### #2 — adopting a worktree of a local-only repo — DONE (this branch, Design A)

Decision: **A. Whole-repo rsync + local worktree recreate.** `workspaces_adopt_synced` now detects `workspace_kind == "worktree"` and delegates to `adopt_worktree_via_repo_rsync`:
1. rsync the PARENT repo (incl. `.git`, from the worktree row's `project_path` = the repo path on the host) into `~/.codemux/projects/<repo>` — skipped when a local repo is already there (adopting a 2nd worktree of the same project reuses it).
2. `git_recreate_worktree_for_adopted_repo` (new in `git.rs`): `git worktree prune` (drops the host's stale worktree admin entries, which point at paths that don't exist locally) then `git worktree add` for the branch into the canonical `~/.codemux/worktrees/<repo>/<branch>`.
3. Register a local workspace at the recreated worktree, clear `host_id`, link the sync row.

Works for local-only repos (no remote) because rsync carries the objects + branch refs — no clone needed. `main`/standard rows keep the single-dir pull path (now sourcing from `origin_path`, #1).

Tests: `git::tests::test_recreate_worktree_for_adopted_repo_prunes_stale_host_entry` (simulates the rsynced-in stale host worktree entry → prune → clean local recreate). The SSH rsync leg is covered by the shared `pull_workspace_back` machinery.

Options not taken: **B. Standalone snapshot** (rsync worktree + copy objects) — diverges from "it's a worktree", duplicates objects. **C. Require a remote** — fails for local-only repos like `passpage`.

### #4 — Vexis skill update — drafted, handed to user (skill lives on pandora; not edited here)

Add a new-project bootstrap (`git init` + project-folder location convention) and correct the remote tool surface (`worktree_create` now available on `codemux-remote mcp`).

## Superset comparison (pre-merge review)

Studied `superset-sh/superset` to validate our approach. Findings:

- **Architectural divergence (the big one).** Superset NEVER copies files across machines. The desktop operates **in place** on the owning host via a WebSocket-tunnelled remote-FS RPC (`relay` → host-service → `node:fs` on `worktreePath`; see `packages/workspace-fs`, `apps/relay/src/tunnel.ts`). Files don't cross the wire except per-request bytes; a host offline ⇒ "files unavailable" screen. So Superset structurally **cannot** hit our "empty worktree after sync" bug — there is no sync. Codemux is copy/rsync-based, which is why we had the bug and why #1/#2 patch the rsync path. Our fixes are correct *within* the copy model; a tunnelled remote-FS is the larger strategic alternative (major rearchitecture — explicitly deferred, tracked here as the long-term direction).
- **Validated: prune-before-add.** Superset's host-service runs `git worktree prune` before every `git worktree add` (and adopts a branch already checked out elsewhere). We now do this in BOTH the #2 recreate path and the #3 headless `worktree_create` (added in review).
- **Validated: git-init bootstrap.** Superset `git init --initial-branch=main` + `--allow-empty` initial commit, with atomic mkdir + a `PRECONDITION_FAILED` translation when git identity is unset. Our #4 skill draft mirrors this (added the `--allow-empty` note).
- **Validated: deterministic identity.** Superset reconciles independent clones by canonical GitHub remote (random UUID + `findByGitHubRemote`/`linkRepoCloneUrl`). Our `project_identity.rs` computes the uid deterministically from the canonical remote — same convergence, no reconcile step. Already shipped.
- **Adopt later (out of scope for this PR):**
  - **Main-workspace normalize sweep + "one main per (project,host)" guarantee.** Superset enforces a partial unique index `(projectId, hostId) WHERE type='main'`, calls `ensureMainWorkspace` at every create, and runs `runMainWorkspaceSweep` on boot ("no orphan worktrees"). Our daemon has `sweep_backfill_identity` (backfills uid/kind) but not a main-normalize/guarantee. Worth adding to `remote/workspace.rs`.
  - **Worktree path keyed by stable project id, not repo basename.** Superset uses `~/.superset/worktrees/<projectId>/<branch>`; ours is `~/.codemux/worktrees/<repo-basename>/<branch>`, which can collide across two different repos sharing a basename. Re-keying by `project_uid` is a broader change to `workspace_paths.rs` (used by push/pull/adopt) — defer.

## Touch points

- `src-tauri/src/remote/git.rs` (new, done), `remote/tools/mod.rs` (done)
- `src-tauri/src/hosts_inventory.rs` (carry real `path`), `database.rs` (sync column + upsert COALESCE), `state.rs`/`commands/workspace.rs` (adopted workspace carries origin path), `commands/hosts.rs` (`workspace_pull_back_impl` uses it), `commands/workspaces_sync.rs` (adoption error propagation)
