import { useCallback, useState } from "react";
import {
  ChevronDown,
  FileCode,
  GitCompare,
  Globe,
  MessageSquare,
  Terminal,
  X,
} from "lucide-react";

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  SessionHistoryList,
  useSessionHistory,
} from "@/components/chat/session-history-menu";
import { PresetIcon } from "@/components/icons/preset-icon";
import { StatusIndicator } from "@/components/ui/status-indicator";
import { useAgentChatSessionActions } from "@/hooks/use-agent-chat-session-actions";
import {
  BAND_ACTIVE_FILL,
  BAND_CONTROL_RADIUS,
} from "@/components/layout/titlebar-control-style";
import { getHighestPriorityStatus } from "@/lib/pane-status";
import { cn } from "@/lib/utils";
import { useTabReorder, type PillReorderHandlers } from "@/lib/tab-reorder";
import { useHorizontalWheelScroll } from "@/lib/wheel";
import { useAppStore } from "@/stores/app-store";
import { activateTab, closeTab, reorderTabs } from "@/tauri/commands";
import type {
  ActivePaneStatus,
  PaneNodeSnapshot,
  PaneStatus,
  TabKind,
  TabSnapshot,
  WorkspaceSnapshot,
} from "@/tauri/types";

import { TabDropIndicator } from "./tab-drop-indicator";

type AgentChatPaneNode = Extract<PaneNodeSnapshot, { kind: "agent_chat" }>;

// Stable empty ref so the pane_statuses selector doesn't return a fresh
// object every render (which would loop useSyncExternalStore).
const EMPTY_PANE_STATUSES: Record<string, PaneStatus> = {};

const tabKindIcon: Record<TabKind, React.ReactNode> = {
  terminal: <Terminal className="h-3 w-3" />,
  browser: <Globe className="h-3 w-3" />,
  diff: <GitCompare className="h-3 w-3" />,
  editor: <FileCode className="h-3 w-3" />,
};

function collectPaneIds(node: PaneNodeSnapshot): string[] {
  if (node.kind === "split") return node.children.flatMap(collectPaneIds);
  return [node.pane_id];
}

function tabIcon(tab: TabSnapshot, isChat: boolean): React.ReactNode {
  if (isChat) return <MessageSquare className="h-3 w-3" />;
  if (tab.icon) return <PresetIcon icon={tab.icon} className="h-3 w-3" />;
  return tabKindIcon[tab.kind];
}

// Only the active tab wears a pill. Inactive tabs are bare label text at
// rest — no fill, no border — and pick up a quiet hover fill on approach,
// so a strip of five tabs reads as one label row with one thing selected
// rather than five competing boxes stacked over the transcript. Radius is
// the band-wide control token, shared with the panel toggle and the 28px
// icon buttons.
const PILL_BASE = cn(
  "group/tab flex h-7 shrink-0 items-center gap-1 pl-2.5 pr-1 text-xs transition-colors",
  BAND_CONTROL_RADIUS,
);
const PILL_ACTIVE = cn(BAND_ACTIVE_FILL, "text-foreground font-semibold");
const PILL_INACTIVE =
  "font-medium text-muted-foreground hover:bg-muted/40 hover:text-foreground";
const CLOSE_BTN =
  "ml-0.5 rounded-sm p-0.5 text-muted-foreground/70 transition-colors hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring";

interface TitleBarTabsProps {
  workspace: WorkspaceSnapshot;
}

/**
 * Workspace tabs merged into the title bar for GUI chrome. Each tab is a
 * compact pill; the active chat tab grows a chevron that opens the shared
 * session-history dropdown. Live subagent status no longer rides an
 * inline pill here — it lives in the docked `SubagentActivityBar` above
 * the composer. Tabs stay backend-owned — activation/close/reorder go through
 * the existing commands. Reorder is a pointer-driven drag (see
 * `@/lib/tab-reorder`) rather than the legacy TabBar's HTML5 DnD.
 */
