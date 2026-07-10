import { describe, it, expect } from "vitest";
import { shareStructural } from "./structural-share";

/**
 * Deep clone via JSON — mirrors what Tauri IPC does to the snapshot: every
 * object/array becomes a fresh reference, primitives/null survive, and there
 * are no Dates/Maps/undefined values. Perfect for simulating a backend tick
 * that ships a structurally-identical payload with all-new refs.
 */
function ipcClone<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

/** Deep freeze so any accidental mutation of the inputs throws. */
function deepFreeze<T>(value: T): T {
  if (value && typeof value === "object") {
    for (const v of Object.values(value)) deepFreeze(v);
    Object.freeze(value);
  }
  return value;
}

describe("shareStructural — identity fast paths", () => {
  it("returns prev when Object.is(prev, next) (same ref / dev-mock case)", () => {
    const obj = { a: { b: 1 } };
    expect(shareStructural(obj, obj)).toBe(obj);
    const arr = [1, 2, 3];
    expect(shareStructural(arr, arr)).toBe(arr);
  });

  it("returns prev for a fully deep-equal next with all fresh refs", () => {
    const prev = deepFreeze({
      workspaces: [
        { id: "w1", surfaces: [{ id: "s1", root: { kind: "term", pane: "p1" } }] },
        { id: "w2", surfaces: [] },
      ],
      pane_statuses: { p1: "idle", p2: "working" },
    });
    const next = deepFreeze(ipcClone(prev));
    // Sanity: next really is all-fresh refs.
    expect(next).not.toBe(prev);
    expect(next.workspaces).not.toBe(prev.workspaces);
    expect(next.workspaces[0]).not.toBe(prev.workspaces[0]);

    const result = shareStructural(prev, next);
    expect(result).toBe(prev);
  });
});

describe("shareStructural — primitives & type mismatch", () => {
  it("returns next for differing primitives", () => {
    expect(shareStructural(1, 2)).toBe(2);
    expect(shareStructural("a", "b")).toBe("b");
    expect(shareStructural(true, false)).toBe(false);
  });

  it("returns next on primitive type change", () => {
    expect(shareStructural(1 as unknown, "1" as unknown)).toBe("1");
    expect(shareStructural(0 as unknown, false as unknown)).toBe(false);
  });

  it("returns next for null-vs-object in either direction", () => {
    const obj = { a: 1 };
    expect(shareStructural(null as unknown, obj)).toBe(obj);
    expect(shareStructural(obj as unknown, null)).toBe(null);
  });

  it("returns next on array-vs-object type mismatch", () => {
    const arr = [1, 2];
    const obj = { 0: 1, 1: 2 };
    expect(shareStructural(obj as unknown, arr)).toBe(arr);
    expect(shareStructural(arr as unknown, obj)).toBe(obj);
  });

  it("compares non-plain objects (class instances) by identity, taking next", () => {
    class Box {
      constructor(public v: number) {}
    }
    const prev = new Box(1);
    const next = new Box(1);
    // Different refs, non-plain → next.
    expect(shareStructural(prev, next)).toBe(next);
  });
});

