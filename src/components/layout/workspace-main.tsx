import { useCallback, useRef, useEffect } from "react";
import { useActiveWorkspace } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import { dbGetUiState, dbSetUiState } from "@/tauri/commands";
import { TabBar } from "./tab-bar";
import { PaneContainer } from "./pane-container";
import { RightPanel } from "./right-panel";
import { DiffPane } from "@/components/diff/DiffPane";
import { OpenFlowWorkspace } from "@/components/openflow/openflow-workspace";

function RightPanelResizer() {
  const setRightPanelWidth = useUIStore((s) => s.setRightPanelWidth);
  const handleRef = useRef<HTMLDivElement>(null);

  const startResize = useCallback(
    (e: React.PointerEvent) => {
      e.preventDefault();
      const handle = handleRef.current;
      if (handle) handle.dataset.dragging = "true";

      const onMove = (ev: PointerEvent) => {
        setRightPanelWidth(window.innerWidth - ev.clientX);
      };

      const onUp = () => {
        if (handle) handle.dataset.dragging = "false";
        window.removeEventListener("pointermove", onMove);
        window.removeEventListener("pointerup", onUp);
        // Persist to SQLite
        dbSetUiState("right_panel_width", String(useUIStore.getState().rightPanelWidth))
          .catch(console.error);
      };

      window.addEventListener("pointermove", onMove);
      window.addEventListener("pointerup", onUp);
    },
    [setRightPanelWidth],
  );

  return (
    <div
      ref={handleRef}
      className="w-px shrink-0 cursor-col-resize bg-border/50 hover:bg-primary/30 data-[dragging=true]:bg-primary/30 transition-colors relative before:absolute before:inset-y-0 before:-inset-x-1 before:content-['']"
      onPointerDown={startResize}
      role="separator"
      aria-orientation="vertical"
    />
  );
}

export function WorkspaceMain() {
  // Load persisted right panel width from SQLite on mount
  useEffect(() => {
    dbGetUiState("right_panel_width").then((val) => {
      if (val) useUIStore.getState().setRightPanelWidth(Number(val));
    }).catch(() => {});
  }, []);

  const activeWorkspace = useActiveWorkspace();
  const rightPanelTab = useUIStore((s) =>
    activeWorkspace
      ? s.rightPanelTabs[activeWorkspace.workspace_id] ?? null
      : null,
  );
  const rightPanelWidth = useUIStore((s) => s.rightPanelWidth);

  if (!activeWorkspace) return null;

  // OpenFlow workspaces get their own dedicated view
  if (activeWorkspace.workspace_type === "open_flow") {
    return (
      <>
        <TabBar workspace={activeWorkspace} />
        <div className="flex flex-1 min-h-0 min-w-0 overflow-hidden">
          <OpenFlowWorkspace workspace={activeWorkspace} />
        </div>
      </>
    );
  }

  const showRightPanel = rightPanelTab !== null;
  const activeTab = activeWorkspace.tabs.find(
    (t) => t.tab_id === activeWorkspace.active_tab_id,
  );

  return (
    <>
      <TabBar workspace={activeWorkspace} />
      <div className="flex flex-1 min-h-0 min-w-0 overflow-hidden">
        <div className="flex-1 min-w-0 min-h-0 overflow-hidden">
          {activeTab?.kind === "diff" ? (
            <DiffPane tabId={activeTab.tab_id} workspace={activeWorkspace} />
          ) : (
            <PaneContainer workspace={activeWorkspace} />
          )}
        </div>
        {showRightPanel && (
          <>
            <RightPanelResizer />
            <div
              className="shrink-0 h-full overflow-hidden"
              style={{ width: rightPanelWidth }}
            >
              <RightPanel
                workspace={activeWorkspace}
                activeTab={rightPanelTab}
              />
            </div>
          </>
        )}
      </div>
    </>
  );
}
