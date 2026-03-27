---
title: AI Merge Conflict Resolver
description: Resolve merge conflicts with AI using a safe temp branch that never touches your real branch until you approve.
---

# AI Merge Conflict Resolver

The resolver uses an AI agent to automatically resolve merge conflicts. It works on a temporary branch so your real branch is never modified until you explicitly approve the result.

## How It Works

1. **Create temp branch** — Codemux creates `bot/resolve-{source}-into-{target}-{timestamp}` from your current branch
2. **Start the merge** — `git merge {target}` runs on the temp branch, producing conflict markers
3. **AI resolves** — The configured CLI tool (Claude Code, Codex, or OpenCode) reads each conflicted file, resolves the markers, and `git add`s the resolved files
4. **Review** — You see the full diff of what the AI changed. The original files with conflict markers vs. the resolved output.
5. **Approve or reject**:
   - **Approve** — The temp branch is merged back into your original branch, then deleted
   - **Reject** — The temp branch is deleted. Your original branch is untouched. Nothing happened.

## Safety Guarantees

- Your original branch is **never modified** during resolution
- The temp branch is always deletable — it's throwaway
- You review the full diff before anything is applied
- Abort at any point returns you to exactly where you started
- No force-pushes, no branch deletions, no destructive operations

## Resolution Strategies

Configure in Settings > Git > AI Tools:

| Strategy | Description |
|----------|-------------|
| **Smart merge** | Understand the intent of both changes and write the optimal resolution (default) |
| **Keep both** | Preserve all functionality from both sides — combine changes so nothing is lost |
| **Prefer my branch** | Keep current branch changes as baseline, carefully integrate theirs |
| **Prefer target** | Keep target branch changes as baseline, carefully integrate ours |

## Configuration

In Settings > Git > AI Tools > Merge Conflict Resolver:

- **Enable/disable** — Toggle the "Resolve with AI" button
- **CLI tool** — Claude Code, Codex, or OpenCode
- **Model override** — Leave empty for the CLI default, or specify a model (e.g., `opus`, `sonnet`)
- **Strategy** — Default resolution approach

## Using the Resolver

1. When conflicts are detected in the Changes panel, a red **Conflicts** section appears
2. Click **Resolve with AI** at the bottom of the section
3. Watch the progress: creating branch → resolving → review
4. In review mode: **Approve** to apply, **Reject** to discard, or edit files manually first
5. After approval, the merge is complete and the temp branch is cleaned up

## Per-File Resolution

You can also resolve conflicts manually alongside the AI:

- **O** (Ours) — Accept your version: `git checkout --ours <file> && git add <file>`
- **T** (Theirs) — Accept their version: `git checkout --theirs <file> && git add <file>`
- **Resolved** — Mark as resolved after manual editing: `git add <file>`

## Local Branch Merge

In addition to AI-powered conflict resolution, Codemux supports direct local branch merging from the Changes panel.

### How It Works

The "Against [base]" section in the Changes panel shows how your branch differs from the base branch. A merge button in the section header lets you merge the base branch into your current branch — the safe direction for keeping your feature branch up to date.

1. Click the **merge icon** next to the file count in the "Against" section
2. Codemux runs `git merge --no-ff <base>` in the workspace directory
3. Three outcomes:
   - **Merged** — New commits from the base branch are integrated. A green confirmation appears.
   - **Already up to date** — The base has no commits your branch doesn't already have.
   - **Conflicts** — The merge pauses with conflict markers. The same Conflicts section appears with all resolution options: per-file Ours/Theirs buttons, manual editing, or AI resolution.
4. After resolving all conflicts, click **Complete Merge** in the merge banner
5. To cancel at any time, click **Abort** to restore your branch to its pre-merge state

### Safety

- Only merges base INTO your branch (never the reverse)
- Refuses to run on a dirty working tree — commit or stash first
- Abort always restores your branch to exactly its pre-merge state
- Works with the same conflict resolution UI as the AI resolver
