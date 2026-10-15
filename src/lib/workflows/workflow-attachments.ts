import type { Attachment } from "@/stores/agent-chat-store";

/** Why a workflow can't take the chat's attachments yet, or null. Workflow
 *  agents can't fetch issues, PRs or conversations themselves, so every
 *  attached text context must be resolved before launch, never dropped. */
export function workflowAttachmentProblem(attachments: readonly Attachment[]): { title: string; description: string } | null {
  if (attachments.some((attachment) => attachment.kind === "image")) {
    return { title: "Workflows can't use images yet", description: "Remove the image, or send it as a normal message." };
  }
  const pending = attachments.find((attachment) => attachment.metadata.isLoading);
  if (pending) return { title: "Attachments are still loading", description: `Wait for ${pending.metadata.label} to finish, then send again.` };
  const missing = attachments.find((attachment) => !attachment.resolvedContent?.trim());
  if (missing) return { title: "An attachment couldn't be loaded", description: `Remove ${missing.metadata.label} or attach it again.` };
  return null;
}
