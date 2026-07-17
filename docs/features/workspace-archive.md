# Workspace Archive

- Purpose: Describe the current capability and constraints of the workspace archive subsystem.
- Audience: Anyone working on workspace lifecycle, the sidebar close affordances, or the Settings → Archive panel.
- Authority: Canonical feature-level reality doc.
- Update when: Archive/restore behavior, safety guards, or touch points change.
- Read next: `docs/features/workspaces-sync.md`, `docs/features/workspace-creation.md`, `docs/features/sidebar.md`

## What This Feature Is

Archiving is the non-destructive way to remove a workspace from the sidebar. It replaces the old X-button Hide/Close flow with a two-tier model: a frictionless, always-reversible **archive** as the primary action, and a **delete** demoted behind an explicit destructive confirm. Nothing on disk is touched by an archive — files, branch, and worktree stay exactly where they are — and every archived workspace can be restored from Settings → Archive.

## Current Model

- **Archive (primary).** The sidebar row's hover button (and the context menu's "Archive Workspace") calls `archive_workspace`. The backend builds an `ArchivedWorkspaceSnapshot` entry (title, cwd, worktree_path, project_root, project_uid, workspace_kind, git_branch, protected, is_git, archived_at), runs the normal close path with `remove_worktree=false` (teardown scripts run; a teardown failure aborts the archive with nothing recorded), then records the entry. The entry list lives in `AppStateSnapshot.archived_workspaces` (`#[serde(default)]`, so old `layout.json` files still load) and persists/emits with the rest of app state. The frontend shows an undoable toast (Undo = immediate unarchive).
- **Every workspace kind is archivable except two:** attach-in-place workspaces (no local files; close is a detach) and remote workspaces (`host_id` set — their durable registry is the cross-device workspaces sync, and archiving would drop the host binding). Both are refused by `archive_refusal_reason` in the backend; the sidebar gives those rows a plain non-destructive **Close** instead.
- **The protected root is archivable but never deletable.** Archiving the repo-root workspace only removes its row; the project group survives as long as any sibling worktree workspace remains (grouping keys off `project_root`, not the root row). Deletion of a protected root (or any workspace without its own worktree) is refused inside `close_workspace_with_worktree_impl` — the single impl every delete surface converges on (sidebar dialog, control socket, MCP `workspace_close`) — so the guard is enforced at the command layer, not just hidden UI affordances.
- **Unarchive.** `unarchive_workspace` restores through the same creation paths used elsewhere: worktree entries go through the worktree-creation flow (which reuses the still-on-disk directory at the conventional path, or checks the surviving branch out fresh if the directory is gone — a hand-deleted worktree dir leaves a stale registration that the create flow now prunes before `git worktree add`), root entries through the add-repository flow (re-stamping `protected`/`project_uid`). A worktree entry whose directory still exists at a **non-conventional** path (an imported worktree) is instead adopted in place — no `git worktree add`, which would fail because the branch is already used by that directory. The archive entry is only removed after a successful restore; the restored workspace keeps its recorded title, files, branch, and worktree, and is activated — but it starts with a **fresh single-pane layout**: the prior pane/tab/session arrangement is not preserved (archive entries snapshot a fixed field set, not the live layout). If a live workspace already sits at the entry's location, unarchive just switches to it. If both the worktree directory and its branch are gone, unarchive errors and keeps the entry.
- **Delete (secondary, destructive).** Worktree rows expose "Delete Worktree…" via context menu or shift-click on the archive button — a destructive confirm dialog with an "Also delete local branch" checkbox. The dirty/unpushed guard is honest end to end: a non-forced delete of a dirty worktree is refused **before** the workspace is closed (pre-flight in the close impl), the dialog stays open, shows the backend message, and offers an explicit "Force delete" that reissues with force. Archived worktree entries can likewise be deleted (same escalation) from Settings → Archive; root entries there can only have their entry removed — files are never touched.
- **Archive Project.** The project header's action archives every member (worktrees first, root last, sequentially so teardown scripts don't race); attach-in-place/remote members get a plain close. Chat drafts for the project are cleared only when every member archived cleanly.

## What Works Today

- One-click archive from the sidebar for every local workspace, including the protected repo root, with an undo toast.
- Settings → Archive panel: entries grouped with the same project-label rule as the sidebar, branch chip, "repo root" chip, relative archived-time, stale hint for entries older than 30 days, Unarchive, and guarded per-entry delete.
- Root-deletion refusal enforced in the Rust command layer across all surfaces (UI, control socket, MCP), with a shared predicate so the close path and the archive path can't drift.
- Honest force semantics: `force_delete=false` really runs the dirty/unpushed check (pre-flight, before any state change); the "Use force to override." wording is pinned by a Rust test because the frontend escalation matches it.
- Control socket commands `archive_workspace`, `unarchive_workspace`, `list_archived_workspaces` and MCP tools `workspace_archive`, `workspace_unarchive`, `workspace_archive_list`.
- Dev-mock parity (`src/dev/tauri-mock.ts`): all three commands, seeded entries, and the dirty-refusal ordering match the real backend, so the full flow is drivable in `npm run dev`.

## Current Constraints

- **The archive list is device-local.** It lives in `layout.json` and does not sync. Archiving still soft-deletes the workspace's `workspaces_sync` row (identical to what closing always did), so sibling devices see the workspace disappear, not "archived". Restore creates a fresh `workspace_id`/sync row. Cross-device archive is future work.
- Attach-in-place and remote (`host_id`) workspaces cannot be archived — they get a plain close; their durable home is the Workspaces Overview / sync registry.
- Archive entries snapshot a fixed field set; workspace fields added later must be added to `build_archive_entry` explicitly or they won't survive an archive/restore round trip.
- Archiving runs teardown scripts (like the old hide); a failing teardown aborts that workspace's archive. "Archive Project" therefore no longer force-skips teardown the way the old "Close Project" did — failures surface in a toast instead.
- No automatic cleanup of archived entries or their on-disk worktrees; the stale chip (>30 days) is a hint only.

## Important Touch Points

- `src-tauri/src/state/state_impl.rs` — `ArchivedWorkspaceSnapshot`, `archived_workspaces` on the snapshot, add/dedupe/remove/find store methods, `build_archive_entry`
- `src-tauri/src/commands/workspace.rs` — `archive_workspace`, `unarchive_workspace`, `delete_archived_workspace` (+ `_impl`s), `refuse_worktree_removal`, `refuse_archived_file_delete`, `archive_refusal_reason`, dirty pre-flight in `close_workspace_with_worktree_impl`
- `src-tauri/src/git.rs` — shared dirty/unpushed guard, worktree reuse-on-restore, branch-existence check
- `src-tauri/src/control.rs`, `src-tauri/src/mcp_server.rs` — socket commands and MCP tools
- `src/components/layout/sidebar-workspace-row.tsx` — archive button, shift-click delete, `DeleteWorktreeDialog`
- `src/components/layout/sidebar-project-group.tsx` — Archive Project flow
- `src/components/settings/archive-section.tsx` — Settings → Archive panel
- `src/hooks/use-force-delete.ts` — shared force-escalation state machine (`USE_FORCE_PATTERN`)
- `src/dev/tauri-mock.ts`, `src/dev/mock-fixtures.ts` — mock handlers and seeds

## Notes

- Keep this file about current truth, not future plans.
- The archive's core promise is losslessness: any change that can make an archive entry unrestorable (or a restore lossy) needs a guard or a refusal, not a silent downgrade.
