import type { ChatViewItem, SubagentView } from "./types";

/**
 * Carry the item ids of a transcript the user is looking at over to a
 * fresh replay of the same rows, matched from the tail backwards.
 *
 * The reducer mints ids from a module counter, so replaying the same
 * rows twice yields the same items under DIFFERENT ids. Ids are the
 * virtualised list's row keys: a full-history replay that lands under a
 * content-ready tail preview would otherwise hand the list an entirely
 * new key set, remounting every visible row and giving
 * `maintainVisibleContentPosition` nothing to anchor on. Matching the
 * suffix keeps every row that was already on screen keyed as it was, so
 * the prepended history slides in above without the viewport moving.
 *
 * Matching stops at the first pair that is not the same item by kind
 * and correlation identity; everything above it is genuinely new.
 * Only the id is adopted — content, `seq` and timestamps come from the
 * new replay, which is the authority.
 */
export function adoptItemIds(
  previous: readonly ChatViewItem[],
  next: readonly ChatViewItem[],
): ChatViewItem[] {
  const out = next.slice();
  let p = previous.length - 1;
  let n = out.length - 1;
  while (p >= 0 && n >= 0) {
    const prev = previous[p];
    const item = out[n];
    if (!sameItem(prev, item)) break;
    out[n] = withAdoptedId(prev, item);
    p -= 1;
    n -= 1;
  }
  return out;
}

function withAdoptedId(prev: ChatViewItem, item: ChatViewItem): ChatViewItem {
  if (item.kind === "subagent_run" && prev.kind === "subagent_run") {
    return {
      ...item,
      id: prev.id,
      subagents: item.subagents.map((view, index) =>
        adoptSubagentItems(prev.subagents[index], view),
      ),
    };
  }
  if (item.id === prev.id) return item;
  return { ...item, id: prev.id };
}

function adoptSubagentItems(
  prev: SubagentView | undefined,
  view: SubagentView,
): SubagentView {
  if (!prev || prev.id !== view.id || prev.items.length === 0) return view;
  return { ...view, items: adoptItemIds(prev.items, view.items) };
}

function sameItem(a: ChatViewItem, b: ChatViewItem): boolean {
  if (a.kind !== b.kind) return false;
  switch (a.kind) {
    case "user_message":
      return (
        b.kind === "user_message" &&
        a.text === b.text &&
        a.clientNonce === b.clientNonce
      );
    case "assistant_message":
      return (
        b.kind === "assistant_message" &&
        a.turn_id === b.turn_id &&
        a.text === b.text
      );
    case "reasoning":
      return b.kind === "reasoning" && a.turn_id === b.turn_id && a.text === b.text;
    case "tool_call":
      return b.kind === "tool_call" && a.tool_use_id === b.tool_use_id;
    case "permission_request":
      return b.kind === "permission_request" && a.request_id === b.request_id;
    case "turn_ended":
      return b.kind === "turn_ended" && a.turn_id === b.turn_id;
    case "subagent_run":
      return (
        b.kind === "subagent_run" &&
        a.subagents.length === b.subagents.length &&
        a.subagents.every((view, index) => view.id === b.subagents[index].id)
      );
    case "workflow_run":
      return b.kind === "workflow_run" && a.workflowId === b.workflowId;
    case "runtime_notice":
      return b.kind === "runtime_notice" && a.message === b.message;
    default:
      return false;
  }
}
