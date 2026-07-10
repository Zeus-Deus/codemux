import { ArrowDown, TriangleAlert } from "lucide-react";
import {
  memo,
  useCallback,
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
import { UserMessage } from "./UserMessage";
import { WorkflowRunCard } from "./WorkflowRunCard";
import { transcriptFadeEnabled } from "./transcript-fade";
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
 * Transcript body on the shadcn **MessageScroller** (design D2). The
 * scroller provides the viewport/rows/jump-button anatomy and
 * `content-visibility:auto` row containment (thousands of turns stay
 * cheap; the reducer's 5,000-message cap bounds the worst case). The
 * engine (`MessageScrollerProvider autoScroll`) is the SINGLE owner of
 * stick-to-bottom: its `following-bottom` state machine re-scrolls to
 * the end on every ResizeObserver/MutationObserver-detected content
 * change while following, unpins on user wheel/touch/keydown gestures,
 * and re-pins once the reader scrolls back within its 8px edge
 * threshold. Scroll stability is split by ownership: native browser
 * scroll anchoring owns free-scroll (while the reader scrolls history it
 * absorbs the `content-visibility:auto` estimated-then-real height
 * settling of rows above the viewport); the engine owns following-bottom.
 * The two never fight because the viewport disables anchoring by default
 * (`[overflow-anchor:none]` — pinned/following, the engine owns the tail
 * snap) and re-enables it only while the reader is away from the bottom
 * (`data-[scrollable~=end]:[overflow-anchor:auto]` — the engine keeps
 * "end" in the viewport's `data-scrollable` attribute exactly while the
 * user is unpinned in history, updated promptly on scroll commits). Do
 * NOT scope this off `data-autoscrolling`: that attribute is only
 * rewritten on engine state commits and stays stale — set — indefinitely
 * after the user scrolls up and goes idle, which is precisely when
 * anchoring is needed. Per-item turn anchoring is still off (see
 * `scrollAnchor={false}` below) — that remains a deliberate, independent
 * setting. Per-slot `contain-intrinsic-size` estimates
 * (`intrinsicSizeClass`) keep each first-reveal settle small so
 * anchoring's corrections stay imperceptible.
 *
 * Layout is derived by the pure `buildTranscriptSlots` (turn grouping,
 * tool-run folding, avatar/turn-boundary metadata), then `reuseTranscriptSlots`
 * swaps unchanged slots back to their previous object identity across rebuilds.
 * Each slot renders one `MessageScrollerItem` (with a stable `messageId`)
 * through a two-level memo: the whole-row wrapper `SlotRowMemo` skips on that
 * reused-slot identity, and the leaf (`ItemRowMemo` / `ActivityRowMemo`) skips
 * independently — so a single streaming token re-renders exactly one wrapper
 * and one leaf while the rest of the transcript skips reconciliation.
 */
export function MessageList({
  messages,
  showThinking = false,
  streaming = false,
  stalled = null,
  interrupted = false,
  sessionStartedAt,
  provider,
  onRespondToRequest,
  onAcceptPlan,
  onRejectPlan,
  onCancelQueued,
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

  return (
    <MessageScrollerProvider autoScroll>
      <MessageScroller>
        {/* Navigation trail — a turn rail in the left gutter (inside the
            provider, sibling of the viewport). Reads the active turn from
            `visibleMessageIds`; hides itself on short threads. */}
        <MessageTrail slots={slots} />
        <MessageScrollerViewport
          style={transcriptFadeEnabled() ? WS_FADE_STYLE : undefined}
        >
          <MessageScrollerContent
            aria-busy={showThinking || undefined}
            className="mx-auto w-full max-w-[760px] gap-0 px-7 pb-[30px] pt-[26px]"
          >
            <SessionStartMarker startedAt={sessionStartedAt} />

            {slots.map((slot) => (
              <SlotRowMemo
                key={slot.key}
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
                onEnterSubagent={onEnterSubagent}
              />
            ))}

            {showBrowserChip && backgroundBrowserSession && workspaceId && (
              <div className="mt-[13px]">
                <BackgroundBrowserChip
                  session={backgroundBrowserSession}
                  workspaceId={workspaceId}
                />
              </div>
            )}

            {/* A silently-stalled mid-turn run (issue #154): amber notice
                in place of the ember streaming marker. Takes priority over
                the marker so the two never stack. */}
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

            {/* The last run never cleanly settled (child exit / crash).
                Mutually exclusive with the streaming marker via `!streaming`. */}
            {interrupted && !streaming && (
              <div className="mt-[13px]">
                <RunInterruptedDivider />
              </div>
            )}
          </MessageScrollerContent>
        </MessageScrollerViewport>
        <MessageScrollerButton
          direction="end"
          behavior="auto"
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
 *  so this is applied inline.
 *
 *  Gated off on Linux WebKitGTK (issue #129, see `transcript-fade.ts`): the
 *  app forces that engine into non-composited (CPU) mode via the
 *  `WEBKIT_DISABLE_COMPOSITING_MODE` / `WEBKIT_DISABLE_DMABUF_RENDERER` env
 *  vars in `src-tauri/src/lib.rs`, where this mask forces a full-viewport CPU
 *  re-rasterization on every scroll frame. The design intent is kept on every
 *  composited platform (macOS / Windows / dev-mock Chromium — byte-identical),
 *  and the `codemux:transcript-fade` localStorage override re-enables it. */
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
      <span className="font-mono text-[10.5px] font-medium tracking-wide">
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
  if (item.kind === "workflow_run" && item.approvalRequestId != null) {
    return requestsById.get(item.approvalRequestId) ?? null;
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

/**
 * Per-slot `content-visibility` first-reveal estimate. Each row is
 * `[content-visibility:auto]`, so before it first paints the browser lays
 * it out at the `contain-intrinsic-size` estimate and only measures the
 * real height on reveal. When the user scrolls up through cold history the
 * difference between estimate and real height is exactly the correction the
 * browser's native scroll anchoring has to absorb — so the closer the
 * estimate is to a row-type's typical height, the smaller (and less
 * visible) that correction is in BOTH scroll directions. Keying the
 * estimate off the slot's body kind gets us most of that accuracy for free
 * without any measurement, observers, or JS scroll compensation. Rows with
 * no strong typical height fall through to the `MessageScrollerItem`
 * default (`auto_6rem`); tailwind-merge lets the per-row estimate here win
 * over that default (last arbitrary-property wins).
 */
function intrinsicSizeClass(slot: TranscriptSlot): string {
  // Folded mechanical-step runs (reasoning + tool calls) render a multi-row
  // Activity block, so they're taller than a single step.
  if (slot.body.kind === "activity") return "[contain-intrinsic-size:auto_8rem]";
  switch (slot.body.item.kind) {
    case "user_message":
      return "[contain-intrinsic-size:auto_3.5rem]";
    case "assistant_message":
      return "[contain-intrinsic-size:auto_7rem]";
    case "tool_call":
    case "reasoning":
      return "[contain-intrinsic-size:auto_5rem]";
    case "runtime_notice":
      return "[contain-intrinsic-size:auto_2.5rem]";
    case "subagent_run":
    case "workflow_run":
      return "[contain-intrinsic-size:auto_14rem]";
    default:
      return "";
  }
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
    return <UserMessage item={item} onCancelQueued={onCancelQueued} />;
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
// wrapper below — it owns the `MessageScrollerItem` element and the
// activity/item branch. `reuseTranscriptSlots` keeps an untouched slot's object
// identity across store rebuilds, so with the reducer's stable `item` /
// `approval` references and a per-session-stable `provider` the wrapper's props
// stay shallow-equal and its default `memo` comparison skips the row entirely —
// including the `MessageScrollerItem` re-render. Level 2 is these leaves
// (`ItemRowMemo` / `ActivityRowMemo`), which independently skip when their own
// props are unchanged. A single streaming token mutates exactly one slot → its
// wrapper AND its leaf are the only things that re-render. (Activity rows
// rebuild their `items` array each pass, like the old tool groups did; the
// stable slot key keeps the scroller row from remounting.)
const ItemRowMemo = memo(ItemRow);
const ActivityRowMemo = memo(ActivityRow);

/**
 * Whole-row wrapper (level-1 memo — see the comment above the leaf memos). It
 * owns the `MessageScrollerItem` element (key stays on the mapped element in
 * the parent) and dispatches to the activity/item leaf. Splitting it out of
 * the `slots.map()` body is what lets the reused-slot identity skip an
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
  onEnterSubagent?: (subagentId: string) => void;
}) {
  return (
    <MessageScrollerItem
      messageId={slot.messageId}
      // Turn anchoring is deliberately OFF: with a fully hydrated transcript
      // (hundreds of pre-existing rows) the engine's anchor handling scrolls
      // the viewport to a stale early anchor when new items register mid-
      // stream, which breaks the stick-to-bottom contract. The scroller
      // engine's `following-bottom` mode owns tail tracking (see the file-top
      // doc comment).
      scrollAnchor={false}
      className={cn(
        slot.turnStart ? "mt-5" : "mt-[13px]",
        intrinsicSizeClass(slot),
      )}
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
          onEnterSubagent={onEnterSubagent}
          workspaceId={workspaceId}
        />
      )}
    </MessageScrollerItem>
  );
}

const SlotRowMemo = memo(SlotRow);
