import { ArrowDown, TriangleAlert } from "lucide-react";
import { LegendList, type LegendListRef } from "@legendapp/list/react";
import {
  memo,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type CSSProperties,
} from "react";

import { Button } from "@/components/ui/button";
import type {
  ChatViewItem,
  PermissionRequestItem,
} from "@/lib/agent-chat/types";
import { cn } from "@/lib/utils";
import { useAppStore } from "@/stores/app-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import type { ApprovalDecision } from "@/tauri/events";
import type { AgentChatProviderKind } from "@/tauri/types";

import { ActivityBlock } from "./ActivityBlock";
import { AssistantAvatar } from "./AssistantAvatar";
import { AssistantMessage } from "./AssistantMessage";
import { BackgroundBrowserChip } from "./BackgroundBrowserChip";
import { MessageTrail } from "./MessageTrail";
import { PermissionRequestBlock } from "./PermissionRequestBlock";
import { PlanProposalBlock } from "./PlanProposalBlock";
import { ReasoningBlock } from "./ReasoningBlock";
import { StreamingMarker } from "./StreamingMarker";
import { SubagentsCard } from "./SubagentsCard";
import { isTaskSummaryTool, TaskSummaryCard } from "./TaskSummaryCard";
import { ToolCallCard } from "./ToolCallCard";
import { UserInputAnswer } from "./UserInputAnswer";
import { UserMessage } from "./UserMessage";
import { WorkflowRunCard } from "./WorkflowRunCard";
import {
  deriveAlwaysRenderKeys,
  lookupApproval,
} from "./always-render-keys";
import { CHAT_COLUMN } from "./chat-column";
import {
  subscribeTranscriptFade,
  transcriptFadeEnabled,
} from "./transcript-fade";
import {
  buildTranscriptSlots,
  reuseTranscriptSlots,
  type ActivityStep,
  type TranscriptSlot,
} from "./transcript-slots";

interface Props {
  messages: ChatViewItem[];
  /** Render the tail "working" shimmer marker as the last row inside the
   *  scroller content (design D9). Gated by `shouldShowThinkingIndicator`
   *  upstream so it never shows while an approval is pending or a row is
   *  already streaming its own affordance. */
  showThinking?: boolean;
  /** Thread-level streaming flag (turn in flight). Passed to the pure slot
   *  builder so the tail mechanical-step run renders as the live "Working"
   *  Activity block; also suppresses the separate StreamingMarker when a
   *  working Activity block is already the tail (one live line). */
  streaming?: boolean;
  /** Non-null while the stall watchdog has flagged this mid-turn thread
   *  as silent (issue #154). Renders an amber "no activity" notice at the
   *  transcript tail in place of the ember streaming marker. */
  stalled?: { silentForSecs: number } | null;
  /** True when the last run never cleanly settled (child exit / crash).
   *  Renders a "Run interrupted" tail divider while not streaming. */
  interrupted?: boolean;
  /** Send → jump-to-latest signal. An incrementing counter bumped by the
   *  composer send handlers (AgentChatPane): each increment snaps the
   *  transcript to the bottom and re-pins following-bottom, so sending a
   *  new prompt always catches the reader up to the latest content — even
   *  when they had scrolled up into history. Absent / unchanged ⇒ no forced
   *  scroll (free-scroll while reading history is preserved). */
  scrollToBottomSignal?: number;
  /** Index-capable request from the docked subagent activity bar. */
  subagentJumpRequest?: { cardId: string; nonce: number } | null;
  /** Optional session-created timestamp for the top session-start marker
   *  (design D2). When absent a plain "Session started" divider renders.
   *  Stage 3 wires the real value through AgentChatPane. */
  sessionStartedAt?: number;
  /** The session's chat provider. Drives the assistant-turn avatar's
   *  official mark (Claude / Codex / OpenCode). Stable per session, so it
   *  is safe to thread into the memoized rows. Absent → sparkle fallback. */
  provider?: AgentChatProviderKind | null;
  onRespondToRequest: (requestId: string, decision: ApprovalDecision) => void;
  onAcceptPlan: (requestId: string) => void | Promise<void>;
  onRejectPlan: (requestId: string) => void | Promise<void>;
  /** Follow-up queueing: cancel a queued user turn. `text` is passed
   *  back so the caller can restore it into the composer. */
  onCancelQueued?: (queuedId: string, text: string) => void;
  /** Follow-up queueing: send a queued user turn now (steer) —
   *  soft-interrupts the active turn and dispatches it immediately. */
  onSendQueuedNow?: (queuedId: string) => void;
  /** Enter a subagent's read-only drill-in (design "Enter subagent").
   *  Wired by AgentChatPane's viewMode state; absent → the card's Enter
   *  affordance is inert. */
  onEnterSubagent?: (subagentId: string) => void;
  /** This pane's workspace id (sourced via `findWorkspaceIdForPane`).
   *  Two consumers: (a) threaded down to `WorkflowRunCard` so its
   *  "Open panel" affordance can flip the right panel to the
   *  Orchestration tab; (b) the GUI-mode background-browser session
   *  lookup for the inline chip (docs/features/browser.md "Background
   *  browser in GUI mode"). Absent → both affordances are inert
   *  (legacy / non-workspace-scoped callers keep byte-identical
   *  output) rather than throwing. */
  workspaceId?: string | null;
}

