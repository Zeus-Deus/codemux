import type { ChatViewItem, PermissionRequestItem } from "@/lib/agent-chat/types";
import {
  assistantReferenceCwds,
  assistantReferencePaths,
} from "@/lib/agent-chat/reference-cwd";

import { deriveAlwaysRenderKeys } from "./always-render-keys";
import {
  buildTranscriptSlots,
  reuseTranscriptSlots,
  type TranscriptSlot,
} from "./transcript-slots";

export interface TranscriptHistory {
  readonly ordered: ChatViewItem[];
  readonly referenceCwdByMessageId: ReadonlyMap<string, string>;
  readonly referencePathsByMessageId: ReadonlyMap<string, readonly string[]>;
  readonly subagentNames: Map<string, string>;
  readonly requestsById: Map<string, PermissionRequestItem>;
}

export interface TranscriptPresentation {
  readonly slots: TranscriptSlot[];
  readonly alwaysRenderKeys: string[];
}

// Store transcript arrays and their items are copy-on-write (including live
// deltas, approvals and hydration). Identity is the invalidation token, just
// as for MessageList's useMemo. No thread-id map, previous-snapshot chain, DOM,
// measurements or interaction state is retained here. Treat results as read-only.
const histories = new WeakMap<readonly ChatViewItem[], TranscriptHistory>();
const collapsedPresentations = new WeakMap<
  TranscriptHistory,
  { idle?: TranscriptPresentation; streaming?: TranscriptPresentation }
>();

/** Same immutable snapshot, including a remount: O(1), no history traversal.
 * A new snapshot still sorts and derives everything. `previous` only seeds
 * structural sharing so unchanged file-link props survive streaming updates;
 * it is never retained by the cache. */
export function getTranscriptHistory(
  messages: readonly ChatViewItem[],
  previous?: TranscriptHistory,
): TranscriptHistory {
  const cached = histories.get(messages);
  if (cached) return cached;

  const ordered = messages.slice();
  ordered.sort((a, b) => a.seq - b.seq || a.id.localeCompare(b.id));
  const subagentNames = new Map<string, string>();
  const requestsById = new Map<string, PermissionRequestItem>();
  for (const item of ordered) {
    if (item.kind === "subagent_run") {
      for (const sub of item.subagents) {
        subagentNames.set(sub.id, sub.name ?? sub.agentType ?? "subagent");
      }
    } else if (item.kind === "permission_request") {
      requestsById.set(item.request_id, item);
    }
  }
  const history: TranscriptHistory = {
    ordered,
    subagentNames,
    requestsById,
    referenceCwdByMessageId: assistantReferenceCwds(
      ordered,
      previous?.referenceCwdByMessageId,
    ),
    referencePathsByMessageId: assistantReferencePaths(
      ordered,
      previous?.referencePathsByMessageId,
    ),
  };
  histories.set(messages, history);
  return history;
}

/** Cache only the default (collapsed) presentation, at most two variants per
 * live snapshot, keyed separately by streaming. Empty disclosure sets from
 * different mounts share this result. Expanded folds remain mount-owned and
 * use the original builder + slot reuse; we do not retain disclosure histories
 * or enumerate their combinations. Provider/workspace/cwd are render props,
 * not inputs to these pure builders, and intentionally are not cache keys. */
export function getTranscriptPresentation(
  history: TranscriptHistory,
  streaming: boolean,
  expandedTurnIds: ReadonlySet<string>,
  previousSlots?: TranscriptSlot[],
): TranscriptPresentation {
  const collapsed = expandedTurnIds.size === 0;
  const mode = streaming ? "streaming" : "idle";
  const cached = collapsedPresentations.get(history);
  if (collapsed && cached?.[mode]) return cached[mode];

  const built = buildTranscriptSlots(history.ordered, streaming, expandedTurnIds);
  const slots = previousSlots ? reuseTranscriptSlots(previousSlots, built) : built;
  const presentation = {
    slots,
    alwaysRenderKeys: deriveAlwaysRenderKeys(slots, history.requestsById),
  };
  if (collapsed) {
    const variants = cached ?? {};
    variants[mode] = presentation;
    collapsedPresentations.set(history, variants);
  }
  return presentation;
}
