import { useEffect } from "react";

import { useTauriEvent } from "@/hooks/use-tauri-event";
import { selectThread, useAgentChatStore } from "@/stores/agent-chat-store";
import {
  agentChatGetCheckpoint,
  type AgentChatCheckpointRecord,
} from "@/tauri/commands";
import {
  onAgentChatCheckpoint,
  type AgentChatCheckpointPayload,
} from "@/tauri/events";

/**
 * Keep a thread's run-start rollback checkpoint (issue #80) in the
 * agent-chat store and return it.
 *
 * Two feeds, because the snapshot is created in the BACKGROUND after
 * `agent_chat_start_session` returns:
 *
 * 1. The `agent_chat_checkpoint` Tauri event — fires when the
 *    background task lands, covering the live path with no polling.
 * 2. A one-shot fetch on mount / thread change — covers pane remounts
 *    mid-session, where the event already fired before this component
 *    existed.
 *
 * Passing `null` disables both feeds (pane without a session yet).
 */
export function useAgentChatCheckpoint(
  threadId: string | null,
): AgentChatCheckpointRecord | null {
  const checkpoint = useAgentChatStore(
    (s) => (threadId ? selectThread(threadId)(s)?.checkpoint ?? null : null),
  );
  const setCheckpoint = useAgentChatStore((s) => s.setCheckpoint);

  useTauriEvent<AgentChatCheckpointPayload>(
    onAgentChatCheckpoint,
    (payload) => {
      if (threadId == null) return;
      if (payload.thread_id !== threadId) return;
      setCheckpoint(threadId, payload.checkpoint);
    },
    [threadId, setCheckpoint],
  );

  useEffect(() => {
    if (threadId == null) return;
    let cancelled = false;
    agentChatGetCheckpoint(threadId)
      .then((record) => {
        // Don't clobber a checkpoint the event feed already delivered
        // with a slower null response.
        if (cancelled || record == null) return;
        setCheckpoint(threadId, record);
      })
      .catch((err) => {
        // Non-fatal: the restore affordance just stays hidden.
        console.warn("[agent-chat] checkpoint fetch failed:", err);
      });
    return () => {
      cancelled = true;
    };
  }, [threadId, setCheckpoint]);

  return checkpoint;
}
