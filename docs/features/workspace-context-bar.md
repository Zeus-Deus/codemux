# Workspace Context Bar

- Purpose: Describe the bottom status bar that carries the active workspace's git/PR/issue detail.
- Audience: Anyone working on the app shell, sidebar density, or git/PR status surfaces.
- Authority: Canonical feature-level reality doc for the workspace context bar.
- Update when: The bar's contents, visibility gates, or placement change.
- Read next: `docs/features/sidebar.md`, `docs/features/settings.md`

## What This Feature Is

A passive, read-only 42px status strip pinned under the work surface (inside
`SidebarInset`, after `WorkspaceMain`). It is the "one home for the details"
half of the sidebar-density split: the sidebar keeps glanceable identity
(name + branch + status), while the context bar holds the full labeled git
detail for the **active** workspace — in every sidebar appearance mode
(`sidebar.workspace_detail`: clean / branch / detailed), so nothing is lost
when the sidebar defaults to Clean.

## Current Model

Left → right:

- **Branch** — git-branch icon + `git_branch` (mono, truncates).
- **Kind** — folder icon + `worktree` / `repo root` (from `workspace_kind`,
  falling back to `worktree_path` presence).
- **Counters** (mono, only non-zero values render): `↓behind` (warning),
  `↑ahead` (success), `+additions` (success), `−deletions` (danger),
  `N file(s)` (muted).
- **PR chip** (right-aligned) — tone-tinted bordered button
  (`PR #19 · Open`), colored by PR state (open green / merged violet /
  closed red / draft muted); click opens `pr_url` via the opener plugin;
  disabled without a URL.
