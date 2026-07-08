import { shouldShowThinkingIndicator } from "@/lib/agent-chat/thinking";
import type { ChatViewItem } from "@/lib/agent-chat/types";
import type { ApprovalDecision } from "@/tauri/events";
import type { AgentChatProviderKind } from "@/tauri/types";

import { MessageList } from "./MessageList";

interface Props {
  messages: ChatViewItem[];
  /** True while a turn is in flight — either the backend has
   *  acknowledged streaming OR the composer's optimistic flag is set.
   *  Drives the transcript-tail "working" marker. */
  streaming: boolean;
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
  /** Enter a subagent's read-only drill-in (design "Enter subagent"). */
  onEnterSubagent?: (subagentId: string) => void;
  /** Forwarded to MessageList for the GUI-mode background-browser chip
   *  lookup (docs/features/browser.md). */
  workspaceId?: string | null;
}

/**
 * Transcript shell. The scroll container, stick-to-bottom pinning and
 * row windowing all live inside the virtualized `MessageList` (the
 * virtualizer must own its scroller to map scroll offsets onto the
 * rendered window) — this layer just sizes it and derives the
 * thinking-pulse flag.
 */
export function ChatTranscript({
  messages,
  streaming,
  sessionStartedAt,
  provider,
  onRespondToRequest,
  onAcceptPlan,
  onRejectPlan,
  onCancelQueued,
  onEnterSubagent,
  workspaceId,
}: Props) {
  const showThinking = shouldShowThinkingIndicator(messages, streaming);

  return (
    <div className="flex-1 min-h-0 w-full">
      <MessageList
        messages={messages}
        showThinking={showThinking}
        streaming={streaming}
        sessionStartedAt={sessionStartedAt}
        provider={provider}
        onRespondToRequest={onRespondToRequest}
        onAcceptPlan={onAcceptPlan}
        onRejectPlan={onRejectPlan}
        onCancelQueued={onCancelQueued}
        onEnterSubagent={onEnterSubagent}
        workspaceId={workspaceId}
      />
    </div>
  );
}
