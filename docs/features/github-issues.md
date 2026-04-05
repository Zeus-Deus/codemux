# GitHub Issue Integration

- Purpose: Describe the issue linking, display, and branch-naming subsystem.
- Audience: Anyone working on GitHub integration or workspace-issue workflows.
- Authority: Canonical feature doc for GitHub issue support.
- Update when: Issue linking behavior, UI surfaces, or CLI commands change.
- Read next: `docs/features/pr-integration.md`, `docs/reference/CONTROL.md`

## What This Feature Is

Workspaces can be linked to GitHub issues. Once linked, the issue appears in the sidebar, provides context to agents, and drives branch naming.

## Current Model

Issues are fetched via the `gh` CLI routed through Rust. A workspace holds at most one linked issue at a time. Linking is optional and can happen during workspace creation or later via the sidebar.

## What Works Today

- list and search open issues from the linked GitHub repo
- link an issue to a workspace (issue picker dialog during creation or from sidebar)
- sidebar display with issue number, title, labels, and state badge
- detail popover on hover/click showing full issue body
- auto-suggest branch names from issue number and title (e.g. `42-fix-login-bug`)
- prompt auto-injection: linked issue context is included in `$CODEMUX_AGENT_CONTEXT` so agents have issue awareness
- unlink an issue from a workspace
- refresh issue data from GitHub
- CLI: `codemux issue list`, `codemux issue view <number>`, `codemux issue link <number>`
- control socket commands for issue operations

## Current Constraints

- requires `gh` CLI installed and authenticated
- one issue per workspace (no multi-issue linking)
- issue state is fetched on demand, not continuously polled
- no issue creation from within Codemux
- no cross-repo issue support (scoped to workspace git remote)

## Important Touch Points

- `src-tauri/src/github.rs` — issue fetching via `gh`
- `src-tauri/src/commands/github.rs` — Tauri commands: `list_github_issues`, `get_github_issue`, `link_workspace_issue`, `unlink_workspace_issue`, `refresh_workspace_issue`, `suggest_issue_branch_name`
- `src-tauri/src/cli.rs` — CLI `issue` subcommands
- `src-tauri/src/control.rs` — socket commands for issue operations
- `src/components/github/issue-picker.tsx` — issue selection overlay
- `src/components/github/issue-detail-popover.tsx` — sidebar hover/click detail
- `src-tauri/src/agent_context.rs` — injects linked issue into agent prompts
