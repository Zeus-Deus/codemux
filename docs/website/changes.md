---
title: Changes
description: Stage, commit, push, and pull from the Changes panel with AI commit messages and conflict resolution.
---

# Changes

The Changes panel is the git workflow hub. Open it with `Ctrl+Shift+G` or by clicking the "Changes" tab in the right panel.

## Commit Bar

At the top of the panel:

- **Commit message input** — Type your message, press Enter to commit
- **AI generate button** — Click the sparkle icon to generate a commit message from staged changes using Claude CLI
- **Commit** — Commits all staged files
- **Push** — Shows ahead count (e.g., "Push 3"). Shows "Publish" for branches without an upstream.
- **Pull** — Appears when behind the remote. Shows behind count (e.g., "Pull 2").
- **Refresh** — Manually refresh git status

## File List

Files are organized into sections:

### Conflicts

When merge conflicts exist, a red **Conflicts** section appears at the top. Each file shows:

- Conflict type badge (e.g., "Both Modified", "Deleted by Them")
- Quick resolve buttons: **O** (accept ours), **T** (accept theirs), **Resolved** (mark as resolved)
- "Resolve with AI" button at the bottom (requires configuration in Settings)

### Staged

Files that have been `git add`-ed. Click "Unstage all" to unstage everything, or hover a file and click the minus icon to unstage individually.

### Changes

Unstaged modifications, new files, and deletions. Click "Stage all" to stage everything, or hover a file and click the plus icon to stage individually.

Files are grouped by directory with collapsible headers. Each file shows:

- Status badge: **A** (added), **M** (modified), **D** (deleted), **R** (renamed), **U** (untracked)
- Addition/deletion counts (e.g., `+12 -3`)
- Hover actions: Stage/Unstage, Discard (with confirmation)

## Inline Diff

Alt+Click any file to expand an inline diff preview directly in the panel (max 192px height). Click normally to open the full diff viewer tab instead.

## AI Commit Messages

When the Claude CLI is installed and AI commit messages are enabled in Settings:

1. Stage your changes
2. Click the sparkle icon next to the commit input
3. A commit message is generated from the staged diff
4. Edit if needed, then commit

Configure the model in Settings > Git > AI Tools.

## Against Base

A collapsible section showing files changed between your branch and a base branch (defaults to `main`).

- **Branch selector** — Dropdown to compare against any remote branch
- **File list** — Directory-grouped files with status badges and diff stats

Two merge buttons in the section header:

- **Merge [base] into current** (GitMerge icon) — Pull the latest changes from the base branch into your feature branch. Safe, non-destructive.
- **Merge into [base]** (ArrowUp icon) — Push your feature branch changes into the base branch. Uses a temporary resolver branch so the base is never modified until the merge is verified clean. See [Merge Resolver](merge-resolver.md#merge-into-main) for the full flow.

### Merge [base] into Current

1. Click the merge icon (GitMerge) next to the file count
2. Codemux runs `git merge --no-ff <base branch>` in the workspace
3. **Clean merge** — Shows "Merged [base] into current branch" with a green checkmark
4. **Already up to date** — Shows "Already up to date with [base]" (nothing to merge)
5. **Conflicts** — The merge banner appears with per-file resolution options (Ours, Theirs, AI Resolve) and an Abort button

### Merge into [base]

1. Click the arrow-up icon next to the merge icon
2. A confirmation dialog appears — confirm with optional "delete branch after merge" checkbox
3. **Clean merge** — Base branch is updated, you're switched to it, success message shown
4. **Conflicts** — A blue banner shows the in-progress merge with resolve/abort options. All conflict resolution tools (Ours, Theirs, AI) work on the safe temp branch. Click **Complete Merge** when done.
5. **Abort** — Returns you to your feature branch. Base branch untouched.

## Recent Commits

A collapsible section at the bottom shows the 10 most recent commits. Each entry displays:

- Commit hash (abbreviated)
- Commit message
- Author and relative timestamp
- Push status: unpushed commits show an arrow icon
- A "pushed" separator line between pushed and unpushed commits

## Merge State

When a merge or rebase is in progress, a warning banner appears:

- "Merge in progress — N conflicts"
- **Complete Merge** button (when all conflicts resolved)
- **Abort** button (cancels the merge)

Commits are disabled while unresolved conflicts exist.

## Keyboard Shortcuts

| Shortcut | Action |
|----------|--------|
| Ctrl+Shift+G | Toggle Changes panel |
