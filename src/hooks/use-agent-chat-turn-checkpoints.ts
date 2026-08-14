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

/** Live, committed per-turn checkpoint timeline for one chat. */
export function useAgentChatTurnCheckpoints(
  threadId: string | null,
): AgentChatTurnCheckpointRecord[] {
  const [checkpoints, setCheckpoints] = useState<
    AgentChatTurnCheckpointRecord[]
  >([]);
  /** A live event landed for this thread, so the in-flight mount fetch —
   * taken before it — is stale and must not overwrite the newer state. */
  const sawLiveUpdate = useRef(false);

  useTauriEvent<AgentChatTurnCheckpointPayload>(
    onAgentChatTurnCheckpoint,
    (payload) => {
      if (payload.thread_id !== threadId) return;
      sawLiveUpdate.current = true;
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
      sawLiveUpdate.current = true;
      setCheckpoints(payload.remaining_checkpoints);
    },
    [threadId],
  );

  useTauriEvent<string>(
    onAgentChatTurnCheckpointsInvalidated,
    (invalidatedThreadId) => {
      if (invalidatedThreadId !== threadId) return;
      sawLiveUpdate.current = true;
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
    sawLiveUpdate.current = false;
    if (threadId == null) return;
    let cancelled = false;
    void agentChatListTurnCheckpoints(threadId)
      .then((records) => {
        // A checkpoint/revert/invalidation event that arrived while this
        // request was in flight already holds the newer timeline.
        if (!cancelled && !sawLiveUpdate.current) setCheckpoints(records);
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
