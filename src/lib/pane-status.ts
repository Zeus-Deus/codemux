import type {
  PaneStatus,
  ActivePaneStatus,
  AgentChatProviderKind,
  PaneNodeSnapshot,
  SurfaceSnapshot,
} from "@/tauri/types";

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

function collectProviders(
  node: PaneNodeSnapshot,
  out: AgentChatProviderKind[],
): void {
  if (node.kind === "split") {
    for (const child of node.children) collectProviders(child, out);
    return;
  }
  if (node.kind === "agent_chat" && node.provider && !out.includes(node.provider)) {
    out.push(node.provider);
  }
}

/**
 * The distinct agent-chat providers active in a workspace, in first-seen
 * order. Drives the provider logos on the sidebar inbox card — a workspace
 * chatting with Claude shows the Claude mark, etc. Terminal-only agent
 * panes carry no provider metadata, so they contribute nothing.
 */
export function getWorkspaceProviders(
  surfaces: SurfaceSnapshot[],
): AgentChatProviderKind[] {
  const out: AgentChatProviderKind[] = [];
  for (const s of surfaces) collectProviders(s.root, out);
  return out;
}

/**
 * Aggregate status across a set of workspaces (e.g. every workspace in a
 * project). Used by the collapsed sidebar rail to surface a single
 * status dot on a project's avatar that reflects its busiest workspace.
 */
export function getProjectStatus(
  workspaces: { surfaces: SurfaceSnapshot[] }[],
  paneStatuses: Record<string, PaneStatus>,
): ActivePaneStatus | null {
  return getHighestPriorityStatus(
    workspaces.map((w) => getWorkspaceStatus(w.surfaces, paneStatuses) ?? undefined),
  );
}
