import { describe, expect, it } from "vitest";
import type { PaneNodeSnapshot } from "@/tauri/types";
import {
  containsPane,
  findPaneInDirection,
  leafPaneIds,
  type PaneRect,
} from "./pane-navigation";

const rect = (left: number, top: number, right: number, bottom: number): PaneRect => ({
  left,
  top,
  right,
  bottom,
});

// ┌──────┬──────┐
// │  tl  │  tr  │
// ├──────┼──────┤
// │  bl  │  br  │
// └──────┴──────┘   (1px gaps between panes)
const grid = {
  tl: rect(0, 0, 100, 100),
  tr: rect(101, 0, 200, 100),
  bl: rect(0, 101, 100, 200),
  br: rect(101, 101, 200, 200),
};
const others = (active: keyof typeof grid) =>
  Object.entries(grid)
    .filter(([id]) => id !== active)
    .map(([id, r]) => ({ id, rect: r }));

describe("findPaneInDirection", () => {
  it("moves to the in-line neighbour, not the diagonal one", () => {
    expect(findPaneInDirection(grid.tl, others("tl"), "right")).toBe("tr");
    expect(findPaneInDirection(grid.tl, others("tl"), "down")).toBe("bl");
    expect(findPaneInDirection(grid.br, others("br"), "left")).toBe("bl");
    expect(findPaneInDirection(grid.br, others("br"), "up")).toBe("tr");
  });

  it("returns null at the edge of the surface", () => {
    expect(findPaneInDirection(grid.tl, others("tl"), "left")).toBeNull();
    expect(findPaneInDirection(grid.tl, others("tl"), "up")).toBeNull();
  });

  it("prefers the nearer pane over one further along the same line", () => {
    const panes = [
      { id: "near", rect: rect(101, 0, 200, 100) },
      { id: "far", rect: rect(201, 0, 300, 100) },
    ];
    expect(findPaneInDirection(rect(0, 0, 100, 100), panes, "right")).toBe("near");
  });

  it("picks the neighbour sharing more of the active pane's height", () => {
    // A tall left pane beside a right column split 30/70.
    const panes = [
      { id: "small", rect: rect(101, 0, 200, 60) },
      { id: "large", rect: rect(101, 61, 200, 200) },
    ];
    expect(findPaneInDirection(rect(0, 0, 100, 200), panes, "right")).toBe("large");
  });
});

describe("pane tree helpers", () => {
  const tree = {
    kind: "split",
    pane_id: "root",
    children: [
      { kind: "terminal", pane_id: "a" },
      {
        kind: "split",
        pane_id: "inner",
        children: [
          { kind: "terminal", pane_id: "b" },
          { kind: "browser", pane_id: "c" },
        ],
      },
    ],
  } as unknown as PaneNodeSnapshot;

  it("lists leaves in tree order", () => {
    expect(leafPaneIds(tree)).toEqual(["a", "b", "c"]);
  });

  it("finds a pane anywhere beneath a node", () => {
    expect(containsPane(tree, "c")).toBe(true);
    expect(containsPane(tree, "inner")).toBe(true);
    expect(containsPane(tree, "zzz")).toBe(false);
  });
});
