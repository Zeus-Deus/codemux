import type { TranscriptSlot } from "./transcript-slots";

export interface ConversationSearchJumpTarget {
  messageId: number | null;
  turnId: string | null;
}

/** Resolve a durable search hit into the virtualized row that presents it.
 * Settled turns may fold the exact assistant prose out of view, so turn id is
 * the intentional fallback anchor. A title hit points at the session start. */
export function resolveConversationSearchTargetIndex(
  slots: readonly TranscriptSlot[],
  target: ConversationSearchJumpTarget,
): number {
  const { messageId, turnId } = target;
  if (messageId == null && turnId == null) return slots.length > 0 ? 0 : -1;

  const exact = slots.findIndex(
    (slot) =>
      slot.body.kind === "item" &&
      (slot.body.item.kind === "user_message" ||
        slot.body.item.kind === "assistant_message") &&
      slot.body.item.source_event_id === messageId,
  );
  if (exact >= 0 || !turnId) return exact;

  return slots.findIndex((slot) => {
    if (slot.body.kind === "turn_fold") return slot.body.turnId === turnId;
    if (slot.body.kind !== "item") return false;
    const item = slot.body.item;
    return "turn_id" in item && item.turn_id === turnId;
  });
}
