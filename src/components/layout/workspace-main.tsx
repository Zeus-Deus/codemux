import { useActiveWorkspace } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
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

  if (!activeWorkspace) return null;

  return (
    <>
      <TabBar workspace={activeWorkspace} />
      <div className="flex-1 overflow-hidden">
        {rightPanelTab !== null ? (
          <ResizablePanelGroup orientation="horizontal">
            <ResizablePanel defaultSize={75} minSize={30}>
              <PaneContainer workspace={activeWorkspace} />
            </ResizablePanel>
            <ResizableHandle />
            <ResizablePanel defaultSize={25} minSize={15} maxSize={40}>
              <RightPanel workspace={activeWorkspace} activeTab={rightPanelTab} />
            </ResizablePanel>
          </ResizablePanelGroup>
        ) : (
          <PaneContainer workspace={activeWorkspace} />
        )}
      </div>
    </>
  );
}
