import { useMemo } from "react";

import { useAppStore, useHomeDir, groupWorkspacesByProject } from "@/stores/app-store";
import { useHosts } from "@/stores/hosts-store";
import { useWorkspacesSync } from "@/stores/workspaces-sync-store";
import type { WorkspaceSnapshot } from "@/tauri/types";
import type { HostView, WorkspaceSyncView } from "@/tauri/commands";

/**
 * One item the overview can render. Either:
 *  - kind="local": exists in this device's `app_state.workspaces`.
 *    Carries the full WorkspaceSnapshot (status dot, git stats,
 *    pane info, etc). May or may not also have a synced row.
 *  - kind="remote": only known via the cross-device sync registry.
 *    Lives on another device of the same account; this device has
 *    title + host + branch + project metadata but no local worktree.
 *    The UI offers a "Pull to this device" affordance for these.
 */
export type OverviewItem =
  | {
      kind: "local";
      /** Stable key for React. */
      key: string;
      workspace: WorkspaceSnapshot;
      /** The corresponding sync row, if one exists. Carries
       *  `dirty` (for the "Pending sync" pill) and `host_server_id`
       *  (cross-device identity, used for bucketing). */
      sync: WorkspaceSyncView | null;
      /** Cross-device host id. Resolved from `workspace.host_id`
       *  via the local hosts table; null when local or when the
       *  host isn't synced yet. */
      hostServerId: string | null;
      /** Project name for display. Falls back to the synced
       *  project_path basename when the local snapshot has no
       *  project_root (rare). */
      projectName: string | null;
      projectPath: string | null;
    }
  | {
      kind: "remote";
      key: string;
      sync: WorkspaceSyncView;
      hostServerId: string | null;
      projectName: string | null;
      projectPath: string | null;
    };

/**
 * Phase-4 divergence detection — when two or more rows in the
 * overview share a `(project_remote, git_branch)` but have
 * DIFFERENT `git_head_sha` values, those rows have diverged in git.
 * The overview surfaces a warning chip on each diverged row so the
 * user can tell at a glance that pushing from one device would
 * overwrite the other's edits.
 *
 * Returned as a Map from sync-row id → divergence summary so the
 * row component can render the chip without re-running the analysis.
 */
export interface DivergenceInfo {
  /** Number of distinct `git_head_sha` values across all rows that
   *  share the row's (project_remote, git_branch). Always ≥2 when
   *  this entry exists. */
  forks: number;
  /** Human label for the OTHER location(s) — e.g. "macbook-air" or
   *  "homedesk + 1 more". Used in the chip tooltip. */
  otherLabel: string;
}

/**
 * One bucket the overview renders. Each unique host (including
 * "local") gets one bucket. Buckets keyed by `(hostServerId, hostId)`
 * — a remote bucket has hostId null on this device when the host
 * itself hasn't synced over yet (orphan workspaces).
 */
export interface DeviceBucket {
  /** Key — `"local"` for the local bucket, otherwise the cross-
   *  device host server id. Used as React key and as the dedupe
   *  key for buckets. */
  key: string;
  /** The local hosts.id, if known on this device. */
  localHostId: number | null;
  /** Cross-device host server id; null for the local bucket. */
  hostServerId: string | null;
  /** Display name. */
  label: string;
  /** Optional sublabel (ssh target for remote, or "host not on
   *  this device" for orphans). */
  sublabel: string | null;
  /** Workspaces that landed in this bucket after filtering. */
  items: OverviewItem[];
  /** Total before filtering — drives the "X hidden by filter" pill. */
  totalCount: number;
  /** Sort hint: locals first, then synced hosts in their
   *  configured order, orphan/removed at the end. */
  sortRank: number;
}

/**
 * Detect git-HEAD divergence across the unified overview-item list.
 * Two rows that share the same (project_remote, git_branch) but
 * have different `git_head_sha` values are considered diverged. The
 * UI surfaces a warning chip on each diverged row.
 *
 * Returns a Map keyed by the row's stable identity (`sync.id` for
 * remote rows, `workspace.workspace_id` for local-with-sync rows).
 * Rows with no `git_head_sha` are excluded — we can't determine
 * divergence without a sha on both sides.
 */
