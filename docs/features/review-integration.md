# Review Integration

- Purpose: Describe the current capability and constraints of the Review tab (pull request review surface).
- Audience: Anyone working on GitHub integration or the Review panel UI.
- Authority: Canonical feature-level reality doc.
- Update when: PR display, review, merge, or checks behavior changes.
- Read next: `docs/features/changes-panel.md`, `docs/features/merge-resolver.md`

## What This Feature Is

The Review tab is a pane in the right-panel deck that displays pull request details, review threads, and CI checks for the current workspace branch. It integrates with GitHub via the `gh` CLI tool.

As a deck pane it is closable like any other and reopens from the `+` menu. Its tab badge (comment count + a check rollup glyph) is unchanged; the status foot reads `PR #N · <state>`, or `no pull request`. The panel deliberately has no chrome of its own — it never had a refresh button (auto-poll owns freshness), so it contributes nothing to the deck's tab-row action slot and that slot renders empty while Review is active.

> The tab was previously named "PR." It was renamed to "Review" in Phase 3 of the right-sidebar work to match Superset's terminology (`pr-panel.tsx` → `review-panel.tsx`, `pr/` subfolder → `review/`).
>
> A later pass **stripped the panel to Superset's resting layout** — title + status badge + checks + comments only. The review composer (Approve / Request changes / Comment), the merge controls, and the deployments section were intentionally removed (see the comment in `review-panel.tsx`). Their Rust backends are retained for a potential future re-wire, so `merge_pull_request` and the review-submission commands still exist and are registered — they simply have no caller in `src/components/`.

## Current Model

PR data is fetched from GitHub through `gh` CLI commands routed via Rust
(`src-tauri/src/github.rs`). The current workspace association uses an explicit
branch query (`gh pr list --head <branch> --state all`) rather than implicit
`gh pr view` inference or exact commit-SHA equality. This keeps a merged/closed
PR attached when review commits advanced its remote head beyond the local
worktree and lets the sidebar see the terminal state. The head argument is
always the bare branch name — gh does not match the `owner:branch` form here —
so a fork-tracking branch is disambiguated client-side by the row's
`headRepositoryOwner`. Open/draft matches take precedence when a branch name has
been reused; historical matches are hidden on the repository default branch.
A successful empty result clears the association, while an unanswerable lookup
(command/auth/network error, or a detached HEAD) preserves the last known value.
The frontend renders sub-components for each PR aspect. Auth status is checked
before fetching. The panel updates when workspace state changes.

**The Review tab is strictly current-branch, deliberately.** The sidebar shows
a PR badge for a *side branch* the worktree merely checked out recently (the
agent-opened-a-PR-from-a-side-branch case — see `docs/features/sidebar.md`
§ "Side-branch PR badges"), and the workspace snapshot records that PR's head
branch in `pr_head_branch`. This panel gates `hasPr` on that head branch
matching `git_branch`, and its detail queries still call the strict
`get_branch_pull_request` command. A badge is a pointer at GitHub; this panel
is a working surface, so honouring a side-branch association here would review
a branch the user is not on *and* hide the "Create PR" affordance for the
branch they are. The Review tab's own badge needs no separate guard — it only
renders from cached PR query data, which a disabled panel never populates.

The guard is a *mismatch* test, not an equality test: when either name is
unknown it holds the association. `git_branch_info` reports no branch on a
detached HEAD — mid-rebase, mid-bisect, `gh pr checkout` of a SHA — while
`pr_head_branch` survives, and treating that unknown as "different branch"
would offer "Create PR" for a workspace that already has an open one, for as
long as the rebase ran.

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
