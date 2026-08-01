import { memo, useLayoutEffect } from "react";
import { PaneNode } from "./PaneNode";
import { EmptyWorkspaceState } from "./empty-workspace-state";
import { markOpenInteraction } from "@/lib/perf/interaction-trace";
import type { WorkspaceSnapshot } from "@/tauri/types";

interface Props {
  workspace: WorkspaceSnapshot;
}

// #127: memo is effective because setAppState performs structural sharing, so
// the `workspace` snapshot keeps a stable ref across backend ticks when nothing
// in it changed — shallow compare then skips the whole pane-tree render.
export const PaneContainer = memo(function PaneContainer({ workspace }: Props) {
  // Closes the workspace-switch trace: a layout effect runs after the new pane
  // tree is in the DOM but before paint, and the perf module arms a double-rAF
  // from this phase to stamp the actual paint. No-op unless a trace is open,
  // and the id scope means the first render for an untraced workspace (boot,
  // backend-driven activation) records nothing.
  useLayoutEffect(() => {
    markOpenInteraction("pane-mounted", { target: workspace.workspace_id });
  }, [workspace.workspace_id]);

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