export function TitleBarTabs({ workspace }: TitleBarTabsProps) {
  const paneStatuses = useAppStore(
    (s) => s.appState?.pane_statuses ?? EMPTY_PANE_STATUSES,
  );

  // Per-tab status = highest-priority status across the tab's panes.
  const tabStatusMap = new Map<string, ActivePaneStatus>();
  for (const tab of workspace.tabs) {
    if (!tab.surface_id) continue;
    const surface = workspace.surfaces.find(
      (s) => s.surface_id === tab.surface_id,
    );
    if (!surface) continue;
    const ids = collectPaneIds(surface.root);
    const statuses: (PaneStatus | undefined)[] = ids.map(
      (id) => paneStatuses[id],
    );
    const highest = getHighestPriorityStatus(statuses);
    if (highest) tabStatusMap.set(tab.tab_id, highest);
  }

  // Tabs stay backend-owned, so a drop hands the new order to the reorder
  // command rather than a store. Shared with the right panel's pane deck —
  // see `@/lib/tab-reorder`.
  const tabIds = workspace.tabs.map((t) => t.tab_id);
  const commitReorder = useCallback(
    (ids: string[]) => {
      reorderTabs(workspace.workspace_id, ids).catch(console.error);
    },
    [workspace.workspace_id],
  );
  const { containerRef, dragTabId, dropIndicatorLeft, getPillProps } =
    useTabReorder<HTMLDivElement>(tabIds, commitReorder);

  // Let tabs scroll with a normal vertical mouse wheel once they overflow,
  // same fix as the preset bar: `overflow-x: auto` only responds to native
  // horizontal wheel/trackpad input on its own (verified on the WebKit
  // webview — a vertical wheel over the bar moved `scrollLeft` by 0).
  // Attached natively so the gesture can be consumed rather than also
  // scrolling an ancestor. See `@/lib/wheel`.
  const attachWheelScroll = useHorizontalWheelScroll<HTMLDivElement>();
  // The scroller is also the reorder hook's measurement container, so both
  // consumers are fed from one ref callback.
  const setScrollerNode = useCallback(
    (node: HTMLDivElement | null) => {
      containerRef.current = node;
      attachWheelScroll(node);
    },
    [attachWheelScroll, containerRef],
  );

  return (
    <div
      ref={setScrollerNode}
      className="relative flex min-w-0 items-center gap-[2px] overflow-x-auto"
      style={{ scrollbarWidth: "none" }}
      data-testid="titlebar-tabs-scroll"
    >
      {dragTabId && dropIndicatorLeft !== null && (
        <TabDropIndicator left={dropIndicatorLeft} />
      )}
      {workspace.tabs.map((tab) => {
        const surface = tab.surface_id
          ? workspace.surfaces.find((s) => s.surface_id === tab.surface_id)
          : undefined;
        const chatPane =
          surface && surface.root.kind === "agent_chat"
            ? (surface.root as AgentChatPaneNode)
            : null;
        const isActive = tab.tab_id === workspace.active_tab_id;
        const status = tabStatusMap.get(tab.tab_id) ?? null;
        const reorderProps = getPillProps(tab.tab_id);
        const isDragging = dragTabId === tab.tab_id;

        if (isActive && chatPane) {
          return (
            <ActiveChatTab
              key={tab.tab_id}
              workspace={workspace}
              tab={tab}
              pane={chatPane}
              status={status}
              reorderProps={reorderProps}
              isDragging={isDragging}
            />
          );
        }

        return (
          <TitleBarTab
            key={tab.tab_id}
            workspace={workspace}
            tab={tab}
            isActive={isActive}
            isChat={!!chatPane}
            status={status}
            reorderProps={reorderProps}
            isDragging={isDragging}
          />
        );
      })}
    </div>
  );
}

interface TitleBarTabProps {
  workspace: WorkspaceSnapshot;
  tab: TabSnapshot;
  isActive: boolean;
  isChat: boolean;
  status: ActivePaneStatus | null;
  reorderProps: PillReorderHandlers;
  isDragging: boolean;
}

