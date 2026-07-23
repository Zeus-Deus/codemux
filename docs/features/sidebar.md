# Left Sidebar

- Purpose: Describe the left sidebar shell — the flat workspace inbox (expanded) and the collapse-to-icon-rail behavior.
- Audience: Anyone working on sidebar layout, navigation, or workspace presentation.
- Authority: Canonical feature-level reality doc for the left sidebar.
- Update when: The sidebar layout, inbox model, collapse model, or rail rendering changes.
- Read next: `docs/reference/SHORTCUTS.md`, `docs/features/notifications.md`

## What This Feature Is

The left sidebar is the primary navigation surface. Expanded, it is a **flat
workspace inbox**: a search affordance + new-agent button, a horizontal repo
filter-chip row, one multi-line card per active workspace, and a "Settled"
section of swept-aside one-line rows. Collapsed, it is a narrow icon rail.

## Current Model

The sidebar uses the shadcn `Sidebar` primitive in **`collapsible="icon"`** mode.
There are exactly two states, toggled by the title-bar button and `Ctrl+B`:

- **Expanded** — the workspace inbox (resizable 180–400px, default **288px** —
  widened from 256 so the card meta line fits).
- **Icon rail** — a 52px (`SIDEBAR_WIDTH_ICON = 3.25rem`) vertical rail of icons.
  The rail is **always visible**; the sidebar never fully disappears.

### The workspace inbox (expanded)

Replaced the nested project tree (project groups, drag-reorder, the pinned
"Needs you" strip, and the "Gather on top" LIVE section) with one flat list:

- **Header** (`sidebar-action-row.tsx` expanded variant): a search box-shaped
  button that opens the command palette (shows the resolved `⌘K` keybind) and
  an ember-accented new-agent button (same click/shift-click semantics as
  before). Add repository moved into the chip row; Automations and Workspaces
  moved into the footer app menu.
