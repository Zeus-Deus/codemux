import { ArrowDown } from "lucide-react";
import {
  memo,
  useCallback,
  useLayoutEffect,
  useMemo,
  useRef,
  type CSSProperties,
} from "react";

import {
  MessageScroller,
  MessageScrollerButton,
  MessageScrollerContent,
  MessageScrollerItem,
  MessageScrollerProvider,
  MessageScrollerViewport,
} from "@/components/ui/message-scroller";
import type {
  ChatViewItem,
  PermissionRequestItem,
} from "@/lib/agent-chat/types";
import type { ApprovalDecision } from "@/tauri/events";
import type { AgentChatProviderKind } from "@/tauri/types";

import { ActivityBlock } from "./ActivityBlock";
import { AssistantAvatar } from "./AssistantAvatar";
import { AssistantMessage } from "./AssistantMessage";
import { MessageTrail } from "./MessageTrail";
import { PermissionRequestBlock } from "./PermissionRequestBlock";
import { PlanProposalBlock } from "./PlanProposalBlock";
import { ReasoningBlock } from "./ReasoningBlock";
import { StreamingMarker } from "./StreamingMarker";
import { SubagentsCard } from "./SubagentsCard";
import { isTaskSummaryTool, TaskSummaryCard } from "./TaskSummaryCard";
import { ToolCallCard } from "./ToolCallCard";
import { UserMessage } from "./UserMessage";
import { buildTranscriptSlots, type ActivityStep } from "./transcript-slots";

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
  /** Enter a subagent's read-only drill-in (design "Enter subagent").
   *  Wired by AgentChatPane's viewMode state; absent → the card's Enter
   *  affordance is inert. */
  onEnterSubagent?: (subagentId: string) => void;
}

/**
 * Transcript body on the shadcn **MessageScroller** (design D2). The
 * scroller provides the viewport/rows/jump-button anatomy and
 * `content-visibility:auto` row containment (thousands of turns stay
 * cheap; the reducer's 5,000-message cap bounds the worst case), while
 * tail tracking uses the preserved stick-to-bottom contract implemented
 * below (mount snap + pinned follow) — the engine's turn anchoring is
 * disabled because it mis-scrolls against fully hydrated transcripts.
 *
 * Layout is derived by the pure `buildTranscriptSlots` (turn grouping,
 * tool-run folding, avatar/turn-boundary metadata). Each slot is one
 * `MessageScrollerItem` with a stable `messageId`; the row leaves are
 * memoized so a single streaming token re-renders exactly one row.
 */
