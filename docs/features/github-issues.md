# GitHub Issue Integration

- Purpose: Describe the issue linking, display, and branch-naming subsystem.
- Audience: Anyone working on GitHub integration or workspace-issue workflows.
- Authority: Canonical feature doc for GitHub issue support.
- Update when: Issue linking behavior, UI surfaces, or CLI commands change.
- Read next: `docs/features/review-integration.md`, `docs/reference/CONTROL.md`

## What This Feature Is

Workspaces can be linked to GitHub issues. Once linked, the issue appears as a chip in the sidebar and drives branch naming. It does **not** feed `$CODEMUX_AGENT_CONTEXT` — see below.

## Current Model

Issues are fetched through the `git_provider` registry — the checkout's detected hosting product is served by its own CLI (`gh` for GitHub, `glab` for GitLab), normalized onto the same `GitHubIssue` struct, so the surfaces below are unchanged. Picker and popover copy names the detected product ("Connect GitLab to link issues", "Open on GitLab"); issue references stay `#N` on every product. A workspace holds at most one linked issue at a time. Linking is optional and happens **during workspace creation only** — `linkWorkspaceIssue` is called solely from `new-workspace-dialog.tsx`. There is no sidebar or context-menu link action.

## What Works Today

- list and search open issues from the linked GitHub repo
- link an issue to a workspace (issue picker dialog during workspace creation)
- sidebar chip shows a state-colored dot plus `#N` only; title, labels, and assignees render inside the popover
- detail popover on click (a Radix `Popover` with a button trigger; there is no hover trigger) showing full issue body
- auto-suggest branch names from issue number and title (e.g. `42-fix-login-bug`)
- **no** prompt auto-injection — `build_agent_context()` takes only `(workspace_name, worktree_path, branch, root_path)` and never sees the linked issue, so `$CODEMUX_AGENT_CONTEXT` carries no issue data. Issue context reaches agents only if the user includes it in a message
- unlink an issue from a workspace
- refresh issue data from GitHub
- CLI: `codemux issue list`, `codemux issue view <number>`, `codemux issue link <number>`
- control socket commands for issue operations

## Current Constraints

- requires the detected product's CLI installed and authenticated (`gh` / `glab`); Bitbucket and Azure DevOps are recognised but not served — see `docs/features/source-control-providers.md`
- one issue per workspace (no multi-issue linking)
- issue state is fetched on demand, not continuously polled
- no issue creation from within Codemux
- no cross-repo issue support (scoped to workspace git remote)

## Unwired commands

Two commands are implemented, registered, and have TS wrappers but **zero
callers** anywhere in `src/`, the control socket, the CLI, or MCP:
`unlink_workspace_issue` and `refresh_workspace_issue`. There is no "unlink
issue" or "refresh issue" affordance in the UI; the popover only refetches when
reopened. Wiring either one is a small change if the affordance is wanted.

## Important Touch Points

- `src-tauri/src/git_provider/` — provider detection + adapters the issue commands route through
- `src-tauri/src/github.rs` — issue fetching via `gh` (behind the GitHub adapter)
- `src-tauri/src/commands/github.rs` — Tauri commands: `list_github_issues`, `get_github_issue`, `link_workspace_issue`, `unlink_workspace_issue`, `refresh_workspace_issue`, `suggest_issue_branch_name`
- `src-tauri/src/cli.rs` — CLI `issue` subcommands
- `src-tauri/src/control.rs` — socket commands for issue operations
- `src/components/github/issue-picker.tsx` — issue selection overlay
- `src/components/github/issue-detail-popover.tsx` — click-triggered detail popover (no hover trigger)
- `src-tauri/src/agent_context.rs` — builds `$CODEMUX_AGENT_CONTEXT`; **does not** include issue data