describe("shareStructural — nested subtree sharing", () => {
  it("shares all unchanged workspaces/surfaces when one nested field changes", () => {
    const prev = deepFreeze({
      active: "w1",
      workspaces: [
        {
          id: "w1",
          surfaces: [
            { id: "s1", root: { kind: "term", pane: "p1" } },
            { id: "s2", root: { kind: "browser", pane: "p2" } },
          ],
          pane_statuses: { p1: "idle" },
        },
        {
          id: "w2",
          surfaces: [{ id: "s3", root: { kind: "term", pane: "p3" } }],
          pane_statuses: { p3: "idle" },
        },
      ],
    });

    // Fresh-ref clone with exactly one leaf changed: w1's pane p1 status.
    const nextRaw = ipcClone(prev);
    nextRaw.workspaces[0].pane_statuses.p1 = "working";
    const next = deepFreeze(nextRaw);

    const result = shareStructural(prev, next);

    // Top ref is new (something changed).
    expect(result).not.toBe(prev);
    // The changed workspace is a new ref...
    expect(result.workspaces[0]).not.toBe(prev.workspaces[0]);
    expect(result.workspaces[0].pane_statuses).not.toBe(
      prev.workspaces[0].pane_statuses,
    );
    // ...but its UNCHANGED surfaces keep prev identity.
    expect(result.workspaces[0].surfaces).toBe(prev.workspaces[0].surfaces);
    // ...and the untouched workspace keeps prev identity entirely.
    expect(result.workspaces[1]).toBe(prev.workspaces[1]);
    // Data correctness: result is deep-equal to next.
    expect(result).toEqual(next);
    expect(result.workspaces[0].pane_statuses.p1).toBe("working");
  });

  it("shares unchanged branches of a recursive pane split tree", () => {
    const prev = deepFreeze({
      root: {
        kind: "split",
        pane: "root",
        children: [
          { kind: "term", pane: "a", title: "A" },
          {
            kind: "split",
            pane: "inner",
            children: [
              { kind: "term", pane: "b", title: "B" },
              { kind: "term", pane: "c", title: "C" },
            ],
          },
        ],
      },
    });

    const nextRaw = ipcClone(prev);
    // Change only the deeply-nested pane "c".
    (nextRaw.root.children[1] as { children: { title: string }[] }).children[1].title =
      "C2";
    const next = deepFreeze(nextRaw);

    const result = shareStructural(prev, next) as typeof prev;

    expect(result).not.toBe(prev);
    // Sibling branch "a" untouched → shared.
    expect(result.root.children[0]).toBe(prev.root.children[0]);
    // Inner split changed (its child c changed) → new ref...
    const prevInner = prev.root.children[1] as { children: unknown[] };
    const resInner = result.root.children[1] as { children: unknown[] };
    expect(resInner).not.toBe(prevInner);
    // ...but the unchanged sibling "b" inside it is shared.
    expect(resInner.children[0]).toBe(prevInner.children[0]);
    expect(result).toEqual(next);
  });
});

describe("shareStructural — arrays", () => {
  it("returns a new array (not prev) when length grows, sharing unchanged elements", () => {
    const prev = deepFreeze({
      workspaces: [{ id: "w1", v: 1 }, { id: "w2", v: 2 }],
    });
    const nextRaw = ipcClone(prev);
    (nextRaw.workspaces as { id: string; v: number }[]).push({ id: "w3", v: 3 });
    const next = deepFreeze(nextRaw);

    const result = shareStructural(prev, next);
    expect(result).not.toBe(prev);
    expect(result.workspaces).not.toBe(prev.workspaces);
    // Existing elements keep identity even though the array ref changed.
    expect(result.workspaces[0]).toBe(prev.workspaces[0]);
    expect(result.workspaces[1]).toBe(prev.workspaces[1]);
    expect(result).toEqual(next);
  });

  it("returns a new array when length shrinks (workspace removed)", () => {
    const prev = deepFreeze({
      workspaces: [{ id: "w1", v: 1 }, { id: "w2", v: 2 }],
    });
    const nextRaw = ipcClone(prev);
    nextRaw.workspaces = [nextRaw.workspaces[0]];
    const next = deepFreeze(nextRaw);

    const result = shareStructural(prev, next);
    expect(result.workspaces).not.toBe(prev.workspaces);
    expect(result.workspaces[0]).toBe(prev.workspaces[0]);
    expect(result.workspaces).toHaveLength(1);
    expect(result).toEqual(next);
  });

  it("reconciles element-wise on reorder (index-based, no keying)", () => {
    const prev = deepFreeze({
      list: [{ id: "a", v: 1 }, { id: "b", v: 2 }],
    });
    const nextRaw = ipcClone(prev);
    nextRaw.list = [nextRaw.list[1], nextRaw.list[0]];
    const next = deepFreeze(nextRaw);

    const result = shareStructural(prev, next);
    // Reordered: each index's content differs from prev's element at that
    // index, so nothing at those indices is shared, and the array ref is new.
    expect(result.list).not.toBe(prev.list);
    expect(result.list[0]).not.toBe(prev.list[0]);
    expect(result.list[1]).not.toBe(prev.list[1]);
    expect(result).toEqual(next);
  });
});

