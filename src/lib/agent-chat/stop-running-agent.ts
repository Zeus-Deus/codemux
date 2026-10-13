import { agentChatInterruptTurn } from "@/tauri/commands";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import { toast } from "@/lib/toast";
import type {
  AgentChatProviderKind,
  PaneNodeSnapshot,
  PaneStatus,
  WorkspaceSnapshot,
} from "@/tauri/types";

export interface RunningAgent {
  paneId: string;
  threadId: string;
  provider: AgentChatProviderKind;
}

function collectChatPanes(node: PaneNodeSnapshot, out: RunningAgent[]): void {
  if (node.kind === "split") {
    for (const child of node.children) collectChatPanes(child, out);
    return;
  }
  if (node.kind === "agent_chat" && node.thread_id && node.provider) {
    out.push({ paneId: node.pane_id, threadId: node.thread_id, provider: node.provider });
  }
}

/**
 * The agent chat a workspace-level "Stop agent" should interrupt: the focused
 * pane when it is the one running, otherwise the first running chat in the
 * workspace. Null when nothing is working or waiting on a permission.
 */
export function findRunningAgent(
  workspace: WorkspaceSnapshot,
  paneStatuses: Record<string, PaneStatus> | undefined,
  focusedPaneId: string | undefined,
): RunningAgent | null {
  if (!paneStatuses) return null;
  const panes: RunningAgent[] = [];
  for (const surface of workspace.surfaces) collectChatPanes(surface.root, panes);
  const running = panes.filter((pane) => {
    const status = paneStatuses[pane.paneId];
    return status === "working" || status === "permission";
  });
  return running.find((pane) => pane.paneId === focusedPaneId) ?? running[0] ?? null;
}

/**
 * Interrupt a running turn from outside its pane. Same contract as the
 * composer's Stop (`AgentChatPane` `handleStop`): `false` means no live
 * session was reached and no settlement event is coming, so the thread is
 * settled locally; a rejection proves nothing stopped, so nothing is settled.
 */
export async function stopRunningAgent(agent: RunningAgent): Promise<void> {
  try {
    const reachedLiveSession = await agentChatInterruptTurn(agent.provider, agent.threadId, null);
    if (!reachedLiveSession) {
      useAgentChatStore.getState().applyEvent(agent.threadId, {
        type: "session_state_changed",
        thread_id: agent.threadId,
        status: { status: "ready" },
      });
    }
  } catch (err) {
    toast.error(`Failed to stop turn: ${err}`);
  }
}
