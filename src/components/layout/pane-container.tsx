import { memo } from "react";
import { PaneNode } from "./PaneNode";
import { EmptyWorkspaceState } from "./empty-workspace-state";
import type { WorkspaceSnapshot } from "@/tauri/types";

interface Props {
  workspace: WorkspaceSnapshot;
}

// #127: memo is effective because setAppState performs structural sharing, so
// the `workspace` snapshot keeps a stable ref across backend ticks when nothing
// in it changed — shallow compare then skips the whole pane-tree render.
export const PaneContainer = memo(function PaneContainer({ workspace }: Props) {
  const activeSurface = workspace.surfaces.find(
    (s) => s.surface_id === workspace.active_surface_id,
  );

  if (!activeSurface) {
    return <EmptyWorkspaceState />;
  }

  return (
    <div className="h-full w-full overflow-hidden p-px">
      <PaneNode
        node={activeSurface.root}
        activePaneId={activeSurface.active_pane_id}
        visible
        isSurfaceRoot
      />
    </div>
  );
});
