import { useEffect, useLayoutEffect, useRef } from "react";

import { shouldShowThinkingIndicator } from "@/lib/agent-chat/thinking";
import type { ChatViewItem } from "@/lib/agent-chat/types";
import type { ApprovalDecision } from "@/tauri/events";

import { MessageList } from "./MessageList";
import { ThinkingIndicator } from "./ThinkingIndicator";

const PIN_THRESHOLD_PX = 80;

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

export function ChatTranscript({
  messages,
  streaming,
  onRespondToRequest,
  onAcceptPlan,
  onRejectPlan,
}: Props) {
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const pinnedToBottomRef = useRef(true);

  // Track whether the user is pinned to the bottom so auto-scroll only
  // fires when the tail is already visible.
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const onScroll = () => {
      const distance = el.scrollHeight - el.scrollTop - el.clientHeight;
      pinnedToBottomRef.current = distance <= PIN_THRESHOLD_PX;
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => el.removeEventListener("scroll", onScroll);
  }, []);

  const showThinking = shouldShowThinkingIndicator(messages, streaming);

  // After each message update — or when the thinking indicator toggles —
  // if we were pinned, stick to the bottom so the pulse stays visible.
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    if (pinnedToBottomRef.current) {
      el.scrollTop = el.scrollHeight;
    }
  }, [messages, showThinking]);

  return (
    <div
      ref={scrollRef}
      className="flex-1 min-h-0 w-full overflow-y-auto px-4 py-4"
    >
      <div className="mx-auto w-full max-w-2xl">
        <MessageList
          messages={messages}
          onRespondToRequest={onRespondToRequest}
          onAcceptPlan={onAcceptPlan}
          onRejectPlan={onRejectPlan}
        />
        {showThinking && (
          <div
            role="status"
            aria-label="Agent is thinking"
            className="mt-3"
          >
            <ThinkingIndicator />
          </div>
        )}
        <div className="h-4" />
      </div>
    </div>
  );
}
