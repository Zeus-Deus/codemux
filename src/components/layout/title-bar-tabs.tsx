import { useCallback, useEffect, useRef, useState } from "react";
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

// Movement (px) before a pointerdown on a pill turns into a reorder drag
// instead of resolving as a plain click. Small enough to feel immediate,
// large enough that a normal click/tap never misfires as a drag.
const DRAG_THRESHOLD = 5;

interface PillReorderHandlers {
  "data-tab-id": string;
  onPointerDown: (e: React.PointerEvent<HTMLDivElement>) => void;
  onClickCapture: (e: React.MouseEvent<HTMLDivElement>) => void;
}

/**
 * Pointer-based drag-to-reorder for the titlebar tab strip. HTML5 DnD (what
 * the legacy `TabBar` uses) is avoided on purpose: the titlebar's center is
 * a live `data-tauri-drag-region`, and stacking native HTML5 drag — which
 * Tauri/WebKit's own drag-region handling already interposes on — on top of
 * that is exactly the "not clean" combination the GUI-chrome doc flagged as
 * a follow-up. Pointer events don't interact with the OS drag region at all
 * and give full control over when a "drag" actually starts.
 *
 * Listeners for pointermove/up/cancel are attached to `document` (not the
 * pill itself) for the lifetime of one pointer session, so fast pointer
 * movement that outruns the small pill's bounds — which would otherwise
 * stop delivering events to a per-element listener — still gets tracked.
 * `setPointerCapture` isn't used: capturing on the pointerdown target would
 * retarget the browser's synthesized `click` to that same target too,
 * which would break the inner activate/close buttons ever receiving a
 * plain click.
 */
