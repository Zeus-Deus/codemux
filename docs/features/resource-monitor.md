# Resource Monitor

- Purpose: Describe the current capability and constraints of the title-bar resource monitor.
- Audience: Anyone working on the title bar, terminal subsystem, or process metrics.
- Authority: Canonical feature-level reality doc.
- Update when: Metric collection, the snapshot shape, or the popover UI changes.
- Read next: `docs/features/terminal.md`, `docs/features/settings.md`

## What This Feature Is

A CPU-chip icon in the title bar (right of the sidebar toggle) that opens a
popover showing how much CPU and memory Codemux itself and every live terminal
process tree are using, grouped by project and workspace.

## Current Model

- **Backend** (`src-tauri/src/resource_metrics.rs`): a single `get_resource_metrics`
  Tauri command builds one snapshot per call. It uses the `sysinfo` crate to
  capture the process table (PID, parent PID, CPU%, resident memory), builds a
  parent → children map, and sums each terminal session's whole process subtree.
  - **Memory is PSS, not RSS, on Linux.** RSS counts every shared page in full
    for every process mapping it, so summing RSS across a WebKit/Chromium/node
    tree overcounts real RAM by tens of GB. After the reported PIDs are known,
    `enrich_with_pss` reads `/proc/<pid>/smaps_rollup` and replaces each value
    with `Pss:` (proportional set size) — the honest physical-RAM number. On
    non-Linux platforms the sysinfo RSS/working-set value stands.
  - The measurable terminal set is `PtyState::get_session_pids()` — every live
    PTY. Each session is attributed to a workspace/project from the hydrated
    `surfaces` pane tree when available; parked workspaces have empty `surfaces`,
    so those sessions fall back to grouping by the session `cwd` (worktrees
    under `.codemux/worktrees/<project>/` collapse to the shared project). A
    live terminal therefore never vanishes from the monitor.
  - The Codemux app's own subtree is reported separately and split into
    `main` / `web_view` / `other` buckets, excluding monitored terminal subtrees.
  - A persistent `System` handle lives in `ResourceMonitorState` (Tauri managed
    state) so CPU% is delta-measured between polls. The first call after launch
    reports `0.0` CPU; every call after that is accurate.
- **Frontend** (`src/components/layout/resource-monitor/`): `ResourceMonitor`
  renders the trigger button + popover. It polls `get_resource_metrics` via
  React Query — every 2 s while the popover is open, every 15 s while closed.
  Workspaces are grouped into a Project → Workspace → Session collapsible tree.
  Rows can be sorted by Memory / CPU / Name. Clicking a workspace or session row
  activates it. Severity dots (amber / red) flag elevated and high usage.
- **Setting**: `appearance.show_resource_monitor` (defaults to `true`) gates the
  icon. Toggle lives in Settings → Appearance. The component renders `null` when
  the setting is off.

## What Works Today

- Per-terminal CPU + memory aggregated across the full process subtree.
- App self-usage split into main / web view / other.
- Project / workspace / session grouping with collapse + sort.
- Host RAM-share readout with a severity-colored progress bar.
- Cross-platform process listing via `sysinfo` (Linux / macOS / Windows).
- Click-through navigation to a workspace or terminal pane.

## Current Constraints

- CPU% is `0.0` on the very first poll after launch (delta needs two refreshes).
- PSS accounting is Linux-only; macOS/Windows use sysinfo's RSS/working-set,
  which still overcounts shared memory (a native `phys_footprint` shim would
  close this gap on macOS).
- `web_view` classification is name-based (`webkit*`, `*webprocess`,
  `msedgewebview*`); helpers with unusual names fall into `other`.
- Project grouping derives from `WorkspaceSnapshot.project_root` (falling back to
  `cwd`); there is no separate project entity.
- Only **workspaces with live terminal PTYs** appear. A parked workspace whose
  terminal processes are not running has nothing to measure, so it is absent —
  same as Superset, which only lists workspaces with active terminal sessions.
- Only terminal panes are measured — browser and agent-chat panes are not, so
  their process trees land in the app's `other` bucket.

## Important Touch Points

- `src-tauri/src/resource_metrics.rs` — collector, command, `ResourceMonitorState`
- `src-tauri/src/lib.rs` — module decl, `.manage(...)`, `generate_handler!` entry
- `src-tauri/src/terminal/mod.rs` — `PtyState::get_session_pids()`
- `src-tauri/src/settings_sync.rs` — `appearance.show_resource_monitor`
- `src/components/layout/resource-monitor/` — popover UI + utils + tests
- `src/components/layout/title-bar.tsx` — mounts `<ResourceMonitor />`
- `src/tauri/commands.ts` / `src/tauri/types.ts` — `getResourceMetrics` + types

## Notes

- Keep this file about current truth, not future plans.
- The UI is a Codemux-token port of the Superset "Resource Consumption" feature.
