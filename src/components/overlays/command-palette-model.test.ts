import { describe, expect, it } from "vitest";
import {
  commandSearchText,
  compareWorkspaceOrder,
  groupCountLabel,
  parsePaletteQuery,
  previewedThemeId,
  rankByQuery,
  rankThemeGroup,
  resultCountLabel,
  themeRowValue,
  workspacePathText,
  workspaceRowSubtitle,
  workspaceSearchText,
  type WorkspaceOrder,
} from "./command-palette-model";

describe("parsePaletteQuery", () => {
  it("treats a bare query as an everything search", () => {
    const q = parsePaletteQuery("codemux");
    expect(q.mode).toBe("all");
    expect(q.needle).toBe("codemux");
    expect(q.pathMode).toBe(false);
  });

  it("narrows to commands behind the '>' prefix and strips it from the needle", () => {
    const q = parsePaletteQuery("> split");
    expect(q.mode).toBe("commands");
    expect(q.needle).toBe("split");
  });

  it("enters command mode on a bare '>' with nothing typed yet", () => {
    const q = parsePaletteQuery(">");
    expect(q.mode).toBe("commands");
    expect(q.needle).toBe("");
  });

  it("switches to path matching once the query contains a separator", () => {
    expect(parsePaletteQuery("~/dev/codemux").pathMode).toBe(true);
    expect(parsePaletteQuery("dev").pathMode).toBe(false);
  });
});

describe("rankByQuery", () => {
  const items = [
    { name: "codemux", path: "/home/z/dev/codemux" },
    { name: "hermes-agent", path: "/home/z/work/hermes-agent" },
    { name: "codemux-sitev2", path: "/home/z/dev/codemux-sitev2" },
  ];
  const rank = (raw: string) =>
    rankByQuery(
      items,
      parsePaletteQuery(raw),
      (i) => i.name,
      (i) => i.path,
    ).map((i) => i.name);

  it("keeps the caller's ordering when nothing is typed", () => {
    expect(rank("")).toEqual(["codemux", "hermes-agent", "codemux-sitev2"]);
  });

  it("ranks the tighter match first", () => {
    expect(rank("codemux")[0]).toBe("codemux");
  });

  it("matches subsequences, not just substrings", () => {
    expect(rank("cdx")).toContain("codemux");
  });

  it("drops non-matches", () => {
    expect(rank("hermes")).toEqual(["hermes-agent"]);
  });

  it("switches to the path haystack in path mode", () => {
    // "work/" appears in no name, only in hermes-agent's path.
    expect(rank("work/")).toEqual(["hermes-agent"]);
  });
});

describe("workspace search haystacks", () => {
  const workspace = {
    title: "Fix missing PR icon",
    git_branch: "fix/missing-pr-icon",
    project_root: "/home/zeus/projects/codemux",
    worktree_path: "/home/zeus/projects/codemux/.worktrees/fix-missing-pr-icon",
    cwd: "/home/zeus/projects/codemux/.worktrees/fix-missing-pr-icon",
  };

  it("keeps name mode to what the row actually shows", () => {
    expect(workspaceSearchText(workspace)).toBe(
      "Fix missing PR icon fix/missing-pr-icon",
    );
  });

  it("indexes every distinct location for path mode", () => {
    expect(workspacePathText(workspace)).toBe(
      "/home/zeus/projects/codemux /home/zeus/projects/codemux/.worktrees/fix-missing-pr-icon",
    );
  });

  it("keeps names out of the path haystack", () => {
    // Regression: folding title/branch in here made `projects/vexis` match
    // unrelated projects whose NAMES supplied the missing characters, and
    // rank them above the real answer.
    const text = workspacePathText(workspace);
    expect(text).not.toContain("Fix missing PR icon");
    expect(text).not.toContain("fix/missing-pr-icon");
  });

  it("omits missing optional metadata without polluting search text", () => {
    expect(
      workspacePathText({
        project_root: null,
        worktree_path: null,
        cwd: "/tmp/scratch",
      }),
    ).toBe("/tmp/scratch");
  });
});

