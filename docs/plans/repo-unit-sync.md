# Repo-Unit Sync (treat a git repo as one connected unit)

- Purpose: Plan the fix for cross-device sync cloning a project's default-branch checkout into a divergent full copy instead of keeping the repo (root + worktrees) as one shared-history unit. Also makes the repo-root / default-branch checkout a protected, non-deletable entry in the overview.
- Audience: Anyone touching the workspace push/pull/adopt pipeline, the `workspaces_sync` schema, project-identity classification, or the Workspaces overview UI.
- Authority: Active work plan only, not current truth. Current behavior lives in `docs/features/workspaces-sync.md` and `docs/features/workspaces-overview.md`.
- Update when: Priorities, open questions, schema shape, or likely touch points change.
- Read next: `docs/features/workspaces-sync.md`, `docs/features/workspaces-overview.md`, `docs/plans/project-identity.md`.

## Goal

Cross-device sync currently carries **loose workspace folders**, not the git repo they belong to. For a project's default branch (e.g. `main`) Codemux cannot `git worktree add` it (the branch is already checked out at the repo root), so the push/pull rsync falls back to copying the whole directory — including `.git` — into `~/.codemux/worktrees/<project>/<branch>`. That copy gets its own object store and **diverges** from the real repo root. Make Codemux treat a git repo as one connected unit: discover the canonical root, register/surface it as the protected default-branch checkout, and on cross-device pull/sync bring the repo over once and recreate sibling workspaces as real shared-history worktrees — never a divergent copy.

## Root Cause (verified against source)

Confirmed against the v0.7.7 tree:

1. **Misclassification.** `derive_kind` (`src-tauri/src/project_identity.rs:135-142`) decides `kind` purely by `.git` type: a `.git` **file** → `"worktree"`, a `.git` **directory** → `"main"`. Its own test (`:211-227`) confirms a full-copy repo (which has a `.git` directory) is labeled `"main"` — i.e. a legitimate root — even though it is a divergent copy.
2. **The divergence engine.** `build_push_rsync_argv` (`src-tauri/src/ssh/push.rs:146-147`) excludes only `.git/index.lock` and `.git/COMMIT_EDITMSG.swp`; it copies the **entire `.git` object store**. So pushing a root checkout to `~/.codemux/worktrees/<project>/main` produces a standalone clone, not a linked worktree.
3. **No root protection in the UI.** `workspace-overview-row.tsx:170` derives `const isWorktree = !!workspace.worktree_path;` and that single flag drives both the delete label (`:510`) and the destructive command (`:283-296`). It cannot distinguish a real worktree from the divergent `main` copy, so the default-branch checkout is deletable like a throwaway worktree.
4. **Real root never registered cross-device.** The headless daemon's `WorkspaceStore` only tracks workspaces created via its tools and does **not** scan `~/projects`, so a real repo root (e.g. `~/projects/passpage`) is invisible to sync — only the copy under `~/.codemux/worktrees/` is ever surfaced.

Field evidence (home server `pandora`): `~/projects/passpage` (real repo, `.git` dir, object-store inode 2097440) and `~/.codemux/worktrees/passpage/main` (Codemux copy, `.git` dir, **different** object-store inode 2503557) — same commit now, independent histories going forward.

## Active Priorities

1. **Phase 0 — Root-discovery helper + classification.** Add `git_canonical_root(path)` (via `git rev-parse --path-format=absolute --show-toplevel --git-common-dir --is-bare-repository`) and `is_divergent_copy(path)` to `src-tauri/src/git.rs` (+ daemon mirror in `src-tauri/src/remote/git.rs`). Unit-test root / worktree / bare / copy. Wire nothing yet. Verify: `cargo test --manifest-path src-tauri/Cargo.toml git::`, `cargo check --manifest-path src-tauri/Cargo.toml`.
2. **Phase 1 — Schema + identity stamping.** Additive columns on `workspaces_sync` (`src-tauri/src/database.rs`, append after the existing migration list): `canonical_root TEXT`, `is_default_branch INTEGER NOT NULL DEFAULT 0`, `protected INTEGER NOT NULL DEFAULT 0`. Thread through the row struct, CRUD helpers, `WorkspaceSyncView` DTO, and `src/tauri/commands.ts`. Stamp in `set_workspace_project_root` (`src-tauri/src/state/state_impl.rs:1358`) and daemon `create` (`src-tauri/src/remote/workspace.rs:153`). `protected = (kind == "main" && canonical_root == toplevel)` — never for a copy. No UI/sync-behavior change yet. Verify: `cargo test ...`, `npm run check`, `npm run verify`.
3. **Phase 2 — Frontend protection + labels.** Propagate `workspace_kind` + `protected` onto local items in `use-overview-items.ts`; replace the lone `isWorktree` in `workspace-overview-row.tsx` with `isRepoRoot`/`protected`; block the destructive worktree-removal path and relabel the menu for protected rows; add a "repo root" badge. Verify: `npm run test`, `npm run check`, `npm run verify`.
4. **Phase 3 — Repo-unit adoption.** Extract `adopt_via_repo_unit` from `adopt_worktree_via_repo_rsync` (`src-tauri/src/commands/workspaces_sync.rs:681`) and route `kind == "main"` through it (rsync canonical root to `~/.codemux/projects/<repo>` once, register the root directly — no spurious `worktree add`; worktrees additionally `git_recreate_worktree_for_adopted_repo`). Idempotent reuse of an already-present `~/.codemux/projects/<repo>`. Verify: `cargo test ... workspaces_sync::`, manual two-profile adopt, `npm run verify`.
5. **Phase 4 — Stop creating divergent copies on push.** Change `workspace_push_to_host` (`src-tauri/src/commands/hosts.rs:449`) so a root/default-branch push rsyncs the canonical root to `~/.codemux/projects/<repo>` (or references the real root) and registers a protected root entry; worktree pushes reference the shared host repo. Verify: round-trip showing siblings share one `git_common_dir`.
6. **Phase 5 — Non-destructive reconciliation of existing copies (e.g. passpage).** Detection + overview prompt: clean copy → re-point at the real root and mark protected; dirty/divergent copy → keep it, show the existing divergence chip, offer "import as a branch" (commit + `git worktree add` off the real root). Never delete files automatically.

