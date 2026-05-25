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
    for (const row of syncRows) {
      if (consumedSyncIds.has(row.id)) continue;
      // Rows we haven't adopted locally — workspace_id is null OR
      // it referenced a local id we no longer have.
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
