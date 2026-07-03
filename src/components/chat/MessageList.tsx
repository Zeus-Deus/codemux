import { ArrowDown } from "lucide-react";
import { memo, useCallback, useMemo, type CSSProperties } from "react";

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
  ToolCallItem,
} from "@/lib/agent-chat/types";
import type { ApprovalDecision } from "@/tauri/events";

import { AssistantAvatar } from "./AssistantAvatar";
import { AssistantMessage } from "./AssistantMessage";
import { PermissionRequestBlock } from "./PermissionRequestBlock";
import { PlanProposalBlock } from "./PlanProposalBlock";
import { ReasoningBlock } from "./ReasoningBlock";
import { StreamingMarker } from "./StreamingMarker";
import { isTaskSummaryTool, TaskSummaryCard } from "./TaskSummaryCard";
import { ToolCallCard } from "./ToolCallCard";
import { ToolGroupCard } from "./ToolGroupCard";
import { UserMessage } from "./UserMessage";
import { buildTranscriptSlots } from "./transcript-slots";

interface Props {
  messages: ChatViewItem[];
  /** Render the tail "working" shimmer marker as the last row inside the
   *  scroller content (design D9). Gated by `shouldShowThinkingIndicator`
   *  upstream so it never shows while an approval is pending or a row is
   *  already streaming its own affordance. */
  showThinking?: boolean;
  /** Optional session-created timestamp for the top session-start marker
   *  (design D2). When absent a plain "Session started" divider renders.
   *  Stage 3 wires the real value through AgentChatPane. */
  sessionStartedAt?: number;
  onRespondToRequest: (requestId: string, decision: ApprovalDecision) => void;
  onAcceptPlan: (requestId: string) => void | Promise<void>;
  onRejectPlan: (requestId: string) => void | Promise<void>;
}

/**
 * Transcript body on the shadcn **MessageScroller** (design D2). The
 * scroller owns auto-follow-at-live-edge, turn anchoring (`scrollAnchor`
 * on user rows), prepend preservation and the jump-to-latest button;
 * `content-visibility:auto` per row keeps thousands of turns cheap, and
 * the reducer's 5,000-message cap bounds the worst case.
 *
 * Layout is derived by the pure `buildTranscriptSlots` (turn grouping,
 * tool-run folding, avatar/turn-boundary metadata). Each slot is one
 * `MessageScrollerItem` with a stable `messageId`; the row leaves are
 * memoized so a single streaming token re-renders exactly one row.
 */
export function MessageList({
  messages,
  showThinking = false,
  sessionStartedAt,
  onRespondToRequest,
  onAcceptPlan,
  onRejectPlan,
}: Props) {
  // Sort by seq so order is a property of the data, not of React
  // reconciliation or store-update timing (stable id tiebreak).
  const ordered = useMemo(() => {
    const copy = messages.slice();
    copy.sort((a, b) => a.seq - b.seq || a.id.localeCompare(b.id));
    return copy;
  }, [messages]);

  // Resolve tool-card approvals in O(1) from the request id the reducer
  // stamped onto the gated tool call.
  const requestsById = useMemo(() => {
    const map = new Map<string, PermissionRequestItem>();
    for (const m of ordered) {
      if (m.kind === "permission_request") map.set(m.request_id, m);
    }
    return map;
  }, [ordered]);

  const slots = useMemo(() => buildTranscriptSlots(ordered), [ordered]);

  return (
    <MessageScrollerProvider autoScroll>
      <MessageScroller>
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
                scrollAnchor={slot.scrollAnchor}
                className={slot.turnStart ? "mt-5" : "mt-[13px]"}
              >
                {slot.body.kind === "toolGroup" ? (
                  <ToolGroupRowMemo
                    items={slot.body.items}
                    showAvatar={slot.showAvatar}
                  />
                ) : (
                  <ItemRowMemo
                    item={slot.body.item}
                    showAvatar={slot.showAvatar}
                    approval={lookupApproval(slot.body.item, requestsById)}
                    onRespondToRequest={onRespondToRequest}
                    onAcceptPlan={onAcceptPlan}
                    onRejectPlan={onRejectPlan}
                  />
                )}
              </MessageScrollerItem>
            ))}

            {showThinking && (
              <div className="mt-[13px]">
                <StreamingMarker messages={ordered} />
              </div>
            )}
          </MessageScrollerContent>
        </MessageScrollerViewport>
        <MessageScrollerButton
          direction="end"
          variant="secondary"
          size="sm"
          className="bottom-4 h-8 w-auto gap-1.5 rounded-full border border-border bg-card px-3.5 text-[11.5px] font-semibold text-muted-foreground shadow-lg hover:bg-card hover:text-foreground"
        >
          Jump to latest
          <ArrowDown className="h-3.5 w-3.5" aria-hidden />
        </MessageScrollerButton>
      </MessageScroller>
    </MessageScrollerProvider>
  );
}

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

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

/** Assistant-side rows share a 29px avatar gutter (drawn once per turn) so
 *  their content aligns under the first row. */
function AssistantGutter({
  showAvatar,
  children,
}: {
  showAvatar: boolean;
  children: React.ReactNode;
}) {
  return (
    <div className="flex gap-[13px]">
      <div className="w-[29px] shrink-0">{showAvatar ? <AssistantAvatar /> : null}</div>
      <div className="min-w-0 flex-1">{children}</div>
    </div>
  );
}

function ItemRow({
  item,
  showAvatar,
  approval,
  onRespondToRequest,
  onAcceptPlan,
  onRejectPlan,
}: {
  item: ChatViewItem;
  showAvatar: boolean;
  approval: PermissionRequestItem | null;
  onRespondToRequest: (requestId: string, decision: ApprovalDecision) => void;
  onAcceptPlan: (requestId: string) => void | Promise<void>;
  onRejectPlan: (requestId: string) => void | Promise<void>;
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
  const handleAcceptPlan = useCallback(() => {
    if (item.kind === "permission_request") return onAcceptPlan(item.request_id);
  }, [item, onAcceptPlan]);
  const handleRejectPlan = useCallback(() => {
    if (item.kind === "permission_request") return onRejectPlan(item.request_id);
  }, [item, onRejectPlan]);

  if (item.kind === "user_message") {
    return <UserMessage item={item} />;
  }

  return (
    <AssistantGutter showAvatar={showAvatar}>
      {renderAssistantBody(item, {
        approval,
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
            <PermissionRequestBlock item={item} onDecide={handlers.handleDecide} />
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
  }
}

function ToolGroupRow({
  items,
  showAvatar,
}: {
  items: ToolCallItem[];
  showAvatar: boolean;
}) {
  return (
    <AssistantGutter showAvatar={showAvatar}>
      <ToolGroupCard items={items} />
    </AssistantGutter>
  );
}

// Default shallow comparison is enough: the parent passes the reducer's
// stable `item` / `approval` references, so an untouched row keeps its
// props identity and skips the whole ItemRow → leaf-component chain. One
// streaming token mutates exactly one item → exactly one row re-renders.
const ItemRowMemo = memo(ItemRow);
const ToolGroupRowMemo = memo(ToolGroupRow);