function TitleBarTab({
  workspace,
  tab,
  isActive,
  isChat,
  status,
  reorderProps,
  isDragging,
}: TitleBarTabProps) {
  const handleActivate = () => {
    if (!isActive) {
      activateTab(workspace.workspace_id, tab.tab_id).catch(console.error);
    }
  };
  const handleClose = (e: React.MouseEvent) => {
    e.stopPropagation();
    closeTab(workspace.workspace_id, tab.tab_id).catch(console.error);
  };

  return (
    <div
      {...reorderProps}
      className={cn(
        PILL_BASE,
        isActive ? PILL_ACTIVE : PILL_INACTIVE,
        isDragging && "opacity-40",
      )}
    >
      <button
        type="button"
        className="flex min-w-0 items-center gap-1.5"
        onClick={handleActivate}
        title={tab.title}
      >
        <span className="shrink-0 opacity-90">{tabIcon(tab, isChat)}</span>
        <span className="max-w-[130px] truncate">{tab.title}</span>
        {status && <StatusIndicator status={status} />}
      </button>
      <button
        type="button"
        data-no-drag
        onClick={handleClose}
        aria-label="Close tab"
        title="Close tab"
        className={cn(
          CLOSE_BTN,
          isActive
            ? "opacity-70"
            : "opacity-0 group-hover/tab:opacity-100 focus-visible:opacity-100",
        )}
      >
        <X className="h-3 w-3" />
      </button>
    </div>
  );
}

interface ActiveChatTabProps {
  workspace: WorkspaceSnapshot;
  tab: TabSnapshot;
  pane: AgentChatPaneNode;
  status: ActivePaneStatus | null;
  reorderProps: PillReorderHandlers;
  isDragging: boolean;
}

function ActiveChatTab({
  workspace,
  tab,
  pane,
  status,
  reorderProps,
  isDragging,
}: ActiveChatTabProps) {
  const [open, setOpen] = useState(false);
  const workspaceId = workspace.workspace_id;

  const { cwd, handleSelect, handleNewChat } = useAgentChatSessionActions(pane);
  const { sessions, loading, handleDelete } = useSessionHistory({
    open,
    workspaceId,
    cwd,
  });
  const handleClose = (e: React.MouseEvent) => {
    e.stopPropagation();
    closeTab(workspaceId, tab.tab_id).catch(console.error);
  };

  return (
    <>
      <div
        {...reorderProps}
        className={cn(PILL_BASE, PILL_ACTIVE, isDragging && "opacity-40")}
      >
        <DropdownMenu open={open} onOpenChange={setOpen}>
          <DropdownMenuTrigger asChild>
            <button
              type="button"
              className="flex min-w-0 items-center gap-1.5"
              title={tab.title}
              data-testid="titlebar-chat-tab-trigger"
            >
              <span className="shrink-0 opacity-90">
                <MessageSquare className="h-3 w-3" />
              </span>
              <span className="max-w-[130px] truncate">{tab.title}</span>
              {status && <StatusIndicator status={status} />}
              <ChevronDown
                className={cn(
                  "h-3 w-3 shrink-0 opacity-60 transition-transform",
                  open && "rotate-180",
                )}
              />
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent
            align="start"
            className="max-h-[400px] w-72 overflow-y-auto"
            data-testid="titlebar-chat-history"
          >
            <SessionHistoryList
              sessions={sessions}
              loading={loading}
              activeThreadId={pane.thread_id}
              onSelect={handleSelect}
              onNewChat={handleNewChat}
              onDelete={handleDelete}
              closeMenu={() => setOpen(false)}
            />
          </DropdownMenuContent>
        </DropdownMenu>
        <button
          type="button"
          data-no-drag
          onClick={handleClose}
          aria-label="Close tab"
          title="Close tab"
          className={cn(CLOSE_BTN, "opacity-70")}
        >
          <X className="h-3 w-3" />
        </button>
      </div>
    </>
  );
}