/**
 * Virtualized transcript. LegendList mounts only the visible range plus a
 * generous directional buffer, measures dynamic rows, and retains those
 * measurements for stable long-distance scrolling. Stable slot keys and
 * object reuse keep token streaming local to the changed row. The list owns
 * both history anchoring and tail following — the windowed-list architecture
 * used by comparable agent transcript UIs — instead of asking the browser to
 * lay out every transcript row.
 *
 * Memoized alongside `ChatTranscript`: with the pane's field-level store
 * subscriptions none of these props move while the composer draft changes,
 * so a keystroke stops at this boundary instead of re-entering the list.
 */
export const MessageList = memo(function MessageList({
  messages,
  showThinking = false,
  streaming = false,
  stalled = null,
  interrupted = false,
  scrollToBottomSignal,
  subagentJumpRequest,
  sessionStartedAt,
  provider,
  onRespondToRequest,
  onAcceptPlan,
  onRejectPlan,
  onCancelQueued,
  onSendQueuedNow,
  onEnterSubagent,
  workspaceId,
}: Props) {
  // GUI-mode background browser session for this pane's workspace (see
  // docs/features/browser.md "Background browser in GUI mode"). Gated on
  // the same predicate the backend's `browser_automation` handler uses to
  // suppress pane creation: Agent Chat beta on, workspace not OpenFlow.
  // `workspaceId` is absent for legacy/non-workspace-scoped callers, so
  // the chip never renders there — byte-identical output preserved.
  const enableAgentChat = useFeatureFlags((s) => s.enableAgentChat);
  const workspaceType = useAppStore((s) => {
    if (!workspaceId) return null;
    return (
      s.appState?.workspaces.find((w) => w.workspace_id === workspaceId)
        ?.workspace_type ?? null
    );
  });
  const backgroundBrowserSession = useAppStore((s) => {
    if (!workspaceId) return null;
    const session = s.appState?.agent_browser_sessions?.find(
      (abs) => abs.workspace_id === workspaceId,
    );
    if (!session || !session.is_active || session.pane_id) return null;
    return session;
  });
  const showBrowserChip =
    !!workspaceId &&
    enableAgentChat &&
    workspaceType !== "open_flow" &&
    !!backgroundBrowserSession;

  // Viewport edge fade. Read through an external store rather than called
  // inline: the decision depends on the renderer mode, which the backend
  // reports asynchronously at boot (see use-renderer-mode.ts), so a transcript
  // mounted before that lands has to pick the answer up when it arrives. The
  // getter is cached, so this settles to a stable value immediately.
  const fadeEnabled = useSyncExternalStore(
    subscribeTranscriptFade,
    transcriptFadeEnabled,
    transcriptFadeEnabled,
  );

  // Sort by seq so order is a property of the data, not of React
  // reconciliation or store-update timing (stable id tiebreak).
  const ordered = useMemo(() => {
    const copy = messages.slice();
    copy.sort((a, b) => a.seq - b.seq || a.id.localeCompare(b.id));
    return copy;
  }, [messages]);

  // Subagent-id → display name, for the "from subagent X" label on a
  // bubbled approval request (locked decision 4).
  const subagentNames = useMemo(() => {
    const map = new Map<string, string>();
    for (const m of ordered) {
      if (m.kind !== "subagent_run") continue;
      for (const sub of m.subagents) {
        map.set(sub.id, sub.name ?? sub.agentType ?? "subagent");
      }
    }
    return map;
  }, [ordered]);

  // Resolve tool-card approvals in O(1) from the request id the reducer
  // stamped onto the gated tool call.
  const requestsById = useMemo(() => {
    const map = new Map<string, PermissionRequestItem>();
    for (const m of ordered) {
      if (m.kind === "permission_request") map.set(m.request_id, m);
    }
    return map;
  }, [ordered]);

  // Slot-object reuse across rebuilds (issue #129). Every store update (each
  // streaming token) rebuilds all slots, but `reuseTranscriptSlots` swaps
  // unchanged slots back to their previous object identity — that identity is
  // exactly what lets the memoized `SlotRowMemo` wrapper skip. The ref holds
  // the last returned array to diff against; mutating it inside `useMemo` is
  // the accepted cache pattern here (deterministic under StrictMode's
  // double-render — the second pass diffs the first pass's result and returns
  // it unchanged).
  const prevSlotsRef = useRef<TranscriptSlot[]>([]);
  const slots = useMemo(() => {
    const next = reuseTranscriptSlots(
      prevSlotsRef.current,
      buildTranscriptSlots(ordered, streaming),
    );
    prevSlotsRef.current = next;
    return next;
  }, [ordered, streaming]);

  // A working Activity block already shows the single live line, so the
  // separate shimmer marker is suppressed when one is the transcript tail
  // (no double indicators). The marker still fills the gap before any step
  // arrives (e.g. right after send), which is not a working Activity tail.
  const tailBody = slots.length > 0 ? slots[slots.length - 1].body : null;
  const tailIsWorkingActivity =
    tailBody?.kind === "activity" && tailBody.working;

  const listRef = useRef<LegendListRef | null>(null);
  const [isAtEnd, setIsAtEnd] = useState(true);
  const [firstVisibleSlotIndex, setFirstVisibleSlotIndex] = useState(() =>
    Math.max(0, slots.length - 1),
  );

  useEffect(() => {
    const state = listRef.current?.getState();
    if (!state) return;
    setIsAtEnd(state.isAtEnd);
    return state.listen("isAtEnd", setIsAtEnd);
  }, []);

  const prevScrollSignalRef = useRef(scrollToBottomSignal);
  useEffect(() => {
    if (scrollToBottomSignal === prevScrollSignalRef.current) return;
    prevScrollSignalRef.current = scrollToBottomSignal;
    void listRef.current?.scrollToEnd({ animated: false });
  }, [scrollToBottomSignal]);

  const subagentTargetIndex = useMemo(() => {
    const cardId = subagentJumpRequest?.cardId;
    if (!cardId) return -1;
    return slots.findIndex(
      (slot) =>
        slot.body.kind === "item" &&
        slot.body.item.kind === "subagent_run" &&
        slot.body.item.id === cardId,
    );
  }, [slots, subagentJumpRequest?.cardId]);

  // Rows with live controls must not unmount while the reader scrolls away:
  // queued-turn cancel/send-now actions and pending approval forms can carry
  // transient local input. Derivation lives in `always-render-keys.ts` so the
  // contract is unit-testable without mounting the virtualizer.
  const alwaysRenderKeys = useMemo(
    () => deriveAlwaysRenderKeys(slots, requestsById),
    [requestsById, slots],
  );

  useEffect(() => {
    if (!subagentJumpRequest || subagentTargetIndex < 0) return;
    let cancelled = false;
    let timer: number | undefined;
    let frame: number | undefined;
    let highlightedCard: HTMLElement | null = null;
    void listRef.current
      ?.scrollToIndex({
        index: subagentTargetIndex,
        animated: false,
        viewOffset: 16,
      })
      .then(() => {
        if (cancelled) return;
        frame = requestAnimationFrame(() => {
          const row = listRef.current
            ?.getState()
            .elementAtIndex(subagentTargetIndex);
          const card = row?.matches("[data-subagent-card]")
            ? row
            : (row?.querySelector("[data-subagent-card]") as HTMLElement | null);
          if (!card) return;
          card.classList.add("subagent-card-highlight");
          highlightedCard = card;
          timer = window.setTimeout(
            () => {
              card.classList.remove("subagent-card-highlight");
              highlightedCard = null;
            },
            1100,
          );
        });
      });
    return () => {
      cancelled = true;
      if (frame != null) cancelAnimationFrame(frame);
      if (timer != null) window.clearTimeout(timer);
      highlightedCard?.classList.remove("subagent-card-highlight");
    };
  }, [subagentJumpRequest, subagentTargetIndex]);

  const renderItem = useCallback(
    ({ item: slot }: { item: TranscriptSlot }) => (
      <div className={CHAT_COLUMN}>
        <SlotRowMemo
          slot={slot}
          provider={provider}
          approval={
            slot.body.kind === "item"
              ? lookupApproval(slot.body.item, requestsById)
              : null
          }
          subagentName={
            slot.body.kind === "item"
              ? subagentNameFor(slot.body.item, subagentNames)
              : null
          }
          workspaceId={workspaceId}
          onRespondToRequest={onRespondToRequest}
          onAcceptPlan={onAcceptPlan}
          onRejectPlan={onRejectPlan}
          onCancelQueued={onCancelQueued}
          onSendQueuedNow={onSendQueuedNow}
          onEnterSubagent={onEnterSubagent}
        />
      </div>
    ),
    [
      onAcceptPlan,
      onCancelQueued,
      onEnterSubagent,
      onRejectPlan,
      onRespondToRequest,
      onSendQueuedNow,
      provider,
      requestsById,
      subagentNames,
      workspaceId,
    ],
  );

  // Stable element identity: LegendList re-mounts / re-lays-out the header
  // when this prop changes, so it must not be rebuilt on unrelated renders.
  const listHeader = useMemo(
    () => (
      <div className={cn(CHAT_COLUMN, "pt-[26px]")}>
        <SessionStartMarker startedAt={sessionStartedAt} />
      </div>
    ),
    [sessionStartedAt],
  );

  const handleFirstVisibleItemChanged = useCallback(
    ({ index }: { index: number }) =>
      setFirstVisibleSlotIndex((current) =>
        current === index ? current : index,
      ),
    [],
  );

  const listFooter = useMemo(
    () => (
      <div className={cn(CHAT_COLUMN, "pb-[30px]")}>
        {showBrowserChip && backgroundBrowserSession && workspaceId && (
          <div className="mt-[13px]">
            <BackgroundBrowserChip
              session={backgroundBrowserSession}
              workspaceId={workspaceId}
            />
          </div>
        )}
        {stalled && streaming && (
          <div className="mt-[13px]">
            <RunStalledNotice silentForSecs={stalled.silentForSecs} />
          </div>
        )}
        {showThinking && !tailIsWorkingActivity && !(stalled && streaming) && (
          <div className="mt-[13px]">
            <StreamingMarker messages={ordered} />
          </div>
        )}
        {interrupted && !streaming && (
          <div className="mt-[13px]">
            <RunInterruptedDivider />
          </div>
        )}
      </div>
    ),
    [
      backgroundBrowserSession,
      interrupted,
      ordered,
      showBrowserChip,
      showThinking,
      stalled,
      streaming,
      tailIsWorkingActivity,
      workspaceId,
    ],
  );

  return (
    <div className="group/transcript-list relative size-full min-h-0 overflow-hidden">
      <MessageTrail
        slots={slots}
        listRef={listRef}
        firstVisibleSlotIndex={firstVisibleSlotIndex}
      />
      <LegendList<TranscriptSlot>
        ref={listRef}
        data={slots}
        keyExtractor={transcriptSlotKey}
        getItemType={transcriptSlotType}
        itemsAreEqual={transcriptSlotsAreEqual}
        renderItem={renderItem}
        estimatedItemSize={112}
        estimatedHeaderSize={48}
        drawDistance={800}
        recycleItems={false}
        alwaysRender={
          alwaysRenderKeys.length > 0 ? { keys: alwaysRenderKeys } : undefined
        }
        initialScrollAtEnd
        maintainScrollAtEnd={{
          animated: false,
          on: {
            dataChange: true,
            footerLayout: true,
            itemLayout: true,
            layout: true,
          },
        }}
        maintainScrollAtEndThreshold={0.03}
        maintainVisibleContentPosition={{ data: true, size: false }}
        onFirstVisibleItemChanged={handleFirstVisibleItemChanged}
        aria-busy={showThinking || undefined}
        data-slot="transcript-list"
        // `scrollbar-gutter: stable both-edges` (not plain `stable`):
        // reserving the gutter on one side only would shrink the box the
        // centered CHAT_COLUMN rows are measured against, sliding the whole
        // transcript half a scrollbar to the left of the composer, which
        // sits outside this scroller. Both edges keeps the column
        // concentric with the pane, so transcript and composer share the
        // same rails (see chat/chat-column.ts).
        className="h-full min-h-0 overflow-x-hidden overscroll-y-contain [overflow-anchor:none] [scrollbar-gutter:stable_both-edges]"
        style={fadeEnabled ? WS_FADE_STYLE : undefined}
        ListHeaderComponent={listHeader}
        ListFooterComponent={listFooter}
      />
      {!isAtEnd && (
        <Button
          type="button"
          onClick={() => void listRef.current?.scrollToEnd({ animated: false })}
          variant="secondary"
          size="sm"
          className="absolute bottom-4 left-1/2 z-10 h-8 w-auto -translate-x-1/2 gap-1.5 rounded-full border border-border bg-card px-3.5 text-[12px] font-semibold text-muted-foreground shadow-lg hover:bg-card hover:text-foreground"
        >
          Jump to latest
          <ArrowDown className="h-3.5 w-3.5" aria-hidden />
        </Button>
      )}
    </div>
  );
});

