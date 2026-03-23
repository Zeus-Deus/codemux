import { useRef, useEffect, useCallback } from "react";
import { useActiveWorkspace } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import type { PanelImperativeHandle } from "react-resizable-panels";
import {
  ResizablePanelGroup,
  ResizablePanel,
  ResizableHandle,
} from "@/components/ui/resizable";
import { TabBar } from "./tab-bar";
import { PaneContainer } from "./pane-container";
import { RightPanel } from "./right-panel";

export function WorkspaceMain() {
  const activeWorkspace = useActiveWorkspace();
  const rightPanelTab = useUIStore((s) =>
    activeWorkspace
      ? s.rightPanelTabs[activeWorkspace.workspace_id] ?? null
      : null,
  );
  const setRightPanelTab = useUIStore((s) => s.setRightPanelTab);
  const rightPanelRef = useRef<PanelImperativeHandle>(null);

  // Sync collapse/expand with UI store state
  useEffect(() => {
    if (rightPanelTab !== null) {
      rightPanelRef.current?.expand();
    } else {
      rightPanelRef.current?.collapse();
    }
  }, [rightPanelTab]);

  // Detect user dragging panel to collapsed size
  const handleRightPanelResize = useCallback(
    (size: { asPercentage: number; inPixels: number }) => {
      if (!activeWorkspace) return;
      if (size.asPercentage === 0 && rightPanelTab !== null) {
        setRightPanelTab(activeWorkspace.workspace_id, null);
      }
    },
    [activeWorkspace, rightPanelTab, setRightPanelTab],
  );

  if (!activeWorkspace) return null;

  const showRightPanel = rightPanelTab !== null;

  return (
    <>
      <TabBar workspace={activeWorkspace} />
      <div className="flex-1 min-h-0 h-full overflow-hidden">
        <ResizablePanelGroup orientation="horizontal" className="h-full">
          <ResizablePanel defaultSize={showRightPanel ? 75 : 100} minSize={30}>
            <PaneContainer workspace={activeWorkspace} />
          </ResizablePanel>
          <ResizableHandle withHandle />
          <ResizablePanel
            panelRef={rightPanelRef}
            defaultSize={showRightPanel ? 25 : 0}
            minSize={15}
            maxSize={40}
            collapsible
            collapsedSize={0}
            onResize={handleRightPanelResize}
          >
            <RightPanel
              workspace={activeWorkspace}
              activeTab={rightPanelTab ?? "changes"}
            />
          </ResizablePanel>
        </ResizablePanelGroup>
      </div>
    </>
  );
}
