import { useState } from "react";

import { useAgentChatCheckpoint } from "@/hooks/use-agent-chat-checkpoint";
import { toast } from "@/lib/toast";
import { selectThread, useAgentChatStore } from "@/stores/agent-chat-store";
import {
  agentChatRestoreCheckpoint,
  type AgentChatCheckpointRecord,
} from "@/tauri/commands";

export interface AgentChatCheckpointRestore {
  /** The run-start checkpoint, or null when none exists / opt-in is off. */
  checkpoint: AgentChatCheckpointRecord | null;
  /** True while a turn is streaming — restoring then would yank files
   *  out from under the agent's tools, so the affordance disables. */
  turnActive: boolean;
  confirmOpen: boolean;
  setConfirmOpen: (open: boolean) => void;
  restoring: boolean;
  handleRestoreConfirmed: () => Promise<void>;
}

/**
 * Run-start rollback checkpoint state (issue #80), extracted so the
 * GUI-chrome title-bar chat tab can offer "Restore checkpoint" in its
 * history dropdown — the affordance the per-pane header hosts in split
 * layouts.
 */
export function useAgentChatCheckpointRestore(
  threadId: string | null,
): AgentChatCheckpointRestore {
  const checkpoint = useAgentChatCheckpoint(threadId);
  const turnActive = useAgentChatStore((s) => {
    const slice = threadId ? selectThread(threadId)(s) : null;
    return slice ? slice.activeTurnId != null || slice.streaming : false;
  });
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [restoring, setRestoring] = useState(false);

  const handleRestoreConfirmed = async () => {
    if (!threadId) return;
    setRestoring(true);
    try {
      await agentChatRestoreCheckpoint(threadId);
      toast.success(
        "Workspace restored to the snapshot taken when this run started.",
      );
    } catch (error) {
      toast.error(`Restore failed: ${error}`);
    } finally {
      setRestoring(false);
      setConfirmOpen(false);
    }
  };

  return {
    checkpoint,
    turnActive,
    confirmOpen,
    setConfirmOpen,
    restoring,
    handleRestoreConfirmed,
  };
}
