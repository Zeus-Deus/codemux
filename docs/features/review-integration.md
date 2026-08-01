# Review Integration

- Purpose: Describe the current capability and constraints of the Review tab (pull request review surface).
- Audience: Anyone working on GitHub integration or the Review panel UI.
- Authority: Canonical feature-level reality doc.
- Update when: PR display, review, merge, or checks behavior changes.
- Read next: `docs/features/changes-panel.md`, `docs/features/merge-resolver.md`

## What This Feature Is

The Review tab is a right-sidebar surface that displays pull request details, review threads, and CI checks for the current workspace branch. It integrates with GitHub via the `gh` CLI tool.

> The tab was previously named "PR." It was renamed to "Review" in Phase 3 of the right-sidebar work to match Superset's terminology (`pr-panel.tsx` → `review-panel.tsx`, `pr/` subfolder → `review/`).
>
> A later pass **stripped the panel to Superset's resting layout** — title + status badge + checks + comments only. The review composer (Approve / Request changes / Comment), the merge controls, and the deployments section were intentionally removed (see the comment in `review-panel.tsx`). Their Rust backends are retained for a potential future re-wire, so `merge_pull_request` and the review-submission commands still exist and are registered — they simply have no caller in `src/components/`.

## Current Model

PR data is fetched from GitHub through `gh` CLI commands routed via Rust
(`src-tauri/src/github.rs`). The current workspace association uses an explicit
branch query (`gh pr list --head <branch-or-owner:branch> --state all`) rather
than implicit `gh pr view` inference or exact commit-SHA equality. This keeps a
merged/closed PR attached when review commits advanced its remote head beyond
the local worktree, supports fork-tracking branches, and lets the sidebar see
the terminal state. Open/draft matches take precedence when a branch name has
been reused; historical matches are hidden on the repository default branch.
A successful empty result clears the association, while command/auth/network
errors preserve the last known value. The frontend renders sub-components for
each PR aspect. Auth status is checked before fetching. The panel updates when
workspace state changes.

## What Works Today

- PR creation from panel when no PR exists for the current branch (title, body, base branch, draft toggle)
- PR header with number, title, state (draft/open/merged/closed), source and target branches, addition/deletion stats, review decision badge, external link to GitHub
- Check status display: pass/fail summary with individual check names, status icons, and clickable detail links
- Review threads: existing review comments and inline review comments, rendered read-only
- Collapsible sections with item counts

Removed from this surface (backends retained, no UI caller):

- Review submission (approve / request changes / comment)
- Merge controls (squash / merge commit / rebase)
- Deployment info (environments, state badges, preview links)
- Conflict detection and the "Resolve with AI" entry point — see `docs/features/merge-resolver.md`, which is currently unreachable from the UI for this reason

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

- `src/components/workspace/review-panel.tsx` — main Review panel (fetching, polling, resting layout)
- `src/components/workspace/review/review-header.tsx` — PR metadata display
- `src/components/workspace/review/review-checks.tsx` — CI check status
- `src/components/workspace/review/review-threads.tsx` — review + inline review comments (read-only)
- `src/components/workspace/review/incoming-prs-view.tsx` — incoming PRs list
- `src/components/workspace/review/collapsible-section.tsx` — collapsible section wrapper with item counts
- `src-tauri/src/github.rs` — GitHub data fetching via gh CLI
- `src-tauri/src/commands/github.rs` — Tauri GitHub commands (incl. the still-registered, currently uncalled `merge_pull_request`)