describe("workspaceRowSubtitle", () => {
  it("shows the project when the title already says the branch", () => {
    expect(
      workspaceRowSubtitle(
        { title: "workflow-approval", git_branch: "demo/workflow-approval" },
        "codemux",
      ),
    ).toBe("codemux");
  });

  it("also collapses an exact title/branch match", () => {
    expect(workspaceRowSubtitle({ title: "main", git_branch: "main" }, "codemux")).toBe(
      "codemux",
    );
  });

  it("adds the branch when it carries information the title doesn't", () => {
    expect(
      workspaceRowSubtitle(
        { title: "Workspace 80", git_branch: "fix/103-model-dropdown" },
        "codemux",
      ),
    ).toBe("codemux · fix/103-model-dropdown");
  });

  it("falls back to the project name on a non-git workspace", () => {
    expect(workspaceRowSubtitle({ title: "scratch", git_branch: null }, "notes")).toBe(
      "notes",
    );
  });
});

describe("compareWorkspaceOrder", () => {
  const order = (o: Partial<WorkspaceOrder>): WorkspaceOrder => ({
    status: null,
    activityAt: undefined,
    parked: false,
    ...o,
  });

  it("puts work that needs you above work that is merely running", () => {
    const rows = [
      order({ status: "working" }),
      order({ status: "permission" }),
      order({ status: "review" }),
    ];
    expect([...rows].sort(compareWorkspaceOrder)[0].status).toBe("permission");
  });

  it("ranks any live status above idle", () => {
    const idleButRecent = order({ activityAt: 5_000 });
    const liveButStale = order({ status: "review", activityAt: 1 });
    expect([idleButRecent, liveButStale].sort(compareWorkspaceOrder)[0]).toBe(
      liveButStale,
    );
  });

  it("breaks ties on recency, newest first", () => {
    const older = order({ activityAt: 10 });
    const newer = order({ activityAt: 99 });
    expect([older, newer].sort(compareWorkspaceOrder)).toEqual([newer, older]);
  });

  it("sinks parked work below everything active, however recent", () => {
    const parked = order({ status: "working", activityAt: 999, parked: true });
    const active = order({ activityAt: 1 });
    expect([parked, active].sort(compareWorkspaceOrder)).toEqual([active, parked]);
  });
});

describe("labels", () => {
  it("searches commands by their hidden synonyms", () => {
    expect(commandSearchText({ label: "Search in files", keywords: "grep" })).toBe(
      "Search in files grep",
    );
    expect(commandSearchText({ label: "Settings" })).toBe("Settings");
  });

  it("reports a capped group honestly instead of under-counting", () => {
    expect(groupCountLabel(8, 77)).toBe("8 of 77");
    expect(groupCountLabel(3, 3)).toBe("3");
  });

  it("pluralises the result count", () => {
    expect(resultCountLabel(1)).toBe("1 result");
    expect(resultCountLabel(0)).toBe("0 results");
    expect(resultCountLabel(12)).toBe("12 results");
  });
});

describe("theme rows", () => {
  // Registry order: built-ins as declared, then custom.
  const themes = ["Graphite", "Warm Stone", "Ember", "Abyss", "Iris", "Sonokai"].map(
    (label) => ({ label }),
  );

  const match = (needle: string) =>
    rankThemeGroup(themes, parsePaletteQuery(needle), (t) => t.label).map((t) => t.label);

  it("surfaces the whole set, in registry order, for the word itself", () => {
    // Regression: scoring every row against a shared `… theme colors palette`
    // haystack matched them all with a score that differed only by name
    // length, so "theme" returned the list sorted by how short each theme was
    // called — Iris first, Warm Stone last.
    expect(match("theme")).toEqual([
      "Graphite",
      "Warm Stone",
      "Ember",
      "Abyss",
      "Iris",
      "Sonokai",
    ]);
  });

  it("answers to the group's other generic words too", () => {
    expect(match("colors")).toHaveLength(themes.length);
    expect(match("appearance")).toHaveLength(themes.length);
    expect(match("palette")).toHaveLength(themes.length);
  });

  it("narrows to the named theme when the query names one", () => {
    expect(match("ember")).toEqual(["Ember"]);
    expect(match("abyss")).toEqual(["Abyss"]);
  });

  it("shows nothing for a query that is neither a name nor a group word", () => {
    expect(match("workspace")).toEqual([]);
    expect(match("")).toEqual([]);
  });

  it("reads back the theme a selected row should preview", () => {
    expect(previewedThemeId(themeRowValue("custom-night-signal"))).toBe(
      "custom-night-signal",
    );
  });

  it("previews nothing for any other kind of row", () => {
    expect(previewedThemeId("ws:abc")).toBeNull();
    expect(previewedThemeId("cmd:settings")).toBeNull();
    expect(previewedThemeId("theme-studio:generate")).toBeNull();
    expect(previewedThemeId("")).toBeNull();
  });
});
