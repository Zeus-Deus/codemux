import { memo, useMemo } from "react";

import { shouldShowThinkingIndicator } from "@/lib/agent-chat/thinking";
import type { ChatViewItem } from "@/lib/agent-chat/types";
import type { ApprovalDecision } from "@/tauri/events";
import type { AgentChatProviderKind } from "@/tauri/types";

import { MessageList } from "./MessageList";
import type { SendAnchorRequest } from "./send-scroll-state";

interface Props {
  messages: ChatViewItem[];
  /** True while a turn is in flight — either the backend has
   *  acknowledged streaming OR the composer's optimistic flag is set.
   *  Drives the transcript-tail "working" marker. */
  streaming: boolean;
  /** Stall-watchdog state — drives the amber "no activity" tail notice. */
  stalled?: { silentForSecs: number } | null;
  /** True when the last run never cleanly settled — drives the
   *  "Run interrupted" tail divider. */
  interrupted?: boolean;
  /** The new-turn scroll contract's navigation intent (`{ clientNonce,
   *  nonce }`). Forwarded verbatim to MessageList, which owns the scroll
   *  intent; this layer only has to not break the memo boundary, so the
   *  object identity must stay stable between sends. */
  sendAnchor?: SendAnchorRequest | null;
  /** Pane-owned "which send nonce is already positioned" record — forwarded
   *  so a MessageList remount under a still-live anchor re-reserves the
   *  response space without re-running the positioning scroll. */
  positionedNonceRef?: { current: number | null };
  /** Thread identity — forwarded so the list resets to `following-end` when
   *  the pane switches threads. */
  threadKey?: string | null;
  /** Virtual-list jump request from the docked subagent activity bar. */
  subagentJumpRequest?: { cardId: string; nonce: number } | null;
  /** Optional session-created timestamp for the top session-start marker
   *  (design D2). Forwarded to MessageList; Stage 3 wires the real value. */
  sessionStartedAt?: number;
  /** The session's chat provider. Forwarded to MessageList to brand the
   *  assistant-turn avatar with the provider's official mark. */
  provider?: AgentChatProviderKind | null;
  onRespondToRequest: (requestId: string, decision: ApprovalDecision) => void;
  onAcceptPlan: (requestId: string) => void | Promise<void>;
  onRejectPlan: (requestId: string) => void | Promise<void>;
  /** Follow-up queueing: cancel a queued turn (X on the greyed bubble).
   *  `text` is passed back so the caller can restore it to the composer. */
  onCancelQueued?: (queuedId: string, text: string) => void;
  /** Follow-up queueing: send a queued turn now (steer) — soft-interrupts
   *  the active turn and dispatches this message immediately. */
  onSendQueuedNow?: (queuedId: string) => void;
  /** Enter a subagent's read-only drill-in (design "Enter subagent"). */
  onEnterSubagent?: (subagentId: string) => void;
  /** Forwarded to MessageList for the WorkflowRunCard "Open panel"
   *  affordance and the GUI-mode background-browser chip lookup. */
  workspaceId?: string | null;
  /** Active worktree root for resolving relative source references. */
  cwd?: string | null;
}

/**
 * Transcript shell. The scroll container and stick-to-bottom tracking
 * (owned by the shadcn scroller engine inside `MessageList`) both live
 * inside `MessageList` — this layer just sizes it and derives the
 * thinking-pulse flag.
 *
 * Memoized: this is the boundary that keeps composer keystrokes out of the
 * timeline. `AgentChatPane` re-renders on every character (it owns the
 * draft), but none of the props below move while the user types, so the
 * whole transcript subtree skips.
 */
export const ChatTranscript = memo(function ChatTranscript({
  messages,
  streaming,
  stalled,
  interrupted,
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
  cwd,
}: Props) {
  const showThinking = useMemo(
    () => shouldShowThinkingIndicator(messages, streaming),
    [messages, streaming],
  );

  return (
    <div className="flex-1 min-h-0 w-full">
      <MessageList
        messages={messages}
        showThinking={showThinking}
        streaming={streaming}
        stalled={stalled}
        interrupted={interrupted}
        sendAnchor={sendAnchor}
        positionedNonceRef={positionedNonceRef}
        threadKey={threadKey}
        subagentJumpRequest={subagentJumpRequest}
        sessionStartedAt={sessionStartedAt}
        provider={provider}
        onRespondToRequest={onRespondToRequest}
        onAcceptPlan={onAcceptPlan}
        onRejectPlan={onRejectPlan}
        onCancelQueued={onCancelQueued}
        onSendQueuedNow={onSendQueuedNow}
        onEnterSubagent={onEnterSubagent}
        workspaceId={workspaceId}
        cwd={cwd}
      />
    </div>
  );
});
