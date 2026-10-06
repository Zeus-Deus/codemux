import { useMobileLayout } from "@/hooks/use-mobile-layout";
import { MobilePaneContainer } from "./mobile-pane-container";
import { memo, useEffect } from "react";
import { PaneNode } from "./PaneNode";
import { EmptyWorkspaceState } from "./empty-workspace-state";
import { usePaneZoomStore } from "@/stores/pane-zoom-store";
import { MenuKeycap } from "@/components/ui/menu-chrome";
import { cn } from "@/lib/utils";
import { Minimize2 } from "lucide-react";
import type { WorkspaceSnapshot } from "@/tauri/types";

interface Props {
  workspace: WorkspaceSnapshot;
}

// #127: memo is effective because setAppState performs structural sharing, so
// the `workspace` snapshot keeps a stable ref across backend ticks when nothing
// in it changed — shallow compare then skips the whole pane-tree render.
export const PaneContainer = memo(function PaneContainer({ workspace }: Props) {
  const mobile = useMobileLayout();
  const activeSurface = workspace.surfaces.find(
    (s) => s.surface_id === workspace.active_surface_id,
  );
  const surfaceId = activeSurface?.surface_id ?? "";
  const zoomRequest = usePaneZoomStore((s) => s.zoomedPaneBySurface[surfaceId]);
  // A zoom only holds while its pane is the active one in a real split. Moving
  // focus elsewhere, closing the pane, or collapsing the split back to one
  // pane all end it, so a later return to that pane never re-zooms by surprise.
  const zoomedPaneId =
    zoomRequest &&
    activeSurface?.root.kind === "split" &&
    activeSurface.active_pane_id === zoomRequest
      ? zoomRequest
      : undefined;
  useEffect(() => {
    if (zoomRequest && !zoomedPaneId) usePaneZoomStore.getState().clear(surfaceId);
  }, [zoomRequest, zoomedPaneId, surfaceId]);

  if (!activeSurface) {
    return <EmptyWorkspaceState />;
  }

  if (mobile) return <MobilePaneContainer workspace={workspace} />;

  return (
    <div className="relative h-full w-full overflow-hidden p-px">
      <PaneNode
        node={activeSurface.root}
        activePaneId={activeSurface.active_pane_id}
        visible
        workspaceId={workspace.workspace_id}
        isSurfaceRoot
        zoomedPaneId={zoomedPaneId}
      />
      {/* The other panes are only hidden, so say so, and give the mouse a
          way back to the split. */}
      {zoomedPaneId && (
        <button
          type="button"
          onClick={() => usePaneZoomStore.getState().clear(surfaceId)}
          className={cn(
            "absolute left-1/2 top-1 z-30 flex h-5 -translate-x-1/2 items-center gap-1.5 rounded-sm px-1.5",
            "border border-hairline-strong bg-card/80 text-caption text-muted-foreground shadow-sm backdrop-blur-md",
            "transition-colors duration-100 hover:bg-card hover:text-foreground",
          )}
          aria-label="Restore split panes"
          data-pane-zoom-chip
        >
          <Minimize2 className="size-3" aria-hidden />
          Zoomed
          <MenuKeycap actionId="togglePaneZoom" className="ml-0" />
        </button>
      )}
    </div>
  );
});