/** Viewport edge fade (design `wsFade`): dissolve content at the top/bottom
 *  of the scroll surface. A mask, not an overlay, so it works over any
 *  background. The installed shadcn build ships no `scroll-fade` utility,
 *  so this is applied inline.
 *
 *  On by default wherever the webview is composited — including Linux
 *  WebKitGTK, which runs accelerated again (issue #129, see
 *  `transcript-fade.ts`). It switches itself off when the backend reports the
 *  compatibility (CPU) renderer, where the mask forces a full-viewport
 *  re-rasterization on every scroll frame, and the `codemux:transcript-fade`
 *  localStorage override still forces it either way. */
const WS_FADE_STYLE: CSSProperties = {
  maskImage:
    "linear-gradient(to bottom, transparent 0, #000 26px, #000 calc(100% - 20px), transparent 100%)",
  WebkitMaskImage:
    "linear-gradient(to bottom, transparent 0, #000 26px, #000 calc(100% - 20px), transparent 100%)",
};

/** Amber "no activity" notice shown at the tail of a silently-stalled
 *  mid-turn run (issue #154). Advisory only — the run may still be alive
 *  (a long quiet tool call), so the copy hedges. Tokens only, per
 *  docs/reference/DESIGN-SYSTEM.md (no hardcoded colors). */
