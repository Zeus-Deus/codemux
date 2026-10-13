import type { ChatViewItem, RemoteTaskItem } from "@/lib/agent-chat/types";
import type { LocalTask } from "@/lib/delegation";

/** Native row IDs and renderer sequence numbers are different namespaces.
 * Receipts live after their last visible persisted event, never in the reducer.
 * An anchor outside the loaded history stays before that visible slice. */
export function projectRemoteTasks(messages: ChatViewItem[], tasks: readonly LocalTask[]): ChatViewItem[] {
  if (!tasks.length) return messages;
  const floor = messages.length ? Math.min(...messages.map(item => item.seq)) - 1 : -1;
  const receipts: RemoteTaskItem[] = tasks.map(task => {
    let anchorSeq = floor;
    let anchorId = -1;
    for (const item of messages) {
      const eventId = "source_event_id" in item ? item.source_event_id : undefined;
      if (eventId != null && task.parent_event_id != null && eventId <= task.parent_event_id && eventId > anchorId) {
        anchorId = eventId;
        anchorSeq = item.seq;
      }
    }
    return { kind: "remote_task", id: `remote:${task.id}`, seq: anchorSeq + 0.5, task };
  });
  return [...messages, ...receipts].sort((a, b) => a.seq - b.seq || (
    a.kind === "remote_task" && b.kind === "remote_task"
      ? a.task.created_at.localeCompare(b.task.created_at) || a.id.localeCompare(b.id)
      : a.id.localeCompare(b.id)
  ));
}
