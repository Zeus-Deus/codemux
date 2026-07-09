# Changes Panel

- Purpose: Describe the current capability and constraints of the git changes panel.
- Audience: Anyone working on git integration or the changes panel UI.
- Authority: Canonical feature-level reality doc.
- Update when: Staging, commit, merge, or diff behavior changes.
- Read next: `docs/features/diff-viewer.md`, `docs/features/review-integration.md`, `docs/plans/git-bot.md`

## What This Feature Is

The Changes panel is a right-sidebar tab covering staging, committing, stashing, pushing/pulling, and merge-state handling.

> The refined-minimal UI pass (`92965c9`, "slim Changes panel + ADE-native right sidebar") removed a large amount of this panel's surface. See "Removed by the slim-panel pass" below — several of those removals look unintentional, since their backends, command wrappers, and settings rows were all left in place.

## Current Model

The panel reads git state from the Rust backend via Tauri commands (`getGitStatus`, `getGitBranchInfo`, `getMergeState`). All mutations (stage, unstage, commit, push, pull, fetch, amend, undo, stash, abort/continue merge) go through typed Tauri command wrappers. The panel auto-refreshes when the backend emits `app-state-changed` events.

## What Works Today

- Three file groups — `Staged`, `Changed`, `Conflicts` — as flat lists (no directory grouping)
- Per-file status icon + label: Added, Modified, Removed, Renamed, Untracked, Conflict (`STATUS_META`)
- Per-file additions/deletions counts
- Per-file hover actions: Stage/Unstage toggle, Discard (click once to arm, again to confirm)
- Commit with message textarea. Committing stages all unstaged files first — there is no separate "stage all" control
- AI-generated commit messages via `useAiCommitStore` (sparkles icon); configured in Settings → Git → AI Tools
- Push, pull, and fetch operations; Publish for branches with no upstream
- Amend commit, undo last commit, stash push/pop
- Branch info display (name, ahead/behind counts)
- Merge-state banner when a merge/rebase is in progress (started outside the panel, e.g. from a terminal): "Merge in progress", conflict count, **Abort**, and **Continue** (rendered only at zero conflicts)
- Commits are disabled while unresolved conflicts exist
- Click a file to open the full diff viewer

## Removed by the slim-panel pass

All of these have live backends, registered Tauri commands, and `src/tauri/commands.ts` wrappers — but **no caller in `src/components/`**:

- `mergeBranch` — the "Merge [base] into current" button
- `mergeIntoBase` — the "Merge into [base]" flow (temp branch, confirmation dialog, delete-branch-after-merge)
- `resolveConflictOurs` / `resolveConflictTheirs` — the per-file **O**urs / **T**heirs quick-resolve buttons
- the "Resolve with AI" entry point into the merge resolver (`ai-merge-store.ts` has zero importers — see `docs/features/merge-resolver.md`)
- `gitLogEntries` — the Recent Commits history section
- the "Against base" compare section (branch selector + base-diff file list)
- Alt+Click inline diff preview (no `altKey` handler remains anywhere in `src/components/`)

## Current Constraints

- No partial staging (hunk-level staging not supported)
- No interactive rebase UI
- No blame view
- No merge can be *started* from the panel — only observed, aborted, or continued
- Merge conflict inline editing not supported — resolve conflicts in the editor or a terminal, then stage

## Related

The sidebar panel's inline per-file diffs are deliberately compact. For a full-tab diff surface with unified/split layouts, section filtering (`staged`, `unstaged`, `against_base`, `all`), hunk/file navigation, and focus mode, see `docs/features/diff-viewer.md`.

## Important Touch Points

- `src/components/workspace/changes-panel.tsx` — main panel component
- `src/stores/ai-commit-store.ts` — AI commit message generation
- `src/stores/ai-merge-store.ts` — merge conflict resolution state (**no importers**; see `docs/features/merge-resolver.md`)
- `src-tauri/src/git.rs` — git operations backend
- `src-tauri/src/commands/git.rs` — Tauri git commands
- `src/tauri/commands.ts` — typed command wrappers