- **Issue chip** — `IssueDetailPopover` in its `chip` variant
  (`Issue #40` with state dot); the detail popover opens upward
  (`side="top"`). Shown while `workspace.linked_issue` is set, and part
  of the bar's "something to show" visibility set. The same chip (same
  props) also lives in the Context Row's `WorkspaceStatusCluster`, so
  when the bar hides for an active agent-chat pane the linked issue
  stays visible there (see `docs/features/agent-chat.md` § "Context
  Row"); its details popover additionally carries an Issue row.
- **Device** — laptop icon + "This device", or cloud icon + host name
  (resolved from `useHosts()` by `host_id`) for remote workspaces.
- **Background browser indicator** (GUI mode only — `docs/features/browser.md`
  § "Background browser in GUI mode") — a sky-tinted pill with a blinking
  amber dot, shown while the active workspace has a live, pane-less
  `agent_browser_sessions` entry (an agent opened a browser and the Agent
  Chat GUI Beta kept it detached instead of splitting a pane). Click opens
  the floating peek overlay (`BrowserPeekOverlay`). Gated on `enableAgentChat
  && workspace.workspace_type !== "open_flow"`, mirroring `useGuiChrome()`.
  The pill + session lookup now live in the shared
  `BackgroundBrowserIndicator` / `useBackgroundBrowserSession` pair
  (`src/components/browser/background-browser-indicator.tsx`), consumed
  by both this bar and the Context Row's `WorkspaceStatusCluster` —
  when the bar hides for an active agent-chat pane (below), the same
  indicator renders in the Context Row instead, so the peek toggle is
  never lost.

The sidebar's expanded footer row (`sidebar-footer-bar.tsx`) is
height-matched to the bar (42px, border-top instead of a separator +
padding) so the two top borders land on the same pixel row — one
continuous line across the bottom of the app.

All data comes off the active `WorkspaceSnapshot` (`useActiveWorkspace()`);
the Rust 5s git poll and PR/issue pollers keep it fresh — the bar adds no
new data plumbing.

**Visibility** — the bar renders `null` when there is nothing to report:

- no active workspace;
- a lazy-creation chat draft is active (unscoped new chat);
- the onboarding wizard covers the active workspace;
- the workspace has no git branch, no PR, no linked issue, and no live
  background browser session (e.g. a home-directory workspace);
- **GUI chrome + an active Agent Chat pane** — `useAgentChatPaneActive()`
  (`src/hooks/use-gui-chrome.ts`) is true when GUI chrome is active AND
  the active pane of the active workspace's active surface is an
  `agent_chat` pane. In that case the pane's own **Context Row**, under
  its composer, now carries this same git/PR detail — and the
  background-browser indicator — inline (see
  `docs/features/agent-chat.md` § "Context Row (running-thread
  status)") — showing both would duplicate the same numbers twice on
  screen, so the permanent strip steps aside. A terminal (or other)
  pane active in GUI mode keeps the bar; legacy chrome (Beta flag off)
  is untouched, since `useGuiChrome()` — which the predicate is built
  on — always resolves `false` there.

## What Works Today

- Live branch / kind / ahead-behind / diff / changed-file readout for the
  active workspace, updating with the backend git poll.
- Clickable PR chip (opens the PR on GitHub) with per-state tone.
- Clickable issue chip opening the full issue-detail popover upward.
- Local vs. remote device label with host-name resolution.
- Fully exercisable in the browser dev runtime — the mock seeds git/PR/issue
  data and implements `get_github_issue` so the popover renders content.
- Non-git project folders: instead of rendering nothing, the bar shows
  "Not a git repository" + an explicit "Initialize Git" button (bare
  `git init` on click, then an immediate git-info refresh). Gated by
  `showNoGitState` — local standard workspaces with `is_git === false`
  only, so Home and host-backed workspaces stay hidden as before. See
  `docs/features/workspace-creation.md` § "Non-Git Projects".

## Current Constraints

- Read-only by design: no commit/push/pull actions live here.
- Shows in all sidebar appearance modes (not gated on
  `sidebar.workspace_detail`), so Detailed mode intentionally duplicates the
  numbers between sidebar row and bar.
- Spans only the content area (right of the sidebar), matching the design.

## Important Touch Points

- `src/components/layout/workspace-context-bar.tsx` — the bar
- `src/components/layout/app-shell.tsx` — mounted after `WorkspaceMain` in `SidebarInset`
- `src/hooks/use-gui-chrome.ts` — `useAgentChatPaneActive()`, the bar's hide-for-Context-Row gate
- `src/components/github/pr-status-icon.tsx` — PR state icon + humanized label + `PR_CHIP_TONE` (shared with the Context Row's status cluster)
- `src/components/github/issue-detail-popover.tsx` — `variant="chip"`, `side`/`align` props
- `src/stores/app-store.ts` — `useActiveWorkspace()`
- `src/components/browser/background-browser-indicator.tsx` — shared `BackgroundBrowserIndicator` pill + `useBackgroundBrowserSession` lookup (bar + Context Row cluster)
- `src/stores/hosts-store.ts` — `useHosts()` host-name lookup
- `src/stores/browser-peek-store.ts` — the indicator's click target (opens the peek)
- `src/dev/tauri-mock.ts` — `get_github_issue` mock for the popover
- `src/components/layout/workspace-context-bar.test.tsx` — unit tests
- `src/components/chat/WorkspaceStatusCluster.tsx` — the Context Row's relocated version of this bar's git/PR/issue detail + browser indicator (PR chip, linked-issue chip, background-browser indicator, details popover; see `docs/features/agent-chat.md`)

## Notes

- Keep this file about current truth, not future plans.
- Adapted from the "workspace context bar" in the CodeMux UI-refresh design
  handoff (main workspace screen): passive status under the terminal / live
  chat, hidden when there is nothing to report.
- The Context Row redesign (`.design-import/Context Row.dc.html`) relocated
  this bar's content into the Agent Chat composer's scope row instead of
  deleting anything — this bar still owns the detail for every non-agent-chat
  active pane (terminal, browser, …) and for legacy chrome.
