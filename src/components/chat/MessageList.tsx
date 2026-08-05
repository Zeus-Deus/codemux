import { ArrowDown, TriangleAlert } from "lucide-react";
import {
  LegendList,
  type AnchoredEndSpaceConfig,
  type LegendListRef,
} from "@legendapp/list/react";
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
import {
  clearTitlebarContentUnder,
  publishTitlebarContentUnder,
  registerTitlebarTranscript,
} from "@/lib/titlebar-content-under";
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
  getAnchoredTurnMetrics,
  realContentOverflowsViewport,
  resolveSendAnchorIndex,
  SEND_ANCHOR_OFFSET,
  type SendAnchorReadyInfo,
  type SendAnchorRequest,
  type SendScrollMode,
} from "./send-scroll-state";
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
  /** The new-turn scroll contract's navigation intent (see
   *  `send-scroll-state.ts`). `AgentChatPane` issues one in the same batch as
   *  the optimistic `appendUserMessage`, carrying that bubble's `clientNonce`
   *  plus an incrementing `nonce`. Each *new* `nonce` puts the list into
   *  `anchoring-turn`: the matching prompt row is parked
   *  `SEND_ANCHOR_OFFSET` below the top and the answer streams into the space
   *  reserved beneath it. Cleared (`null`) on failed-send rollback and on
   *  thread change, which also removes that reserved space. Absent /
   *  unchanged ⇒ no forced scroll, so free-scroll through history and
   *  provider events that arrive without a send intent are untouched. */
  sendAnchor?: SendAnchorRequest | null;
  /** Thread (or pane) identity. A change resets scroll intent to
   *  `following-end` so switching away and back behaves exactly like a fresh
   *  hydrated open rather than resuming a stale follow/anchor decision. */
  threadKey?: string | null;
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
  sendAnchor,
  threadKey,
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
  // suppress pane creation: Agent Chat beta on.
  // `workspaceId` is absent for legacy/non-workspace-scoped callers, so
  // the chip never renders there — byte-identical output preserved.
  const enableAgentChat = useFeatureFlags((s) => s.enableAgentChat);
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
  const titlebarScrollSourceRef = useRef(Symbol("chat-scroll-viewport"));
  const [firstVisibleSlotIndex, setFirstVisibleSlotIndex] = useState(() =>
    Math.max(0, slots.length - 1),
  );

  // -------------------------------------------------------------------------
  // New-turn scroll contract (see send-scroll-state.ts for the state model)
  // -------------------------------------------------------------------------
  const modeRef = useRef<SendScrollMode>("following-end");
  const isAtEndRef = useRef(true);
  const [showJumpToLatest, setShowJumpToLatest] = useState(false);
  const pillTimerRef = useRef<number | null>(null);
  // Bumped whenever a pending "show the pill" decision is invalidated. The
  // timer callback compares against it instead of being cleared, so the
  // invalidation is a pure ref write and therefore safe to run during render.
  const pillEpochRef = useRef(0);
  // The generation pair *is* the predicate "we still own the viewport". A
  // real reader gesture bumps `userScrollGenerationRef` and drops the follow
  // claim, which invalidates every in-flight continuation below in one move —
  // each re-checks `ownsScroll()` before touching the scroll position.
  const userScrollGenerationRef = useRef(0);
  const followGenerationRef = useRef<number | null>(0);
  // Anchor lifecycle for one `clientNonce`: measured (`onReady` handed us the
  // resolved row index) → positioned (`scrollToIndex` issued). Positioning is
  // instant per Codemux's "immediate" contract, so there is no animation to
  // settle and two stages is the whole ladder.
  const anchorIndexRef = useRef<number | null>(null);
  const positionedAnchorRef = useRef<string | null>(null);
  const anchorFrameRef = useRef<number | null>(null);

  const ownsScroll = useCallback(
    () => followGenerationRef.current === userScrollGenerationRef.current,
    [],
  );

  /** Pure ref writes only — callable from the render-phase intent block. */
  const claimScroll = useCallback((mode: SendScrollMode) => {
    modeRef.current = mode;
    isAtEndRef.current = true;
    followGenerationRef.current = userScrollGenerationRef.current;
    anchorIndexRef.current = null;
    positionedAnchorRef.current = null;
    pillEpochRef.current += 1;
  }, []);

  const hideJumpToLatest = useCallback(() => {
    pillEpochRef.current += 1;
    if (pillTimerRef.current !== null) {
      window.clearTimeout(pillTimerRef.current);
      pillTimerRef.current = null;
    }
    setShowJumpToLatest(false);
  }, []);

  /** Showing is debounced, hiding never is. LegendList reports
   *  `isAtEnd: false` throughout mount and layout settling, so an immediate
   *  show would flash the pill on every thread open. */
  const scheduleJumpToLatest = useCallback(() => {
    // Re-arm rather than bail: a timer left in flight by an intent that has
    // since been invalidated must not block the next honest request.
    if (pillTimerRef.current !== null) {
      window.clearTimeout(pillTimerRef.current);
    }
    const epoch = pillEpochRef.current;
    pillTimerRef.current = window.setTimeout(() => {
      pillTimerRef.current = null;
      if (pillEpochRef.current !== epoch) return;
      setShowJumpToLatest(true);
    }, JUMP_PILL_SHOW_DELAY_MS);
  }, []);

  /** A real reader gesture (or a deliberate jump elsewhere in the
   *  transcript) permanently retires the current live-follow claim. */
  const cancelFollowForUserNavigation = useCallback(() => {
    userScrollGenerationRef.current += 1;
    followGenerationRef.current = null;
    modeRef.current = "free-scrolling";
    anchorIndexRef.current = null;
    positionedAnchorRef.current = null;
    // LegendList reports `isAtEnd` only on TRANSITIONS, so a gesture landing
    // while the value is already false gets no event of its own. This is the
    // moment the viewport changes hands, so it is also where the pill
    // decision belongs for a reader who is already off the edge — otherwise
    // they browse history with no way back.
    const atEnd = listRef.current?.getState()?.isAtEnd;
    if (atEnd === undefined) return;
    isAtEndRef.current = atEnd;
    if (atEnd) {
      hideJumpToLatest();
    } else {
      scheduleJumpToLatest();
    }
  }, [hideJumpToLatest, scheduleJumpToLatest]);

  // Apply a send's navigation intent synchronously, during the same render
  // that first sees it — not in an effect. The composer's optimistic row and
  // this intent land in one commit, so the list must already be in
  // `anchoring-turn` (and the pill already hidden) by the time LegendList
  // measures that row.
  const anchorClientNonce = sendAnchor?.clientNonce ?? null;
  const sendNonce = sendAnchor?.nonce ?? null;
  // Deliberately seeded `null`, not with the current nonce: a list that
  // mounts with an anchor already in hand (a pane remounting after a
  // workspace switch) still has to honour it.
  const lastSendNonceRef = useRef<number | null>(null);
  const lastThreadKeyRef = useRef<string | null | undefined>(threadKey);
  if (lastThreadKeyRef.current !== threadKey) {
    // Thread / pane identity changed: behave like a fresh hydrated open
    // rather than resuming a stale follow or anchor decision.
    lastThreadKeyRef.current = threadKey;
    lastSendNonceRef.current = sendNonce;
    claimScroll("following-end");
    setShowJumpToLatest(false);
  } else if (sendNonce !== lastSendNonceRef.current) {
    lastSendNonceRef.current = sendNonce;
    if (sendAnchor) {
      claimScroll("anchoring-turn");
      setShowJumpToLatest(false);
    } else if (ownsScroll()) {
      // The anchor was cleared — a failed-send rollback, or the turn
      // settling — while we still own the viewport. The reserved end space
      // goes away and plain tail following takes over.
      claimScroll("following-end");
      setShowJumpToLatest(false);
    } else {
      // Same clear, but the reader has gestured into free-scrolling. Drop
      // the anchor bookkeeping only: a turn finishing must never yank a
      // reader who deliberately scrolled away back to the tail.
      anchorIndexRef.current = null;
      positionedAnchorRef.current = null;
    }
  }

  useEffect(
    () => () => {
      if (pillTimerRef.current !== null) {
        window.clearTimeout(pillTimerRef.current);
      }
      if (anchorFrameRef.current !== null) {
        cancelAnimationFrame(anchorFrameRef.current);
      }
    },
    [],
  );

  const handleIsAtEndChange = useCallback(
    (atEnd: boolean) => {
      // Record reality FIRST, on every event, including ones we go on to
      // swallow. The listener is edge-triggered, so a swallowed value that
      // never reaches this ref leaves us disagreeing with the list for as
      // long as that value holds — and the disagreement then eats the
      // *next* real transition via the dedup below.
      const changed = isAtEndRef.current !== atEnd;
      isAtEndRef.current = atEnd;
      if (!atEnd && ownsScroll()) {
        // We are the one driving. A transient "not at end" while mounting,
        // while the anchored row is being positioned, or while the reserved
        // end space is consumed is layout settling — not the reader leaving.
        hideJumpToLatest();
        return;
      }
      if (!changed) return;
      if (atEnd) {
        // Scrolling back to the edge re-claims live follow, even after a
        // gesture had released it.
        modeRef.current = "following-end";
        followGenerationRef.current = userScrollGenerationRef.current;
        hideJumpToLatest();
      } else {
        modeRef.current = "free-scrolling";
        followGenerationRef.current = null;
        scheduleJumpToLatest();
      }
    },
    [hideJumpToLatest, ownsScroll, scheduleJumpToLatest],
  );

  useEffect(() => {
    const state = listRef.current?.getState();
    if (!state) return;
    handleIsAtEndChange(state.isAtEnd);
    return state.listen("isAtEnd", handleIsAtEndChange);
  }, [handleIsAtEndChange]);

  // Only genuine navigation gestures release follow. Deliberately NOT
  // `scroll`: every programmatic correction below emits one, and treating
  // those as reader intent is exactly the bug that left the pill stuck on.
  useEffect(() => {
    let removeListeners: (() => void) | null = null;
    const frame = requestAnimationFrame(() => {
      const viewport = listRef.current?.getScrollableNode();
      if (!viewport) return;
      const onGesture = () => cancelFollowForUserNavigation();
      // `pointerdown` is here for scrollbar drags, which target the scroll
      // container itself. A press on a row targets a descendant — accepting
      // a plan, answering an approval, expanding a tool card, or just
      // starting a text selection — and must NOT retire follow: while an
      // anchor is mounted the built-in end pin is off and the advance effect
      // is the only thing moving the viewport, so cancelling on an ordinary
      // click would freeze the transcript for the rest of a live run.
      const onPointerDown = (event: Event) => {
        if (event.target !== viewport) return;
        cancelFollowForUserNavigation();
      };
      viewport.addEventListener("wheel", onGesture, { passive: true });
      viewport.addEventListener("touchmove", onGesture, { passive: true });
      viewport.addEventListener("pointerdown", onPointerDown, {
        passive: true,
      });
      removeListeners = () => {
        viewport.removeEventListener("wheel", onGesture);
        viewport.removeEventListener("touchmove", onGesture);
        viewport.removeEventListener("pointerdown", onPointerDown);
      };
    });
    return () => {
      cancelAnimationFrame(frame);
      removeListeners?.();
    };
  }, [cancelFollowForUserNavigation, threadKey]);

  /** LegendList has measured the anchored row and reserved the end space
   *  below it. Position the row exactly once per send. */
  const handleAnchorReady = useCallback(
    (info: SendAnchorReadyInfo) => {
      const clientNonce = anchorClientNonce;
      const anchorIndex = info.anchorIndex;
      if (!clientNonce || anchorIndex === undefined) return;
      // Always refresh the index: rows landing above the prompt shift it.
      anchorIndexRef.current = anchorIndex;
      if (positionedAnchorRef.current === clientNonce) return;
      positionedAnchorRef.current = clientNonce;

      const position = (attemptsLeft: number) => {
        anchorFrameRef.current = requestAnimationFrame(() => {
          anchorFrameRef.current = null;
          if (positionedAnchorRef.current !== clientNonce) return;
          if (!ownsScroll()) return;
          const list = listRef.current;
          if (!list) {
            // The ref can still be empty on the frame the list mounts;
            // retry on later frames rather than assuming a fixed delay.
            if (attemptsLeft > 0) position(attemptsLeft - 1);
            return;
          }
          void list.scrollToIndex({
            index: anchorIndexRef.current ?? anchorIndex,
            animated: false,
            viewPosition: 0,
            viewOffset: SEND_ANCHOR_OFFSET,
          });
        });
      };
      position(ANCHOR_POSITION_ATTEMPTS);
    },
    [anchorClientNonce, ownsScroll],
  );

  // The reserved response area. Resolving by `clientNonce` (last match wins)
  // rather than "the last row" is what keeps the anchor on the prompt the
  // reader just sent when queued or control rows follow it.
  const anchoredEndSpace = useMemo<AnchoredEndSpaceConfig | undefined>(() => {
    const anchorIndex = resolveSendAnchorIndex(
      slots,
      anchorClientNonce,
      slotClientNonce,
    );
    if (anchorIndex === null) return undefined;
    return {
      anchorIndex,
      anchorOffset: SEND_ANCHOR_OFFSET,
      onReady: handleAnchorReady,
    };
  }, [anchorClientNonce, handleAnchorReady, slots]);

  // Advance the viewport as the answer streams in. Two nested frames let
  // LegendList lay out and measure the new content first; every guard below
  // re-checks that we still own the viewport, so a gesture landing mid-flight
  // wins. In `anchoring-turn` we move by exactly the distance needed to
  // reveal the tail — which is zero while the turn still fits, so the prompt
  // stays parked near the top until the turn actually outgrows the viewport.
  useEffect(() => {
    if (!ownsScroll()) return;
    let second: number | null = null;
    const first = requestAnimationFrame(() => {
      second = requestAnimationFrame(() => {
        second = null;
        if (!ownsScroll()) return;
        const list = listRef.current;
        if (!list) return;
        const state = list.getState();
        if (modeRef.current === "anchoring-turn") {
          const anchorIndex = anchorIndexRef.current;
          // Not measured yet — `onReady` owns the first positioning.
          if (anchorIndex === null) return;
          const metrics = getAnchoredTurnMetrics({ state, anchorIndex });
          if (!metrics || metrics.scrollDeltaToRevealEnd <= 1) return;
          void list.scrollToOffset({
            offset: state.scroll + metrics.scrollDeltaToRevealEnd,
            animated: false,
          });
          return;
        }
        if (modeRef.current !== "following-end") return;
        // With no anchor mounted, LegendList's own end pin is enabled and
        // already owns this — driving it twice would be redundant work on
        // every streamed token. We only take over for the window where an
        // anchor has disabled that pin but the reader is back at the edge.
        if (!anchoredEndSpace) return;
        // Without this the reserved blank space would be a scroll target.
        if (!realContentOverflowsViewport(state)) return;
        void list.scrollToEnd({ animated: false });
      });
    });
    return () => {
      cancelAnimationFrame(first);
      if (second !== null) cancelAnimationFrame(second);
    };
  }, [anchoredEndSpace, ownsScroll, slots, threadKey]);

  /** The pill is the deliberate way back to the live edge, so it re-claims
   *  follow rather than cancelling it. */
  const handleJumpToLatest = useCallback(() => {
    claimScroll("following-end");
    setShowJumpToLatest(false);
    void listRef.current?.scrollToEnd({ animated: false });
  }, [claimScroll]);

  // Publish this viewport to the titlebar's live-element registry so its
  // overlap measurement always runs against mounted nodes. `PaneContainer`
  // renders only the active surface, so switching tabs or workspaces
  // unmounts this list and mounts a fresh one — register/unregister is what
  // tells the titlebar to re-measure. Not gated on `workspaceId`: the
  // measurement is purely geometric, and a hidden/unsized transcript is
  // filtered out there by its zero-area rect.
  useEffect(() => {
    const viewport = listRef.current?.getScrollableNode();
    if (!viewport) return;
    return registerTitlebarTranscript(viewport);
  }, []);

  // Drive the overlay from the transcript's actual scroll position. This
  // publishes only a boolean transition, not every scroll frame, and the
  // external store safely aggregates multiple chat panes in one workspace.
  useEffect(() => {
    if (!workspaceId) return;
    const viewport = listRef.current?.getScrollableNode();
    if (!viewport) return;
    const source = titlebarScrollSourceRef.current;
    const sync = () =>
      publishTitlebarContentUnder(workspaceId, source, viewport.scrollTop > 4);

    sync();
    viewport.addEventListener("scroll", sync, { passive: true });
    return () => {
      viewport.removeEventListener("scroll", sync);
      clearTitlebarContentUnder(workspaceId, source);
    };
  }, [workspaceId]);

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
    // Jumping to a card is deliberate navigation away from the live tail:
    // release follow so an in-flight turn does not drag the reader back.
    cancelFollowForUserNavigation();
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
  }, [cancelFollowForUserNavigation, subagentJumpRequest, subagentTargetIndex]);

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
        anchoredEndSpace={anchoredEndSpace}
        // Follow-the-tail and anchor-the-new-turn are two different targets
        // for the same viewport, so they must not both be live. While an
        // anchor is reserving response space, the effect above is the sole
        // driver — otherwise the built-in end pin would immediately yank the
        // freshly parked prompt off the top of the screen.
        maintainScrollAtEnd={
          anchoredEndSpace ? false : MAINTAIN_SCROLL_AT_END
        }
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
      {showJumpToLatest && (
        <Button
          type="button"
          onClick={handleJumpToLatest}
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

/** Trailing debounce before the "Jump to latest" pill is allowed to appear.
 *  Long enough to cover mount and layout settling, short enough that a reader
 *  who scrolls up gets the affordance without noticing the wait. Hiding is
 *  always immediate — an unwanted pill is worse than a late one. */
const JUMP_PILL_SHOW_DELAY_MS = 150;

/** Frames the anchor positioner will wait for the list ref to exist before
 *  giving up. A frame budget, not a fixed timeout: it cannot assume layout
 *  completes in N milliseconds on a given machine. */
const ANCHOR_POSITION_ATTEMPTS = 12;

/** Hoisted so a re-render never hands LegendList a fresh config object. */
const MAINTAIN_SCROLL_AT_END = {
  animated: false,
  on: {
    dataChange: true,
    footerLayout: true,
    itemLayout: true,
    layout: true,
  },
} as const;

/** The optimistic-send correlation token of a slot's user bubble, if it has
 *  one. Everything else in the transcript is unanchorable by construction. */
function slotClientNonce(slot: TranscriptSlot): string | null {
  if (slot.body.kind !== "item") return null;
  const item = slot.body.item;
  return item.kind === "user_message" ? item.clientNonce ?? null : null;
}

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