function RunStalledNotice({ silentForSecs }: { silentForSecs: number }) {
  const minutes = Math.max(1, Math.floor(silentForSecs / 60));
  return (
    <div
      role="status"
      data-testid="run-stalled-notice"
      className="flex items-center gap-2 rounded-md bg-warning/10 px-3 py-2 text-[12px] text-warning"
    >
      <TriangleAlert className="h-3.5 w-3.5 shrink-0" aria-hidden />
      <span>
        No activity for {minutes}m — the agent may have stopped.
      </span>
    </div>
  );
}

/** "Run interrupted" tail divider — clones the SessionStartMarker hairline
 *  pattern with an amber (warning-token) label. Shown when the last run
 *  died without cleanly settling and no turn is in flight. */
function RunInterruptedDivider() {
  return (
    <div
      data-testid="run-interrupted-divider"
      className="flex items-center gap-3 text-warning"
    >
      <span className="h-px flex-1 bg-warning/40" />
      <span className="font-mono text-[11px] font-medium tracking-wide">
        Run interrupted
      </span>
      <span className="h-px flex-1 bg-warning/40" />
    </div>
  );
}

function SessionStartMarker({ startedAt }: { startedAt?: number }) {
  return (
    <div className="flex items-center gap-3 text-muted-foreground/70">
      <span className="h-px flex-1 bg-border/60" />
      <span className="font-mono text-[11px] font-medium tracking-wide">
        {formatSessionStart(startedAt)}
      </span>
      <span className="h-px flex-1 bg-border/60" />
    </div>
  );
}

