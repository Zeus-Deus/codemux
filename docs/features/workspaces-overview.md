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
  2. Each configured host in the order they appear in `Settings → Hosts` (sky-blue accent + cloud icon). **Configured-host buckets stay visible even when empty** — a device the user has set up shows in the overview the moment it's added, not only after the first workspace lands on it. The local `This device` bucket and any "Removed host" orphan bucket still follow the items-or-totalCount rule so a brand-new account with zero rows doesn't render a dangling header.
  3. Any `Removed host` orphan section, if a workspace still references a deleted host id (so rows never silently disappear)
- Each device section header shows the host name + ssh target, a workspace count, and a "filtered" pill if any of the bucket's workspaces are hidden by the active filter.
- **Within a device section, workspaces of the same project cluster under a subtle project header** (folder glyph · project name · count) when that project has 2+ workspaces in the bucket — so a repo's root checkout and its worktrees visibly read as one project instead of unrelated cards. Projects with a single workspace stay in a flat grid with no header, so a device full of one-off projects isn't littered with single-item headers. The cluster key is the stable `projectKey` (`partitionByProject` in `workspaces-overview-section.tsx`): the deterministic `project_uid` when known (historical design at `docs/archive/project-identity.md`), else the project path/name. So a root checkout and a worktree — different paths, same `project_uid` — cluster, while two unrelated repos sharing a basename do not. Sibling (remote) rows whose originating host never recorded a project root fall back to the workspace title for their label (`remoteProjectName` in `use-overview-items.ts`) instead of a bare "—".
- **Sibling rows render a `main`/`worktree` kind badge** from `WorkspaceSyncView.workspace_kind`, so the root checkout is distinguishable from per-branch worktrees at a glance.
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
- Every workspace this device tracks is listed, grouped by device — plus every sibling-device workspace on the same account via cross-device sync (see `docs/features/workspaces-sync.md`).
- Pushed workspaces visibly migrate to the matching device bucket once the push succeeds.
- Filters: search (title / branch / project), project, device, status, sort.
- Per-row actions: open, copy branch, rename, push to any configured host, pull back, delete.
- **Sibling-device adoption** (Phase 3): `Pull to this device` on any sibling row goes through `PullToDeviceDialog` and picks the right variant automatically — host-backed (rsync via the shared host), clone-from-git fallback (git clone + worktree add), already-adopted (offers Open instead), or **`SameBranchProjectBlock`** when another local workspace already matches `(basename(project_path), git_branch)` of the remote row (offers "Open the existing workspace" and hides Pull so a pull can't silently clobber work in flight).
- **Asymmetric auto-publish from `codemux-remote` hosts**: workspaces an agent creates directly on a host via the MCP `workspace_create` tool surface in the overview within ~90 s without an explicit push, courtesy of the `hosts_inventory` poller — see `docs/features/workspaces-sync.md` § "Asymmetric publish model" for the full design.
- **Confirm-before-push + undo** (Phase 4a–b): every push opens `ConfirmPushDialog` (per-host "don't ask again" persists in localStorage); every successful push, pull, and adoption surfaces a 10-second `Undo` toast that fires the reverse action.
- **Cross-device divergence chip** (Phase 4c): an amber `diverged` chip appears in the title line when the same workspace exists on multiple devices via clone-adoption and their git HEADs diverge. Tooltip suggests push or pull to reconcile.
- **Repo-root protection + labels** (repo-unit sync — `docs/plans/repo-unit-sync.md`): a workspace that is the repo's protected default-branch root checkout (`WorkspaceSnapshot.protected`, stamped divergence-safely by `crate::git::is_protected_repo_root`) shows a `repo root` badge and can't be deleted like a disposable worktree — its row offers "Close workspace…" (detach, files untouched), never "Delete worktree…". Sibling (`RemoteRow`) rows render `main`-kind as `repo root` too. A legacy divergent full copy (`divergent_copy`, a `.git` directory sitting in the worktrees tree) instead shows an amber `standalone copy` warning chip whose tooltip tells the user to delete + re-pull (new pulls land repo roots cleanly under `~/.codemux/projects/`).
- **One-click "Pull project"** (`v0.7.9`): a project cluster of un-adopted sibling rows shows a single **Pull project** button (header-level) that materialises the protected repo root *first* (at `~/.codemux/projects/<repo>`) and then recreates every worktree as a real linked worktree under it, in one action — via `workspacesAdoptProject(project_uid)`. The protected root floats to the top of its cluster so it reads as the project's anchor.
- **"Reconcile copy…" row action** (`v0.7.9`): when a row is flagged `divergent_copy`, its `⋯` menu gains **Reconcile copy…**, which calls `workspacesReconcileCopy(workspace_id)` to non-destructively detach the standalone-copy card (files left on disk) so the user can then "Pull project" for a clean protected root. Refuses (with a toast) while the copy has uncommitted/unpushed work.
- **Elapsed-time pill** (Phase 4d): when a push or pull takes longer than ~2s, a compact `12s` pill renders next to the spinner so the user knows the operation is still working.
- Empty states for "no workspaces yet" (offers `New workspace`) and "filters hide everything" (offers `Clear filters`).
- "Removed host" orphan bucket prevents workspaces from silently disappearing when a host row is deleted.
- One push/pull spinner per workspace, shared with the sidebar — and `workspacePushPullStartedAt` drives the elapsed-time pill in both surfaces.
- Live agent status (working / needs-input / ready-to-review) reflected on every row in real time — same source as the sidebar's `StatusIndicator`, no polling.
- First-run welcome banner (`WelcomeBanner` component) with three state-aware variants — brand-new (no devices, no siblings, with `Add a device` CTA), device-configured (nudges first push), has-siblings (counts visible cross-device workspaces with pull instructions). Dismissable; persists in localStorage and never re-shows.
- "How it works" popover (`HowItWorksPopover`) off the overview header explains the local/host/sibling buckets at a glance.

## Current Constraints

- **No created-at field** on `WorkspaceSnapshot` yet, so the "Created within" time filter UI is intentionally **not** rendered — would be misleading without the data. The `Recently active` sort uses a proxy (notification count + dirty-git heuristic + name tie-break) until a real `last_active_at` is plumbed through.
- **No bulk actions.** Push N workspaces in one go isn't supported; each row pushes individually.
- **Window-prompt rename.** Matches the sidebar context menu; if/when that switches to an inline edit the overview should follow.
- **Delete uses window.confirm**, not the richer `RemoveWorkspaceDialog` the sidebar pops. Fine for now — the overview is the "manage many at once" surface where a heavyweight modal per row would be noisy — but if `RemoveWorkspaceDialog` grows more functionality we should reconsider.

## Important Touch Points

- `src/components/workspaces-overview/workspaces-overview-view.tsx` — full-screen overlay shell (WindowChrome + back button + Escape).
- `src/components/workspaces-overview/workspaces-overview-section.tsx` — filter bar, device bucketing, sort.
- `src/components/workspaces-overview/workspace-overview-row.tsx` — card component + action menu + push/pull wiring + `DivergenceChip` + elapsed-time pill.
- `src/components/workspaces-overview/pull-to-device-dialog.tsx` — sibling-device adoption dialog (host-backed + clone-from-git variants).
- `src/components/workspaces-overview/welcome-banner.tsx` — first-run banner with three state-aware variants.
- `src/components/workspaces-overview/how-it-works-popover.tsx` — overview-header explainer popover.
- `src/components/workspaces-overview/use-overview-items.ts` — merges local + synced rows into the unified `OverviewItem[]`; computes `DivergenceInfo`.
- `src/components/overlays/confirm-push-dialog.tsx` — per-host "don't ask again" confirm-before-push dialog (also wired into the sidebar context menu).
- `src/lib/toast.ts` — `fireUndoable` powers the 10-second `Undo` toast on push / pull / adopt.
- `src/components/layout/sidebar-action-row.tsx` — sidebar entry point.
- `src/components/layout/app-shell.tsx` — overlay mount.
- `src/stores/ui-store.ts` — `showWorkspacesOverview` boolean + setter.
- `src/stores/app-store.ts` — `workspaces`, `active_workspace_id`, `workspacePushPullInFlight`, `workspacePushPullStartedAt`, `groupWorkspacesByProject`.
- `src/stores/hosts-store.ts` — shared `useHostsStore` cache.
- `src/stores/workspaces-sync-store.ts` — sibling-device rows (sync mirror).
- `src/tauri/commands.ts` — `workspacePushToHost`, `workspacePullBack`, `workspacesAdoptSynced`, `workspacesAdoptViaClone`, `workspacesAdoptProject` (Pull project), `workspacesReconcileCopy` (Reconcile copy), `activateWorkspace`, `renameWorkspace`, `closeWorkspace`, `closeWorkspaceWithWorktree`.
- `src/tauri/types.ts` — `WorkspaceSnapshot.host_id` (the field the overview keys off).

## Notes

- The local "open / push / pull / delete / rename" actions reuse the sidebar context menu's Tauri commands; the overview just makes them discoverable from one place. The Phase 3 sibling-device adoption added two new commands (`workspaces_adopt_synced` for host-backed adoption, `workspaces_adopt_via_clone` for the clone-from-git fallback) that the overview's `PullToDeviceDialog` is the primary consumer of.
- Cross-device workspaces sync is now live (see `docs/features/workspaces-sync.md`). `useOverviewItems` merges live `WorkspaceSnapshot` rows and `WorkspaceSyncView` sibling rows into a single `OverviewItem[]`; bucketing is by `host_server_id` so the same host shows up in the same bucket on every device.
- Status colors and density follow `docs/features/automations.md`'s reference implementation so the two pane types feel like the same family.