- **Repo filter chips** (`sidebar-inbox.tsx`): `All` · one chip per project
  (mini avatar + dedup'd project name from `useProjectGroupedWorkspaces`) ·
  a dashed `+` add-repo chip (Open project / New project dropdown). The active
  chip is ember-tinted. Chips filter **both** the active cards and the settled
  rows; a filtered-empty list shows "Nothing active in `<repo>`". The filter is
  session-only. The chip row is sticky above the scrolling list.
- **Workspace cards** (`sidebar-inbox-card.tsx`): each active workspace is a
  card — repo avatar + name eyebrow; work title (linked-issue title while an
  agent is live, worktree name when idle) + issue chip; a red blocker line
  (needs-you only); and a mono meta line (branch · `↑ahead` · `+/−` diff · PR
  chip (`PR #n` green / `merged` violet, opens the PR) · remote cloud icon ·
  notification badge). The right side of the eyebrow shows the agent state —
  Working (configurable `WorkingIndicator`, amber text) / Needs you (pulsing
  red dot) / Done · review (green ✓) / elapsed time for idle — and swaps to a
  **"✓ Settle"** button on hover or focus (CSS-only swap). The selected card
  gets an ember-tinted border + slightly lighter background; needs-you cards a
  red-tinted border. Click activates; the right-click context menu is the same
  `WorkspaceContextMenuItems` (rename, editors, move-to-host, archive, delete)
  shared with the old row, including the delete/push-confirm dialogs.
- **Settle / un-settle** (`sidebar-inbox-store.ts`): Settle collapses the card
  (~200ms height/opacity), then moves it below a "Settled" divider as a compact
  one-line row (repo avatar · violet merge icon when its PR merged · title ·
  elapsed-since-settle). Hover/focus reveals **Un-settle**, which reverses it
  (the returning card eases back in via the shared `rise-in` keyframe).
  Settling is **visual only** — nothing is archived, closed, or deleted. The
  settled list persists via UI-state key `sidebar.inbox.settled`, pruned when
  a workspace vanishes. The settled list is **flat and recency-ordered** —
  repo identity is carried by each row's avatar, not by project grouping.
  Both row shapes share the full workspace right-click menu via
  `workspace-inbox-menu.tsx`: cards get a "Settle workspace" entry (guardrail
  permitting), settled rows get "Un-settle workspace" on top, and both keep
  rename / archive / delete / move-to-host.
- **Settle safety net**: live work can never be buried. A card whose agent is
  working or blocked ("needs you") offers no Settle button (its state cluster
  stays visible on hover), and a *settled* workspace whose agent becomes
  working/blocked is **auto-un-settled** (persistently, with the rise-in
  ease). Finished ("review") and idle cards settle normally and stay settled —
  sweeping completed work aside is the point of the gesture.
- **Auto-settle** — the Settled section fills itself. A fully idle card
  (status null — never working/blocked/review) auto-settles when its PR is
  **merged or closed**, or after **N days without agent activity**
  (Settings → Appearance → Sidebar → "Auto-settle idle work":
  Off / 1d / 3d / 7d / 14d, `sidebar.auto_settle_days`, default 3d).
  **Un-settling sets a keep-active pin** that suppresses auto-settle until
  the agent runs again (explicit settle or new activity clears it). Activity
  is stamped client-side into the persisted blob (60s write-throttle;
  first-seen baseline so a fresh install never mass-settles). The persisted
  UI-state value is now `{settled, keepActive, activity}` with transparent
  migration from the older bare-array shape. Anti-oscillation invariants:
  auto-settle fires only at idle, auto-un-settle only at working/blocked,
  the pin gates the middle.
- **Settled-tail pagination**: 10 settled rows render initially; a quiet mono
  "Show N more (X hidden)" button appends 25 per click. Paging resets when
  the repo filter changes.
- **Keyboard jumps**: `Alt+1`–`Alt+9` (rebindable `workspaceJump1..9`
  registry actions) activate the Nth visible active card — filter-scoped,
  settled rows excluded. Holding Alt overlays index badges on the first nine
  cards (`sidebar-inbox-jump.ts` holds the visual-order targets for the
  central keyboard handler). `Ctrl+1..9` remain terminal-tab switching.
- **Provider marks**: each card's meta line shows the official logo of every
  agent-chat provider active in that workspace (Claude / Codex / OpenCode) via
  `ProviderLogo` + `getWorkspaceProviders` (pane-status). Terminal-only agent
  panes carry no provider metadata and contribute nothing.
- **Status derivation**: agent state comes from `getWorkspaceStatus`
  (pane-status) — covering terminal and Agent Chat panes alike; elapsed labels
  come from the non-persisted `sidebar-density-store` status observations.
- **Settings** (Settings → Appearance → Sidebar): **Show git stats**
  (`sidebar.show_git_stats`, default on) hides the `↑ahead` and `+/−` numbers
  on cards when off; the branch name always shows. The `sidebar.live_agents`
  grouping setting was removed with the tree; the working-indicator settings
  (`sidebar.working_indicator`, `sidebar.working_indicator_color`) remain and
  drive the card's working glyph.

### Footer nav (fixed chrome, both states)

`sidebar-footer-bar.tsx` is a slim app-destination row — never inside the
scrolling list. Expanded: **Automations** and **Workspaces** as equal-width
labeled ghost buttons (28px, 7px radius, transparent with a subtle hover
fill), then icon-only **Ports** (`SidebarPortsPopover`, keeps its count
badge) and the **Settings gear** (the app-menu dropdown — Settings, command
palette, shortcuts, docs, version, sign out; its old Automations/Workspaces
items were removed since they're visible buttons now). Collapsed: the same
four, restacked vertically in the same order with right-side tooltips.

### Rail rendering (collapsed — 52px workspace strip)

Replaced the old project-avatar rail (aggregate dots + hover flyout,
`sidebar-rail-projects.tsx`, deleted):

- **Header** (`sidebar-action-row.tsx` collapsed variant): the accented
  new-agent square + a search icon (opens the command palette), then a slim
  centered divider. Add repository lives only in the expanded chip row.
- **Workspace strip** (`sidebar-rail-workspaces.tsx`): one 28px button per
  **active (unsettled) workspace** — repo avatar with that workspace's own
  status dot (red pulse = needs you, amber = working, green = done-review,
  none = idle), right-side tooltip = workspace title. Clicking selects the
  workspace **without expanding**; the selected button gets the ember border
  + tint. The strip scrolls (scrollbar hidden); settled workspaces never
  appear; the repo filter does not apply here.
- **Footer**: Automations, Workspaces, Ports (badge), Settings — same order
  as the expanded footer row.
- **Setup banner** (`sidebar-setup-banner.tsx`): hidden in the rail.

## What Works Today

- Two-state toggle (expanded inbox ↔ icon rail) via title-bar button and `Ctrl+B`.
- Flat inbox cards with live agent state, blocker lines, git/PR/issue/remote/
  notification detail, and work-based titling while an agent is live.
- Repo chips filtering active + settled lists; add-repo chip; sticky chip row.
- Settle/un-settle with ~200ms motion, persisted across restarts, prune-safe.
- Search affordance opening the command palette; accented new-agent button.
- Show git stats toggle (Settings → Appearance → Sidebar).
- Rail: per-active-workspace avatar buttons with individual status dots,
  select-without-expand, and the shared footer destinations.
- Per-workspace agent status covers both terminal and Agent Chat (Beta) agents
  (chat sessions publish into the same `pane_statuses` snapshot).

## Current Constraints

- The collapsed/expanded choice is **not persisted across app restarts** (the app
  boots expanded), matching the pre-existing behavior.
- The rail is desktop-only; on the mobile breakpoint the sidebar still uses the
  off-canvas `Sheet`.
- Drag-and-drop workspace/project reordering was an affordance of the removed
  project tree and does not exist in the inbox (cards keep the stored
  workspace order).
- Idle cards show elapsed time only when a status transition was observed this
  session (no backend status timestamps exist).
- The superseded tree components (`sidebar-project-group.tsx`,
  `sidebar-needs-you-strip.tsx`, `sidebar-live-section.tsx`,
  `sidebar-live-grouping.ts`, and the `SidebarWorkspaceRow` component) are still
  in the repo but **unmounted** — kept because `sidebar-workspace-row.tsx` still
  exports the shared `WorkspaceContextMenuItems`/`DeleteWorktreeDialog` the
  inbox card reuses (and their test suites cover that machinery). Removing the
  dead remainder is a pending cleanup.

## Important Touch Points

- `src/components/layout/app-sidebar.tsx` — `collapsible="icon"`, rail overflow override
- `src/components/layout/sidebar-workspace-list.tsx` — expanded → inbox, collapsed → rail
- `src/components/layout/sidebar-inbox.tsx` — chips, card list, settled section, settle motion
- `src/components/layout/sidebar-inbox-card.tsx` — the workspace card + context menu wiring
- `src/stores/sidebar-inbox-store.ts` — persisted settled list + session repo filter
- `src/components/layout/sidebar-action-row.tsx` — expanded search/new-agent header + collapsed rail header
- `src/components/layout/sidebar-rail-workspaces.tsx` — collapsed per-workspace strip
- `src/components/layout/sidebar-footer-bar.tsx` — footer nav (Automations/Workspaces/Ports/app menu)
- `src/components/layout/sidebar-workspace-row.tsx` — shared `WorkspaceContextMenuItems` + `DeleteWorktreeDialog` (the row component itself is unmounted)
- `src/components/layout/use-project-appearance.ts` — shared avatar appearance loader
- `src/components/ui/sidebar.tsx` — width defaults (288px expanded, 52px rail)
- `src/components/ui/working-indicator.tsx` — configurable working indicator
- `src/stores/sidebar-density-store.ts` — non-persisted status-transition timestamps
- `src/lib/pane-status.ts` — `getWorkspaceStatus` / `getProjectStatus` helpers
- `src/stores/app-store.ts` — project grouping + duplicate-label disambiguation

## Notes

- Keep this file about current truth, not future plans.
- The inbox was implemented from the `Sidebar Inbox.dc.html` design handoff
  (flat inbox of workspace cards); colors map to the existing status/accent
  tokens — no hardcoded palette values.
