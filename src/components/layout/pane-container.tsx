import { PaneNode } from "./PaneNode";
import { EmptyWorkspaceState } from "./empty-workspace-state";
import type { WorkspaceSnapshot } from "@/tauri/types";

interface Props {
  workspace: WorkspaceSnapshot;
}

export function PaneContainer({ workspace }: Props) {
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
      />
    </div>
  );
}
