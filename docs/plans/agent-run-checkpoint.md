# Agent Run Checkpoint (issue #80)

- Purpose: Track the design and rollout of the optional background rollback checkpoint taken at agent-chat run start.
- Audience: Anyone changing agent-chat session start, git snapshot helpers, or the restore UI.
- Authority: Active work plan only, not current truth.
- Update when: Scope, restore semantics, or follow-ups change.
- Read next: `docs/features/agent-chat.md`, `docs/core/STATUS.md`

## Goal

When an agent-chat session starts, take an **opt-in, background** snapshot of the
workspace working tree so the user can roll back everything the run changed. The
snapshot must add **zero latency** to the first token and must **not disturb** the
user's index, worktree, or stash list.

## Design

### Snapshot (non-destructive, shadow ref)

`git stash create` ignores untracked files, so the snapshot instead builds a commit
through a **temporary index** (`GIT_INDEX_FILE`), which never touches the user's real
index or worktree:

1. `git read-tree HEAD` into a temp index file
2. `git add -A` (temp index — captures tracked changes **and** untracked files, respects `.gitignore`)
3. `git write-tree` → snapshot tree
4. `git commit-tree <tree> -p HEAD` → snapshot commit (no hooks run, identity forced via `-c user.*`)
5. `git update-ref refs/codemux/checkpoints/<sanitized-thread-id> <commit>` → protects the snapshot from gc

Recorded against the thread in SQLite (`agent_chat_checkpoints`, FK → `agent_chat_sessions`
ON DELETE CASCADE): snapshot commit, HEAD commit, branch name, repo path, ref name.

Skipped silently (no checkpoint row) when: the setting is off, `cwd` is not a git repo,
or the repo has an unborn HEAD (no commits yet).

### Background execution

`agent_chat_start_session` spawns `tauri::async_runtime::spawn` + `spawn_blocking`
**after** the provider session has started and the session row is persisted. Nothing
git-related runs on the start-session (or first-token) path. On success the backend
emits `agent_chat_checkpoint` (`{ thread_id, checkpoint }`) so the pane header can
reveal the restore affordance.

### Restore semantics

`agent_chat_restore_checkpoint(thread_id)`:

1. Refuse if the snapshot/HEAD commits are gone (pruned) or the repo is now on a different branch.
2. Safety snapshot of the **current** state to `refs/codemux/pre-restore/<id>` (parents at the
   agent's last commit, so even agent-made commits stay reachable after the branch resets).
3. `git read-tree --reset -u <snapshot>` — worktree + index now match the snapshot tree
   (restores modified/deleted files, including formerly-untracked ones).
4. `git clean -fd` — deletes run-created files (ignored files are spared).
5. `git reset --mixed <head-at-checkpoint>` — undoes run-made commits, leaves the restored
   content as **unstaged** changes; formerly-untracked files show as untracked again.

Known, documented loss: the staged/unstaged split of the pre-run state is flattened to
unstaged (the snapshot records one tree, not the index).

### Pruning

Each successful checkpoint prunes both `refs/codemux/checkpoints/*` and
`refs/codemux/pre-restore/*` to the 20 newest per namespace (by committer date) and
deletes the matching `agent_chat_checkpoints` rows, so shadow refs cannot grow unboundedly.

### Opt-in setting

`UserSettings.agent_chat.checkpoints_enabled` (synced settings, **default `false`**).
Toggle lives in Settings → Agent (visible when the agent-chat beta flag is on). The
background task reads the settings cache (`settings_sync::load_cache()`), same pattern
as session-restore.

## Active Priorities

1. ~~Backend: git helpers + DB table + commands + start-session hook~~ (landed)
2. ~~Frontend: setting toggle, store slice, header restore button + confirm dialog~~ (landed)
3. Follow-ups below

## Open Questions / Follow-ups

- Checkpoint continuity across silent restarts (permission-mode change restarts the
  session under a new thread id, which takes a fresh checkpoint — "before this run"
  then means "before the restart"). Possible fix: carry the checkpoint forward when
  `resume_cursor` is set.
- Surface checkpoints over the socket API / MCP server so agents can self-rollback.
- Per-turn checkpoint history (explicitly out of scope for v1 per issue #80).
- Restore button for *past* sessions from the SessionSelector dropdown.

## Likely Touch Points

- `src-tauri/src/git.rs` — `git_checkpoint_create` / `git_checkpoint_restore` / `git_checkpoint_prune`
- `src-tauri/src/database.rs` — `agent_chat_checkpoints` table + CRUD
- `src-tauri/src/commands/agent_chat.rs` — spawn hook, `agent_chat_get_checkpoint`, `agent_chat_restore_checkpoint`
- `src-tauri/src/settings_sync.rs` — `AgentChatSettings`
- `src/components/chat/AgentChatPaneHeader.tsx` — restore affordance
- `src/stores/agent-chat-store.ts` — per-thread checkpoint slice
- `src/components/settings/settings-view.tsx` — opt-in toggle
- `src/dev/tauri-mock.ts` — mock handlers for browser-pane smoke tests

## Already Landed

- (see git history of this branch)

## Notes

- The anti-pattern this feature exists to avoid: any synchronous `git add -A`-style
  walk between the user pressing send and the provider's first token.
