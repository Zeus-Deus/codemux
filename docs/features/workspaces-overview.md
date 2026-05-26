# Workspaces Overview

- Purpose: Describe the full-screen "Workspaces" overlay that lists every workspace this device tracks — local + every remote host it has pushed to — with filters, search, and per-row push/pull/open actions.
- Audience: Anyone touching the sidebar, the workspace push/pull flow, or the overview UI itself.
- Authority: Canonical feature doc for the overview surface.
- Update when: The sidebar entry, the overview layout, the filter set, or the per-row actions change.
- Read next: `docs/features/remote-hosts.md` for the push/pull pipeline underneath, `docs/features/workspace-creation.md` for the new-workspace flow.

## What This Feature Is

A single pane that answers "where do all my workspaces live?" — across this device and every host it has pushed to. Reached from the left sidebar (the `Workspaces` button under `Automations`), opens as a full-screen overlay with the same chrome as Settings and Automations.

Before this feature the sidebar only showed workspaces grouped by project on the current device. Once you pushed a workspace to a remote host the cloud icon told you it was remote, but there was no surface for "show me everything I have, everywhere." The overview is that surface.

## Current Model

### Entry point

- `src/components/layout/sidebar-action-row.tsx` renders the `Workspaces` button under `Automations`. Click → `useUIStore.setShowWorkspacesOverview(true)`.
- `src/components/layout/app-shell.tsx` early-returns `<WorkspacesOverviewView />` whenever `showWorkspacesOverview` is true — same pattern as `showAutomations` and `showSettings`.
- Escape and the back button both close the overlay.

### Data source

- Workspaces come from `useAppStore((s) => s.appState?.workspaces)`. Every `WorkspaceSnapshot` already carries `host_id?: number | null` (added in the cloud-push series — see `remote-hosts.md`). `null` means local; a number references the local `hosts` table row id.
- Host metadata comes from `useHostsStore()` — the same shared cache the `DevicePicker` and workspace context menu use, so the overview pays one IPC round-trip on first mount and reads from cache thereafter.
- Project names + the basename / "Home" disambiguation reuse `groupWorkspacesByProject(workspaces, homeDir)` from `src/stores/app-store.ts`, so projects appear with the exact labels the sidebar gives them.

### Layout

- Sticky filter bar at the top: search, project, device, status, sort.
- Result-count row underneath with a `New workspace` shortcut.
- Body groups workspaces into **device sections**, in this order:
  1. `This device` (the local bucket — emerald accent + custom laptop glyph)
  2. Each configured host in the order they appear in `Settings → Hosts` (sky-blue accent + cloud icon)
  3. Any `Removed host` orphan section, if a workspace still references a deleted host id (so rows never silently disappear)
- Each device section header shows the host name + ssh target, a workspace count, and a "filtered" pill if any of the bucket's workspaces are hidden by the active filter.
- Workspaces render as compact two-column cards (single column under `md`): status dot · title · project name · branch · git stats · hover-reveal action menu.

### Status semantics

The status dot to the left of each workspace title has two precedence layers. **Live agent status wins** when present — sourced from `getWorkspaceStatus(workspace.surfaces, appState.pane_statuses)`, the same path the sidebar uses, so when a pane flips between idle / working / waiting-on-input / review the overview row repaints automatically without polling:

| Live status | Treatment |
|---|---|
| `working` | Amber pulsing dot via `<StatusIndicator>`; meta line shows `· agent working` |
| `permission` | Red pulsing dot; meta line shows `· needs input` |
| `review` | Emerald dot; meta line shows `· ready to review` |

When no live status is set, the static dot falls back to:

| Color | Meaning |
|---|---|
| Emerald | Currently open in this app (matches `appState.active_workspace_id`) |
| Sky blue | Lives on a remote host |
| Violet | OpenFlow workspace |
| Amber | Push or pull in flight (replaces the action menu with a spinner) |
| Muted | Local, not currently attached |

The attached card also gets an emerald border + soft ring so it's findable at a glance even when the dot scrolls off.

### Filters

- **Search** — case-insensitive substring match against title, branch, or project name. Clear button appears once non-empty.
- **Project** — exact-path match using the project paths that `groupWorkspacesByProject` derives.
- **Device** — `All`, `This device`, or a specific host.
- **Status** — `Any status`, `Currently open`, `On a remote host`, `Has uncommitted work`.
- **Sort** — `Recently active` (notifications + dirty-git proxy → name tie-break), `Name`, `Branch`.
- A `Clear` chip appears whenever any non-default filter is set.

### Per-row actions

The trailing `⋯` menu (hover- and focus-revealed) holds:

- **Open workspace** — calls `activateWorkspace(workspaceId)`, closes the overlay, lands the user in the workspace.
- **Copy branch name** — clipboard copy, disabled when no branch.
- **Rename…** — `window.prompt` + `renameWorkspace`.
- **Push to host…** — submenu of every configured host. Each entry calls `workspacePushToHost(workspaceId, hostId)` (reuses the same Tauri command the sidebar context menu uses). Disabled with a tooltip when zero hosts are configured.
- **Pull back to this device** — only appears for workspaces with a non-null `host_id`. Calls `workspacePullBack(workspaceId)`.
- **Delete worktree… / Close workspace…** — destructive, gated by `window.confirm`. Worktrees call `closeWorkspaceWithWorktree` with the same flags as the sidebar's remove dialog.

Whole card is also clickable — clicking opens. The action menu stops event propagation so it never accidentally opens the workspace while you're navigating the submenu.

The push/pull flight indicator lives on the shared `useAppStore.workspacePushPullInFlight` slot, so the same workspace shows a spinner in the sidebar **and** the overview at the same time — there's exactly one in-flight push or pull per workspace.

## What Works Today

- Sidebar button under Automations opens the full-screen overview; Escape and back button close it.
- Every workspace this device tracks is listed, grouped by device.
- Pushed workspaces visibly migrate to the matching device bucket once the push succeeds.
- Filters: search (title / branch / project), project, device, status, sort.
- Per-row actions: open, copy branch, rename, push to any configured host, pull back, delete.
- Empty states for "no workspaces yet" (offers `New workspace`) and "filters hide everything" (offers `Clear filters`).
- "Removed host" orphan bucket prevents workspaces from silently disappearing when a host row is deleted.
- One push/pull spinner per workspace, shared with the sidebar.
- Live agent status (working / needs-input / ready-to-review) reflected on every row in real time — same source as the sidebar's `StatusIndicator`, no polling.
- First-run welcome banner (`WelcomeBanner` component) with three state-aware variants — brand-new (no devices, no siblings, with `Add a device` CTA), device-configured (nudges first push), has-siblings (counts visible cross-device workspaces with pull instructions). Dismissable; persists in localStorage and never re-shows.

## Current Constraints

- **Sibling-device adoption is deferred.** The overview SHOWS workspaces from other devices of the same account (via the cross-device workspace sync — see `docs/features/workspaces-sync.md`). The "Pull to this device" action on sibling rows is disabled in v1; the affordance is visible with a "coming soon" tooltip and will land in a follow-up. Until then, sibling-device rows are read-only — you can see them and their metadata, but you adopt them by visiting the device they live on.
- **No created-at field** on `WorkspaceSnapshot` yet, so the "Created within" time filter UI is intentionally **not** rendered — would be misleading without the data. The `Recently active` sort uses a proxy (notification count + dirty-git heuristic + name tie-break) until a real `last_active_at` is plumbed through.
- **No bulk actions.** Push N workspaces in one go isn't supported; each row pushes individually.
- **Window-prompt rename.** Matches the sidebar context menu; if/when that switches to an inline edit the overview should follow.
- **Delete uses window.confirm**, not the richer `RemoveWorkspaceDialog` the sidebar pops. Fine for now — the overview is the "manage many at once" surface where a heavyweight modal per row would be noisy — but if `RemoveWorkspaceDialog` grows more functionality we should reconsider.

## Important Touch Points

- `src/components/workspaces-overview/workspaces-overview-view.tsx` — full-screen overlay shell (WindowChrome + back button + Escape).
- `src/components/workspaces-overview/workspaces-overview-section.tsx` — filter bar, device bucketing, sort.
- `src/components/workspaces-overview/workspace-overview-row.tsx` — card component + action menu + push/pull wiring.
- `src/components/layout/sidebar-action-row.tsx` — sidebar entry point.
- `src/components/layout/app-shell.tsx` — overlay mount.
- `src/stores/ui-store.ts` — `showWorkspacesOverview` boolean + setter.
- `src/stores/app-store.ts` — `workspaces`, `active_workspace_id`, `workspacePushPullInFlight`, `groupWorkspacesByProject`.
- `src/stores/hosts-store.ts` — shared `useHostsStore` cache.
- `src/tauri/commands.ts` — `workspacePushToHost`, `workspacePullBack`, `activateWorkspace`, `renameWorkspace`, `closeWorkspace`, `closeWorkspaceWithWorktree`.
- `src/tauri/types.ts` — `WorkspaceSnapshot.host_id` (the field the overview keys off).

## Notes

- The overview is read-only with regard to the workspace data model — it does not introduce new persistence, new Tauri commands, or new sync paths. Every action it surfaces was already shipping in the sidebar context menu; the overview just makes them discoverable from one place.
- When cross-device workspace sync ships, the only change here should be that `useAppStore.workspaces` includes the synced remote rows. The bucketing already groups by `host_id` correctly, so no UI rework is expected.
- Status colors and density follow `docs/features/automations.md`'s reference implementation so the two pane types feel like the same family.