export function MessageList({
  messages,
  showThinking = false,
  streaming = false,
  sessionStartedAt,
  provider,
  onRespondToRequest,
  onAcceptPlan,
  onRejectPlan,
  onEnterSubagent,
}: Props) {
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

  const slots = useMemo(
    () => buildTranscriptSlots(ordered, streaming),
    [ordered, streaming],
  );

  // A working Activity block already shows the single live line, so the
  // separate shimmer marker is suppressed when one is the transcript tail
  // (no double indicators). The marker still fills the gap before any step
  // arrives (e.g. right after send), which is not a working Activity tail.
  const tailBody = slots.length > 0 ? slots[slots.length - 1].body : null;
  const tailIsWorkingActivity =
    tailBody?.kind === "activity" && tailBody.working;

  // --- Tail snap ------------------------------------------------------
  // `content-visibility:auto` rows expose only ESTIMATED heights until
  // they render, so the engine's mount-time "end" positioning (and a
  // smooth scrollToEnd across tens of thousands of estimated pixels)
  // can land far from the real tail. Direct scrollTop assignment is
  // reliable — the browser re-clamps as row sizes materialise — so we
  // snap the DOM directly: once on first mount (re-snapping across a
  // few frames until the viewport actually rests at the bottom), and on
  // every jump-button press.
  const shellRef = useRef<HTMLDivElement | null>(null);
  const snapToTail = useCallback(() => {
    const viewport = shellRef.current?.querySelector<HTMLElement>(
      '[data-slot="message-scroller-viewport"]',
    );
    if (viewport) viewport.scrollTop = viewport.scrollHeight;
  }, []);
  const hasContent = slots.length > 0;
  const initialSnapDoneRef = useRef(false);
  useLayoutEffect(() => {
    if (!hasContent || initialSnapDoneRef.current) return;
    initialSnapDoneRef.current = true;
    snapToTail();
    let tries = 0;
    let frame = 0;
    const settle = () => {
      const viewport = shellRef.current?.querySelector<HTMLElement>(
        '[data-slot="message-scroller-viewport"]',
      );
      if (!viewport) return;
      const distance =
        viewport.scrollHeight - viewport.scrollTop - viewport.clientHeight;
      if (distance > 4 && tries < 20) {
        tries += 1;
        viewport.scrollTop = viewport.scrollHeight;
        frame = requestAnimationFrame(settle);
      }
    };
    frame = requestAnimationFrame(settle);
    return () => cancelAnimationFrame(frame);
  }, [hasContent, snapToTail]);

  // --- Pinned follow ---------------------------------------------------
  // The preserved stick-to-bottom contract (issue #77 semantics):
  // pinned-ness is tracked from REAL scroll events (≤80px from the
  // bottom counts as pinned); after every transcript change, if pinned,
  // snap to the tail. Content growth alone never fires a scroll event,
  // so streaming can never unpin a reader who scrolled up.
  const pinnedRef = useRef(true);
  useLayoutEffect(() => {
    const viewport = shellRef.current?.querySelector<HTMLElement>(
      '[data-slot="message-scroller-viewport"]',
    );
    if (!viewport) return;
    const onScroll = () => {
      const distance =
        viewport.scrollHeight - viewport.scrollTop - viewport.clientHeight;
      pinnedRef.current = distance <= PIN_THRESHOLD_PX;
    };
    viewport.addEventListener("scroll", onScroll, { passive: true });
    return () => viewport.removeEventListener("scroll", onScroll);
  }, []);
  useLayoutEffect(() => {
    if (pinnedRef.current) snapToTail();
  }, [slots, showThinking, snapToTail]);

  return (
    <div ref={shellRef} style={SHELL_STYLE}>
    <MessageScrollerProvider autoScroll>
      <MessageScroller>
        {/* Navigation trail — a turn rail in the left gutter (inside the
            provider, sibling of the viewport). Reads the active turn from
            `visibleMessageIds`; hides itself on short threads. */}
        <MessageTrail slots={slots} />
        <MessageScrollerViewport style={WS_FADE_STYLE}>
          <MessageScrollerContent
            aria-busy={showThinking || undefined}
            className="mx-auto w-full max-w-[760px] gap-0 px-7 pb-[30px] pt-[26px]"
          >
            <SessionStartMarker startedAt={sessionStartedAt} />

            {slots.map((slot) => (
              <MessageScrollerItem
                key={slot.key}
                messageId={slot.messageId}
                // Turn anchoring is deliberately OFF: with a fully
                // hydrated transcript (hundreds of pre-existing rows)
                // the engine's anchor handling scrolls the viewport to
                // a stale early anchor when new items register mid-
                // stream, which breaks the stick-to-bottom contract.
                // The pinned-follow effect below owns tail tracking.
                scrollAnchor={false}
                className={slot.turnStart ? "mt-5" : "mt-[13px]"}
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
                    approval={lookupApproval(slot.body.item, requestsById)}
                    subagentName={subagentNameFor(slot.body.item, subagentNames)}
                    onRespondToRequest={onRespondToRequest}
                    onAcceptPlan={onAcceptPlan}
                    onRejectPlan={onRejectPlan}
                    onEnterSubagent={onEnterSubagent}
                  />
                )}
              </MessageScrollerItem>
            ))}

            {showThinking && !tailIsWorkingActivity && (
              <div className="mt-[13px]">
                <StreamingMarker messages={ordered} />
              </div>
            )}
          </MessageScrollerContent>
        </MessageScrollerViewport>
        <MessageScrollerButton
          direction="end"
          behavior="auto"
          variant="secondary"
          size="sm"
          onClick={snapToTail}
          className="bottom-4 h-8 w-auto gap-1.5 rounded-full border border-border bg-card px-3.5 text-[11.5px] font-semibold text-muted-foreground shadow-lg hover:bg-card hover:text-foreground"
        >
          Jump to latest
          <ArrowDown className="h-3.5 w-3.5" aria-hidden />
        </MessageScrollerButton>
      </MessageScroller>
    </MessageScrollerProvider>
    </div>
  );
}

