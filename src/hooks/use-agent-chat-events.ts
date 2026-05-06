import { useCallback } from "react";

import { useTauriEvent } from "@/hooks/use-tauri-event";
import {
  onAgentChatEvent,
  type AgentChatEventPayload,
  type EventCallback,
} from "@/tauri/events";

/**
 * Subscribe to provider runtime events, filtered by thread id.
 *
 * The Rust side fans every provider's canonical event stream into a
 * single `agent_chat_event` Tauri channel. Each pane is interested in
 * exactly one thread, so the hook takes a thread id and invokes the
 * handler only when the payload's `thread_id` matches.
 *
 * Passing a `null` thread id disables the subscription — useful for
 * panes that have not yet started a session.
 */
export function useAgentChatEvents(
  threadId: string | null,
  handler: (payload: AgentChatEventPayload) => void,
) {
  const filtered = useCallback<EventCallback<AgentChatEventPayload>>(
    (payload) => {
      if (threadId == null) return;
      if (payload.thread_id !== threadId) return;
      handler(payload);
    },
    [threadId, handler],
  );

  useTauriEvent<AgentChatEventPayload>(onAgentChatEvent, filtered, [
    threadId,
    handler,
  ]);
}