function useTabReorder(workspace: WorkspaceSnapshot) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [dragTabId, setDragTabId] = useState<string | null>(null);
  const [dropIndex, setDropIndexState] = useState<number | null>(null);

  // Refs mirror state the pointer listeners need to read/write without
  // forcing a hook re-subscription (workspace) or a render on every write
  // that doesn't need one.
  const dropIndexRef = useRef<number | null>(null);
  const workspaceRef = useRef(workspace);
  workspaceRef.current = workspace;

  const pendingTabIdRef = useRef<string | null>(null);
  const pointerIdRef = useRef<number | null>(null);
  const startPosRef = useRef({ x: 0, y: 0 });
  const draggingRef = useRef(false);
  const suppressClickRef = useRef(false);
  const cleanupRef = useRef<(() => void) | null>(null);

  const setDropIndex = useCallback((v: number | null) => {
    dropIndexRef.current = v;
    setDropIndexState(v);
  }, []);

  const computeDropIndex = useCallback(
    (clientX: number) => {
      const el = containerRef.current;
      if (!el) return;
      const tabEls = el.querySelectorAll<HTMLElement>("[data-tab-id]");
      if (tabEls.length === 0) return;

      let closestIdx = 0;
      let closestDist = Infinity;
      let insertBefore = true;
      tabEls.forEach((tabEl, i) => {
        const rect = tabEl.getBoundingClientRect();
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
    [setDropIndex],
  );

  const endSession = useCallback(
    (commit: boolean) => {
      cleanupRef.current?.();
      cleanupRef.current = null;

      const tabId = pendingTabIdRef.current;
      const wasDragging = draggingRef.current;
      const finalDropIndex = dropIndexRef.current;

      pendingTabIdRef.current = null;
      pointerIdRef.current = null;
      draggingRef.current = false;
      setDragTabId(null);
      setDropIndex(null);

      if (commit && wasDragging && tabId != null && finalDropIndex != null) {
        const ws = workspaceRef.current;
        const currentIds = ws.tabs.map((t) => t.tab_id);
        const dragIdx = currentIds.indexOf(tabId);
        if (dragIdx >= 0) {
          const newIds = [...currentIds];
          newIds.splice(dragIdx, 1);
          const insertAt =
            finalDropIndex > dragIdx ? finalDropIndex - 1 : finalDropIndex;
          newIds.splice(Math.min(insertAt, newIds.length), 0, tabId);
          if (newIds.join(",") !== currentIds.join(",")) {
            reorderTabs(ws.workspace_id, newIds).catch(console.error);
          }
        }
      }
    },
    [setDropIndex],
  );

  const handlePointerDown = useCallback(
    (tabId: string) => (e: React.PointerEvent<HTMLDivElement>) => {
      // Only the primary button/contact starts a reorder session, never one
      // already in flight, and never from a close/chevron control
      // explicitly opted out via `data-no-drag` — those need their plain
      // click behavior preserved untouched.
      if (e.button !== 0) return;
      if ((e.target as HTMLElement).closest("[data-no-drag]")) return;
      if (pendingTabIdRef.current != null) return;

      pendingTabIdRef.current = tabId;
      pointerIdRef.current = e.pointerId;
      startPosRef.current = { x: e.clientX, y: e.clientY };
      draggingRef.current = false;

      const handleMove = (ev: PointerEvent) => {
        if (ev.pointerId !== pointerIdRef.current) return;
        const dx = ev.clientX - startPosRef.current.x;
        const dy = ev.clientY - startPosRef.current.y;
        if (!draggingRef.current) {
          if (Math.hypot(dx, dy) < DRAG_THRESHOLD) return;
          draggingRef.current = true;
          setDragTabId(pendingTabIdRef.current);
        }
        ev.preventDefault();
        computeDropIndex(ev.clientX);
      };

      const handleUp = (ev: PointerEvent) => {
        if (ev.pointerId !== pointerIdRef.current) return;
        if (draggingRef.current) suppressClickRef.current = true;
        endSession(true);
      };

      const handleCancel = (ev: PointerEvent) => {
        if (ev.pointerId !== pointerIdRef.current) return;
        endSession(false);
      };

      document.addEventListener("pointermove", handleMove);
      document.addEventListener("pointerup", handleUp);
      document.addEventListener("pointercancel", handleCancel);
      cleanupRef.current = () => {
        document.removeEventListener("pointermove", handleMove);
        document.removeEventListener("pointerup", handleUp);
        document.removeEventListener("pointercancel", handleCancel);
      };
    },
    [computeDropIndex, endSession],
  );

  // Belt-and-suspenders: drop any live document listeners if the strip
  // unmounts mid-drag (e.g. workspace switch).
  useEffect(() => () => cleanupRef.current?.(), []);

  const getPillProps = useCallback(
    (tabId: string): PillReorderHandlers => ({
      "data-tab-id": tabId,
      onPointerDown: handlePointerDown(tabId),
      onClickCapture: (e: React.MouseEvent<HTMLDivElement>) => {
        // Swallow the click synthesized after a real drag so it doesn't
        // activate the tab (or, for the active chat tab, pop the history
        // dropdown) as a side effect of the reorder gesture.
        if (suppressClickRef.current) {
          e.preventDefault();
          e.stopPropagation();
          suppressClickRef.current = false;
        }
      },
    }),
    [handlePointerDown],
  );

  // Drop-indicator screen position — a vertical mirror of TabBar's
  // leading-dot + thin line. Converted into the strip's scrolled content
  // coordinate space (`+ scrollLeft`) since the strip scrolls
  // horizontally, unlike TabBar's non-scrolling list.
  let dropIndicatorLeft: number | null = null;
  if (dragTabId && dropIndex !== null && containerRef.current) {
    const el = containerRef.current;
    const tabEls = el.querySelectorAll<HTMLElement>("[data-tab-id]");
    const listRect = el.getBoundingClientRect();
    if (tabEls.length > 0) {
      if (dropIndex >= tabEls.length) {
        const lastRect = tabEls[tabEls.length - 1].getBoundingClientRect();
        dropIndicatorLeft = lastRect.right - listRect.left + el.scrollLeft;
      } else {
        const targetRect = tabEls[dropIndex].getBoundingClientRect();
        dropIndicatorLeft = targetRect.left - listRect.left + el.scrollLeft;
      }
    }
  }

  return { containerRef, dragTabId, dropIndicatorLeft, getPillProps };
}

/**
 * Workspace tabs merged into the title bar for GUI chrome. Each tab is a
 * compact pill; the active chat tab grows a chevron that opens the shared
 * session-history dropdown. Live subagent status no longer rides an
 * inline pill here — it lives in the docked `SubagentActivityBar` above
 * the composer. Tabs stay backend-owned — activation/close/reorder go through
 * the existing commands. Reorder is a pointer-driven drag (see
 * `useTabReorder` below) rather than the legacy TabBar's HTML5 DnD.
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

  const { containerRef, dragTabId, dropIndicatorLeft, getPillProps } =
    useTabReorder(workspace);

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
      {/* Drop indicator — vertical mirror of the sidebar's leading-dot +
          thin neutral line. No accent color, so it reads as a UI cue
          rather than an alert. */}
      {dragTabId && dropIndicatorLeft !== null && (
        <div
          className="absolute inset-y-0.5 z-30 flex flex-col items-center pointer-events-none"
          style={{ left: dropIndicatorLeft - 1, width: 2 }}
        >
          <div className="-mt-0.5 h-1.5 w-1.5 shrink-0 rounded-full bg-foreground/70" />
          <div className="w-px flex-1 rounded-full bg-foreground/40" />
        </div>
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
