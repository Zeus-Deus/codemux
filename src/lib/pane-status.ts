import type { PaneStatus, ActivePaneStatus, PaneNodeSnapshot, SurfaceSnapshot } from "@/tauri/types";

const STATUS_PRIORITY: Record<PaneStatus, number> = {
  idle: 0,
  review: 1,
  working: 2,
  permission: 3,
};

export function pickHigherStatus(
  a: PaneStatus | undefined,
  b: PaneStatus | undefined,
): PaneStatus {
  const pa = a ? STATUS_PRIORITY[a] : 0;
  const pb = b ? STATUS_PRIORITY[b] : 0;
  return pa >= pb ? (a ?? "idle") : (b ?? "idle");
}

export function getHighestPriorityStatus(
  statuses: Iterable<PaneStatus | undefined>,
): ActivePaneStatus | null {
  let best: PaneStatus = "idle";
  for (const s of statuses) {
    if (!s || s === "idle") continue;
    if (s === "permission") return "permission"; // early exit — highest possible
    best = pickHigherStatus(best, s);
  }
  return best === "idle" ? null : (best as ActivePaneStatus);
}

/** Collect all pane IDs from a surface's layout tree. */
function collectPaneIds(node: PaneNodeSnapshot): string[] {
  if (node.kind === "split") {
    return node.children.flatMap(collectPaneIds);
  }
  return [node.pane_id];
}

/** Get the highest priority status across all panes in surfaces. */
export function getWorkspaceStatus(
  surfaces: SurfaceSnapshot[],
  paneStatuses: Record<string, PaneStatus>,
): ActivePaneStatus | null {
  const statuses = surfaces.flatMap((s) =>
    collectPaneIds(s.root).map((id) => paneStatuses[id]),
  );
  return getHighestPriorityStatus(statuses);
}
