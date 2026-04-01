# Changes Panel

- Purpose: Describe the current capability and constraints of the git changes panel.
- Audience: Anyone working on git integration or the changes panel UI.
- Authority: Canonical feature-level reality doc.
- Update when: Staging, commit, merge, or diff behavior changes.
- Read next: `docs/features/pr-integration.md`, `docs/plans/git-bot.md`

## What This Feature Is

The Changes panel is a right-sidebar tab that provides full git workflow without leaving Codemux. It covers staging, committing, diffing, stashing, pushing/pulling, merge handling, and commit history.

## Current Model

The panel reads git state from the Rust backend via Tauri commands (`getGitStatus`, `getGitBranchInfo`, `getGitDiff`, `gitLogEntries`). All mutations (stage, unstage, commit, push, pull, stash, merge) go through typed Tauri command wrappers. The panel auto-refreshes when the backend emits `app-state-changed` events.

## What Works Today

- Staged and unstaged file lists organized by directory with status icons (added, modified, removed, renamed, untracked)
- Stage/unstage individual files or all files
- Commit with message textarea
- AI-generated commit messages via `useAiCommitStore` (sparkles icon)
- Inline diff display per file
- Discard changes with confirmation dialog
- Push, pull, and fetch operations
- Stash push and stash pop
- Branch info display (name, ahead/behind counts)
- Merge operations: merge branch, merge into base, conflict resolution (ours/theirs), merge continuation
- AI merge conflict resolution entry point (delegates to merge resolver)
- Commit history with relative timestamps and per-file diffs

## Current Constraints

- No partial staging (hunk-level staging not supported)
- No interactive rebase UI
- No blame view
- Merge conflict inline editing not supported — conflicts go through AI resolver or manual terminal workflow

## Important Touch Points

- `src/components/workspace/changes-panel.tsx` — main panel component
- `src/stores/ai-commit-store.ts` — AI commit message generation
- `src/stores/ai-merge-store.ts` — merge conflict resolution state
- `src-tauri/src/git.rs` — git operations backend
- `src-tauri/src/commands/git.rs` — Tauri git commands
- `src/tauri/commands.ts` — typed command wrappers
