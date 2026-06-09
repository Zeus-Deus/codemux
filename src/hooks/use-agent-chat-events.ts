import { useEffect, useRef } from "react";

import {
  attachAgentChatOutput,
  Channel,
  detachAgentChatOutput,
} from "@/tauri/commands";
import type { AgentChatEventPayload } from "@/tauri/events";

/**
 * Subscribe to a thread's live provider runtime events.
 *
 * The Rust side routes each thread's canonical event stream to the
 * per-thread `Channel`s registered via `attach_agent_chat_output` —
 * the same streaming mechanism PTY output uses — instead of fanning
 * every token of every thread through the global event bus. The hook
 * attaches a channel for its thread on mount (and whenever the thread
 * id changes) and detaches it on unmount, so a pane only ever receives
 * its own thread's events.
 *
 * Passing a `null` thread id disables the subscription — useful for
 * panes that have not yet started a session.
 *
 * The handler is kept in a ref so a new handler identity never forces
 * a detach/re-attach round-trip.
 */
export function useAgentChatEvents(
  threadId: string | null,
  handler: (payload: AgentChatEventPayload) => void,
) {
  const handlerRef = useRef(handler);
  handlerRef.current = handler;

  useEffect(() => {
    if (threadId == null) return;

    let cancelled = false;
    let subscriptionId: number | null = null;

    const channel = new Channel<AgentChatEventPayload>((payload) => {
      if (cancelled) return;
      // The backend routes per thread already; this guard is defense
      // in depth against a stale channel delivering after a re-bind.
      if (payload.thread_id !== threadId) return;
      handlerRef.current(payload);
    });

    void attachAgentChatOutput(threadId, channel)
      .then((id) => {
        subscriptionId = id;
        if (cancelled) {
          // Unmounted while the attach round-trip was in flight —
          // detach immediately so the backend doesn't hold a channel
          // for a dead pane.
          void detachAgentChatOutput(threadId, id).catch(() => {});
        }
      })
      .catch((error) => {
        console.error("[agent-chat] failed to attach event channel:", error);
      });

    return () => {
      cancelled = true;
      if (subscriptionId != null) {
        void detachAgentChatOutput(threadId, subscriptionId).catch(() => {});
      }
    };
  }, [threadId]);
}
