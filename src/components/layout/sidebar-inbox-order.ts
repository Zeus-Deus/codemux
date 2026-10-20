/** Ranks are presentation hints, never pin membership or workspace order. */
export function orderInboxWorkspaces<T extends { workspace_id: string }>(
  workspaces: readonly T[],
  order: readonly string[],
): T[] {
  const ranks = new Map(order.map((id, index) => [id, index]));
  return workspaces
    .map((workspace, index) => ({ workspace, index }))
    .sort((a, b) => {
      const ar = ranks.get(a.workspace.workspace_id);
      const br = ranks.get(b.workspace.workspace_id);
      if (ar === undefined || br === undefined) {
        if (ar !== undefined) return 1;
        if (br !== undefined) return -1;
        return b.index - a.index;
      }
      return ar - br;
    })
    .map(({ workspace }) => workspace);
}

export type InboxSection = "pinned" | "active";
export type InboxDestination = InboxSection | "wrapping";
export interface InboxDrop {
  id: string;
  pinned: boolean;
  order: string[];
  destination: InboxDestination;
}

/** Insert around visible neighbours without dropping or rearranging hidden ids. */
export function planInboxDrop(input: {
  id: string;
  from: InboxSection;
  to: InboxSection;
  index: number;
  visible: readonly string[];
  destinationIds: readonly string[];
  allIds: readonly string[];
  order: readonly string[];
  wraps: boolean;
}): InboxDrop | null {
  const { id, from, to, allIds, order } = input;
  if (!allIds.includes(id)) return null;
  const destination = to === "active" && input.wraps ? "wrapping" : to;
  const visible = input.visible.filter((key) => key !== id);
  const index = Math.max(0, Math.min(input.index, visible.length));
  if (from === to && input.visible.indexOf(id) === index) return null;
  const before = visible[index];
  const after = visible[index - 1];
  const insert = (ids: readonly string[]) => {
    const result = ids.filter((key) => key !== id);
    const slot =
      before !== undefined
        ? result.indexOf(before)
        : after !== undefined
          ? result.indexOf(after) + 1
          : 0;
    result.splice(Math.max(0, slot), 0, id);
    return result;
  };
  if (from === to) {
    const next = insert(input.destinationIds);
    if (next.every((key, position) => key === input.destinationIds[position]))
      return null;
  }
  const completeOrder = orderInboxWorkspaces(
    allIds.map((workspace_id) => ({ workspace_id })),
    order,
  ).map((workspace) => workspace.workspace_id);
  return {
    id,
    pinned: to === "pinned",
    order: destination === "wrapping" ? completeOrder : insert(completeOrder),
    destination,
  };
}