/** Layout-transparent wrapper so the tail-snap logic can reach the
 *  scroller viewport without adding a real box to the flex chain. */
const SHELL_STYLE: CSSProperties = { display: "contents" };

/** Distance from the bottom (px) still counted as "pinned to the tail"
 *  — matches the pre-redesign transcript contract. */
const PIN_THRESHOLD_PX = 80;

/** Viewport edge fade (design `wsFade`): dissolve content at the top/bottom
 *  of the scroll surface. A mask, not an overlay, so it works over any
 *  background. The installed shadcn build ships no `scroll-fade` utility,
 *  so this is applied inline. */
const WS_FADE_STYLE: CSSProperties = {
  maskImage:
    "linear-gradient(to bottom, transparent 0, #000 26px, #000 calc(100% - 20px), transparent 100%)",
  WebkitMaskImage:
    "linear-gradient(to bottom, transparent 0, #000 26px, #000 calc(100% - 20px), transparent 100%)",
};

function SessionStartMarker({ startedAt }: { startedAt?: number }) {
  return (
    <div className="flex items-center gap-3 text-muted-foreground/70">
      <span className="h-px flex-1 bg-border/60" />
      <span className="font-mono text-[10.5px] font-medium tracking-wide">
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

function lookupApproval(
  item: ChatViewItem,
  requestsById: Map<string, PermissionRequestItem>,
): PermissionRequestItem | null {
  if (item.kind === "tool_call" && item.approval_request_id != null) {
    return requestsById.get(item.approval_request_id) ?? null;
  }
  return null;
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
  onEnterSubagent,
}: {
  item: ChatViewItem;
  showAvatar: boolean;
  provider?: AgentChatProviderKind | null;
  approval: PermissionRequestItem | null;
  subagentName: string | null;
  onRespondToRequest: (requestId: string, decision: ApprovalDecision) => void;
  onAcceptPlan: (requestId: string) => void | Promise<void>;
  onRejectPlan: (requestId: string) => void | Promise<void>;
  onEnterSubagent?: (subagentId: string) => void;
}) {
  const requestId =
    item.kind === "tool_call"
      ? item.approval_request_id
      : item.kind === "permission_request"
        ? item.request_id
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
    return <UserMessage item={item} />;
  }

  // The orchestration card is a full-width standalone surface (no avatar
  // gutter), matching the design.
  if (item.kind === "subagent_run") {
    return <SubagentsCard item={item} onEnter={handleEnterSubagent} />;
  }

  return (
    <AssistantGutter showAvatar={showAvatar} provider={provider}>
      {renderAssistantBody(item, {
        approval,
        subagentName,
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
        <TaskSummaryCard item={item} />
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
          // The interactive panel for user-input prompts lives above the
          // composer (ComposerPendingInputPanel); the transcript keeps a
          // one-line pointer so the thread of events stays readable.
          const label =
            item.resolution.state === "pending"
              ? "Input requested — answer above the composer."
              : item.resolution.state === "responding"
                ? "Submitting answers…"
                : "Answered";
          return (
            <div className="py-0.5 text-xs text-muted-foreground">{label}</div>
          );
        }
        default:
          return (
            <div className="space-y-1">
              {handlers.subagentName && (
                <div className="font-mono text-[10.5px] font-semibold uppercase tracking-wide text-muted-foreground">
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
    case "subagent_run":
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

// Default shallow comparison is enough: the parent passes the reducer's
// stable `item` / `approval` references, so an untouched row keeps its
// props identity and skips the whole ItemRow → leaf-component chain. One
// streaming token mutates exactly one item → exactly one row re-renders.
// (Activity rows rebuild their `items` array each pass, like the old tool
// groups did; stable slot keys keep the scroller row from remounting.)
const ItemRowMemo = memo(ItemRow);
const ActivityRowMemo = memo(ActivityRow);