function formatSessionStart(startedAt?: number): string {
  if (startedAt == null) return "Session started";
  const d = new Date(startedAt);
  const time = d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  const sameDay = d.toDateString() === new Date().toDateString();
  const datePart = sameDay
    ? "Today"
    : d.toLocaleDateString([], { month: "short", day: "numeric" });
  return `${datePart} · ${time}`;
}

/** The originating subagent's display name for a bubbled approval request
 *  (null for ordinary parent requests / non-request rows). */
function subagentNameFor(
  item: ChatViewItem,
  subagentNames: Map<string, string>,
): string | null {
  if (item.kind !== "permission_request" || !item.subagent_id) return null;
  return subagentNames.get(item.subagent_id) ?? "subagent";
}

/** Module scope, not a per-render closure: LegendList keys, type buckets and
 *  identity comparisons are pure functions of the slot, and a fresh closure
 *  each render invalidates the list's own memoization for no reason. */
function transcriptSlotKey(slot: TranscriptSlot): string {
  return slot.key;
}

/** The row-identity contract that pairs with `reuseTranscriptSlots`: an
 *  untouched slot keeps its object identity across rebuilds, so reference
 *  equality is exactly "this row did not change". */
function transcriptSlotsAreEqual(
  previous: TranscriptSlot,
  item: TranscriptSlot,
): boolean {
  return previous === item;
}

