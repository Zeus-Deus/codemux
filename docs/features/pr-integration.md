# PR Integration

- Purpose: Describe the current capability and constraints of the pull request panel.
- Audience: Anyone working on GitHub integration or the PR panel UI.
- Authority: Canonical feature-level reality doc.
- Update when: PR display, review, merge, or checks behavior changes.
- Read next: `docs/features/changes-panel.md`, `docs/features/merge-resolver.md`

## What This Feature Is

The PR panel is a right-sidebar tab that displays pull request details, reviews, checks, deployments, and merge controls for the current workspace branch. It integrates with GitHub via the `gh` CLI tool.

## Current Model

PR data is fetched from GitHub through `gh` CLI commands routed via Rust (`src-tauri/src/github.rs`). The frontend renders sub-components for each PR aspect. Auth status is checked before fetching. The panel updates when workspace state changes.

## What Works Today

- PR header with number, title, state (draft/open/merged/closed), source and target branches, addition/deletion stats, review decision badge, external link to GitHub
- Review management: approve, request changes, or comment with textarea
- Check status display: pass/fail summary with individual check names, status icons, and clickable detail links
- Deployment info: environment names, state badges (success/pending/failure), preview links
- Merge controls: squash merge, create merge commit, or rebase merge with dual-confirmation safety
- Conflict detection and "Resolve with AI" entry point
- Collapsible sections with item counts

## Current Constraints

- Requires `gh` CLI to be installed and authenticated
- No inline PR diff view (diffs are in the Changes panel)
- No PR creation from the panel (creation is via command palette or terminal)
- Review comments are displayed but cannot be replied to inline
- No draft PR promotion UI

## Important Touch Points

- `src/components/workspace/pr-panel.tsx` — main PR panel
- `src/components/workspace/pr/pr-header.tsx` — PR metadata display
- `src/components/workspace/pr/pr-merge-controls.tsx` — merge method selection and execution
- `src/components/workspace/pr/pr-review-actions.tsx` — review submission
- `src/components/workspace/pr/pr-checks.tsx` — CI check status
- `src/components/workspace/pr/pr-deployments.tsx` — deployment status
- `src-tauri/src/github.rs` — GitHub data fetching via gh CLI
- `src-tauri/src/commands/github.rs` — Tauri GitHub commands
