import { describe, expect, it } from "vitest";
import type { PluginTreeNode } from "./contract";
import { buildPluginTreeLayout, PLUGIN_TREE_OTHER_SECTION_KEY, pluginTreeRowKeys, togglePluginTreeFold } from "./native-tree";

interface Row {
  id: string;
  parent: string | null;
  thread: string;
}

const rows: Row[] = [
  { id: "ws-a", parent: null, thread: "t-a" },
  { id: "ws-b", parent: null, thread: "t-b" },
  { id: "ws-b-child", parent: "ws-b", thread: "t-b2" },
  { id: "ws-c", parent: null, thread: "t-c" },
];

function layout(nodes: PluginTreeNode[], options: { folds?: string[]; routeKey?: string | null; live?: string[] } = {}) {
  return buildPluginTreeLayout<string, Row>({
    nodes,
    items: rows,
    keyOf: (row) => row.id,
    parentKeyOf: (row) => row.parent,
    rowForThread: (threadId) => rows.find((row) => row.thread === threadId)?.id ?? null,
    liveKeys: new Set(options.live ?? rows.map((row) => row.id)),
    project: (projectId) => (projectId === "p1" ? { key: "/p1", project: "Project One" } : null),
    showsEmptyProject: () => false,
    folds: new Set(options.folds ?? []),
    routeKey: options.routeKey ?? null,
  });
}

describe("buildPluginTreeLayout", () => {
  it("places chats under headings in plugin order and keeps unplaced live rows reachable", () => {
    const { sections } = layout([
      { id: "g", kind: "group", title: "Shipping" },
      { id: "n2", kind: "thread", threadId: "t-b", parentId: "g" },
      { id: "n1", kind: "thread", threadId: "t-a", parentId: "g" },
    ]);
    expect(sections[0]).toMatchObject({ key: "tree:g", heading: { kind: "group", title: "Shipping" }, count: 3 });
    // The child workspace follows its placed parent.
    expect(sections[0]!.rows.map((row) => [row.key, row.children.map((child) => child.key)])).toEqual([
      ["ws-b", ["ws-b-child"]],
      ["ws-a", []],
    ]);
    const other = sections.find((section) => section.key === PLUGIN_TREE_OTHER_SECTION_KEY);
    expect(other?.items.map((row) => row.id)).toEqual(["ws-c"]);
  });

  it("skips references the host does not list and moves their children up", () => {
    const { sections } = layout([
      { id: "gone", kind: "thread", threadId: "t-missing" },
      { id: "n", kind: "thread", threadId: "t-a", parentId: "gone" },
      { id: "nobody", kind: "project", projectId: "unknown" },
    ]);
    expect(sections[0]!.rows.map((row) => row.key)).toEqual(["ws-a"]);
  });

  it("leaves parked rows on the host's shelf when the tree does not place them", () => {
    const { sections } = layout([{ id: "n", kind: "thread", threadId: "t-a" }], { live: ["ws-a", "ws-b"] });
    const other = sections.find((section) => section.key === PLUGIN_TREE_OTHER_SECTION_KEY);
    expect(other?.items.map((row) => row.id)).toEqual(["ws-b"]);
  });

  it("folds a heading down to the open row and honours explicit unfolds", () => {
    const nodes: PluginTreeNode[] = [
      { id: "p", kind: "project", projectId: "p1", collapsedByDefault: true },
      { id: "n1", kind: "thread", threadId: "t-a", parentId: "p" },
      { id: "n2", kind: "thread", threadId: "t-c", parentId: "p" },
    ];
    const folded = layout(nodes, { routeKey: "ws-c" }).sections[0]!;
    expect(folded).toMatchObject({ key: "project:/p1", folded: true, count: 2 });
    expect(folded.rows.map((row) => row.key)).toEqual(["ws-c"]);
    const open = layout(nodes, { folds: ["+project:/p1"] }).sections[0]!;
    expect(open.folded).toBe(false);
    expect(pluginTreeRowKeys([open])).toEqual(["ws-a", "ws-c"]);
  });

  it("survives parent cycles in the snapshot", () => {
    const { sections } = layout([
      { id: "x", kind: "group", title: "X", parentId: "y" },
      { id: "y", kind: "group", title: "Y", parentId: "x" },
      { id: "n", kind: "thread", threadId: "t-a", parentId: "x" },
    ]);
    expect(sections.flatMap((section) => section.items.map((row) => row.id))).toContain("ws-a");
  });
});

describe("togglePluginTreeFold", () => {
  it("keeps at most one entry per section and none at the default", () => {
    expect(togglePluginTreeFold([], "s", false)).toEqual(["s"]);
    expect(togglePluginTreeFold(["s"], "s", false)).toEqual([]);
    expect(togglePluginTreeFold([], "s", true)).toEqual(["+s"]);
    expect(togglePluginTreeFold(["+s"], "s", true)).toEqual([]);
  });
});
