import { describe, it, expect } from "vitest";

import { computeWorkspaceReorder, type ReorderGroup } from "./workspace-reorder";

const ws = (id: string) => ({ workspace_id: id });
const group = (projectPath: string, ids: string[]): ReorderGroup => ({
  projectPath,
  workspaces: ids.map(ws),
});

describe("computeWorkspaceReorder", () => {
  it("drops a downward drag at the intended slot (no off-by-one)", () => {
    // [A, B, C], drag A, drop between B and C → dropIndex 2 (data-ws-index of
    // B is 1, cursor below midpoint → +1). Must land [B, A, C], not [B, C, A].
    const groups = [group("P", ["A", "B", "C"])];
    expect(computeWorkspaceReorder(groups, "A", "P", 2)).toEqual(["B", "A", "C"]);
  });

  it("handles upward drags (no adjustment needed)", () => {
    // [A, B, C], drag C up to between A and B → dropIndex 1.
    const groups = [group("P", ["A", "B", "C"])];
    expect(computeWorkspaceReorder(groups, "C", "P", 1)).toEqual(["A", "C", "B"]);
  });

  it("moves to the very top and very bottom", () => {
    const groups = [group("P", ["A", "B", "C"])];
    expect(computeWorkspaceReorder(groups, "C", "P", 0)).toEqual(["C", "A", "B"]);
    expect(computeWorkspaceReorder(groups, "A", "P", 3)).toEqual(["B", "C", "A"]);
  });

  it("a no-op drop preserves order", () => {
    const groups = [group("P", ["A", "B", "C"])];
    // Drag B, drop back at its own slot (dropIndex 1 == sourceIdx).
    expect(computeWorkspaceReorder(groups, "B", "P", 1)).toEqual(["A", "B", "C"]);
  });

  it("only reorders within the source group, preserving other groups", () => {
    const groups = [
      group("P1", ["A", "B"]),
      group("P2", ["C", "D"]),
    ];
    // Reorder within P2: drag C below D (dropIndex 2) → [D, C]; P1 untouched.
    expect(computeWorkspaceReorder(groups, "C", "P2", 2)).toEqual([
      "A",
      "B",
      "D",
      "C",
    ]);
  });
});