/** Stable row type lets LegendList keep separate measured-size averages. */
function transcriptSlotType(slot: TranscriptSlot): string {
  if (slot.body.kind === "activity") return "activity";
  return slot.body.item.kind;
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

/** Assistant-side rows share a 29px avatar gutter (drawn once per turn) so
 *  their content aligns under the first row. */
function AssistantGutter({
  showAvatar,
  provider,
  children,
}: {
  showAvatar: boolean;
  provider?: AgentChatProviderKind | null;
  children: React.ReactNode;
}) {
  return (
    <div className="flex gap-[13px]">
      <div className="w-[29px] shrink-0">
        {showAvatar ? <AssistantAvatar provider={provider} /> : null}
      </div>
      <div className="min-w-0 flex-1">{children}</div>
    </div>
  );
}

function ItemRow({
  item,
  showAvatar,
  provider,
  approval,
  subagentName,
  onRespondToRequest,
  onAcceptPlan,
  onRejectPlan,
  onCancelQueued,
  onSendQueuedNow,
  onEnterSubagent,
  workspaceId,
}: {
  item: ChatViewItem;
  showAvatar: boolean;
  provider?: AgentChatProviderKind | null;
  approval: PermissionRequestItem | null;
  subagentName: string | null;
  onRespondToRequest: (requestId: string, decision: ApprovalDecision) => void;
  onAcceptPlan: (requestId: string) => void | Promise<void>;
  onRejectPlan: (requestId: string) => void | Promise<void>;
  onCancelQueued?: (queuedId: string, text: string) => void;
  onSendQueuedNow?: (queuedId: string) => void;
  onEnterSubagent?: (subagentId: string) => void;
  workspaceId?: string | null;
}) {
  const requestId =
    item.kind === "tool_call"
      ? item.approval_request_id
      : item.kind === "permission_request"
        ? item.request_id
        : item.kind === "workflow_run"
          ? item.approvalRequestId
          : null;
  const handleDecide = useCallback(
    (decision: ApprovalDecision) => {
      if (requestId) onRespondToRequest(requestId, decision);
    },
    [requestId, onRespondToRequest],
  );
  const handleEnterSubagent = useCallback(
    (subagentId: string) => onEnterSubagent?.(subagentId),
    [onEnterSubagent],
  );
  const handleAcceptPlan = useCallback(() => {
    if (item.kind === "permission_request") return onAcceptPlan(item.request_id);
  }, [item, onAcceptPlan]);
  const handleRejectPlan = useCallback(() => {
    if (item.kind === "permission_request") return onRejectPlan(item.request_id);
  }, [item, onRejectPlan]);

  if (item.kind === "user_message") {
    return (
      <UserMessage
        item={item}
        onCancelQueued={onCancelQueued}
        onSendQueuedNow={onSendQueuedNow}
      />
    );
  }

  // The orchestration card is a full-width standalone surface (no avatar
  // gutter), matching the design.
  if (item.kind === "subagent_run") {
    return <SubagentsCard item={item} onEnter={handleEnterSubagent} />;
  }

  // Same full-width, no-gutter treatment for a Workflow tool run.
  if (item.kind === "workflow_run") {
    return (
      <WorkflowRunCard
        item={item}
        approval={approval}
        onDecide={handleDecide}
        workspaceId={workspaceId}
      />
    );
  }

  return (
    <AssistantGutter showAvatar={showAvatar} provider={provider}>
      {renderAssistantBody(item, {
        approval,
        subagentName,
        workspaceId,
        handleDecide,
        handleAcceptPlan,
        handleRejectPlan,
      })}
    </AssistantGutter>
  );
}

function renderAssistantBody(
  item: Exclude<ChatViewItem, { kind: "user_message" }>,
  handlers: {
    approval: PermissionRequestItem | null;
    subagentName: string | null;
    workspaceId?: string | null;
    handleDecide: (decision: ApprovalDecision) => void;
    handleAcceptPlan: () => void | Promise<void>;
    handleRejectPlan: () => void | Promise<void>;
  },
) {
  switch (item.kind) {
    case "assistant_message":
      return <AssistantMessage item={item} />;
    case "reasoning":
      return <ReasoningBlock item={item} />;
    case "tool_call":
      return isTaskSummaryTool(item) ? (
        <TaskSummaryCard item={item} workspaceId={handlers.workspaceId} />
      ) : (
        <ToolCallCard
          item={item}
          approval={handlers.approval}
          onDecide={handlers.handleDecide}
        />
      );
    case "permission_request":
      switch (item.request_kind) {
        case "plan":
          return (
            <PlanProposalBlock
              item={item}
              onAccept={handlers.handleAcceptPlan}
              onReject={handlers.handleRejectPlan}
            />
          );
        case "user-input": {
          // The interactive picker for user-input prompts lives above the
          // composer (ComposerPendingInputPanel); the transcript keeps a
          // one-line pointer while it's still open.
          if (item.resolution.state === "pending") {
            return (
              <div className="py-0.5 text-xs text-muted-foreground">
                Input requested — answer above the composer.
              </div>
            );
          }
          if (item.resolution.state === "responding") {
            return (
              <div className="py-0.5 text-xs text-muted-foreground">
                Submitting answers…
              </div>
            );
          }
          if (item.resolution.state === "failed") {
            return (
              <div className="py-0.5 text-xs text-muted-foreground">
                {item.resolution.message}
              </div>
            );
          }
          // Resolved: echo the user's selection as a reply bubble so the
          // transcript actually shows that they answered.
          return <UserInputAnswer item={item} />;
        }
        default:
          return (
            <div className="space-y-1">
              {handlers.subagentName && (
                <div className="font-mono text-[11px] font-semibold uppercase tracking-wide text-muted-foreground">
                  From subagent {handlers.subagentName}
                </div>
              )}
              <PermissionRequestBlock item={item} onDecide={handlers.handleDecide} />
            </div>
          );
      }
    case "turn_ended":
      if (item.status.kind !== "error") return null;
      return (
        <div className="py-0.5 text-xs text-muted-foreground">
          Turn ended: {item.status.subtype}
          {item.status.message ? ` — ${item.status.message}` : ""}
        </div>
      );
    case "runtime_notice":
      // Compact muted-amber inline notice (provider rate-limit rejection,
      // enumerated assistant error) — a left-bordered line in the
      // assistant gutter. Tokens only (design-system no-hardcoded-color).
      return (
        <div className="border-l-2 border-status-working/40 bg-status-working/10 px-3 py-1.5 text-[12px] text-status-working">
          {item.message}
        </div>
      );
    case "subagent_run":
    case "workflow_run":
      // Rendered full-width above (before the AssistantGutter wrap); this
      // arm only keeps the switch exhaustive.
      return null;
  }
}

function ActivityRow({
  items,
  working,
  showAvatar,
  provider,
}: {
  items: ActivityStep[];
  working: boolean;
  showAvatar: boolean;
  provider?: AgentChatProviderKind | null;
}) {
  return (
    <AssistantGutter showAvatar={showAvatar} provider={provider}>
      <ActivityBlock items={items} working={working} />
    </AssistantGutter>
  );
}

// Two-level memoization (issue #129). Level 1 is `SlotRowMemo`, the whole-row
// wrapper below owns the activity/item branch. `reuseTranscriptSlots` keeps an untouched slot's object
// identity across store rebuilds, so with the reducer's stable `item` /
// `approval` references and a per-session-stable `provider` the wrapper's props
// stay shallow-equal and its default `memo` comparison skips the row entirely —
// including the virtualized row re-render. Level 2 is these leaves
// (`ItemRowMemo` / `ActivityRowMemo`), which independently skip when their own
// props are unchanged. A single streaming token mutates exactly one slot → its
// wrapper AND its leaf are the only things that re-render. (Activity rows
// rebuild their `items` array each pass, like the old tool groups did; the
// stable slot key keeps the scroller row from remounting.)
const ItemRowMemo = memo(ItemRow);
const ActivityRowMemo = memo(ActivityRow);

/**
 * Whole-row wrapper (level-1 memo — see the comment above the leaf memos). It
 * dispatches to the activity/item leaf. Splitting it out is what lets the
 * reused-slot identity skip an
 * untouched row's wrapper re-render, not just its leaf.
 */
function SlotRow({
  slot,
  provider,
  approval,
  subagentName,
  workspaceId,
  onRespondToRequest,
  onAcceptPlan,
  onRejectPlan,
  onCancelQueued,
  onSendQueuedNow,
  onEnterSubagent,
}: {
  slot: TranscriptSlot;
  provider?: AgentChatProviderKind | null;
  approval: PermissionRequestItem | null;
  subagentName: string | null;
  workspaceId?: string | null;
  onRespondToRequest: (requestId: string, decision: ApprovalDecision) => void;
  onAcceptPlan: (requestId: string) => void | Promise<void>;
  onRejectPlan: (requestId: string) => void | Promise<void>;
  onCancelQueued?: (queuedId: string, text: string) => void;
  onSendQueuedNow?: (queuedId: string) => void;
  onEnterSubagent?: (subagentId: string) => void;
}) {
  return (
    <div
      data-message-id={slot.messageId}
      className={cn(slot.turnStart ? "mt-5" : "mt-[13px]")}
    >
      {slot.body.kind === "activity" ? (
        <ActivityRowMemo
          items={slot.body.items}
          working={slot.body.working}
          showAvatar={slot.showAvatar}
          provider={provider}
        />
      ) : (
        <ItemRowMemo
          item={slot.body.item}
          showAvatar={slot.showAvatar}
          provider={provider}
          approval={approval}
          subagentName={subagentName}
          onRespondToRequest={onRespondToRequest}
          onAcceptPlan={onAcceptPlan}
          onRejectPlan={onRejectPlan}
          onCancelQueued={onCancelQueued}
          onSendQueuedNow={onSendQueuedNow}
          onEnterSubagent={onEnterSubagent}
          workspaceId={workspaceId}
        />
      )}
    </div>
  );
}

const SlotRowMemo = memo(SlotRow);
