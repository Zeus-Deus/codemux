import { COPY_FAILED_MESSAGE, copyToClipboard } from "@/lib/clipboard";
import { toast } from "@/lib/toast";
import { useChatDraftStore } from "@/stores/chat-draft-store";

/** A terminal launch does not send the task. Keep it beside the workspace
 * and offer an immediate copy action even when the new workspace takes focus. */
export function saveWorkspacePrompt(
  workspaceId: string,
  prompt: string,
  commandDelivered: boolean,
  message: string,
  tone: "success" | "warning" = "success",
  openTerminal?: () => Promise<unknown>,
): void {
  if (!prompt) return;
  useChatDraftStore.getState().saveWorkspacePrompt(workspaceId, prompt, commandDelivered);
  toast[tone](message, {
    description: commandDelivered
      ? "The exact prompt is saved in Drafts. Paste it only when the agent's input is visible."
      : "The exact prompt is saved in Drafts.",
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
    ...(openTerminal ? {
      cancel: {
        label: "Open terminal",
        onClick: (event: { preventDefault: () => void }) => {
          event.preventDefault();
          void openTerminal().catch((error) => toast.failure("Couldn't open terminal tab", error));
        },
      },
    } : {}),
  });
}
