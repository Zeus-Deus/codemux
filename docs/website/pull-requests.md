---
title: Pull Requests
description: Create, review, and merge pull requests with CI checks, review comments, and deployment previews.
---

# Pull Requests

The PR tab in the right panel lets you create, review, and merge pull requests without leaving Codemux. All operations use the `gh` CLI under the hood.

## Requirements

- **GitHub CLI** (`gh`) must be installed and authenticated (`gh auth login`)
- The workspace must be a GitHub repository

## Creating a PR

When no PR exists for the current branch:

1. Click "Create Pull Request"
2. Title auto-fills from the branch name (e.g., `feature/auth` becomes "Auth")
3. Add an optional description
4. Select the base branch (defaults to `main`)
5. Check "Draft" to create as a draft PR
6. Click "Create PR"

## PR Info Header

When a PR exists, the header shows:

- **State badge** — Open (green), Merged (purple), Closed (red), Draft (gray)
- **PR number** — e.g., #42
- **Title**
- **Branches** — `feature-branch` → `main` with monospace styling
- **Stats** — `+150 -23` additions/deletions
- **Review decision** — Approved, Changes Requested, or Review Pending
- **Last updated** — Relative timestamp (e.g., "2h ago")
- **View on GitHub** link and **Copy URL** button

## CI Checks

The Checks section shows GitHub Actions status:

- Summary line: "3/4 checks passed" (green if all pass, yellow if pending, red if failures)
- Each check shows: status icon, name (clickable link to details), elapsed time
- Collapsible section

## Review Comments

The Reviews section groups comments by reviewer:

- Author avatar (initial circle), name, review state icon, timestamp
- Review body text
- Inline code comments grouped under their parent review with file:line references
- Copy button on each comment for sharing with agents

## Review Actions

Submit reviews directly from Codemux:

- **Approve** — Green button with shield icon
- **Request Changes** — Red button
- **Comment** — Neutral button (requires body text)

Each action calls `gh pr review` via the backend.

## Deployment Previews

If the PR has deployment environments (Vercel, Netlify, etc.), a Deployments section shows:

- Environment name and state badge
- "Preview" button to open the deployment URL

## Merge Controls

At the bottom of the PR tab:

- **Merge method selector** — Squash and merge, Create merge commit, or Rebase and merge
- **Merge button** — Click once to arm, click again to confirm (5-second timeout)
- **Conflict warning** — When the PR has merge conflicts, the button is disabled and a "Check Locally" button lets you probe for specific conflicting files

## Data Fetching

All PR data (info, checks, reviews, inline comments, deployments) is fetched in parallel via `Promise.all` on tab open. Use the refresh button to fetch updated data. No auto-polling.
