import { COPY_FAILED_MESSAGE, copyToClipboard } from "@/lib/clipboard";
import { toast } from "@/lib/toast";
import { useChatDraftStore } from "@/stores/chat-draft-store";

/** A terminal launch does not send the task. Keep it beside the workspace
 * and offer an immediate copy action even when the new workspace takes focus. */
export function saveWorkspacePrompt(
  workspaceId: string,
  prompt: string,
  agentOpened: boolean,
  message: string,
): void {
  if (!prompt) return;
  useChatDraftStore.getState().saveWorkspacePrompt(workspaceId, prompt, agentOpened);
  toast.success(message, {
    description: "The exact prompt is saved in Drafts.",
    duration: 10_000,
    action: {
      label: "Copy prompt",
      onClick: (event) => {
        event.preventDefault();
        void copyToClipboard(prompt).then((copied) => {
          if (!copied) toast.error(COPY_FAILED_MESSAGE);
        });
      },
    },
  });
}
