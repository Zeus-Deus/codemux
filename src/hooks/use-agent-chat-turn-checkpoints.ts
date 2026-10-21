import { useEffect, useRef, useState } from "react";

import { useTauriEvent } from "@/hooks/use-tauri-event";
import {
  agentChatListTurnCheckpoints,
  type AgentChatTurnCheckpointRecord,
} from "@/tauri/commands";
import {
  onAgentChatTurnCheckpoint,
  onAgentChatTurnCheckpointReverted,
  onAgentChatTurnCheckpointsInvalidated,
  type AgentChatTurnCheckpointPayload,
  type AgentChatTurnCheckpointRevertedPayload,
} from "@/tauri/events";

function applyCheckpoint(
  current: AgentChatTurnCheckpointRecord[],
  payload: AgentChatTurnCheckpointPayload,
): AgentChatTurnCheckpointRecord[] {
  const withoutSame = current.filter(
    (record) =>
      record.turn_index >= payload.oldest_turn_index &&
      record.turn_index !== payload.checkpoint.turn_index,
  );
  return [...withoutSame, payload.checkpoint].sort(
    (a, b) => a.turn_index - b.turn_index,
  );
}

/** Live, committed per-turn checkpoint timeline for one chat. */
export function useAgentChatTurnCheckpoints(
  threadId: string | null,
): AgentChatTurnCheckpointRecord[] {
  const [checkpoints, setCheckpoints] = useState<
    AgentChatTurnCheckpointRecord[]
  >([]);
  // Additions are incremental: replay them over the fetched history. A
  // revert or invalidation replaces the entire timeline, so null disables
  // hydration after either event (and once the fetch has settled).
  const pendingAdditions = useRef<AgentChatTurnCheckpointPayload[] | null>(null);

  useTauriEvent<AgentChatTurnCheckpointPayload>(
    onAgentChatTurnCheckpoint,
    (payload) => {
      if (payload.thread_id !== threadId) return;
      pendingAdditions.current?.push(payload);
      setCheckpoints((current) => applyCheckpoint(current, payload));
    },
    [threadId],
  );

  useTauriEvent<AgentChatTurnCheckpointRevertedPayload>(
    onAgentChatTurnCheckpointReverted,
    (payload) => {
      if (payload.thread_id !== threadId) return;
      pendingAdditions.current = null;
      setCheckpoints(payload.remaining_checkpoints);
    },
    [threadId],
  );

  useTauriEvent<string>(
    onAgentChatTurnCheckpointsInvalidated,
    (invalidatedThreadId) => {
      if (invalidatedThreadId !== threadId) return;
      pendingAdditions.current = null;
      setCheckpoints([]);
    },
    [threadId],
  );

  useEffect(() => {
    // Reset only when there is something to clear. A fresh `[]` literal is a
    // new reference, so an unconditional write would force an extra render of
    // the whole pane on every mount — wasted work, and enough to clobber
    // one-shot render-time signals the pane consumes in its own mount effects
    // (e.g. the promoted-draft composer focus handoff).
    setCheckpoints((current) => (current.length === 0 ? current : []));
    pendingAdditions.current = null;
    if (threadId == null) return;
    pendingAdditions.current = [];
    let cancelled = false;
    void agentChatListTurnCheckpoints(threadId)
      .then((records) => {
        if (cancelled) return;
        const additions = pendingAdditions.current;
        pendingAdditions.current = null;
        if (additions !== null) {
          setCheckpoints(additions.reduce(applyCheckpoint, records));
        }
      })
      .catch((error) => {
        if (cancelled) return;
        pendingAdditions.current = null;
        console.warn("[agent-chat] turn checkpoint fetch failed:", error);
      });
    return () => {
      cancelled = true;
      pendingAdditions.current = null;
    };
  }, [threadId]);

  return checkpoints;
}
