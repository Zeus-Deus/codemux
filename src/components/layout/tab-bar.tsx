import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Button } from "@/components/ui/button";
import { Plus, X, Terminal, Globe, GitCompare, PanelRight } from "lucide-react";
import { activateTab, closeTab, createTab, createBrowserPane } from "@/tauri/commands";
import { useUIStore } from "@/stores/ui-store";
import type { WorkspaceSnapshot, TabKind } from "@/tauri/types";

interface Props {
  workspace: WorkspaceSnapshot;
}

const tabIcon: Record<TabKind, React.ReactNode> = {
  terminal: <Terminal className="h-3 w-3" />,
  browser: <Globe className="h-3 w-3" />,
  diff: <GitCompare className="h-3 w-3" />,
};

export function TabBar({ workspace }: Props) {
  const toggleRightPanel = useUIStore((s) => s.toggleRightPanel);
  const rightPanelTab = useUIStore(
    (s) => s.rightPanelTabs[workspace.workspace_id] ?? null,
  );

  const handleTabChange = (tabId: string) => {
    if (tabId !== workspace.active_tab_id) {
      activateTab(workspace.workspace_id, tabId).catch(console.error);
    }
  };

  const handleCloseTab = (e: React.MouseEvent, tabId: string) => {
    e.stopPropagation();
    closeTab(workspace.workspace_id, tabId).catch(console.error);
  };

  const handleCreateTab = () => {
    createTab(workspace.workspace_id, "terminal").catch(console.error);
  };

  return (
    <div className="flex h-8 shrink-0 items-center border-b border-border bg-card px-1.5">
      <Tabs
        value={workspace.active_tab_id}
        onValueChange={handleTabChange}
        className="flex-1 min-w-0"
      >
        <TabsList variant="line" className="h-full gap-0">
          {workspace.tabs.map((tab) => (
            <TabsTrigger
              key={tab.tab_id}
              value={tab.tab_id}
              className="group relative gap-1 px-2 py-0.5 text-xs"
            >
              {tabIcon[tab.kind]}
              <span className="truncate max-w-[120px]">{tab.title}</span>
              {workspace.tabs.length > 1 && (
                <button
                  className="ml-0.5 rounded-sm p-0.5 opacity-0 hover:bg-muted group-hover:opacity-100 transition-opacity focus-visible:opacity-100 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                  onClick={(e) => handleCloseTab(e, tab.tab_id)}
                  aria-label="Close tab"
                  title="Close tab"
                >
                  <X className="h-2.5 w-2.5" />
                </button>
              )}
            </TabsTrigger>
          ))}
        </TabsList>
      </Tabs>

      <Button
        variant="ghost"
        size="icon-sm"
        className="ml-1 shrink-0"
        onClick={handleCreateTab}
        title="New terminal tab"
        aria-label="New terminal tab"
      >
        <Plus className="h-3.5 w-3.5" />
      </Button>
      <Button
        variant="ghost"
        size="icon-sm"
        className="shrink-0"
        onClick={() => {
          const surface = workspace.surfaces.find((s) => s.surface_id === workspace.active_surface_id);
          if (surface) createBrowserPane(surface.active_pane_id).catch(console.error);
        }}
        title="Open browser"
        aria-label="Open browser"
      >
        <Globe className="h-3.5 w-3.5" />
      </Button>

      <Button
        variant={rightPanelTab ? "secondary" : "ghost"}
        size="icon-sm"
        className="ml-1 shrink-0"
        onClick={() => toggleRightPanel(workspace.workspace_id, "changes")}
        title="Toggle panel"
      >
        <PanelRight className="h-3.5 w-3.5" />
      </Button>
    </div>
  );
}
