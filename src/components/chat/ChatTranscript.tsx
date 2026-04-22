import { useEffect, useLayoutEffect, useRef } from "react";

import type { ChatViewItem } from "@/lib/agent-chat/types";
import type { ApprovalDecision } from "@/tauri/events";

import { MessageList } from "./MessageList";

const PIN_THRESHOLD_PX = 80;

interface Props {
  messages: ChatViewItem[];
  onRespondToRequest: (requestId: string, decision: ApprovalDecision) => void;
}

export function ChatTranscript({ messages, onRespondToRequest }: Props) {
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

  // After each message update, if we were pinned, stick to the bottom.
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    if (pinnedToBottomRef.current) {
      el.scrollTop = el.scrollHeight;
    }
  }, [messages]);

  return (
    <div
      ref={scrollRef}
      className="flex-1 min-h-0 w-full overflow-y-auto px-4 py-4"
    >
      <div className="mx-auto w-full max-w-2xl">
        <MessageList
          messages={messages}
          onRespondToRequest={onRespondToRequest}
        />
        <div className="h-4" />
      </div>
    </div>
  );
}
