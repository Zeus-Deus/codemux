import { Tabs, TabsList, TabsTrigger, TabsContent } from "@/components/ui/tabs";
import { Button } from "@/components/ui/button";
import { X } from "lucide-react";
import { useUIStore, type RightPanelTab } from "@/stores/ui-store";
import { ChangesPanel } from "@/components/workspace/changes-panel";
import { FileTreePanel } from "@/components/workspace/file-tree-panel";
import { PrPanel } from "@/components/workspace/pr-panel";
import type { WorkspaceSnapshot } from "@/tauri/types";

interface Props {
  workspace: WorkspaceSnapshot;
  activeTab: RightPanelTab;
}

export function RightPanel({ workspace, activeTab }: Props) {
  const setRightPanelTab = useUIStore((s) => s.setRightPanelTab);

  const handleTabChange = (value: string) => {
    setRightPanelTab(workspace.workspace_id, value as RightPanelTab);
  };

  const handleClose = () => {
    setRightPanelTab(workspace.workspace_id, null);
  };

  return (
    <div className="flex h-full min-h-0 flex-col border-l border-border/50 bg-card overflow-hidden">
      <Tabs
        value={activeTab}
        onValueChange={handleTabChange}
        className="flex h-full flex-col"
      >
        <div className="flex items-center border-b border-border/50 px-1">
          <TabsList variant="line" className="h-9 flex-1">
            <TabsTrigger value="changes" className="px-3 py-1.5 text-sm after:!hidden data-active:!bg-sidebar-accent data-active:!text-sidebar-accent-foreground data-active:!font-medium hover:bg-sidebar-accent/50 rounded-md">
              Changes
              {workspace.git_changed_files > 0 && (
                <span className="ml-1 text-[10px] tabular-nums text-muted-foreground">
                  {workspace.git_changed_files}
                </span>
              )}
            </TabsTrigger>
            <TabsTrigger value="files" className="px-3 py-1.5 text-sm after:!hidden data-active:!bg-sidebar-accent data-active:!text-sidebar-accent-foreground data-active:!font-medium hover:bg-sidebar-accent/50 rounded-md">
              Files
            </TabsTrigger>
            <TabsTrigger value="pr" className="px-3 py-1.5 text-sm after:!hidden data-active:!bg-sidebar-accent data-active:!text-sidebar-accent-foreground data-active:!font-medium hover:bg-sidebar-accent/50 rounded-md">
              PR
              {workspace.pr_number && (
                <span className="ml-1 text-[10px] tabular-nums text-muted-foreground">
                  #{workspace.pr_number}
                </span>
              )}
            </TabsTrigger>
          </TabsList>
          <Button
            variant="ghost"
            size="icon-xs"
            className="shrink-0"
            onClick={handleClose}
            title="Close panel"
          >
            <X className="h-3 w-3" />
          </Button>
        </div>
        <TabsContent value="changes" className="flex-1 overflow-hidden">
          <ChangesPanel workspace={workspace} />
        </TabsContent>
        <TabsContent value="files" className="flex-1 overflow-hidden">
          <FileTreePanel workspace={workspace} />
        </TabsContent>
        <TabsContent value="pr" className="flex-1 overflow-hidden">
          <PrPanel workspace={workspace} />
        </TabsContent>
      </Tabs>
    </div>
  );
}
