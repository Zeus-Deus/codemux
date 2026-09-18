import { isRemoteClient } from "@/components/remote/is-remote-client";
import { activateWorkspaceInteraction } from "@/lib/perf/instrumented-activate";
import { toast } from "@/lib/toast";
import { useConversationSearchStore } from "@/stores/conversation-search-store";
import {
  agentChatOpenSearchResult,
  type AgentChatSearchResult,
} from "@/tauri/commands";

/** Open a global conversation hit. Navigation intent is written first so the
 * pane can consume it even when the backend state emit mounts the transcript
 * before the invoke promise resolves. */
export async function openConversationSearchResult(
  result: AgentChatSearchResult,
): Promise<void> {
  const target = useConversationSearchStore.getState().navigateTo({
    threadId: result.thread_id,
    messageId: result.message_id,
    turnId: result.turn_id,
  });
  try {
    const opened = await agentChatOpenSearchResult(result.thread_id);
    if (isRemoteClient()) await activateWorkspaceInteraction(opened.workspace_id);
  } catch (error) {
    useConversationSearchStore.getState().clearHandled(target.nonce);
    toast.error(`Failed to open conversation: ${error}`);
    throw error;
  }
}
