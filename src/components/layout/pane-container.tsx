import { Terminal } from "lucide-react";
import { PaneNode } from "./PaneNode";
import type { WorkspaceSnapshot } from "@/tauri/types";

interface Props {
  workspace: WorkspaceSnapshot;
}

export function PaneContainer({ workspace }: Props) {
  const activeSurface = workspace.surfaces.find(
    (s) => s.surface_id === workspace.active_surface_id,
  );

  if (!activeSurface) {
    return (
      <div className="flex h-full items-center justify-center bg-background text-muted-foreground">
        <div className="text-center space-y-3">
          <Terminal className="h-12 w-12 mx-auto opacity-30" />
          <p className="text-sm">No active surface</p>
        </div>
      </div>
    );
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
