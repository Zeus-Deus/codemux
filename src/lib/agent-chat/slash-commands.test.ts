import { describe, expect, it, vi } from "vitest";

import {
  buildModeCommands,
  filterSlashItems,
  findSlashAtCursor,
  groupSlashItems,
  MODE_CYCLE_ORDER,
  nextModeInCycle,
  type SlashCommandItem,
} from "./slash-commands";

describe("findSlashAtCursor", () => {
  it.each<[string, number, ReturnType<typeof findSlashAtCursor>]>([
    ["", 0, null],
    ["/", 1, { start: 0, query: "" }],
    ["/pl", 3, { start: 0, query: "pl" }],
    ["hello /", 7, { start: 6, query: "" }],
    ["hello /pl", 9, { start: 6, query: "pl" }],
    ["hello/pl", 8, null],
    ["/hello /pl", 10, { start: 7, query: "pl" }],
    ["a/b", 3, null],
    ["hello world\n/pl", 15, { start: 12, query: "pl" }],
    ["/pl world", 9, null],
    ["/", 0, null], // cursor at 0 — slash not yet typed-into
    ["abc /xy", 7, { start: 4, query: "xy" }],
    [" /", 2, { start: 1, query: "" }], // leading space then slash
  ])("findSlashAtCursor(%j, %i)", (value, cursor, expected) => {
    expect(findSlashAtCursor(value, cursor)).toEqual(expected);
  });

  it("returns null when cursor is out of bounds", () => {
    expect(findSlashAtCursor("/", -1)).toBeNull();
    expect(findSlashAtCursor("/", 99)).toBeNull();
  });

  it("treats tabs and multi-char whitespace runs as separators", () => {
    expect(findSlashAtCursor("hi\t/x", 5)).toEqual({ start: 3, query: "x" });
  });
});

describe("filterSlashItems", () => {
  const items: SlashCommandItem[] = [
    {
      id: "mode:plan",
      label: "Plan",
      command: "/plan",
      group: "MODES",
      onSelect: () => {},
    },
    {
      id: "mode:ask",
      label: "Ask",
      command: "/ask",
      group: "MODES",
      onSelect: () => {},
    },
    {
      id: "mode:debug",
      label: "Debug",
      command: "/debug",
      group: "MODES",
      onSelect: () => {},
    },
  ];

  it("returns everything when query is empty", () => {
    expect(filterSlashItems(items, "")).toEqual(items);
  });

  it("filters by command prefix", () => {
    expect(filterSlashItems(items, "pl").map((i) => i.id)).toEqual([
      "mode:plan",
    ]);
  });

  it("filters by label substring (case-insensitive)", () => {
    // "ebu" only appears inside "Debug".
    expect(filterSlashItems(items, "ebu").map((i) => i.id)).toEqual([
      "mode:debug",
    ]);
  });

  it("returns empty when nothing matches", () => {
    expect(filterSlashItems(items, "xyz")).toEqual([]);
  });
});

describe("buildModeCommands", () => {
  it("returns all three modes when default is active", () => {
    const onActivate = vi.fn();
    const items = buildModeCommands({ activeMode: "default", onActivate });
    expect(items.map((i) => i.id)).toEqual([
      "mode:plan",
      "mode:ask",
      "mode:debug",
    ]);
    items[0]!.onSelect();
    expect(onActivate).toHaveBeenCalledWith("plan");
  });

  it("hides the active mode from the list", () => {
    const items = buildModeCommands({
      activeMode: "plan",
      onActivate: vi.fn(),
    });
    expect(items.map((i) => i.id)).toEqual(["mode:ask", "mode:debug"]);
  });

  it("hides ask when ask is active", () => {
    const items = buildModeCommands({
      activeMode: "ask",
      onActivate: vi.fn(),
    });
    expect(items.map((i) => i.id)).toEqual(["mode:plan", "mode:debug"]);
  });
});

describe("nextModeInCycle", () => {
  it("cycles default → plan → ask → debug → default", () => {
    let m = nextModeInCycle("default");
    expect(m).toBe("plan");
    m = nextModeInCycle(m);
    expect(m).toBe("ask");
    m = nextModeInCycle(m);
    expect(m).toBe("debug");
    m = nextModeInCycle(m);
    expect(m).toBe("default");
  });

  it("exposes the cycle order constant", () => {
    expect(MODE_CYCLE_ORDER).toEqual(["default", "plan", "ask", "debug"]);
  });
});

describe("groupSlashItems", () => {
  it("groups by `group` field, preserving insertion order", () => {
    const items: SlashCommandItem[] = [
      {
        id: "a",
        label: "A",
        command: "/a",
        group: "G1",
        onSelect: () => {},
      },
      {
        id: "b",
        label: "B",
        command: "/b",
        group: "G2",
        onSelect: () => {},
      },
      {
        id: "c",
        label: "C",
        command: "/c",
        group: "G1",
        onSelect: () => {},
      },
    ];
    const grouped = groupSlashItems(items);
    expect(grouped).toEqual([
      { group: "G1", items: [items[0], items[2]] },
      { group: "G2", items: [items[1]] },
    ]);
  });

  it("returns empty array for empty input", () => {
    expect(groupSlashItems([])).toEqual([]);
  });
});
