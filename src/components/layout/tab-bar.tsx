import { useState, useRef, useCallback, memo } from "react";
import { Tabs, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Button } from "@/components/ui/button";
import {
  ContextMenu,
  ContextMenuTrigger,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
} from "@/components/ui/context-menu";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Plus, X, Terminal, Globe, GitCompare, PanelRight, FileCode } from "lucide-react";
import { PresetIcon } from "@/components/icons/preset-icon";
import {
  activateTab,
  closeTab,
  createTab,
  createBrowserPane,
  reorderTabs,
  renameTab,
  splitPane,
} from "@/tauri/commands";
import { RIGHT_PANEL_EMPTY, useUIStore } from "@/stores/ui-store";
import type { WorkspaceSnapshot, TabKind, ActivePaneStatus, PaneStatus, PaneNodeSnapshot } from "@/tauri/types";
import { useAppStore } from "@/stores/app-store";
import { useEditorStore } from "@/stores/editor-store";
import { getHighestPriorityStatus } from "@/lib/pane-status";
import { StatusIndicator } from "@/components/ui/status-indicator";

interface Props {
  workspace: WorkspaceSnapshot;
}

const tabIcon: Record<TabKind, React.ReactNode> = {
  terminal: <Terminal className="h-3 w-3" />,
  browser: <Globe className="h-3 w-3" />,
  diff: <GitCompare className="h-3 w-3" />,
  editor: <FileCode className="h-3 w-3" />,
};

function collectPaneIds(node: PaneNodeSnapshot): string[] {
  if (node.kind === "split") return node.children.flatMap(collectPaneIds);
  return [node.pane_id];
}

