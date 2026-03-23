import { Terminal } from "lucide-react";
import type { WorkspaceSnapshot } from "@/tauri/types";

interface Props {
  workspace: WorkspaceSnapshot;
}

export function PaneContainer({ workspace }: Props) {
  const activeSurface = workspace.surfaces.find(
    (s) => s.surface_id === workspace.active_surface_id,
  );

  return (
    <div className="flex h-full items-center justify-center bg-background text-muted-foreground">
      <div className="text-center space-y-3">
        <Terminal className="h-12 w-12 mx-auto opacity-30" />
        <p className="text-sm">
          {activeSurface
            ? `Surface: ${activeSurface.title}`
            : "No active surface"}
        </p>
        <p className="text-xs text-muted-foreground/60">
          Pane rendering coming next
        </p>
      </div>
    </div>
  );
}
