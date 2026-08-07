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
  getSendAnchorTargetOffset,
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
  /** Pane-owned record of the send nonce whose anchor row has already been
   *  positioned. The anchor now outlives the turn (it clears only on the
   *  next send, a rollback, or a thread switch), so this bookkeeping must
   *  live above the component: a `MessageList` remount mid- or post-turn
   *  (the subagent drill-in/out swap) re-resolves the reserved space but
   *  must never re-run the one-time positioning scroll. Absent → a local
   *  ref is used (legacy callers keep the old per-mount behavior). */
  positionedNonceRef?: { current: number | null };
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
  positionedNonceRef,
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
  // Anchor lifecycle for one send nonce: measured (`onReady` handed us the
  // resolved row index) → positioned (the smooth `scrollToIndex` glide was
  // issued) → settled (`scrollend`, or the fallback timer where the event is
  // unsupported, confirmed the glide finished and the offset was re-pinned).
  // "Positioned" is nonce-keyed and pane-owned when the caller provides the
  // ref, so a remount while the anchor is still mounted never re-parks the
  // prompt; "settled" gates the stream-advance effect so a fast first token
  // cannot fight the glide.
  const anchorIndexRef = useRef<number | null>(null);
  const localPositionedNonceRef = useRef<number | null>(null);
  const positionedRef = positionedNonceRef ?? localPositionedNonceRef;
  const settledNonceRef = useRef<number | null>(null);
  // Which nonce *this mount* actually issued the glide for. "Positioned" is
  // pane-owned and "settled" is per-mount, so without this the two records
  // disagree after a remount: the fresh mount sees an already-positioned
  // nonce, has no settle handshake in flight, and would gate the
  // stream-advance for the rest of the turn.
  const glideOwnerNonceRef = useRef<number | null>(null);
  // A positioning chain waiting on a frame. Nonce-keyed so a re-fired
  // `onReady` joins the in-flight attempt instead of starting a second one.
  const pendingPositionNonceRef = useRef<number | null>(null);
  const settleCleanupRef = useRef<(() => void) | null>(null);
  const anchorFrameRef = useRef<number | null>(null);
  const driftFrameRef = useRef<number | null>(null);
  // The anchored turn's reserved space has collapsed to nothing — the reply
  // outgrew it, so LegendList stops firing `onSizeChanged` from here on.
  const spacerCollapsedNonceRef = useRef<number | null>(null);
  // Render-visible half of the same fact: which nonce's turn has handed the
  // built-in end pin back (spacer spent *and* glide settled).
  const [endPinRestoredNonce, setEndPinRestoredNonce] = useState<number | null>(
    null,
  );

  const ownsScroll = useCallback(
    () => followGenerationRef.current === userScrollGenerationRef.current,
    [],
  );

  /** Pure ref writes only — callable from the render-phase intent block.
   *  Deliberately does not touch the positioned/settled nonce records: those
   *  are monotonic per-send facts, and a stale settle handshake left in
   *  flight no-ops on its own nonce guard. */
  const claimScroll = useCallback((mode: SendScrollMode) => {
    modeRef.current = mode;
    isAtEndRef.current = true;
    followGenerationRef.current = userScrollGenerationRef.current;
    anchorIndexRef.current = null;
    spacerCollapsedNonceRef.current = null;
    pillEpochRef.current += 1;
  }, []);

  /** True while this send's positioning glide is in flight — the row has been
   *  positioned but no settle handshake has confirmed it landed. Reads the
   *  nonce through a ref so the callback identity stays stable across sends
   *  (the edge listener below is subscribed with it). */
  const sendNonceRef = useRef<number | null>(null);
  const glideInFlight = useCallback(() => {
    const nonce = sendNonceRef.current;
    return (
      nonce !== null &&
      positionedRef.current === nonce &&
      settledNonceRef.current !== nonce
    );
  }, [positionedRef]);

  /** Hand the built-in end pin back, but only once the anchored turn is
   *  genuinely spent: the reserved space has collapsed to 0 (so nothing is
   *  left to yank the parked prompt) *and* the glide has settled. Both facts
   *  arrive asynchronously and in either order, so both producers call this. */
  const maybeRestoreEndPin = useCallback((nonce: number | null) => {
    if (nonce === null) return;
    if (spacerCollapsedNonceRef.current !== nonce) return;
    if (settledNonceRef.current !== nonce) return;
    setEndPinRestoredNonce((current) => (current === nonce ? current : nonce));
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
    // LegendList reports the edge signal only on TRANSITIONS, so a gesture
    // landing while the value is already false gets no event of its own.
    // This is the moment the viewport changes hands, so it is also where the
    // pill decision belongs for a reader who is already off the edge —
    // otherwise they browse history with no way back.
    const state = listRef.current?.getState();
    const atEnd = state ? (state.isNearEnd ?? state.isAtEnd) : undefined;
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
  sendNonceRef.current = sendNonce;
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
    setEndPinRestoredNonce(null);
  } else if (sendNonce !== lastSendNonceRef.current) {
    lastSendNonceRef.current = sendNonce;
    setEndPinRestoredNonce(null);
    if (sendAnchor) {
      claimScroll("anchoring-turn");
      setShowJumpToLatest(false);
    } else if (ownsScroll()) {
      // The anchor was cleared — a failed-send rollback — while we still
      // own the viewport. The reserved end space goes away and plain tail
      // following takes over. (A turn settling no longer clears the anchor:
      // it persists until the next send or a thread switch, so the reserved
      // space collapsing can never yank a parked prompt mid-read.)
      claimScroll("following-end");
      setShowJumpToLatest(false);
    } else {
      // Same clear, but the reader has gestured into free-scrolling. Drop
      // the anchor bookkeeping only: a rollback must never yank a reader
      // who deliberately scrolled away back to the tail.
      anchorIndexRef.current = null;
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
      if (driftFrameRef.current !== null) {
        cancelAnimationFrame(driftFrameRef.current);
      }
      // A chain still waiting on a frame must not resume after unmount, and
      // a nonce that never reached the positioning step must not read as
      // positioned to the next mount.
      pendingPositionNonceRef.current = null;
      settleCleanupRef.current?.();
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
      if (atEnd && ownsScroll() && glideInFlight()) {
        // Mid-glide the viewport passes inside LegendList's near-end
        // threshold (half a viewport), so the list truthfully reports "at
        // end" while we are still travelling. Promoting that to
        // `following-end` would hand the next streamed token the follow
        // branch, whose instant `scrollToEnd` cuts the glide to wherever it
        // happened to be. Stay in `anchoring-turn` until the settle
        // handshake lands; a real reader gesture still wins, because it
        // drops the follow claim and `ownsScroll()` goes false.
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
    [glideInFlight, hideJumpToLatest, ownsScroll, scheduleJumpToLatest],
  );

  // The edge signal is `isNearEnd` (LegendList's `onEndReachedThreshold`,
  // default 0.5 — within half a viewport of the end), not the hairline
  // `isAtEnd`. Two consequences, both deliberate: the pill only appears once
  // the reader is meaningfully away from the live edge, and drifting back
  // within half a viewport of the end re-claims follow. `isAtEnd` remains as
  // the fallback for list doubles that do not model the near signal.
  useEffect(() => {
    const state = listRef.current?.getState();
    if (!state) return;
    handleIsAtEndChange(state.isNearEnd ?? state.isAtEnd);
    return state.listen("isNearEnd", handleIsAtEndChange);
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
   *  below it. Position the row exactly once per send nonce — as a smooth
   *  glide, so a follow-up in a long thread visibly travels to its parked
   *  spot instead of teleporting. A `scrollend` listener (with a fallback
   *  timer, since the event is not universal) then re-pins the landed offset
   *  instantly to kill any residual momentum and marks the nonce settled,
   *  which is what re-arms the stream-advance effect. */
  const handleAnchorReady = useCallback(
    (info: SendAnchorReadyInfo) => {
      const clientNonce = anchorClientNonce;
      const nonce = sendNonce;
      const anchorIndex = info.anchorIndex;
      if (!clientNonce || nonce === null || anchorIndex === undefined) return;
      // Always refresh the index: rows landing above the prompt shift it.
      anchorIndexRef.current = anchorIndex;

      if (positionedRef.current === nonce) {
        // Already parked. If *this* mount ran the glide, its settle handshake
        // owns the settled mark and must not be short-circuited here — a
        // re-fired `onReady` mid-glide would otherwise open the advance gate
        // and cut the animation. If it did not, this is a remount under a
        // still-live anchor: there is no glide in flight and nothing else
        // will ever mark the nonce, so record it now. Without this the
        // advance decision stays gated for the rest of the turn.
        if (glideOwnerNonceRef.current !== nonce) {
          settledNonceRef.current = nonce;
          maybeRestoreEndPin(nonce);
        }
        return;
      }
      // A chain for this nonce is already waiting on a frame — the refreshed
      // index above is all it needed.
      if (pendingPositionNonceRef.current === nonce) return;
      pendingPositionNonceRef.current = nonce;

      const position = (attemptsLeft: number) => {
        anchorFrameRef.current = requestAnimationFrame(() => {
          anchorFrameRef.current = null;
          if (pendingPositionNonceRef.current !== nonce) return;
          if (!ownsScroll()) {
            pendingPositionNonceRef.current = null;
            return;
          }
          const list = listRef.current;
          if (!list) {
            // The ref can still be empty on the frame the list mounts;
            // retry on later frames rather than assuming a fixed delay.
            if (attemptsLeft > 0) position(attemptsLeft - 1);
            else pendingPositionNonceRef.current = null;
            return;
          }
          pendingPositionNonceRef.current = null;
          // Recorded HERE, not before the frame: everything above can bail
          // (unmount, a reader gesture, a list ref that never appears), and a
          // nonce marked positioned on a path that never parks it and never
          // settles it gates the stream-advance for the whole turn.
          positionedRef.current = nonce;
          glideOwnerNonceRef.current = nonce;

          const targetIndex = anchorIndexRef.current ?? anchorIndex;
          const reposition = (animated: boolean) =>
            listRef.current?.scrollToIndex({
              index: anchorIndexRef.current ?? targetIndex,
              animated,
              viewPosition: 0,
              viewOffset: SEND_ANCHOR_OFFSET,
            });

          const animated = !prefersReducedMotion();
          if (animated) {
            const viewport = list.getScrollableNode();
            settleCleanupRef.current?.();
            let finished = false;
            /** `viaTimeout` distinguishes the two ways a glide can end, which
             *  need different landings — see the branches below. */
            const settle = (viaTimeout: boolean) => {
              if (finished) return;
              finished = true;
              window.clearTimeout(fallback);
              viewport?.removeEventListener("scrollend", onScrollEnd);
              settleCleanupRef.current = null;
              settledNonceRef.current = nonce;
              maybeRestoreEndPin(nonce);
              // A newer send or a reader gesture owns the viewport now —
              // re-pinning would fight the new owner.
              if (positionedRef.current !== nonce || !ownsScroll()) return;
              const current = listRef.current;
              if (!current) return;
              if (viaTimeout) {
                // No `scrollend` arrived. On WebKitGTK the event is never
                // emitted at all, so this is the *primary* path there and the
                // animation may still be mid-travel: pinning the current
                // offset would park the prompt at an arbitrary point. Re-issue
                // the instant placement instead — it kills the momentum AND
                // lands on the contract's 16px offset.
                void reposition(false);
                return;
              }
              void current.scrollToOffset({
                offset: current.getState().scroll,
                animated: false,
              });
            };
            /** `scrollend` carries no identity, so correlate it with this
             *  glide's intended landing before trusting it. A stray event from
             *  an unrelated smooth scroll (the "Jump to latest" pill) would
             *  otherwise settle the nonce from the middle of the travel. A
             *  rejected event costs nothing: the fallback timer below still
             *  bounds the wait, and it lands on the target authoritatively. */
            const onScrollEnd = () => {
              if (finished) return;
              if (!landedOnAnchorTarget(listRef.current, anchorIndexRef.current ?? targetIndex)) {
                // Not our glide (or not there yet) — keep listening.
                viewport?.addEventListener("scrollend", onScrollEnd, {
                  once: true,
                });
                return;
              }
              settle(false);
            };
            const fallback = window.setTimeout(
              () => settle(true),
              SEND_ANCHOR_SETTLE_FALLBACK_MS,
            );
            viewport?.addEventListener("scrollend", onScrollEnd, { once: true });
            settleCleanupRef.current = () => {
              finished = true;
              window.clearTimeout(fallback);
              viewport?.removeEventListener("scrollend", onScrollEnd);
              settleCleanupRef.current = null;
            };
          } else {
            settledNonceRef.current = nonce;
            maybeRestoreEndPin(nonce);
          }
          void reposition(animated);
        });
      };
      position(ANCHOR_POSITION_ATTEMPTS);
    },
    [anchorClientNonce, maybeRestoreEndPin, ownsScroll, positionedRef, sendNonce],
  );

  // Two nested frames let LegendList lay out and measure new content before
  // the advance decision reads its geometry. One scheduler is shared by the
  // streaming effect below and the spacer's `onSizeChanged`; re-scheduling
  // cancels the in-flight pair so bursts coalesce to one decision per frame.
  const advanceRef = useRef<() => void>(() => {});
  const advanceFramesRef = useRef<{ first: number | null; second: number | null }>(
    { first: null, second: null },
  );
  const cancelScheduledAdvance = useCallback(() => {
    const frames = advanceFramesRef.current;
    if (frames.first !== null) cancelAnimationFrame(frames.first);
    if (frames.second !== null) cancelAnimationFrame(frames.second);
    frames.first = null;
    frames.second = null;
  }, []);
  const scheduleAdvance = useCallback(() => {
    cancelScheduledAdvance();
    const frames = advanceFramesRef.current;
    frames.first = requestAnimationFrame(() => {
      frames.first = null;
      frames.second = requestAnimationFrame(() => {
        frames.second = null;
        advanceRef.current();
      });
    });
  }, [cancelScheduledAdvance]);
  useEffect(() => cancelScheduledAdvance, [cancelScheduledAdvance]);

  /** The reserved space resized under the reader. Following: re-run the
   *  advance decision — this is what keeps the tail revealed for layout
   *  growth that arrives with no data change (a late-loading image), the job
   *  LegendList's disabled end pin can no longer do while an anchor is
   *  mounted. Free-scrolling: restore the captured offset if the browser
   *  drifted it by a hair, so the resize is imperceptible. */
  const handleAnchorSizeChanged = useCallback(
    (size?: number) => {
      // The reply has eaten the whole reserved space. LegendList pins the
      // spacer at 0 and stops calling back from here on, so this is the last
      // chance to bound the window in which the built-in end pin stays off —
      // otherwise, with the anchor no longer expiring, late layout growth
      // would never reveal the tail again for the rest of the thread.
      if (typeof size === "number" && size <= 0) {
        const nonce = sendNonceRef.current;
        if (nonce !== null) {
          spacerCollapsedNonceRef.current = nonce;
          maybeRestoreEndPin(nonce);
        }
      }
      if (ownsScroll()) {
        scheduleAdvance();
        return;
      }
      const list = listRef.current;
      if (!list) return;
      const offset = list.getState().scroll;
      // Coalesce like every other scheduled correction here: a burst of
      // resizes must settle on one restore, against the offset captured by
      // the newest of them.
      if (driftFrameRef.current !== null) {
        cancelAnimationFrame(driftFrameRef.current);
      }
      driftFrameRef.current = requestAnimationFrame(() => {
        driftFrameRef.current = null;
        if (ownsScroll()) return;
        const current = listRef.current;
        if (!current) return;
        // Already clamped at the bottom of a now-shorter scroll range: the
        // "drift" is the browser enforcing that range, and restoring would
        // just be undone on the next frame.
        const node = current.getScrollableNode();
        if (
          node &&
          node.scrollHeight > node.clientHeight &&
          node.scrollTop >= node.scrollHeight - node.clientHeight - 1
        ) {
          return;
        }
        const drifted = current.getState().scroll;
        if (drifted !== offset && Math.abs(drifted - offset) <= 2) {
          void current.scrollToOffset({ offset, animated: false });
        }
      });
    },
    [maybeRestoreEndPin, ownsScroll, scheduleAdvance],
  );

  /** The anchored turn is spent — reserved space consumed and glide settled —
   *  so LegendList's own end pin has been handed back for the rest of this
   *  thread. Nonce-scoped: the next send re-takes it. */
  const endPinReleased =
    endPinRestoredNonce !== null && endPinRestoredNonce === sendNonce;

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
      onSizeChanged: handleAnchorSizeChanged,
    };
  }, [anchorClientNonce, handleAnchorReady, handleAnchorSizeChanged, slots]);

  // The advance decision. Re-assigned every render so the scheduler always
  // runs the latest closure (the accepted mutable-ref pattern used for the
  // slot cache above). Every guard re-checks that we still own the viewport,
  // so a gesture landing mid-flight wins. In `anchoring-turn` we move by
  // exactly the distance needed to reveal the tail — which is zero while the
  // turn still fits, so the prompt stays parked near the top until the turn
  // actually outgrows the viewport.
  advanceRef.current = () => {
    if (!ownsScroll()) return;
    const list = listRef.current;
    if (!list) return;
    const state = list.getState();
    // The positioning glide owns the viewport until it settles; any advance
    // mid-glide — anchored OR following — would cut the animation to wherever
    // it happened to be. Hoisted above the mode branch because the edge
    // listener can legitimately see "at end" mid-glide, and the follow
    // branch's instant `scrollToEnd` is the harsher of the two cuts.
    if (glideInFlight()) return;
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
    // Whenever LegendList's own end pin is enabled it already owns this —
    // driving it twice would be redundant work on every streamed token. We
    // only take over for the window where an anchor has disabled that pin
    // but the reader is back at the edge.
    if (!anchoredEndSpace || endPinReleased) return;
    // Without this the reserved blank space would be a scroll target.
    if (!realContentOverflowsViewport(state)) return;
    void list.scrollToEnd({ animated: false });
  };

  // Advance the viewport as the answer streams in.
  useEffect(() => {
    if (!ownsScroll()) return;
    scheduleAdvance();
  }, [anchoredEndSpace, ownsScroll, scheduleAdvance, slots, threadKey]);

  /** The pill is the deliberate way back to the live edge, so it re-claims
   *  follow rather than cancelling it. Animated for the same reason the send
   *  glide is: a long-distance return should read as travel. */
  const handleJumpToLatest = useCallback(() => {
    claimScroll("following-end");
    setShowJumpToLatest(false);
    void listRef.current?.scrollToEnd({ animated: !prefersReducedMotion() });
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
        // freshly parked prompt off the top of the screen. That hand-off is
        // bounded to the anchored turn: once the reply has consumed the whole
        // reserved space AND the glide has settled, there is nothing left to
        // yank and the pin comes back, so item/footer layout growth the
        // anchor does not model (a late-loading image) still reveals the tail
        // for the rest of the thread's life.
        maintainScrollAtEnd={
          anchoredEndSpace && !endPinReleased ? false : MAINTAIN_SCROLL_AT_END
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

/** Upper bound on the positioning glide before the settle handshake gives up
 *  waiting for `scrollend` — the event is not implemented everywhere this
 *  webview runs, and a glide that covers no distance may never emit one. */
const SEND_ANCHOR_SETTLE_FALLBACK_MS = 750;

/** How close to the intended landing offset a `scrollend` has to be before it
 *  counts as "this glide finished". Wide enough to absorb sub-pixel rounding
 *  in the platform's smooth-scroll implementation, tight enough that an
 *  unrelated animated scroll cannot pass for the anchor's. */
const SEND_ANCHOR_SETTLE_TOLERANCE_PX = 4;

/** Correlate a `scrollend` with the anchor glide: did the viewport actually
 *  land where `scrollToIndex({ viewPosition: 0, viewOffset })` was aiming?
 *  Clamped against the scroller's real maximum, because a target past the end
 *  of the scroll range legitimately lands short. Falls back to trusting the
 *  event when nothing is measurable (an unmeasured row, a list double with no
 *  layout) rather than stalling the handshake on the fallback timer. */
function landedOnAnchorTarget(
  list: LegendListRef | null,
  anchorIndex: number,
): boolean {
  if (!list) return true;
  const state = list.getState();
  const target = getSendAnchorTargetOffset(state, anchorIndex);
  if (target === null) return true;
  const node = list.getScrollableNode();
  const maxScroll =
    node && node.scrollHeight > node.clientHeight
      ? node.scrollHeight - node.clientHeight
      : null;
  const reachable = maxScroll === null ? target : Math.min(target, maxScroll);
  return Math.abs(state.scroll - reachable) <= SEND_ANCHOR_SETTLE_TOLERANCE_PX;
}

/** Smooth scrolling is a comfort feature, not a contract — readers who ask
 *  the platform for reduced motion get the instant placement instead. */
function prefersReducedMotion(): boolean {
  return (
    typeof window.matchMedia === "function" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches
  );
}

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
              <div className="select-text py-0.5 text-xs text-muted-foreground">
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
        <div className="select-text py-0.5 text-xs text-muted-foreground">
          Turn ended: {item.status.subtype}
          {item.status.message ? ` — ${item.status.message}` : ""}
        </div>
      );
    case "runtime_notice":
      // Compact muted-amber inline notice (provider rate-limit rejection,
      // enumerated assistant error) — a left-bordered line in the
      // assistant gutter. Tokens only (design-system no-hardcoded-color).
      return (
        <div className="select-text border-l-2 border-status-working/40 bg-status-working/10 px-3 py-1.5 text-[12px] text-status-working">
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
