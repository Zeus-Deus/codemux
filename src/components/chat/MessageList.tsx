import { ChevronDown, ChevronUp } from "lucide-react";
import {
  memo,
  useCallback,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { Virtuoso, type VirtuosoHandle } from "react-virtuoso";

import type {
  ChatViewItem,
  PermissionRequestItem,
  ToolCallItem,
} from "@/lib/agent-chat/types";
import type { ApprovalDecision } from "@/tauri/events";

import { AssistantMessage } from "./AssistantMessage";
import { PermissionRequestBlock } from "./PermissionRequestBlock";
import { PlanProposalBlock } from "./PlanProposalBlock";
import { ThinkingIndicator } from "./ThinkingIndicator";
import { ToolCallCard } from "./ToolCallCard";
import { UserMessage } from "./UserMessage";

/** A contiguous stretch of this many tool-call status lines gets
 *  collapsed by default. Anything shorter renders inline as before. */
const RUN_COLLAPSE_THRESHOLD = 6;
/** When a run is collapsed, keep this many most-recent tool calls
 *  visible — "a handful of the most recent tool calls happening." */
const RUN_TAIL_VISIBLE = 4;
/** How close to the bottom (px) the user must be for auto-scroll to
 *  keep following the streaming tail. Matches the pre-virtualization
 *  ChatTranscript pin threshold. */
const PIN_THRESHOLD_PX = 80;
/** Extra off-screen pixels Virtuoso keeps rendered above/below the
 *  viewport. Trades a few more mounted rows for smoother scrolling. */
const OVERSCAN_PX = 600;

interface Props {
  messages: ChatViewItem[];
  /** Render the tail "thinking" pulse as the last virtual row. Lives
   *  inside the virtualized list (not a footer) so the stick-to-bottom
   *  scroll keeps it visible while streaming. */
  showThinking?: boolean;
  onRespondToRequest: (requestId: string, decision: ApprovalDecision) => void;
  /** Plan-accept: parent flips the live session to `default` mode
   *  and sends a "Proceed with the plan." synthetic turn. The
   *  request_id is the id of the plan `PermissionRequestItem` the
   *  user clicked — the parent uses it to collapse the card locally
   *  (no sidecar `request-resolved` will ever arrive). */
  onAcceptPlan: (requestId: string) => void | Promise<void>;
  /** Plan-reject: parent sends a generic "Please revise the plan."
   *  turn. No feedback is collected from the user — matching the
   *  plan UIs of other agentic editors. */
  onRejectPlan: (requestId: string) => void | Promise<void>;
}

/**
 * Virtualized transcript body (issue #77). Only the on-screen window
 * of rows (plus OVERSCAN_PX of slack) is mounted in the DOM, so a
 * 5,000-message session scrolls like a 50-message one.
 *
 * Library choice: `react-virtuoso` (MIT). Evaluated against
 * `@tanstack/react-virtual` and `react-window` — Virtuoso is the only
 * one with built-in dynamic row measurement (ResizeObserver-driven,
 * required because messages / tool cards / plan blocks / collapses
 * vary wildly in height and resize on expand) plus first-class
 * bottom-anchoring primitives. The commercially licensed
 * `@virtuoso.dev/message-list` package is NOT used.
 *
 * MessageList owns the scroll container now (Virtuoso must control
 * its own scroller to translate scroll offsets into the rendered
 * window). The stick-to-bottom contract is unchanged from the
 * pre-virtualization ChatTranscript:
 *   - track "pinned" from scroll events (distance ≤ PIN_THRESHOLD_PX);
 *   - after every transcript change (and thinking-pulse toggle), if
 *     pinned, snap to the tail;
 *   - content growth alone never unpins (growth fires no scroll
 *     event), and a user wheel-up unpins on the next scroll event,
 *     so auto-scroll never fights the user.
 */
export function MessageList({
  messages,
  showThinking = false,
  onRespondToRequest,
  onAcceptPlan,
  onRejectPlan,
}: Props) {
  // Sort by seq so order is a property of the data, not of React
  // reconciliation or store-update timing. Stable for equal seq via
  // id tiebreak (should never happen in practice — seq is unique
  // per-thread — but cheap insurance for future callers).
  const ordered = useMemo(() => {
    const copy = messages.slice();
    copy.sort((a, b) => a.seq - b.seq || a.id.localeCompare(b.id));
    return copy;
  }, [messages]);

  // Index permission requests by request_id so the tool_call branch can
  // look up the matching approval for its inline footer in O(1).
  const requestsById = useMemo(() => {
    const map = new Map<string, PermissionRequestItem>();
    for (const m of ordered) {
      if (m.kind === "permission_request") {
        map.set(m.request_id, m);
      }
    }
    return map;
  }, [ordered]);

  // Requests merged into a ToolCallCard via approval_request_id should
  // not also render as a standalone row — the card already owns their
  // approval footer. Build the set once per render.
  const mergedRequestIds = useMemo(() => {
    const set = new Set<string>();
    for (const m of ordered) {
      if (m.kind === "tool_call" && m.approval_request_id) {
        set.add(m.approval_request_id);
      }
    }
    return set;
  }, [ordered]);

  // Group consecutive tool-call rows into runs so long bursts (e.g. a
  // burst of Read / Ran calls at the start of a plan) collapse to a
  // "Show N earlier tool calls" toggle instead of cluttering the
  // transcript. Non-tool items break a run. Tool calls carrying an
  // active approval footer stay out of runs — the approval needs to
  // remain interactable without the user hunting for it.
  //
  // Each slot is one virtual row: a collapsed run is a single row that
  // expands in place (Virtuoso re-measures it via ResizeObserver).
  const slots = useMemo<RenderSlot[]>(() => {
    const out: RenderSlot[] = [];
    let run: ToolCallItem[] = [];
    const flush = () => {
      if (run.length === 0) return;
      if (run.length >= RUN_COLLAPSE_THRESHOLD) {
        out.push({ kind: "toolRun", items: run });
      } else {
        for (const t of run) out.push({ kind: "item", item: t });
      }
      run = [];
    };
    for (const item of ordered) {
      if (
        item.kind === "tool_call" &&
        item.approval_request_id == null
      ) {
        run.push(item);
      } else {
        flush();
        out.push({ kind: "item", item });
      }
    }
    flush();
    if (showThinking) out.push({ kind: "thinking" });
    return out;
  }, [ordered, showThinking]);

  const virtuosoRef = useRef<VirtuosoHandle | null>(null);
  // Whether the user is at (or near) the bottom. Starts true so a
  // freshly mounted pane follows the tail immediately — matching the
  // pre-virtualization behavior.
  const pinnedToBottomRef = useRef(true);

  // Track pinned-ness from real scroll events on Virtuoso's scroller
  // element. Content growth doesn't fire `scroll`, so streaming can
  // never unpin by itself; only the user (or our own snap) moves it.
  const scrollerCleanupRef = useRef<(() => void) | null>(null);
  const handleScrollerRef = useCallback((el: HTMLElement | Window | null) => {
    scrollerCleanupRef.current?.();
    scrollerCleanupRef.current = null;
    if (!(el instanceof HTMLElement)) return;
    const onScroll = () => {
      const distance = el.scrollHeight - el.scrollTop - el.clientHeight;
      pinnedToBottomRef.current = distance <= PIN_THRESHOLD_PX;
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    scrollerCleanupRef.current = () =>
      el.removeEventListener("scroll", onScroll);
  }, []);

  // After each transcript update — or when the thinking pulse toggles —
  // if we were pinned, stick to the tail. `scrollToIndex(LAST, end)`
  // also covers the tail-row-grows-without-count-change case (token
  // deltas mutate the trailing assistant row in place). Runs on mount
  // too (pinned starts true), so a remounted pane with an existing
  // transcript opens at the tail, like a conversation should.
  useLayoutEffect(() => {
    if (!pinnedToBottomRef.current) return;
    virtuosoRef.current?.scrollToIndex({ index: "LAST", align: "end" });
  }, [slots]);

  const itemContent = useCallback(
    (_index: number, slot: RenderSlot) => {
      if (slot.kind === "thinking") {
        return (
          <div className="px-4 pb-3">
            <div
              className="mx-auto w-full max-w-2xl"
              role="status"
              aria-label="Agent is thinking"
            >
              <ThinkingIndicator />
            </div>
          </div>
        );
      }
      if (slot.kind === "item") {
        return (
          <div className="px-4 pb-3">
            <div className="mx-auto w-full max-w-2xl">
              <MessageRowMemo
                item={slot.item}
                approval={lookupApproval(slot.item, requestsById)}
                isMerged={isMerged(slot.item, mergedRequestIds)}
                onRespondToRequest={onRespondToRequest}
                onAcceptPlan={onAcceptPlan}
                onRejectPlan={onRejectPlan}
              />
            </div>
          </div>
        );
      }
      return (
        <div className="px-4 pb-3">
          <div className="mx-auto w-full max-w-2xl">
            <ToolRunCollapse
              items={slot.items}
              requestsById={requestsById}
              onRespondToRequest={onRespondToRequest}
              onAcceptPlan={onAcceptPlan}
              onRejectPlan={onRejectPlan}
            />
          </div>
        </div>
      );
    },
    [
      requestsById,
      mergedRequestIds,
      onRespondToRequest,
      onAcceptPlan,
      onRejectPlan,
    ],
  );

  return (
    <Virtuoso<RenderSlot>
      ref={virtuosoRef}
      scrollerRef={handleScrollerRef}
      style={VIRTUOSO_STYLE}
      data={slots}
      computeItemKey={computeSlotKey}
      itemContent={itemContent}
      increaseViewportBy={OVERSCAN_PX}
      components={VIRTUOSO_COMPONENTS}
    />
  );
}

const VIRTUOSO_STYLE = { height: "100%" } as const;

/** Top/bottom breathing room inside the scroller — replaces the old
 *  ChatTranscript `py-4` + trailing `h-4` spacer. Stable component
 *  refs (module scope) so Virtuoso never remounts them. */
const VIRTUOSO_COMPONENTS = {
  Header: () => <div className="h-4" />,
  Footer: () => <div className="h-4" />,
};

type RenderSlot =
  | { kind: "item"; item: ChatViewItem }
  | { kind: "toolRun"; items: ToolCallItem[] }
  | { kind: "thinking" };

/** Stable per-slot keys: the same id-based keys the plain `.map()`
 *  render used, so React row identity (and `MessageRowMemo` skips)
 *  survives the virtualization window sliding around. */
function computeSlotKey(_index: number, slot: RenderSlot): string {
  if (slot.kind === "item") return slot.item.id;
  if (slot.kind === "toolRun") return `run:${slot.items[0].id}`;
  return "thinking";
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

function isMerged(
  item: ChatViewItem,
  mergedRequestIds: Set<string>,
): boolean {
  return (
    item.kind === "permission_request" &&
    mergedRequestIds.has(item.request_id)
  );
}

/**
 * Collapses a stretch of ≥ RUN_COLLAPSE_THRESHOLD consecutive tool
 * calls into a "Show N earlier tool calls" toggle + the last
 * RUN_TAIL_VISIBLE rows. Click to expand; click again to collapse.
 *
 * The whole run is a single virtual row; expanding it grows the row
 * in place and Virtuoso re-measures via its ResizeObserver. The inner
 * rows reproduce the transcript's `gap-3` vertical rhythm.
 */
function ToolRunCollapse({
  items,
  requestsById,
  onRespondToRequest,
  onAcceptPlan,
  onRejectPlan,
}: {
  items: ToolCallItem[];
  requestsById: Map<string, PermissionRequestItem>;
  onRespondToRequest: (requestId: string, decision: ApprovalDecision) => void;
  onAcceptPlan: (requestId: string) => void | Promise<void>;
  onRejectPlan: (requestId: string) => void | Promise<void>;
}) {
  const [expanded, setExpanded] = useState(false);
  const hiddenCount = items.length - RUN_TAIL_VISIBLE;
  const visible = expanded ? items : items.slice(-RUN_TAIL_VISIBLE);
  const pluralSuffix = hiddenCount === 1 ? "" : "s";

  return (
    <div className="flex flex-col gap-3">
      <button
        type="button"
        onClick={() => setExpanded((v) => !v)}
        className="flex items-center gap-1.5 py-0.5 text-xs text-muted-foreground/80 hover:text-foreground transition-colors"
      >
        {expanded ? (
          <ChevronUp className="h-3 w-3" aria-hidden />
        ) : (
          <ChevronDown className="h-3 w-3" aria-hidden />
        )}
        <span>
          {expanded
            ? `Hide ${hiddenCount} earlier tool call${pluralSuffix}`
            : `Show ${hiddenCount} earlier tool call${pluralSuffix}`}
        </span>
      </button>
      {visible.map((item) => (
        <MessageRowMemo
          key={item.id}
          item={item}
          approval={lookupApproval(item, requestsById)}
          isMerged={false}
          onRespondToRequest={onRespondToRequest}
          onAcceptPlan={onAcceptPlan}
          onRejectPlan={onRejectPlan}
        />
      ))}
    </div>
  );
}

function MessageRow({
  item,
  approval,
  isMerged,
  onRespondToRequest,
  onAcceptPlan,
  onRejectPlan,
}: {
  item: ChatViewItem;
  approval: PermissionRequestItem | null;
  isMerged: boolean;
  onRespondToRequest: (requestId: string, decision: ApprovalDecision) => void;
  onAcceptPlan: (requestId: string) => void | Promise<void>;
  onRejectPlan: (requestId: string) => void | Promise<void>;
}) {
  // Stable per-row decision callback. Without useCallback, ToolCallCard
  // and PermissionRequestBlock receive a fresh function on every render
  // and their own React.memo wrappers can't skip work.
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
    if (item.kind === "permission_request") {
      return onAcceptPlan(item.request_id);
    }
  }, [item, onAcceptPlan]);
  const handleRejectPlan = useCallback(() => {
    if (item.kind === "permission_request") {
      return onRejectPlan(item.request_id);
    }
  }, [item, onRejectPlan]);

  switch (item.kind) {
    case "user_message":
      return <UserMessage item={item} />;
    case "assistant_message":
      return <AssistantMessage item={item} />;
    case "tool_call": {
      return (
        <ToolCallCard
          item={item}
          approval={approval}
          onDecide={handleDecide}
        />
      );
    }
    case "permission_request": {
      // Requests already consumed by a ToolCallCard render there.
      if (isMerged) return null;
      // Stage 2 dispatch by request_kind: plan / user-input get
      // specialized renderers; everything else stays on the generic
      // PermissionRequestBlock fallback so future kinds (MCP server,
      // directory approval, …) keep working without a new branch.
      switch (item.request_kind) {
        case "plan":
          return (
            <PlanProposalBlock
              item={item}
              onAccept={handleAcceptPlan}
              onReject={handleRejectPlan}
            />
          );
        case "user-input": {
          // The interactive panel for user-input prompts lives attached
          // to the composer (see `ComposerPendingInputPanel` mounted in
          // AgentChatPane), not inline. Render a tiny status marker in
          // the transcript so the turn's thread-of-events stays
          // readable and the resolution state still surfaces.
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
            <PermissionRequestBlock
              item={item}
              onDecide={handleDecide}
            />
          );
      }
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

// Default reference comparator is enough now that the parent passes the
// resolved `approval` ref (not the whole map). The reducer mutates one
// item at a time via `replaceItem`, so non-changing rows keep their
// identity and skip the entire MessageRow → leaf-component chain.
const MessageRowMemo = memo(MessageRow);
