/**
 * Pure transform behind sidebar workspace drag-reorder. Extracted from the
 * React drop handler so it can be unit-tested without a DOM.
 *
 * The dragged row stays mounted during the drag (only dimmed), so `dropIndex`
 * — derived from the rows' `data-ws-index` — is measured in the group's
 * "with-dragged-row" index space. When the dragged workspace originally sat
 * above the drop point, removing it shifts every later slot down by one, so a
 * downward drop must be adjusted by -1. (The sibling project-reorder branch
 * already does this; the workspace branch used to omit it and dropped
 * downward drags one slot too low.)
 */
export interface ReorderGroup {
  projectPath: string;
  workspaces: ReadonlyArray<{ workspace_id: string }>;
}

export function computeWorkspaceReorder(
  groups: ReadonlyArray<ReorderGroup>,
  draggedId: string,
  sourceProjectPath: string | null,
  dropIndex: number,
): string[] {
  const newIds: string[] = [];

  for (const group of groups) {
    const sourceIdx = group.workspaces.findIndex(
      (w) => w.workspace_id === draggedId,
    );
    const groupIds = group.workspaces
      .map((w) => w.workspace_id)
      .filter((id) => id !== draggedId);

    if (group.projectPath === sourceProjectPath) {
      const adjusted =
        sourceIdx >= 0 && dropIndex > sourceIdx ? dropIndex - 1 : dropIndex;
      groupIds.splice(Math.min(adjusted, groupIds.length), 0, draggedId);
    }

    newIds.push(...groupIds);
  }

  // Safety net: never lose the dragged id (e.g. source group not found).
  if (!newIds.includes(draggedId)) {
    newIds.push(draggedId);
  }
  return newIds;
}
