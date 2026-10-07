import { afterEach, describe, expect, it, vi } from "vitest";

let remoteClient = false;
vi.mock("@/components/remote/is-remote-client", () => ({
  isRemoteClient: () => remoteClient,
}));

import {
  searchSettings,
  settingsPagesForQuery,
  splitHighlight,
} from "./settings-search";

const labels = (query: string, agentChat = true) =>
  searchSettings(query, agentChat).flatMap((group) =>
    group.results.map((result) => `${group.sectionLabel} › ${result.label}`),
  );

describe("searchSettings", () => {
  afterEach(() => {
    remoteClient = false;
  });

  it("leaves out rows the web remote does not render", () => {
    expect(labels("reset")).toContain("About › Reset settings");
    expect(labels("log file")).toContain("About › Logs");
    remoteClient = true;
    expect(labels("reset")).not.toContain("About › Reset settings");
    expect(labels("log file")).not.toContain("About › Logs");
    // The page itself is still there.
    expect(labels("about")).toContain("About › About");
  });

  it("finds a row by a word in its label, on the page that holds it", () => {
    expect(labels("scrollback")).toContain("Session Restore › Scrollback lines");
  });

  it("finds a row by a synonym its label does not contain", () => {
    expect(labels("checkpoint")).toEqual(["Agent › Per-turn revert checkpoints"]);
  });

  it("names the synonym that matched when the label shows no match", () => {
    const [agent] = searchSettings("snapshot", true);
    expect(agent.results[0]).toMatchObject({
      label: "Per-turn revert checkpoints",
      matchedKeyword: "snapshot",
    });
    const [appearance] = searchSettings("palette", true);
    expect(appearance.results[0]).toMatchObject({ label: "Theme", matchedKeyword: "palette" });
    // A label that already shows the match needs no hint.
    expect(searchSettings("scrollback", true)[0].results[0].matchedKeyword).toBeUndefined();
  });

  it("requires every word, so a second word narrows", () => {
    expect(labels("branch")).toContain("Appearance › Show git stats");
    const narrowed = labels("base branch");
    expect(narrowed[0]).toBe("Git › Default base branch");
    expect(narrowed).not.toContain("Appearance › Show git stats");
  });

  it("lists the page itself, plus only the rows the query also names", () => {
    const results = labels("terminal");
    expect(results[0]).toBe("Terminal › Terminal");
    expect(results).toContain("Terminal › Font");
    // Cursor style and Color theme say nothing about "terminal" themselves.
    expect(results).not.toContain("Terminal › Cursor style");
  });

  it("ranks a label that starts with the query above one that merely contains it", () => {
    const [first] = searchSettings("font", true);
    expect(first.results[0].label).toBe("Font");
  });

  it("hides chat-only pages when Agent Chat is off", () => {
    expect(labels("mcp servers", true)).toContain("MCP Servers › MCP Servers");
    expect(labels("mcp servers", false)).not.toContain("MCP Servers › MCP Servers");
  });

  it("returns nothing for an empty query", () => {
    expect(searchSettings("   ", true)).toEqual([]);
  });
});

describe("settingsPagesForQuery", () => {
  it("lists every page for a bare 'settings'", () => {
    const pages = settingsPagesForQuery("settings", true);
    expect(pages).toContain("about");
    expect(pages.length).toBeGreaterThan(20);
  });

  it("narrows with words after 'settings'", () => {
    expect(settingsPagesForQuery("settings scrollback", true)).toEqual(["session_restore"]);
  });

  it("finds a page by one of its rows without the prefix", () => {
    expect(settingsPagesForQuery("scrollback", true)).toEqual(["session_restore"]);
  });

  it("stays quiet for one-letter queries and caps short ones", () => {
    expect(settingsPagesForQuery("s", true)).toEqual([]);
    expect(settingsPagesForQuery("co", true).length).toBeLessThanOrEqual(5);
  });
});

describe("splitHighlight", () => {
  it("puts matches at odd indexes, case-insensitively", () => {
    expect(splitHighlight("Default base branch", "BASE")).toEqual(["Default ", "base", " branch"]);
  });

  it("treats regex characters in the query literally", () => {
    expect(splitHighlight("Auto-configure (.mcp.json)", ".mcp")).toEqual([
      "Auto-configure (",
      ".mcp",
      ".json)",
    ]);
  });
});
