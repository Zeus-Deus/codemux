import type { PaneNodeSnapshot, WorkspaceSnapshot } from "@/tauri/types";
import { PaneNode } from "./PaneNode";
export function leafPanes(node: PaneNodeSnapshot): PaneNodeSnapshot[] {
  return node.kind === "split" ? node.children.flatMap(leafPanes) : [node];
}
/** Selection is local to this browser; never rewrite the desktop's split tree. */
export function MobilePaneContainer({
  workspace,
}: {
  workspace: WorkspaceSnapshot;
}) {
  const surface = workspace.surfaces.find(
    (s) => s.surface_id === workspace.active_surface_id,
  );
  if (!surface) return null;
  const leaves = leafPanes(surface.root);
  const selected =
    leaves.find((p) => p.pane_id === surface.active_pane_id) ?? leaves[0];
  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="min-h-0 flex-1">
        {selected && (
          <PaneNode
            node={selected}
            activePaneId={selected.pane_id}
            visible
            workspaceId={workspace.workspace_id}
            isSurfaceRoot
          />
        )}
      </div>
    </div>
  );
}
