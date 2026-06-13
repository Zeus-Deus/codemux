import { shouldShowThinkingIndicator } from "@/lib/agent-chat/thinking";
import type { ChatViewItem } from "@/lib/agent-chat/types";
import type { ApprovalDecision } from "@/tauri/events";

import { MessageList } from "./MessageList";

interface Props {
  messages: ChatViewItem[];
  /** True while a turn is in flight — either the backend has
   *  acknowledged streaming OR the composer's optimistic flag is set.
   *  Drives the transcript-tail "thinking" pulse. */
  streaming: boolean;
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
        onRespondToRequest={onRespondToRequest}
        onAcceptPlan={onAcceptPlan}
        onRejectPlan={onRejectPlan}
      />
    </div>
  );
}
