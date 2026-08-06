import { getWorkspaceStatus } from "@/lib/pane-status";
import { isWorkspaceLive } from "@/stores/sidebar-density-store";
import type { ActivePaneStatus, PaneStatus } from "@/tauri/types";
import type { ProjectGroup } from "@/stores/app-store";
import type { LiveEntry } from "./sidebar-live-section";

/** Live-bucket sort rank: reds first, then working, then monitoring, then
 *  done. `monitoring` is listed for completeness — `isWorkspaceLive` never
 *  admits a monitoring workspace into this bucket (a calm watch loop is not
 *  something to gather on top of the list), so the rank is unreachable in
 *  practice and only exists so the map stays exhaustive. */
const LIVE_RANK: Record<ActivePaneStatus, number> = {
  permission: 0,
  working: 1,
  monitoring: 2,
  review: 3,
};

/**
 * The "gather on top" membership + ordering. Walks the project groups in
 * tree order, keeps every workspace whose aggregate status is live (working /
 * permission / unseen-fresh review — via the shared `isWorkspaceLive`
 * predicate), and sorts them permission → working → review, stable by tree
 * order within each bucket. Pure so it is unit-testable without a DOM.
 */
export function computeLiveEntries(
  projectGroups: ReadonlyArray<ProjectGroup>,
  paneStatuses: Record<string, PaneStatus>,
  settledAt: Record<string, number>,
  lastSeenAt: Record<string, number>,
  now: number,
): LiveEntry[] {
  const rows: (LiveEntry & { status: ActivePaneStatus; order: number })[] = [];
  let order = 0;
  for (const group of projectGroups) {
    for (const ws of group.workspaces) {
      const status = getWorkspaceStatus(ws.surfaces, paneStatuses);
      if (
        isWorkspaceLive(
          status,
          settledAt[ws.workspace_id],
          lastSeenAt[ws.workspace_id],
          now,
        )
      ) {
        rows.push({
          workspace: ws,
          projectName: group.projectName,
          projectPath: group.projectPath,
          status: status!,
          order: order++,
        });
      }
    }
  }
  rows.sort(
    (a, b) => LIVE_RANK[a.status] - LIVE_RANK[b.status] || a.order - b.order,
  );
  return rows.map(({ workspace, projectName, projectPath }) => ({
    workspace,
    projectName,
    projectPath,
  }));
}
