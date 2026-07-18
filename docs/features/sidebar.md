# Left Sidebar

- Purpose: Describe the left sidebar shell and its collapse-to-icon-rail behavior.
- Audience: Anyone working on sidebar layout, navigation, or workspace presentation.
- Authority: Canonical feature-level reality doc for the left sidebar.
- Update when: The sidebar layout, collapse model, or rail rendering changes.
- Read next: `docs/reference/SHORTCUTS.md`, `docs/features/notifications.md`

## What This Feature Is

The left sidebar is the primary navigation surface: top action row (New agent, Add
repository, Automations, Workspaces overview), the project-grouped workspace list
(per-workspace agent status, notifications, git/PR state), and a footer (app menu +
ports). It can be **collapsed to a narrow icon rail** instead of being hidden
entirely.

## Current Model

The sidebar uses the shadcn `Sidebar` primitive in **`collapsible="icon"`** mode.
There are exactly two states, toggled by the title-bar button and `Ctrl+B`:

- **Expanded** — full sidebar (resizable 180–400px, default 256px).
- **Icon rail** — a 52px (`SIDEBAR_WIDTH_ICON = 3.25rem`) vertical rail of icons.
  The rail is **always visible**; the sidebar never fully disappears.

State is the `sidebarOpen` boolean in `app-shell.tsx` fed into `SidebarProvider`
(`open ? "expanded" : "collapsed"`). Each sidebar section reads `useSidebar().state`
and renders a dedicated rail variant when `collapsed`, rather than relying on the
primitive's CSS-hide classes (the custom sections aren't built for raw clipping).

### Rail rendering

- **Action row** (`sidebar-action-row.tsx`): vertical icon buttons (New agent —
  accented; Add repository dropdown; Automations; Workspaces) with right-side
  tooltips.
- **Project list** (`sidebar-rail-projects.tsx`): one **project avatar** per project
  group. An aggregate **status dot / notification badge** overlays the avatar so
  agent activity stays visible while collapsed. Corner-indicator priority:
  needs-input (red) > notification count (amber) > working (amber pulse) >
  ready-for-review (green). The active project's avatar is highlighted.
- **Hover flyout**: hovering a project avatar opens a `HoverCard` (side="right")
  listing that project's workspaces with **live per-workspace status** (working
  spinner / status dot), notification counts, branch, active-row highlight, and a
  "+ New workspace" action. Each row is clickable to switch workspace — so the rail
  is fully operable without expanding. This answers "can I still see agents working
  and completion notifications when collapsed?" — yes.
- **Footer** (`sidebar-footer-bar.tsx`): app menu + ports popover stacked vertically.
- **Setup banner** (`sidebar-setup-banner.tsx`): hidden in the rail (it is a wide,
  text-heavy card with no place at 52px).

The expanded project group, the expanded workspace row, and the rail flyout share
`StatusIndicator` (which gained a `withTooltip` prop so the rail can render a bare
dot without a tooltip fighting the HoverCard), the `AsciiSpinner`
(`src/components/ui/ascii-spinner.tsx`), and the project avatar appearance loader
(`use-project-appearance.ts`).

## What Works Today

- Two-state toggle (expanded ↔ icon rail) via title-bar button and `Ctrl+B`.
- Rail shows nav icons, project avatars, and footer icons; never fully hides.
- Live aggregate agent status + notification badges on collapsed project avatars.
- Per-workspace agent status covers **both terminal and Agent Chat (Beta)
  agents** — chat sessions publish into the same `pane_statuses` snapshot
  via `set_pane_status_by_thread` (see `docs/features/agent-chat.md`
  § "Sidebar status indicators"), so a chat workspace shows the same
  working/needs-input/review indicator in every density mode.
- Per-project hover flyout with live per-workspace status, notifications, switch, and
  new-workspace action.
- Project avatar custom colors/images carry into the rail.
- Re-expanding restores the full sidebar (DnD reorder, diff stats, etc.) unchanged.
- **Workspace-row density is state-driven** (the "living sidebar"): each row
  derives its height from agent + git state instead of a global setting (the
  old `sidebar.workspace_detail` Clean/Branch/Detailed control was removed).
  Working → 3-line card (working indicator + work title + issue chip /
  activity + elapsed / mono git line with branch, ↑↓, +/−); needs-input →
  2-line red-tinted card with the blocker text; done (review) → 2-line
  green-tinted card ("Done · review when ready") that collapses once the
  workspace is opened or after ~1h; idle → one-liner (a dirty worktree keeps
  its mono git line). A green ✓ on a settled row fades out over ~1h; a
  `n shipped` mono tally (with history popover) appears on rows whose merged
  PRs retired after new work started. Timestamps for elapsed/decay live in the
  non-persisted `sidebar-density-store`. The full labeled detail for the
  **active** workspace still lives in the **workspace context bar**
  (`docs/features/workspace-context-bar.md`).
