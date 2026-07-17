import { describe, expect, it, vi } from "vitest";

import {
  buildModeCommands,
  buildModelCommand,
  buildProviderCommands,
  buildWorkflowCommand,
  filterSlashItems,
  findMentionAtCursor,
  findSlashAtCursor,
  findTriggerAtCursor,
  groupSlashItems,
  MODE_CYCLE_ORDER,
  nextModeInCycle,
  parseMentionQuery,
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

describe("findMentionAtCursor (Step 8 Stage 2)", () => {
  it.each<[string, number, ReturnType<typeof findMentionAtCursor>]>([
    ["", 0, null],
    ["@", 1, { start: 0, query: "" }],
    ["@composer", 9, { start: 0, query: "composer" }],
    ["hi @", 4, { start: 3, query: "" }],
    ["hi @comp", 8, { start: 3, query: "comp" }],
    ["a@b", 3, null], // `@` inside a word — must not open
    ["hi@", 3, null], // `@` glued to a word
    ["@a @b", 5, { start: 3, query: "b" }], // cursor on second mention wins
    ["hello world\n@file", 17, { start: 12, query: "file" }],
    ["@a b", 4, null], // cursor past the space
    [" @", 2, { start: 1, query: "" }],
    ["abc @xy", 7, { start: 4, query: "xy" }],
    ["abc\t@xy", 7, { start: 4, query: "xy" }],
  ])("findMentionAtCursor(%j, %i)", (value, cursor, expected) => {
    expect(findMentionAtCursor(value, cursor)).toEqual(expected);
  });

  it("returns null when cursor is out of bounds", () => {
    expect(findMentionAtCursor("@", -1)).toBeNull();
    expect(findMentionAtCursor("@", 99)).toBeNull();
  });

  it("does not fire on slash characters", () => {
    expect(findMentionAtCursor("/foo", 4)).toBeNull();
    expect(findMentionAtCursor("/", 1)).toBeNull();
  });
});

describe("findTriggerAtCursor (shared primitive)", () => {
  it("backs both findSlashAtCursor and findMentionAtCursor", () => {
    expect(findTriggerAtCursor("/foo", 4, "/")).toEqual({
      start: 0,
      query: "foo",
    });
    expect(findTriggerAtCursor("@foo", 4, "@")).toEqual({
      start: 0,
      query: "foo",
    });
    // Crossed triggers: `/foo` queried with `@` returns null.
    expect(findTriggerAtCursor("/foo", 4, "@")).toBeNull();
    expect(findTriggerAtCursor("@foo", 4, "/")).toBeNull();
  });

  it("never matches when whitespace separates the trigger from the cursor", () => {
    expect(findTriggerAtCursor("/x foo", 6, "/")).toBeNull();
    expect(findTriggerAtCursor("@x foo", 6, "@")).toBeNull();
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

  it("adds /default when a mode is active and onDeactivate is wired", () => {
    const onDeactivate = vi.fn();
    const items = buildModeCommands({
      activeMode: "plan",
      onActivate: vi.fn(),
      onDeactivate,
    });
    expect(items.map((i) => i.id)).toEqual([
      "mode:ask",
      "mode:debug",
      "mode:default",
    ]);
    const def = items.find((i) => i.id === "mode:default")!;
    expect(def.command).toBe("/default");
    def.onSelect();
    expect(onDeactivate).toHaveBeenCalledTimes(1);
  });

  it("omits /default when default mode is already active", () => {
    const items = buildModeCommands({
      activeMode: "default",
      onActivate: vi.fn(),
      onDeactivate: vi.fn(),
    });
    expect(items.map((i) => i.id)).toEqual([
      "mode:plan",
      "mode:ask",
      "mode:debug",
    ]);
  });
});

describe("buildModelCommand", () => {
  it("is a state-only row that opens the model picker", () => {
    const onOpen = vi.fn();
    const item = buildModelCommand({ onOpen });
    expect(item.id).toBe("composer:model");
    expect(item.command).toBe("/model");
    expect(item.group).toBe("SETTINGS");
    item.onSelect();
    expect(onOpen).toHaveBeenCalledTimes(1);
  });
});

describe("buildProviderCommands", () => {
  const commands = [
    {
      name: "compact",
      description: "Clear history but keep a summary",
      argumentHint: "",
    },
    { name: "review", description: "", argumentHint: "<pr-url>" },
    { name: "plan", description: "colliding custom command", argumentHint: "" },
    { name: "opaque", description: "", argumentHint: "" },
  ];

  it("maps discovered commands into the COMMANDS group", () => {
    const items = buildProviderCommands({
      commands,
      reservedNames: new Set(),
    });
    expect(items[0]).toMatchObject({
      id: "provider-command:compact",
      label: "compact",
      command: "/compact",
      group: "COMMANDS",
      description: "Clear history but keep a summary",
    });
  });

  it("falls back to the argument hint, then a generic description", () => {
    const items = buildProviderCommands({
      commands,
      reservedNames: new Set(),
    });
    expect(items.find((i) => i.label === "review")!.description).toBe(
      "/review <pr-url>",
    );
    expect(items.find((i) => i.label === "opaque")!.description).toBe(
      "Provider command",
    );
  });

  it("drops commands whose names are reserved by local rows", () => {
    const items = buildProviderCommands({
      commands,
      reservedNames: new Set(["plan"]),
    });
    expect(items.map((i) => i.label)).toEqual([
      "compact",
      "review",
      "opaque",
    ]);
  });
});

describe("buildWorkflowCommand", () => {
  it("is enabled with the orchestration description when isClaude is true", () => {
    const item = buildWorkflowCommand({ isClaude: true });
    expect(item.id).toBe("workflow");
    expect(item.command).toBe("/workflow");
    expect(item.disabled).toBe(false);
    expect(item.description).toBe(
      "Orchestrate this task with many subagents",
    );
  });

  it("is disabled with a reason when isClaude is false", () => {
    const item = buildWorkflowCommand({ isClaude: false });
    expect(item.disabled).toBe(true);
    expect(item.description).toBe("Only available with Claude models");
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

describe("parseMentionQuery", () => {
  it("defaults to file when no prefix is supplied", () => {
    expect(parseMentionQuery("foo.ts")).toEqual({
      category: "file",
      filter: "foo.ts",
    });
    expect(parseMentionQuery("")).toEqual({ category: "file", filter: "" });
  });

  it("parses recognised category prefixes", () => {
    expect(parseMentionQuery("issue:1234")).toEqual({
      category: "issue",
      filter: "1234",
    });
    expect(parseMentionQuery("pr:bug")).toEqual({
      category: "pr",
      filter: "bug",
    });
    expect(parseMentionQuery("folder:src")).toEqual({
      category: "folder",
      filter: "src",
    });
    expect(parseMentionQuery("file:readme")).toEqual({
      category: "file",
      filter: "readme",
    });
  });

  it("accepts an empty filter (popup-on-trigger)", () => {
    expect(parseMentionQuery("issue:")).toEqual({
      category: "issue",
      filter: "",
    });
  });

  it("is case-insensitive on the prefix", () => {
    expect(parseMentionQuery("ISSUE:42")).toEqual({
      category: "issue",
      filter: "42",
    });
  });

  it("treats unknown prefixes as plain filter text", () => {
    // No special routing — caller falls back to file search.
    expect(parseMentionQuery("user:zeus")).toEqual({
      category: "file",
      filter: "user:zeus",
    });
  });
});
