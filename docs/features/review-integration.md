# Review Integration

- Purpose: Describe the current capability and constraints of the Review tab (pull request review surface).
- Audience: Anyone working on GitHub integration or the Review panel UI.
- Authority: Canonical feature-level reality doc.
- Update when: PR display, review, merge, or checks behavior changes.
- Read next: `docs/features/changes-panel.md`, `docs/features/merge-resolver.md`

## What This Feature Is

The Review tab is a right-sidebar surface that displays pull request details, reviews, checks, deployments, and merge controls for the current workspace branch. It integrates with GitHub via the `gh` CLI tool.

> The tab was previously named "PR." It was renamed to "Review" in Phase 3 of the right-sidebar work to match Superset's terminology. The underlying data flow and feature set are unchanged; only the tab id, label, and component file names moved (`pr-panel.tsx` → `review-panel.tsx`, `pr/` subfolder → `review/`).

## Current Model

PR data is fetched from GitHub through `gh` CLI commands routed via Rust (`src-tauri/src/github.rs`). The frontend renders sub-components for each PR aspect. Auth status is checked before fetching. The panel updates when workspace state changes.

## What Works Today

- PR creation from panel when no PR exists for the current branch (title, body, base branch, draft toggle)
- PR header with number, title, state (draft/open/merged/closed), source and target branches, addition/deletion stats, review decision badge, external link to GitHub
- Review submission: approve, request changes, or comment with textarea
- Check status display: pass/fail summary with individual check names, status icons, and clickable detail links
- Deployment info: environment names, state badges (success/pending/failure), preview links
- Merge controls: squash merge, create merge commit, or rebase merge with dual-confirmation safety
- Conflict detection and "Resolve with AI" entry point
- Collapsible sections with item counts

## Incoming PRs View

The PR panel includes an incoming PRs list that shows all open pull requests targeting the repo's base branch. Each PR row displays:

- PR number, title, author, and head branch
- Draft badge for draft PRs
- Review decision indicator (Approved, Changes Requested)
- CI checks status (success/failure/pending icons)
- Addition/deletion stats and relative timestamp
- Hover actions: "View" opens PR on GitHub, "Checkout" creates a worktree workspace from the PR branch (or switches to an existing workspace if one already tracks that branch)
- PR number is passed through to workspace creation for sidebar badge display

The view fetches up to 50 PRs via `gh pr list` and shows a "View all on GitHub" link when the limit is reached.

## Current Constraints

- Requires `gh` CLI to be installed and authenticated
- No inline PR diff view (diffs are in the Changes panel)
- Review comments are displayed but cannot be replied to inline
- No draft PR promotion UI

## Important Touch Points

- `src/components/workspace/pr-panel.tsx` — main PR panel
- `src/components/workspace/pr/pr-header.tsx` — PR metadata display
- `src/components/workspace/pr/pr-merge-controls.tsx` — merge method selection and execution
- `src/components/workspace/pr/pr-review-actions.tsx` — review submission
- `src/components/workspace/pr/pr-checks.tsx` — CI check status
- `src/components/workspace/pr/pr-deployments.tsx` — deployment status
- `src/components/workspace/pr/incoming-prs-view.tsx` — incoming PRs list
- `src-tauri/src/github.rs` — GitHub data fetching via gh CLI
- `src-tauri/src/commands/github.rs` — Tauri GitHub commands
