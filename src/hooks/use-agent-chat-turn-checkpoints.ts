import { useEffect, useState } from "react";

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

/** Live, committed per-turn checkpoint timeline for one chat. */
export function useAgentChatTurnCheckpoints(
  threadId: string | null,
): AgentChatTurnCheckpointRecord[] {
  const [checkpoints, setCheckpoints] = useState<
    AgentChatTurnCheckpointRecord[]
  >([]);

  useTauriEvent<AgentChatTurnCheckpointPayload>(
    onAgentChatTurnCheckpoint,
    (payload) => {
      if (payload.thread_id !== threadId) return;
      setCheckpoints((current) => {
        const withoutSame = current.filter(
          (record) =>
            record.turn_index >= payload.oldest_turn_index &&
            record.turn_index !== payload.checkpoint.turn_index,
        );
        return [...withoutSame, payload.checkpoint].sort(
          (a, b) => a.turn_index - b.turn_index,
        );
      });
    },
    [threadId],
  );

  useTauriEvent<AgentChatTurnCheckpointRevertedPayload>(
    onAgentChatTurnCheckpointReverted,
    (payload) => {
      if (payload.thread_id !== threadId) return;
      setCheckpoints(payload.remaining_checkpoints);
    },
    [threadId],
  );

  useTauriEvent<string>(
    onAgentChatTurnCheckpointsInvalidated,
    (invalidatedThreadId) => {
      if (invalidatedThreadId === threadId) setCheckpoints([]);
    },
    [threadId],
  );

  useEffect(() => {
    setCheckpoints([]);
    if (threadId == null) return;
    let cancelled = false;
    void agentChatListTurnCheckpoints(threadId)
      .then((records) => {
        if (!cancelled) setCheckpoints(records);
      })
      .catch((error) => {
        console.warn("[agent-chat] turn checkpoint fetch failed:", error);
      });
    return () => {
      cancelled = true;
    };
  }, [threadId]);

  return checkpoints;
}
