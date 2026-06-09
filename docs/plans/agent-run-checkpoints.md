# Agent-Run Rollback Checkpoints (issue #80)

- Purpose: Track the design + implementation of the opt-in background rollback checkpoint taken at agent-chat run start.
- Audience: Anyone changing agent-chat run lifecycle or the checkpoint/restore git helpers.
- Authority: Active work plan only, not current truth.
- Update when: Scope, restore semantics, or touch points change.
- Read next: `docs/features/agent-chat.md`, `docs/core/STATUS.md`

## Goal

Give users a rollback point for an agent run without ever delaying the
agent's first token. When the (opt-in) setting is enabled, starting an
agent-chat session fires a **background** snapshot of the workspace
working tree; the chat pane exposes a "Restore checkpoint" action that
returns the tree to that state.

## Design decisions (locked)

1. **Trigger**: `agent_chat_start_session` — one checkpoint per run
   (session start). Per-message checkpoint history is explicitly out of
   scope for v1.
2. **Zero first-token latency**: the snapshot runs on
   `tauri::async_runtime::spawn` + blocking pool *after* the command
   has already returned the `ThreadId`. Nothing on the start path
   awaits it.
3. **Non-destructive snapshot mechanism**: a *temporary index file*
   (`GIT_INDEX_FILE` pointed at a scratch path) seeded from `HEAD`,
   `git add -A` into that index (captures modified **and untracked**
   files; respects `.gitignore`), `git write-tree`, then
   `git commit-tree` with `HEAD` as parent. The user's real index and
   working tree are never touched — unlike `git stash create`, this
   also captures untracked files, which is the common agent-output
   case.
4. **Pinning**: the snapshot commit is pinned under
   `refs/codemux/checkpoints/<thread_id>` so GC can't reap it, and the
   commit + `HEAD`-at-checkpoint hashes are persisted on the
   `agent_chat_sessions` row (`checkpoint_commit`, `checkpoint_head`).
5. **Restore semantics (tree-only)**: restore makes the working tree +
   index match the snapshot exactly:
   - take a fresh *safety* snapshot of the current state first (so
     restore is itself undoable from the ref, `refs/codemux/pre-restore/<thread_id>`),
   - `git read-tree --reset -u <snapshot_commit>` (checks out the
     snapshot tree, removing tracked files that didn't exist then),
   - `git clean -fd` (removes files created after the snapshot that
     are untracked; ignored files like `node_modules`/`.env` survive
     because `git clean` without `-x` skips them, and files that were
     untracked at snapshot time were captured in the snapshot tree and
     are re-tracked by the `read-tree`, so they survive too).
   - Branch refs / `HEAD` are **not** moved: commits the agent made
     during the run stay in history; only the tree contents revert.
6. **Opt-in**: `UserSettings.git.agent_checkpoint_enabled`
   (default **false**), toggle in Settings → Git. The empty-repo /
   non-git-workspace case is a silent no-op.
7. **UI**: a "Restore checkpoint" item in the chat pane header menu,
   visible only when the thread has a recorded checkpoint; destructive
   confirm (AlertDialog, `variant="destructive"`); resolution surfaces
   via toast. Per chat-UI rules: no accent color, no modal approval in
   the transcript — this is pane chrome, not conversation content.

## Active Priorities

1. ~~git helpers + tests~~ → see Already Landed
2. ~~start-session hook + persistence~~
3. ~~settings plumbing + UI toggle~~
4. ~~restore command + pane-header action~~

## Open Questions

- Should restore also offer "reset branch to checkpoint HEAD"
  (un-commit agent commits)? Deferred — tree-only restore is the safe
  v1; the `checkpoint_head` hash is already recorded for a future
  follow-up.
- Checkpoint retention/pruning (refs accumulate one per thread).
  Cheap; revisit if `refs/codemux/*` ever becomes noisy.

## Likely Touch Points

- `src-tauri/src/git.rs` — `git_create_workspace_checkpoint`,
  `git_restore_workspace_checkpoint`
- `src-tauri/src/commands/agent_chat.rs` — start-session hook,
  `agent_chat_get_checkpoint`, `agent_chat_restore_checkpoint`
- `src-tauri/src/database.rs` — `checkpoint_commit`/`checkpoint_head`
  columns on `agent_chat_sessions`
- `src-tauri/src/settings*` / `src/tauri/types.ts` — new setting
- `src/components/chat/AgentChatPane.tsx` (+ header menu component)
- `src/components/settings/*` — toggle
- `src/dev/tauri-mock.ts` — mock checkpoint state for browser e2e

## Already Landed

- `git_create_workspace_checkpoint` / `git_restore_workspace_checkpoint`
  + 4 unit tests against real repos (`src-tauri/src/git.rs`)
- `checkpoint_commit` / `checkpoint_head` columns on
  `agent_chat_sessions` + `set/get_agent_chat_checkpoint`,
  `get_agent_chat_session_cwd` (`src-tauri/src/database.rs`)
- Background run-start hook (`spawn_run_checkpoint` /
  `perform_run_checkpoint`) in `agent_chat_start_session`, plus
  `agent_chat_get_checkpoint` / `agent_chat_restore_checkpoint`
  commands (`src-tauri/src/commands/agent_chat.rs`)
- `git.agent_checkpoint_enabled` setting (Rust `settings_sync.rs` +
  TS types + Settings → Git toggle)
- "Restore checkpoint" header action with destructive confirm dialog
  (`src/components/chat/AgentChatPaneHeader.tsx`), dev-mock support
- Integration test `run_checkpoint_records_and_restores_against_real_repo`
  (`src-tauri/tests/agent_chat_commands.rs`)

## Notes

- The snapshot is best-effort: failures log to stderr and never
  surface as run errors.
- A second session start on the same thread overwrites the thread's
  checkpoint (latest run wins), keeping "revert to before this run"
  semantics.
