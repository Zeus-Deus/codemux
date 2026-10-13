import { isRemoteClient } from "@/components/remote/is-remote-client";
import { activateWorkspaceInteraction } from "@/lib/perf/instrumented-activate";
import { toast } from "@/lib/toast";
import { agentChatInterruptTurn, agentChatOpenSearchResult } from "@/tauri/commands";
import type { AgentChatProviderKind } from "@/tauri/types";

/**
 * The two things a user can do to a delegated task from the parent chat.
 * Both reuse existing commands: a child is an ordinary chat, so Open is the
 * search-result opener (focuses its tab, or reopens it from history) and
 * Stop is that chat's own interrupt, which the backend also reads as
 * "stop this delegated task quietly".
 */

/** Focus the child chat's tab. */
export async function openDelegatedChat(childThreadId: string): Promise<void> {
  try {
    const opened = await agentChatOpenSearchResult(childThreadId);
    if (isRemoteClient()) await activateWorkspaceInteraction(opened.workspace_id);
  } catch (error) {
    toast.error("Couldn't open the delegated chat", {
      description: error instanceof Error ? error.message : String(error),
    });
  }
}

/**
 * Stop one delegated task. Errors are expected and ignored: a child that is
 * idle, or still starting, has no turn to interrupt ("no active turn"), yet
 * the backend has already marked the task Stopped before it tried.
 */
export async function stopDelegatedTask(
  provider: AgentChatProviderKind,
  childThreadId: string,
): Promise<void> {
  try {
    await agentChatInterruptTurn(provider, childThreadId);
  } catch (error) {
    console.warn("[delegation] stop reported an error", error);
  }
}
