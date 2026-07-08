import { useState } from "react";
import {
  ChevronDown,
  FileCode,
  GitCompare,
  Globe,
  History,
  MessageSquare,
  Terminal,
  X,
} from "lucide-react";

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { RestoreCheckpointDialog } from "@/components/chat/restore-checkpoint-dialog";
import {
  SessionHistoryList,
  useSessionHistory,
} from "@/components/chat/session-history-menu";
import { PresetIcon } from "@/components/icons/preset-icon";
import { StatusIndicator } from "@/components/ui/status-indicator";
import { useAgentChatCheckpointRestore } from "@/hooks/use-agent-chat-checkpoint-restore";
import { useAgentChatSessionActions } from "@/hooks/use-agent-chat-session-actions";
import { countRunningSubagents } from "@/lib/agent-chat/subagents";
import { getHighestPriorityStatus } from "@/lib/pane-status";
import { cn } from "@/lib/utils";
import { selectThread, useAgentChatStore } from "@/stores/agent-chat-store";
import { useAppStore } from "@/stores/app-store";
import { activateTab, closeTab } from "@/tauri/commands";
import type {
  ActivePaneStatus,
  PaneNodeSnapshot,
  PaneStatus,
  TabKind,
  TabSnapshot,
  WorkspaceSnapshot,
} from "@/tauri/types";

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

const PILL_BASE =
  "group/tab flex h-7 shrink-0 items-center gap-1 rounded-lg pl-2.5 pr-1 text-xs transition-colors";
const PILL_ACTIVE = "bg-background text-foreground font-semibold";
const PILL_INACTIVE =
  "font-medium text-muted-foreground/80 hover:bg-muted/40 hover:text-muted-foreground";
const CLOSE_BTN =
  "ml-0.5 rounded-sm p-0.5 text-muted-foreground/70 transition-colors hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring";

interface TitleBarTabsProps {
  workspace: WorkspaceSnapshot;
}

/**
 * Workspace tabs merged into the title bar for GUI chrome. Each tab is a
 * compact pill; the active chat tab grows a chevron that opens the shared
 * session-history dropdown, and a live "N subagents running" pill rides
 * alongside it. Tabs stay backend-owned — activation/close go through the
 * existing commands.
 *
 * Tab drag-reorder is intentionally not ported here: the title bar owns
 * the OS drag region, and reusing TabBar's manual HTML5-drag logic inside
 * a `data-tauri-drag-region` parent is not clean. Reorder remains
 * available via the legacy TabBar (flag off) and is a follow-up for GUI
 * chrome.
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
    const statuses: (PaneStatus | undefined)[] = ids.map((id) => paneStatuses[id]);
    const highest = getHighestPriorityStatus(statuses);
    if (highest) tabStatusMap.set(tab.tab_id, highest);
  }

  return (
    <div
      className="flex min-w-0 items-center gap-1 overflow-x-auto"
      style={{ scrollbarWidth: "none" }}
    >
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

        if (isActive && chatPane) {
          return (
            <ActiveChatTab
              key={tab.tab_id}
              workspace={workspace}
              tab={tab}
              pane={chatPane}
              status={status}
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
}

function TitleBarTab({
  workspace,
  tab,
  isActive,
  isChat,
  status,
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
    <div className={cn(PILL_BASE, isActive ? PILL_ACTIVE : PILL_INACTIVE)}>
      <button
        type="button"
        className="flex min-w-0 items-center gap-1.5"
        onClick={handleActivate}
        title={tab.title}
      >
        <span className="shrink-0 opacity-90">{tabIcon(tab, isChat)}</span>
        <span className="max-w-[140px] truncate">{tab.title}</span>
        {status && <StatusIndicator status={status} />}
      </button>
      <button
        type="button"
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
}

function ActiveChatTab({ workspace, tab, pane, status }: ActiveChatTabProps) {
  const [open, setOpen] = useState(false);
  const workspaceId = workspace.workspace_id;

  const { cwd, handleSelect, handleNewChat } = useAgentChatSessionActions(pane);
  const { sessions, loading, handleDelete } = useSessionHistory({
    open,
    workspaceId,
    cwd,
  });
  const {
    checkpoint,
    turnActive,
    confirmOpen,
    setConfirmOpen,
    restoring,
    handleRestoreConfirmed,
  } = useAgentChatCheckpointRestore(pane.thread_id);
  const runningSubagents = useAgentChatStore((s) => {
    const slice = pane.thread_id ? selectThread(pane.thread_id)(s) : null;
    return slice ? countRunningSubagents(slice.messages) : 0;
  });

  const handleClose = (e: React.MouseEvent) => {
    e.stopPropagation();
    closeTab(workspaceId, tab.tab_id).catch(console.error);
  };

  return (
    <>
      <div className={cn(PILL_BASE, PILL_ACTIVE)}>
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
              <span className="max-w-[140px] truncate">{tab.title}</span>
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
              footerItem={
                checkpoint ? (
                  <DropdownMenuItem
                    disabled={turnActive || restoring}
                    onSelect={() => setConfirmOpen(true)}
                    data-testid="titlebar-restore-checkpoint"
                  >
                    <History className="h-3.5 w-3.5" />
                    Restore checkpoint
                  </DropdownMenuItem>
                ) : undefined
              }
            />
          </DropdownMenuContent>
        </DropdownMenu>
        <button
          type="button"
          onClick={handleClose}
          aria-label="Close tab"
          title="Close tab"
          className={cn(CLOSE_BTN, "opacity-70")}
        >
          <X className="h-3 w-3" />
        </button>
      </div>

      {/* "N subagents running" — status signal that used to sit on the
          pane sub-header; kept alive inline next to the active chat tab. */}
      {runningSubagents > 0 && (
        <span
          className="inline-flex shrink-0 items-center gap-1.5 pl-0.5 text-[11px] font-semibold text-status-working"
          data-testid="titlebar-subagents-pill"
        >
          <span
            className="cm-blink h-1.5 w-1.5 rounded-full bg-status-working"
            aria-hidden
          />
          {runningSubagents} subagent{runningSubagents === 1 ? "" : "s"} running
        </span>
      )}

      <RestoreCheckpointDialog
        open={confirmOpen}
        onOpenChange={setConfirmOpen}
        checkpoint={checkpoint}
        restoring={restoring}
        onConfirm={() => void handleRestoreConfirmed()}
      />
    </>
  );
}