describe("shareStructural — Record<string, ...> key changes", () => {
  it("returns new object when a key is added", () => {
    const prev = deepFreeze({ pane_statuses: { p1: "idle" } });
    const next = deepFreeze({ pane_statuses: { p1: "idle", p2: "working" } });
    const result = shareStructural(prev, next);
    expect(result).not.toBe(prev);
    expect(result.pane_statuses).not.toBe(prev.pane_statuses);
    expect(result).toEqual(next);
  });

  it("returns new object when a key is removed", () => {
    const prev = deepFreeze({ pane_statuses: { p1: "idle", p2: "working" } });
    const next = deepFreeze({ pane_statuses: { p1: "idle" } });
    const result = shareStructural(prev, next);
    expect(result.pane_statuses).not.toBe(prev.pane_statuses);
    expect(result).toEqual(next);
  });

  it("returns new object when a value changes, sharing sibling object values", () => {
    const prev = deepFreeze({
      statuses: { p1: { s: "idle" }, p2: { s: "idle" } },
    });
    const nextRaw = ipcClone(prev);
    nextRaw.statuses.p1.s = "working";
    const next = deepFreeze(nextRaw);
    const result = shareStructural(prev, next);
    expect(result.statuses).not.toBe(prev.statuses);
    expect(result.statuses.p1).not.toBe(prev.statuses.p1);
    // Unchanged sibling value keeps identity.
    expect(result.statuses.p2).toBe(prev.statuses.p2);
    expect(result).toEqual(next);
  });

  it("returns new object when a same-size key set differs (swap)", () => {
    const prev = deepFreeze({ m: { a: 1, b: 2 } as Record<string, number> });
    const next = deepFreeze({ m: { a: 1, c: 2 } as Record<string, number> });
    const result = shareStructural(prev, next);
    expect(result.m).not.toBe(prev.m);
    expect(result).toEqual(next);
  });
});

describe("shareStructural — optional-field presence", () => {
  it("does not share when an optional key is present in one and absent in the other", () => {
    // next gained an optional field (e.g. project_uid) → new object at that level.
    const prev = deepFreeze({ ws: { id: "w1" } });
    const next = deepFreeze({ ws: { id: "w1", project_uid: "puid" } });
    const result = shareStructural(prev, next);
    expect(result.ws).not.toBe(prev.ws);
    expect(result).toEqual(next);

    // And the reverse: next dropped an optional field.
    const prev2 = deepFreeze({ ws: { id: "w1", host_id: 7 } });
    const next2 = deepFreeze({ ws: { id: "w1" } });
    const result2 = shareStructural(prev2, next2);
    expect(result2.ws).not.toBe(prev2.ws);
    expect(result2).toEqual(next2);
  });
});

describe("shareStructural — no input mutation", () => {
  it("never mutates prev or next", () => {
    const prev = deepFreeze({
      workspaces: [{ id: "w1", surfaces: [{ id: "s1" }] }],
      pane_statuses: { p1: "idle" },
    });
    const nextRaw = ipcClone(prev);
    nextRaw.pane_statuses.p1 = "working";
    const next = deepFreeze(nextRaw);
    // deepFreeze above guarantees a throw on mutation; the call must not throw.
    expect(() => shareStructural(prev, next)).not.toThrow();
    // Values are unchanged.
    expect(prev.pane_statuses.p1).toBe("idle");
    expect(next.pane_statuses.p1).toBe("working");
  });
});
