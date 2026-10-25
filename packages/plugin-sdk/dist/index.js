/** Adapted from T3 Tools' MIT author contract; see THIRD_PARTY_NOTICES.md.
 * Public author API. Capability-gated additions require host discovery. Plugins run as the host's OS user; process isolation is not an OS sandbox. */
export const apiVersion = 1;
/** Discovery reflects supported AND granted capabilities, not just manifest requests. */
export function hasCapability(context, capability) {
    return context.capabilities.some(value => value === capability);
}
/** A native receipt never proves the agent's task completed, even in state succeeded. */
export function operationDisposition(receipt) {
    switch (receipt.state) {
        case "accepted":
        case "running": return "pending";
        case "succeeded": return "accepted";
        case "failed": return "failed";
        default: return "uncertain";
    }
}
/** Historical text stays useful when a card's plugin is disabled, missing, or incompatible. */
export function conversationCardSummary(card) {
    return card.summary;
}
