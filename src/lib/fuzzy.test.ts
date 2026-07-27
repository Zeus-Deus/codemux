import { describe, it, expect } from "vitest";
import { fuzzyFilter, fuzzyMatch, fuzzyScore } from "./fuzzy";

/** The real project list from the "Run in" picker. */
const PROJECTS = [
  "vexis-agent",
  "codemux",
  "vexis-agent-site",
  "codemux-sitev2",
  "ai-project_25-26",
  "dpcode",
  "odysseus",
  "passpage",
  "partpilot · pandora",
  "ai-solutions-belgium",
  "partpilot",
  "hermes-agent",
  "world-bench",
  "openclaw",
  "hermes-agent-personal",
  "rgbpc",
];

function rank(query: string): string[] {
  return fuzzyFilter(PROJECTS, query, (name) => name);
}

describe("fuzzyMatch", () => {
  it("matches an exact string", () => {
    expect(fuzzyMatch("codemux", "codemux")).toBe(true);
  });

  it("matches a prefix", () => {
    expect(fuzzyMatch("codemux", "cod")).toBe(true);
  });

  it("matches a scattered subsequence", () => {
    expect(fuzzyMatch("codemux", "cdx")).toBe(true);
  });

  it("is case-insensitive both ways", () => {
    expect(fuzzyMatch("Home directory (~)", "HOME")).toBe(true);
    expect(fuzzyMatch("CODEMUX", "cdx")).toBe(true);
  });

  it("rejects out-of-order characters", () => {
    expect(fuzzyMatch("codemux", "xoc")).toBe(false);
  });

  it("rejects characters that are not present", () => {
    expect(fuzzyMatch("codemux", "codez")).toBe(false);
  });

  it("rejects a query longer than the text", () => {
    expect(fuzzyMatch("cod", "codemux")).toBe(false);
  });

  it("matches everything on an empty query", () => {
    expect(fuzzyMatch("codemux", "")).toBe(true);
  });
});

describe("fuzzyScore", () => {
  it("returns null for a non-match", () => {
    expect(fuzzyScore("codemux", "zzz")).toBeNull();
  });

  it("scores a prefix above a mid-string substring", () => {
    const prefix = fuzzyScore("codemux", "co")!;
    const middle = fuzzyScore("dpcode", "co")!;
    expect(prefix).toBeGreaterThan(middle);
  });

  it("scores a substring above a scattered subsequence", () => {
    const substring = fuzzyScore("dpcode", "code")!;
    const scattered = fuzzyScore("codemux-sitev2", "cdmx")!;
    expect(substring).toBeGreaterThan(scattered);
  });

  it("rewards word-boundary hits (typing initials)", () => {
    const initials = fuzzyScore("hermes-agent-personal", "hap")!;
    const buried = fuzzyScore("hermes-agent-personal", "rmp")!;
    expect(initials).toBeGreaterThan(buried);
  });

  it("rewards camelCase humps", () => {
    expect(fuzzyScore("agentChatPane", "acp")).toBeGreaterThan(
      fuzzyScore("agentchatpane", "acp")!,
    );
  });

  it("prefers the shorter candidate on an equal match", () => {
    expect(fuzzyScore("codemux", "co")!).toBeGreaterThan(
      fuzzyScore("codemux-sitev2", "co")!,
    );
  });
});

describe("fuzzyFilter", () => {
  it("returns every item, in order, for an empty query", () => {
    expect(fuzzyFilter(PROJECTS, "", (n) => n)).toEqual(PROJECTS);
  });

  it("ignores surrounding whitespace", () => {
    expect(fuzzyFilter(PROJECTS, "  codemux  ", (n) => n)[0]).toBe("codemux");
  });

  it("puts the exact project first when typing its prefix", () => {
    expect(rank("cod")[0]).toBe("codemux");
    expect(rank("odys")[0]).toBe("odysseus");
    expect(rank("rgb")[0]).toBe("rgbpc");
  });

  it("narrows a prefix to the projects that share it", () => {
    expect(rank("codemux")).toEqual(["codemux", "codemux-sitev2"]);
  });

  it("finds a project from the first letters of its words", () => {
    expect(rank("hap")[0]).toBe("hermes-agent-personal");
    expect(rank("asb")[0]).toBe("ai-solutions-belgium");
    expect(rank("vas")[0]).toBe("vexis-agent-site");
  });

  it("drops non-matching projects entirely", () => {
    expect(rank("zzz")).toEqual([]);
    expect(rank("openclaw")).toEqual(["openclaw"]);
  });

  it("matches whatever haystack the caller selects", () => {
    const rows = [
      { name: "passpage", path: "/home/zeus/projects/passpage" },
      { name: "odysseus", path: "/home/zeus/greek/odysseus" },
    ];
    expect(fuzzyFilter(rows, "greek", (r) => r.path).map((r) => r.name)).toEqual(
      ["odysseus"],
    );
    expect(fuzzyFilter(rows, "greek", (r) => r.name)).toEqual([]);
  });
});
