import type {
  ChatViewItem,
  PermissionRequestItem,
} from "@/lib/agent-chat/types";

import type { TranscriptSlot } from "./transcript-slots";

/**
 * Resolve a gated row's approval request in O(1) from the request id the
 * reducer stamped onto the tool call / workflow run.
 */
export function lookupApproval(
  item: ChatViewItem,
  requestsById: Map<string, PermissionRequestItem>,
): PermissionRequestItem | null {
  if (item.kind === "tool_call" && item.approval_request_id != null) {
    return requestsById.get(item.approval_request_id) ?? null;
  }
  if (item.kind === "workflow_run" && item.approvalRequestId != null) {
    return requestsById.get(item.approvalRequestId) ?? null;
  }
  return null;
}

/**
 * Keys of the transcript rows the virtualizer must keep mounted regardless of
 * the visible window.
 *
 * Windowing unmounts off-screen rows, which would discard transient local
 * interaction state: a queued turn's cancel / send-now controls and any
 * approval form the user has already started answering. Those rows opt out of
 * recycling by key. The set is naturally tiny even in a very long thread —
 * only rows with *live* controls qualify, so this never degrades back into
 * mounting the whole transcript.
 *
 * Qualifying rows:
 * - queued `user_message` rows (cancel / send now),
 * - `permission_request` rows still `pending` or `responding`,
 * - `tool_call` / `workflow_run` rows whose linked approval is `pending` or
 *   `responding`.
 */
export function deriveAlwaysRenderKeys(
  slots: readonly TranscriptSlot[],
  requestsById: Map<string, PermissionRequestItem>,
): string[] {
  const keys: string[] = [];
  for (const slot of slots) {
    if (slot.body.kind !== "item") continue;
    const item = slot.body.item;
    if (item.kind === "user_message" && item.queued) {
      keys.push(slot.key);
      continue;
    }
    if (
      item.kind === "permission_request" &&
      (item.resolution.state === "pending" ||
        item.resolution.state === "responding")
    ) {
      keys.push(slot.key);
      continue;
    }
    const approval = lookupApproval(item, requestsById);
    if (
      approval &&
      (approval.resolution.state === "pending" ||
        approval.resolution.state === "responding")
    ) {
      keys.push(slot.key);
    }
  }
  return keys;
}
