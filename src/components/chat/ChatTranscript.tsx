import { shouldShowThinkingIndicator } from "@/lib/agent-chat/thinking";
import type { ChatViewItem } from "@/lib/agent-chat/types";
import type { ApprovalDecision } from "@/tauri/events";

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
  onRespondToRequest: (requestId: string, decision: ApprovalDecision) => void;
  onAcceptPlan: (requestId: string) => void | Promise<void>;
  onRejectPlan: (requestId: string) => void | Promise<void>;
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
  onRespondToRequest,
  onAcceptPlan,
  onRejectPlan,
}: Props) {
  const showThinking = shouldShowThinkingIndicator(messages, streaming);

  return (
    <div className="flex-1 min-h-0 w-full">
      <MessageList
        messages={messages}
        showThinking={showThinking}
        sessionStartedAt={sessionStartedAt}
        onRespondToRequest={onRespondToRequest}
        onAcceptPlan={onAcceptPlan}
        onRejectPlan={onRejectPlan}
      />
    </div>
  );
}