## Open Questions

- **Where does a pushed root land on the host?** Recommend rsync-to-`~/.codemux/projects/<repo>` rather than mutating the user's real `~/projects/<repo>` tree. Confirm we never write into a user's hand-managed project dir.
- **No-remote repos:** `project_uid` falls back to the absolute path (device-local, won't converge cross-device). Repo-rsync (not clone) is mandatory there — already what `adopt_worktree_via_repo_rsync` does. Is rsync-only acceptable as the long-term answer for remote-less repos?
- **Reconciliation trigger:** lazy (detect on overview render) vs an explicit "Fix duplicate" affordance. Default to non-destructive re-classification; never auto-delete.
- **Windows:** repo-unit rsync is `#[cfg(unix)]` today. Root protection + classification are cross-platform; keep the transport guarded. Acceptable for v1?
- **Daemon scanning real roots:** out of scope here, but should the daemon ever discover a real `~/projects/<repo>` so it can be adopted directly? Tracked separately.

## Likely Touch Points

- `src-tauri/src/git.rs` — new `git_canonical_root` / `is_divergent_copy`; existing `git_create_worktree:1697`, `git_recreate_worktree_for_adopted_repo:1685`, `find_default_branch`, `git_remove_worktree:1806` (guard so a protected/root entry never reaches `git worktree remove`).
- `src-tauri/src/project_identity.rs` — `derive_kind:135` is the root-cause classifier; replace the `.git`-type heuristic with divergence-safe classification.
- `src-tauri/src/database.rs` — `workspaces_sync` schema (`:313`, migrations ~`:354-374`), `WorkspacesSyncRow` struct (`:919`), CRUD helpers.
- `src-tauri/src/commands/workspaces_sync.rs` — `workspaces_adopt_synced:448` dispatch (`:458-469`), `adopt_worktree_via_repo_rsync:681` → generalize to `adopt_via_repo_unit`, `workspaces_adopt_via_clone:932`, `WorkspaceSyncView` DTO (`:25-64`).
- `src-tauri/src/commands/hosts.rs` — `workspace_push_to_host:449`, `workspace_pull_back_impl:793`.
- `src-tauri/src/state/state_impl.rs:1358` and `src-tauri/src/remote/workspace.rs:153` — the two identity-stamp sites.
- `src-tauri/src/hosts_inventory.rs:387` — reconcile column mapping (`project_path`/`origin_path`/`workspace_kind`).
- `src-tauri/src/ssh/push.rs:121` — `build_push_rsync_argv` (the rsync that copies `.git`).
- `src/components/workspaces-overview/use-overview-items.ts` — propagate `workspace_kind` + `protected` onto local items.
- `src/components/workspaces-overview/workspace-overview-row.tsx:170` — replace `isWorktree`; protect delete; "repo root" badge.
- `src/tauri/commands.ts` / `src/tauri/types.ts` — `WorkspaceSyncView` + `WorkspaceSnapshot` field plumbing (most fields already present).
- Server (out of scope for v1): the three new columns are local-only unless we decide to sync them; `~/codemux-api` untouched for now.

## Edge Cases To Cover

- Bare repos (`--is-bare-repository` true, empty `--show-toplevel`): treat as root, don't recreate-in-place.
- Default branch not named `main` — always use `find_default_branch`, never string-match `"main"`/`"master"`.
- Worktree branch already checked out elsewhere — `git_create_worktree` already switches the main repo to default first; prune stale admin entries after repo-rsync.
- Uncommitted/divergent work in an existing copy — migration must be non-destructive (no `--delete` rsync, no `worktree remove --force`); detect via `git status --porcelain` + `@{upstream}..HEAD`.
- Multiple devices — repo-unit adoption idempotent; dedupe root rows by `project_uid` + `is_default_branch`.
- Host repo at a non-standard path (`~/projects/passpage`) — record the host's real path; land the pushed root under `~/.codemux/projects/<repo>`.

## Already Landed

All six phases are implemented and unit-tested (`cargo test`, `npm run check`, `npm run test` green; the only failing tests in the suite — `commands::mcp::tests::project_codemux_entry_is_filtered_out` and `agent_browser::tests::resolve_binary_finds_native_binary_from_project_root` — fail identically on the base commit and are pre-existing environment artifacts that read the real `~/.claude.json` / execute a sandbox-downloaded binary).

- **Phase 0** — `RepoRoot` + `git_canonical_root` + `is_divergent_copy` + `is_protected_repo_root` in `src-tauri/src/git.rs`, with `repo_root_tests` (main / worktree / bare / copy / protected). `git_canonical_root` uses `git rev-parse --path-format=absolute --git-dir --git-common-dir` (+ permissive `--show-toplevel` / `--is-bare-repository`); `is_divergent_copy` flags a `.git` **directory** living under `~/.codemux/worktrees/`.
- **Phase 1** — `WorkspaceSnapshot` gained `protected: bool` + `divergent_copy: bool` (`#[serde(default)]`), stamped divergence-safely in `set_workspace_project_root`, with a background-thread boot backfill (`backfill_workspace_protection`, wired in `lib.rs`). `src/tauri/types.ts` mirrors both. (Decision: kept these on the snapshot, NOT on `workspaces_sync` — `RemoteRow` has no delete action, so cross-device propagation isn't needed; documented below.)
- **Phase 2** — `workspace-overview-row.tsx`: `isRepoRoot`/`canRemoveWorktree` replace the lone `isWorktree` so a protected root is never deleted as a worktree; "repo root" badge on protected local rows; RemoteRow `main` kind renders as "repo root". Tests in `sibling-device-row.test.tsx`.
- **Phase 3** — `workspaces_adopt_synced` routes a `main` row through `create_synced_root_shell` landing at `~/.codemux/projects/<repo>` (a genuine, protected root) instead of the divergent single-dir worktree landing; `protected` stamped post-pull. The rsync SOURCE resolution in `workspace_pull_back_impl` is unchanged (resolved independently of the local target). Null/legacy rows keep the old single-dir path.
- **Phase 4** — `workspace_push_to_host` pushes a repo root to `~/.codemux/projects/<repo>` on the host via the new `conventional_remote_root_path` helper; `workspace_pull_back_impl` is root-aware and tries `projects/` first, falling back to the legacy `worktrees/` path on `RemoteNotFound` so roots pushed before this change still pull.
- **Phase 5** — non-destructive DETECTION: `divergent_copy` stamp + a "standalone copy" warning chip in the overview guiding the user to re-pull cleanly. The auto-rewrite reconcile actions (re-point / import-as-branch) were intentionally NOT shipped — see Notes.

Prior related work this builds on: project-identity `project_uid` + `kind` (`docs/plans/project-identity.md`), the worktree-kind repo-rsync adoption path (`adopt_worktree_via_repo_rsync`, `docs/plans/remote-workspace-pull-fix.md`), and the v0.7.6 adopt-failure rollback hardening (`docs/features/workspaces-sync.md`).

## Not Yet Done / Validation Gaps

- **SSH round-trip not exercised end-to-end.** Phases 3–4 change the host landing path for repo roots; the path-selection logic is unit-tested but the actual cross-device rsync round-trip could not be run in the dev sandbox (no second machine). Validate on real hardware: push a repo root to a host → confirm it lands at `~/.codemux/projects/<repo>` (not the worktrees tree); adopt it on a third device → confirm a protected root, not a divergent copy; confirm a legacy root pushed before this change still pulls (the `worktrees/` fallback).
- **Phase 5 auto-reconcile actions** (re-point a clean copy at the real root; import a dirty copy as a branch) were scoped OUT — they rewrite git state and can't be safely validated in-sandbox. The shipped detection chip lets the user resolve manually (delete + re-pull). Revisit if users want one-click reconciliation.
- The three columns were NOT added to `workspaces_sync` / the cloud schema (kept snapshot-local). Promote only if cross-device protection/labels prove necessary.

## Notes

- The fix is largely a **generalization of an existing pattern**: `adopt_worktree_via_repo_rsync` already does "rsync the whole repo, recreate the worktree" for `kind == "worktree"`. We extend that to the default-branch/root case instead of the loose single-dir copy, and we fix the classifier so a full copy is never mistaken for a legitimate root.
- Tooltip/label-only UX polish ("Track A": cloud-icon tooltip, clearer "Pull" wording, "Working here · home: <host>" dual badge) is a separate, cosmetic change — it makes the current model read better but does **not** fix the divergence. Keep it out of this plan unless bundled deliberately.
- Keep the `projects` table out of scope — it's local UI/recents state, not linked to `workspaces_sync` and not synced.
- The three new columns start local-only. Promote to the cloud schema (`codemux_workspaces`) only if cross-device protection/labels prove necessary; mirror the additive `ADD COLUMN IF NOT EXISTS` pattern used for `git_head_sha` / `project_uid` if so.