export function detectDivergence(
  items: OverviewItem[],
  hostLabel: (hostServerId: string | null) => string,
): Map<string, DivergenceInfo> {
  // Group all rows by (project_remote, git_branch). A row contributes
  // its sync data if available (remote items always have it; local
  // items have it only after the first reconcile sync).
  type Entry = {
    key: string;
    sha: string | null;
    hostServerId: string | null;
  };
  const groups = new Map<string, Entry[]>();
  for (const it of items) {
    const sync = it.kind === "local" ? it.sync : it.sync;
    if (!sync) continue;
    if (!sync.project_remote || !sync.git_branch) continue;
    const groupKey = `${sync.project_remote}::${sync.git_branch}`;
    const rowKey =
      it.kind === "local"
        ? `local:${it.workspace.workspace_id}`
        : `remote:${sync.id}`;
    const list = groups.get(groupKey) ?? [];
    list.push({
      key: rowKey,
      sha: sync.git_head_sha,
      hostServerId: sync.host_server_id,
    });
    groups.set(groupKey, list);
  }

  // For each group with >=2 entries AND >=2 distinct shas, mark
  // every entry in that group as diverged. Skip groups where any
  // entry's sha is null (insufficient info).
  const result = new Map<string, DivergenceInfo>();
  for (const entries of groups.values()) {
    if (entries.length < 2) continue;
    const shas = new Set<string>();
    for (const e of entries) {
      if (e.sha) shas.add(e.sha);
    }
    if (shas.size < 2) continue;
    for (const entry of entries) {
      const others = entries.filter((e) => e.key !== entry.key);
      const otherLabels = others.map((e) => hostLabel(e.hostServerId));
      const distinctLabels = Array.from(new Set(otherLabels));
      const label =
        distinctLabels.length <= 1
          ? distinctLabels[0] ?? "another device"
          : `${distinctLabels[0]} + ${distinctLabels.length - 1} more`;
      result.set(entry.key, { forks: shas.size, otherLabel: label });
    }
  }
  return result;
}

/** Compose the unified overview-item list from the three sources. */
export function useOverviewItems(): {
  items: OverviewItem[];
  hosts: HostView[];
  hostsLoaded: boolean;
} {
  const workspaces = useAppStore((s) => s.appState?.workspaces ?? null);
  const hosts = useHosts();
  const syncRows = useWorkspacesSync();
  const homeDir = useHomeDir();

  // Project name lookup so both local and remote rows render the
  // same way as the rest of the app (sidebar uses
  // `groupWorkspacesByProject` for the basename + "Home" rules).
  const projectByWid = useMemo(() => {
    const map = new Map<string, { name: string; path: string }>();
    if (!workspaces) return map;
    for (const group of groupWorkspacesByProject(workspaces, homeDir)) {
      for (const ws of group.workspaces) {
        map.set(ws.workspace_id, {
          name: group.projectName,
          path: group.projectPath,
        });
      }
    }
    return map;
  }, [workspaces, homeDir]);

  // Local host id → server id, so a local workspace's `host_id`
  // resolves to the cross-device bucket key.
  const hostIdToServer = useMemo(() => {
    const map = new Map<number, string>();
    for (const h of hosts) {
      if (h.server_id) map.set(h.id, h.server_id);
    }
    return map;
  }, [hosts]);

  // Sync rows indexed by their local workspace_id, so a local
  // snapshot can find its matching sync row to read host_server_id
  // and the dirty flag.
  const syncByWid = useMemo(() => {
    const map = new Map<string, WorkspaceSyncView>();
    for (const row of syncRows) {
      if (row.workspace_id) map.set(row.workspace_id, row);
    }
    return map;
  }, [syncRows]);

  const items = useMemo<OverviewItem[]>(() => {
    const result: OverviewItem[] = [];
    const consumedSyncIds = new Set<number>();

    // 1) Every local workspace contributes a "local" item. The sync
    //    row (if any) tags it with host_server_id and dirty.
    for (const ws of workspaces ?? []) {
      const sync = syncByWid.get(ws.workspace_id) ?? null;
      if (sync) consumedSyncIds.add(sync.id);
      const proj = projectByWid.get(ws.workspace_id) ?? null;
      const hostServerId =
        sync?.host_server_id ??
        (ws.host_id != null ? hostIdToServer.get(ws.host_id) ?? null : null);
      result.push({
        kind: "local",
        key: `local:${ws.workspace_id}`,
        workspace: ws,
        sync,
        hostServerId,
        projectName: proj?.name ?? null,
        projectPath: proj?.path ?? null,
      });
    }

    // 2) Every sync row with NO local workspace counterpart is a
    //    sibling-device workspace. Render as "remote".
    //
    //    Defense in depth: a row whose `workspace_id` IS set (non-null)
    //    used to map to a local workspace on this device — it's not a
    //    sibling-device row, it's a self-orphan from a workspace that
    //    was just closed and whose `workspaces_sync` soft-delete hasn't
    //    reached this snapshot yet (the close path's reconcile + sync
    //    runs before the next emit, but if it ever fails, the
    //    background tick is still ~30 s away). Skipping these rows here
    //    keeps the closed workspace from briefly appearing as
    //    "lives on another device" in the overview. True sibling-device
    //    rows always have `workspace_id === null` until adopted.
    for (const row of syncRows) {
      if (consumedSyncIds.has(row.id)) continue;
      if (row.workspace_id !== null) continue;
      result.push({
        kind: "remote",
        key: `remote:${row.server_id ?? row.id}`,
        sync: row,
        hostServerId: row.host_server_id,
        projectName: row.project_path
          ? row.project_path.split("/").filter(Boolean).slice(-1)[0] ?? null
          : null,
        projectPath: row.project_path,
      });
    }

    return result;
  }, [workspaces, syncRows, syncByWid, projectByWid, hostIdToServer]);

  const hostsLoaded = true; // useHosts() lazily loads; readers don't depend on the loaded flag for rendering. UI shows skeleton via the rows-null path elsewhere.

  return { items, hosts, hostsLoaded };
}