- **Work-based row naming**: while a workspace's agent is live and a
  linked issue exists, the row title is the issue title (+ `#n` chip); the
  worktree/branch name moves to the mono git line. Idle rows keep the
  worktree name.
- **Pinned "Needs you" strip** at the top of the workspace tree while ≥1
  workspace needs input (red-tinted, project chip + blocker + age); entries
  are jump-links that activate the workspace, expanding its project group if
  collapsed. Hidden in "Gather on top" mode.
- **Live agents grouping** (Settings → Appearance → Agents,
  `sidebar.live_agents`): "Stay in project" (default) keeps today's grouping;
  "Gather on top" hoists all live rows (needs-input → working → done-unseen)
  into a `LIVE` section above the project tree, each tagged with a project
  chip, leaving idle one-liners in their groups.
- **Configurable working indicator** (`sidebar.working_indicator`:
  braille / ring / blink / sweep / typing, and
  `sidebar.working_indicator_color`: amber / white / ember / green / sky /
  violet — no red, reserved for needs-input). Rendered by
  `src/components/ui/working-indicator.tsx` in the expanded rows and the rail
  flyout; picked via the Agents section's tile picker + swatches.
- **Duplicate project names are disambiguated** (PR #109): two project roots that
  share a basename (a local `~/projects/app` and the same app on a remote host, or
  sibling dirs) no longer collapse to identical labels. The host is preferred as the
  distinguishing tag — the local copy keeps its clean basename, each remote copy is
  suffixed with ` · <host>`; when the host can't separate them (both local, same
  host, or host names unavailable) the label grows its trailing path tail until every
  label is unique. Wired through the sidebar, the project picker, and the automations
  project dropdown.

## Current Constraints

- The collapsed/expanded choice is **not persisted across app restarts** (the app
  boots expanded), matching the pre-existing offcanvas behavior. Persisting it is a
  possible follow-up.
- The rail is desktop-only; on the mobile breakpoint the sidebar still uses the
  off-canvas `Sheet`.
- Aggregate status shows a single corner indicator by priority; the full per-state
  detail lives in the flyout.
- Drag-and-drop reordering is an expanded-only affordance.

## Important Touch Points

- `src/components/layout/app-sidebar.tsx` — `collapsible="icon"`, rail overflow override
- `src/components/layout/sidebar-action-row.tsx` — collapsed action rail
- `src/components/layout/sidebar-rail-projects.tsx` — collapsed project rail + flyout
- `src/components/layout/sidebar-workspace-list.tsx` — branches to the rail when collapsed
- `src/components/layout/sidebar-footer-bar.tsx` — collapsed footer
- `src/components/layout/sidebar-setup-banner.tsx` — hidden in rail
- `src/components/layout/use-project-appearance.ts` — shared avatar appearance loader
- `src/components/ui/sidebar.tsx` — `SIDEBAR_WIDTH_ICON` (52px rail)
- `src/components/ui/status-indicator.tsx` — `withTooltip` prop
- `src/components/ui/ascii-spinner.tsx` — braille frames (color-configurable)
- `src/components/ui/working-indicator.tsx` — configurable working indicator
- `src/components/layout/sidebar-workspace-row.tsx` — state-driven row density
- `src/components/layout/sidebar-needs-you-strip.tsx` — pinned "Needs you" strip
- `src/components/layout/sidebar-live-section.tsx` + `sidebar-live-grouping.ts` — "Gather on top" LIVE section
- `src/stores/sidebar-density-store.ts` — non-persisted elapsed/decay/work-history state
- `src/lib/pane-status.ts` — `getProjectStatus` aggregate helper
- `src/lib/path.ts` — `tailSegments` / `segmentCount` project-label helpers
- `src/stores/app-store.ts` — duplicate-project-label disambiguation (host tag → path tail)

## Notes

- Keep this file about current truth, not future plans.
- Adapted from the Superset icon-rail sidebar; Codemux maps Superset's per-workspace
  rail items onto project avatars + a hover flyout, which fits Codemux's
  project-grouped model better than a flat workspace rail.