function TabBarImpl({ workspace }: Props) {
  const setRightPanelTab = useUIStore((s) => s.setRightPanelTab);
  const collapseRightPanel = useUIStore((s) => s.collapseRightPanel);
  const rightPanelTab = useUIStore(
    (s) => s.rightPanelTabs[workspace.workspace_id] ?? null,
  );

  // True toggle: any open tab closes the panel; closed state opens on the
  // picker sentinel, which resolves to the panes the deck already has (and
  // never re-opens a Files pane the user closed). `toggleRightPanel` on the
  // store flips by *tab identity*, which would switch from Review→Files
  // instead of closing, taking two clicks to dismiss. Collapsing goes
  // through `collapseRightPanel` so the docked agent browser is released —
  // a collapsed panel is not a surface.
  const togglePanel = () => {
    if (rightPanelTab == null) {
      setRightPanelTab(workspace.workspace_id, RIGHT_PANEL_EMPTY);
    } else {
      collapseRightPanel(workspace.workspace_id);
    }
  };

  // Compute per-tab status from pane statuses
  const paneStatuses = useAppStore((s) => s.appState?.pane_statuses ?? {});
  const tabStatusMap = new Map<string, ActivePaneStatus>();
  for (const tab of workspace.tabs) {
    if (!tab.surface_id) continue;
    const surface = workspace.surfaces.find((s) => s.surface_id === tab.surface_id);
    if (!surface) continue;
    const ids = collectPaneIds(surface.root);
    const statuses: (PaneStatus | undefined)[] = ids.map((id) => paneStatuses[id]);
    const highest = getHighestPriorityStatus(statuses);
    if (highest) tabStatusMap.set(tab.tab_id, highest);
  }

  // Drag state
  const [dragTabId, setDragTabId] = useState<string | null>(null);
  const [dropIndex, setDropIndex] = useState<number | null>(null);
  const tabListRef = useRef<HTMLDivElement>(null);

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

  // --- Drag-and-drop handlers ---
  const handleDragStart = useCallback(
    (tabId: string) => (e: React.DragEvent) => {
      setDragTabId(tabId);
      e.dataTransfer.effectAllowed = "move";
      e.dataTransfer.setData("text/plain", tabId);
    },
    [],
  );

  const computeDropIndex = useCallback(
    (clientX: number) => {
      const listEl = tabListRef.current;
      if (!listEl) return;
      const tabEls = listEl.querySelectorAll<HTMLElement>("[data-tab-id]");
      if (tabEls.length === 0) return;

      let closestIdx = 0;
      let closestDist = Infinity;
      let insertBefore = true;

      tabEls.forEach((el, i) => {
        const rect = el.getBoundingClientRect();
        const midX = rect.left + rect.width / 2;
        const dist = Math.abs(clientX - midX);
        if (dist < closestDist) {
          closestDist = dist;
          closestIdx = i;
          insertBefore = clientX < midX;
        }
      });

      setDropIndex(insertBefore ? closestIdx : closestIdx + 1);
    },
    [],
  );

  const handleDragOver = useCallback(
    (e: React.DragEvent) => {
      if (!dragTabId) return;
      e.preventDefault();
      e.dataTransfer.dropEffect = "move";
      computeDropIndex(e.clientX);
    },
    [dragTabId, computeDropIndex],
  );

  const handleDrop = useCallback(
    async (e: React.DragEvent) => {
      e.preventDefault();
      if (!dragTabId || dropIndex === null) {
        setDragTabId(null);
        setDropIndex(null);
        return;
      }

      const currentIds = workspace.tabs.map((t) => t.tab_id);
      const dragIdx = currentIds.indexOf(dragTabId);
      if (dragIdx < 0) {
        setDragTabId(null);
        setDropIndex(null);
        return;
      }

      const newIds = [...currentIds];
      newIds.splice(dragIdx, 1);
      const insertAt = dropIndex > dragIdx ? dropIndex - 1 : dropIndex;
      newIds.splice(Math.min(insertAt, newIds.length), 0, dragTabId);

      if (newIds.join(",") !== currentIds.join(",")) {
        await reorderTabs(workspace.workspace_id, newIds).catch(console.error);
      }

      setDragTabId(null);
      setDropIndex(null);
    },
    [dragTabId, dropIndex, workspace],
  );

  const handleDragEnd = useCallback(() => {
    setDragTabId(null);
    setDropIndex(null);
  }, []);

  // Clear the indicator when the cursor leaves the tab list. Without
  // this it hangs at its last position whenever the user drags into
  // the pane content area and back. relatedTarget is the element
  // entered; only clear when leaving the subtree entirely.
  const handleDragLeave = useCallback(
    (e: React.DragEvent) => {
      const listEl = tabListRef.current;
      const next = e.relatedTarget as Node | null;
      if (!listEl || (next && listEl.contains(next))) return;
      setDropIndex(null);
    },
    [],
  );

  // --- Context menu handlers ---
  const handleCloseOtherTabs = async (keepTabId: string) => {
    for (const tab of workspace.tabs) {
      if (tab.tab_id !== keepTabId) {
        await closeTab(workspace.workspace_id, tab.tab_id).catch(console.error);
      }
    }
  };

  const handleCloseTabsToRight = async (tabId: string) => {
    const idx = workspace.tabs.findIndex((t) => t.tab_id === tabId);
    for (let i = workspace.tabs.length - 1; i > idx; i--) {
      await closeTab(workspace.workspace_id, workspace.tabs[i].tab_id).catch(console.error);
    }
  };

  const handleSplit = (direction: "horizontal" | "vertical") => {
    const surface = workspace.surfaces.find(
      (s) => s.surface_id === workspace.active_surface_id,
    );
    if (surface) {
      splitPane(surface.active_pane_id, direction).catch(console.error);
    }
  };

  const handleRenameTab = (tabId: string, currentTitle: string) => {
    const newTitle = window.prompt("Rename tab", currentTitle);
    if (newTitle && newTitle !== currentTitle) {
      renameTab(workspace.workspace_id, tabId, newTitle).catch(console.error);
    }
  };

  // Compute drop indicator position
  let dropIndicatorLeft: number | null = null;
  if (dropIndex !== null && tabListRef.current) {
    const tabEls = tabListRef.current.querySelectorAll<HTMLElement>("[data-tab-id]");
    const listRect = tabListRef.current.getBoundingClientRect();
    if (tabEls.length > 0) {
      if (dropIndex >= tabEls.length) {
        const lastRect = tabEls[tabEls.length - 1].getBoundingClientRect();
        dropIndicatorLeft = lastRect.right - listRect.left;
      } else {
        const targetRect = tabEls[dropIndex].getBoundingClientRect();
        dropIndicatorLeft = targetRect.left - listRect.left;
      }
    }
  }

  return (
    <div className="flex h-[45px] shrink-0 items-center border-b border-border bg-background">
      <Tabs
        value={workspace.active_tab_id}
        onValueChange={handleTabChange}
        className="flex-1 min-w-0 h-full"
      >
        <div
          ref={tabListRef}
          className="relative flex items-center h-full"
          onDragOver={handleDragOver}
          onDragLeave={handleDragLeave}
          onDrop={handleDrop}
          onDragEnd={handleDragEnd}
        >
          {/* Drop indicator — vertical mirror of the sidebar's
              leading-dot + thin neutral line. No accent color, so it
              reads as a UI cue rather than an alert. */}
          {dragTabId && dropIndicatorLeft !== null && (
            <div
              className="absolute top-1 bottom-1 z-30 pointer-events-none flex flex-col items-center"
              style={{ left: dropIndicatorLeft - 1, width: 2 }}
            >
              <div className="h-1.5 w-1.5 rounded-full bg-foreground/70 shrink-0 -mt-0.5" />
              <div className="w-px flex-1 bg-foreground/40 rounded-full" />
            </div>
          )}
          <TabsList variant="line" className="!h-full !p-0 gap-0">
            {workspace.tabs.map((tab, idx) => (
              <ContextMenu key={tab.tab_id}>
                <ContextMenuTrigger asChild>
                  <div
                    data-tab-id={tab.tab_id}
                    data-tab-index={idx}
                    draggable
                    onDragStart={handleDragStart(tab.tab_id)}
                    className={`h-full ${dragTabId === tab.tab_id ? "opacity-40" : ""}`}
                  >
                    <TabsTrigger
                      value={tab.tab_id}
                      className="group relative gap-1 px-3 !h-full !py-0 !m-0 text-xs !rounded-none !border-transparent !shadow-none after:!hidden data-[state=active]:!bg-card data-[state=active]:!text-foreground data-[state=inactive]:!text-muted-foreground/70 data-[state=inactive]:!border-r data-[state=inactive]:!border-r-border/40 data-[state=inactive]:hover:!text-muted-foreground data-[state=inactive]:hover:!bg-muted/20"
                    >
                      {tab.icon ? <PresetIcon icon={tab.icon} className="h-3 w-3" /> : tabIcon[tab.kind]}
                      <span className="truncate max-w-[120px]">{tab.title}</span>
                      {tab.kind === "editor" && <EditorDirtyDot tabId={tab.tab_id} />}
                      {tabStatusMap.has(tab.tab_id) && (
                        <StatusIndicator status={tabStatusMap.get(tab.tab_id)!} />
                      )}
                      <span
                        role="button"
                        tabIndex={0}
                        className="ml-0.5 rounded-sm p-0.5 opacity-0 hover:bg-muted group-hover:opacity-100 transition-opacity focus-visible:opacity-100 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring cursor-pointer"
                        onClick={(e) => handleCloseTab(e, tab.tab_id)}
                        onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") handleCloseTab(e as unknown as React.MouseEvent, tab.tab_id); }}
                        aria-label="Close tab"
                        title="Close tab"
                      >
                        <X className="h-2.5 w-2.5" />
                      </span>
                    </TabsTrigger>
                  </div>
                </ContextMenuTrigger>
                <ContextMenuContent>
                  <ContextMenuItem
                    onClick={() => closeTab(workspace.workspace_id, tab.tab_id).catch(console.error)}
                  >
                    Close tab
                  </ContextMenuItem>
                  <ContextMenuItem
                    onClick={() => handleCloseOtherTabs(tab.tab_id)}
                    disabled={workspace.tabs.length <= 1}
                  >
                    Close other tabs
                  </ContextMenuItem>
                  <ContextMenuItem
                    onClick={() => handleCloseTabsToRight(tab.tab_id)}
                    disabled={idx >= workspace.tabs.length - 1}
                  >
                    Close tabs to the right
                  </ContextMenuItem>
                  <ContextMenuSeparator />
                  <ContextMenuItem onClick={() => handleSplit("horizontal")}>
                    Split right
                  </ContextMenuItem>
                  <ContextMenuItem onClick={() => handleSplit("vertical")}>
                    Split down
                  </ContextMenuItem>
                  <ContextMenuSeparator />
                  <ContextMenuItem onClick={() => handleRenameTab(tab.tab_id, tab.title)}>
                    Rename tab
                  </ContextMenuItem>
                </ContextMenuContent>
              </ContextMenu>
            ))}
          </TabsList>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button
                variant="ghost"
                size="icon-sm"
                className="ml-1 shrink-0 bg-muted/50 text-muted-foreground"
                title="New tab"
                aria-label="New tab"
              >
                <Plus className="h-3.5 w-3.5" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start">
              <DropdownMenuItem onClick={handleCreateTab}>
                <Terminal className="h-3.5 w-3.5 mr-2" />
                Terminal
              </DropdownMenuItem>
              <DropdownMenuItem
                onClick={() => {
                  const surface = workspace.surfaces.find((s) => s.surface_id === workspace.active_surface_id);
                  if (surface) createBrowserPane(surface.active_pane_id).catch(console.error);
                }}
              >
                <Globe className="h-3.5 w-3.5 mr-2" />
                Browser
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </Tabs>

      <Button
        variant="ghost"
        size="icon-sm"
        className={`ml-1 shrink-0 ${rightPanelTab ? "bg-card text-foreground" : "text-muted-foreground hover:text-foreground"}`}
        onClick={togglePanel}
        title={rightPanelTab ? "Close panel" : "Open panel"}
        aria-pressed={rightPanelTab != null}
      >
        <PanelRight className="h-3.5 w-3.5" />
      </Button>
    </div>
  );
}

function EditorDirtyDot({ tabId }: { tabId: string }) {
  const isDirty = useEditorStore((s) => s.getTab(tabId)?.isDirty ?? false);
  if (!isDirty) return null;
  return <span className="w-1.5 h-1.5 rounded-full bg-foreground/50 shrink-0" title="Unsaved changes" />;
}

// #127: memo is effective because setAppState performs structural sharing, so
// the `workspace` snapshot keeps a stable ref across backend ticks that don't
// change it — shallow compare then skips this tab strip's re-render.
export const TabBar = memo(TabBarImpl);
TabBar.displayName = "TabBar";
