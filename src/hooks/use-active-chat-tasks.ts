import { useMemo } from "react";

import { useAgentChatStore } from "@/stores/agent-chat-store";
import type { TasksSnapshot } from "@/tauri/events";
import type { PaneNodeSnapshot, WorkspaceSnapshot } from "@/tauri/types";

function findPane(
  node: PaneNodeSnapshot,
  paneId: string,
): PaneNodeSnapshot | null {
  if (node.pane_id === paneId) return node;
  if (node.kind !== "split") return null;
  for (const child of node.children) {
    const match = findPane(child, paneId);
    if (match) return match;
  }
  return null;
}

/** Resolve task state for the focused leaf in the workspace's active surface. */
export function useActiveChatTasks(workspace: WorkspaceSnapshot | null): {
  threadId: string | null;
  tasks: TasksSnapshot | null;
  /** When the snapshot was last applied (ms), for the panel caption. */
  updatedAt?: number | null;
} {
  const threadId = useMemo(() => {
    if (!workspace) return null;
    const surface = workspace.surfaces.find(
      (candidate) => candidate.surface_id === workspace.active_surface_id,
    );
    if (!surface) return null;
    const pane = findPane(surface.root, surface.active_pane_id);
    return pane?.kind === "agent_chat" ? pane.thread_id : null;
  }, [workspace]);

  const tasks = useAgentChatStore((state) =>
    threadId ? (state.threads[threadId]?.tasks ?? null) : null,
  );
  const updatedAt = useAgentChatStore((state) =>
    threadId ? (state.threads[threadId]?.tasksUpdatedAt ?? null) : null,
  );
  return { threadId, tasks, updatedAt };
}
