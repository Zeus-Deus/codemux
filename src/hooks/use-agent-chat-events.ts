import { useEffect, useRef } from "react";

import {
  clearAgentChatAttach,
  registerAgentChatAttach,
} from "@/lib/agent-chat/attach-registry";
import {
  attachAgentChatOutput,
  Channel,
  detachAgentChatOutput,
} from "@/tauri/commands";
import type { AgentChatEventPayload } from "@/tauri/events";

/**
 * Subscribe to provider runtime events for one thread over a
 * per-thread Tauri `Channel`.
 *
 * The Rust side routes each thread's live events — including the
 * high-frequency streaming `content_delta` tokens — to the `Channel`
 * registered via `attach_agent_chat_output` (mirroring how PTY output
 * streams through `attach_pty_output`), instead of broadcasting every
 * thread's traffic on the global event bus. A pane therefore only
 * ever receives its own thread's events; the `thread_id` check below
 * is defense-in-depth, not the routing mechanism.
 *
 * The channel carries live events only. Transcript history for a
 * late-attaching / resumed pane comes from the DB hydrate
 * (`agentChatListMessages`) that `AgentChatPane` runs on mount —
 * same split as before this hook moved off the event bus.
 *
 * Passing a `null` thread id disables the subscription — useful for
 * panes that have not yet started a session.
 *
 * The handler is kept on a ref so a new handler identity does NOT
 * re-attach the backend channel; only a thread id change does.
 */
export function useAgentChatEvents(
  threadId: string | null,
  handler: (payload: AgentChatEventPayload) => void,
) {
  const handlerRef = useRef(handler);
  useEffect(() => {
    handlerRef.current = handler;
  }, [handler]);

  useEffect(() => {
    if (threadId == null) return;
    let cancelled = false;
    const channel = new Channel<AgentChatEventPayload>((payload) => {
      // `cancelled` guards the gap between unmount and the async
      // detach landing on the backend.
      if (cancelled) return;
      if (payload.thread_id !== threadId) return;
      handlerRef.current(payload);
    });
    const attached = attachAgentChatOutput(threadId, channel);
    // Published so the cursor hydrate can read the persisted tail only
    // AFTER this channel is live — a row persisted while the attach is in
    // flight reaches no channel, and a tail read that preceded the attach
    // would miss it for good (see lib/agent-chat/attach-registry.ts).
    registerAgentChatAttach(threadId, attached);
    attached.catch((error) => {
      console.error("[agent-chat] attach_agent_chat_output failed:", error);
    });
    return () => {
      cancelled = true;
      clearAgentChatAttach(threadId, attached);
      // Serialize detach behind the attach so out-of-order delivery
      // can't detach before the attach registers. The generation
      // token makes a late detach a no-op if a newer pane already
      // re-attached this thread.
      void attached
        .then((generation) => detachAgentChatOutput(threadId, generation))
        .catch(() => {
          // Attach failed — nothing to detach.
        });
    };
  }, [threadId]);
}
