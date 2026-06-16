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
- Per-project hover flyout with live per-workspace status, notifications, switch, and
  new-workspace action.
- Project avatar custom colors/images carry into the rail.
- Re-expanding restores the full sidebar (DnD reorder, diff stats, etc.) unchanged.

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
- `src/components/ui/ascii-spinner.tsx` — shared working spinner
- `src/lib/pane-status.ts` — `getProjectStatus` aggregate helper

## Notes

- Keep this file about current truth, not future plans.
- Adapted from the Superset icon-rail sidebar; Codemux maps Superset's per-workspace
  rail items onto project avatars + a hover flyout, which fits Codemux's
  project-grouped model better than a flat workspace rail.
