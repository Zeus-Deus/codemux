# Retired Workspace Context Bar

- Purpose: Record why the full-width bottom status strip no longer renders and where its useful controls moved.
- Audience: Anyone working on the app shell, terminal chrome, sidebar detail, or background browser surfaces.
- Authority: Canonical feature-level reality for the retired workspace context bar.
- Update when: Workspace status placement or the background-browser launcher changes.
- Read next: `docs/features/sidebar.md`, `docs/features/browser.md`, `docs/features/agent-chat.md`

## Current Behavior

Codemux no longer mounts a full-width status bar below `WorkspaceMain`.
Terminals, browser panes, editors, and diff views use the entire available
work-surface height. `src/components/layout/app-shell.tsx` mounts
`WorkspaceMain` directly beside the floating `BrowserPeekOverlay`; the old
`WorkspaceContextBar` component and its tests were deleted.

The bar had duplicated branch, worktree, ahead/behind, diff, PR, issue, and
device detail that is already available from the workspace inbox and its
shared hover-details card. Removing it avoids a permanent 42px tax on every
terminal for information that is not terminal-local.

## Relocated Surfaces

- **Terminal background browser** — the active terminal pane header shows a
  compact, always-visible `Browser` control while the workspace has a live,
  detached `agent_browser_sessions` entry. Clicking it opens the existing
  floating `BrowserPeekOverlay`; attaching that session to a pane removes the
  control. The header already exists, so this reserves no additional height
  and does not cover terminal output.
- **Agent Chat** — the Context Row below the composer keeps
  `WorkspaceStatusCluster`: background browser, behind, PR, issue,
  Initialize Git, and workspace-details affordances remain beside the chat
  input where they are relevant.
- **Workspace detail** — the sidebar workspace row and
  `WorkspaceHoverCard` remain the cross-surface home for branch, diff, PR,
  issue, and device information.
- **Sidebar footer** — Automations, Workspaces, Ports, and Menu remain in the
  sidebar's own 42px footer. That footer no longer aligns with or dictates a
  corresponding work-surface row.

## Visibility Contract

The terminal-header browser control renders only when all are true:

- GUI chrome applies to the active workspace;
- the terminal is the active pane;
- a workspace browser session is active and has no `pane_id`.

The detached-browser session lookup remains shared with Agent Chat through
`useBackgroundBrowserSession`, so the two launch surfaces cannot drift.

## Important Touch Points

- `src/components/layout/app-shell.tsx` — no bottom status-strip mount
- `src/components/layout/PaneNode.tsx` — terminal pane-header placement
- `src/components/browser/background-browser-indicator.tsx` — shared session lookup, chip variants, and terminal-header gate
- `src/components/browser/BrowserPeekOverlay.tsx` — unchanged floating preview and promote-to-pane action
- `src/components/chat/WorkspaceStatusCluster.tsx` — Agent Chat status placement
- `src/components/layout/workspace-hover-card.tsx` — sidebar detail surface
- `src/components/layout/sidebar-footer-bar.tsx` — independent sidebar-only footer
