import type { ChatViewItem, ChatWorkflowItem } from "@/lib/agent-chat/types";
import type { ChatWorkflowLink } from "@/stores/workflow-ui-store";

/** The newest persisted provider event in the transcript, used as the anchor
 *  for a run launched now. */
export function latestTranscriptEventId(messages: readonly ChatViewItem[]): number | null {
  let latest: number | null = null;
  for (const item of messages) {
    const eventId = "source_event_id" in item ? item.source_event_id : undefined;
    if (eventId != null && (latest === null || eventId > latest)) latest = eventId;
  }
  return latest;
}

/** Place each `/workflow` card where it was launched: before the first event
 *  persisted after launch, or at the end when nothing newer has arrived.
 *  Rows without a persisted event id keep their place relative to the card. */
export function projectChatWorkflows(messages: ChatViewItem[], links: readonly ChatWorkflowLink[], threadId: string | null): ChatViewItem[] {
  if (!threadId || !links.length) return messages;
  const ordered = [...messages].sort((a, b) => a.seq - b.seq);
  const cards: ChatWorkflowItem[] = links.map((link) => {
    const next = ordered.findIndex((item) => {
      const eventId = "source_event_id" in item ? item.source_event_id : undefined;
      return eventId != null && (link.anchorEventId == null || eventId > link.anchorEventId);
    });
    const seq = next === -1
      ? (ordered.length ? ordered[ordered.length - 1].seq : 0) + 0.75
      : next === 0 ? ordered[0].seq - 0.25 : (ordered[next - 1].seq + ordered[next].seq) / 2;
    return { kind: "chat_workflow", id: `workflow:${link.runId}`, seq, runId: link.runId, workspaceId: link.workspaceId, threadId };
  });
  const created = new Map(links.map((link) => [`workflow:${link.runId}`, link.createdAt]));
  return [...messages, ...cards].sort((a, b) => a.seq - b.seq || (
    a.kind === "chat_workflow" && b.kind === "chat_workflow"
      ? (created.get(a.id) ?? 0) - (created.get(b.id) ?? 0)
      : a.kind === "chat_workflow" ? 1 : b.kind === "chat_workflow" ? -1 : a.id.localeCompare(b.id)
  ));
}
