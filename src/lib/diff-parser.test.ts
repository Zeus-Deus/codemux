import { describe, expect, it } from "vitest";

import { parseDiff } from "./diff-parser";

describe("parseDiff", () => {
  it("returns nothing for empty input", () => {
    expect(parseDiff("")).toEqual([]);
  });

  it("parses adds, dels, and context with correct line numbers", () => {
    const diff = ["@@ -1,2 +1,2 @@", " keep", "-old", "+new"].join("\n");
    expect(parseDiff(diff)).toEqual([
      { type: "hunk-header", content: "@@ -1,2 +1,2 @@", oldLine: null, newLine: null },
      { type: "context", content: "keep", oldLine: 1, newLine: 1 },
      { type: "del", content: "old", oldLine: 2, newLine: null },
      { type: "add", content: "new", oldLine: null, newLine: 2 },
    ]);
  });

  it("skips file-header metadata outside a hunk", () => {
    const diff = [
      "diff --git a/f b/f",
      "index 111..222 100644",
      "--- a/f",
      "+++ b/f",
      "@@ -1 +1 @@",
      "-x",
      "+y",
    ].join("\n");
    const lines = parseDiff(diff);
    // The two file-header lines must not appear as content.
    expect(lines.filter((l) => l.type === "del")).toHaveLength(1);
    expect(lines.filter((l) => l.type === "add")).toHaveLength(1);
    expect(lines.find((l) => l.type === "del")?.content).toBe("x");
    expect(lines.find((l) => l.type === "add")?.content).toBe("y");
  });

  it("keeps a deleted `-- comment` line and preserves line numbers", () => {
    // Deleting a SQL/Lua `-- comment` line renders as `--- comment` in the
    // unified diff — it must be parsed as a deletion, not a file header.
    const diff = ["@@ -1,2 +1,1 @@", "--- comment: drop index", " keep this line"].join("\n");
    const lines = parseDiff(diff);
    const del = lines.find((l) => l.type === "del");
    expect(del).toBeDefined();
    expect(del?.content).toBe("-- comment: drop index");
    expect(del?.oldLine).toBe(1);
    // The surviving context line is original line 2, not 1.
    const ctx = lines.find((l) => l.type === "context");
    expect(ctx?.oldLine).toBe(2);
  });

  it("keeps an added `++ x` line", () => {
    const diff = ["@@ -1 +1,2 @@", " keep", "++ added marker"].join("\n");
    const lines = parseDiff(diff);
    const add = lines.find((l) => l.type === "add");
    expect(add).toBeDefined();
    expect(add?.content).toBe("+ added marker");
    expect(add?.newLine).toBe(2);
  });

  it("skips `\\ No newline at end of file` inside a hunk", () => {
    const diff = ["@@ -1 +1 @@", "-x", "+y", "\\ No newline at end of file"].join("\n");
    const lines = parseDiff(diff);
    expect(lines.some((l) => l.content.includes("No newline"))).toBe(false);
  });

  it("handles a multi-file diff, resetting hunk context per file", () => {
    const diff = [
      "diff --git a/a.sql b/a.sql",
      "--- a/a.sql",
      "+++ b/a.sql",
      "@@ -1 +1 @@",
      "--- a comment",
      "+-- a comment changed",
      "diff --git a/b.txt b/b.txt",
      "--- a/b.txt",
      "+++ b/b.txt",
      "@@ -1 +1 @@",
      "-foo",
      "+bar",
    ].join("\n");
    const lines = parseDiff(diff);
    // First file: a deleted `-- a comment`, an added `-- a comment changed`.
    // Second file: a deleted `foo`, an added `bar`. The four `--- a/...` /
    // `+++ b/...` headers must not leak in as content.
    const dels = lines.filter((l) => l.type === "del").map((l) => l.content);
    const adds = lines.filter((l) => l.type === "add").map((l) => l.content);
    expect(dels).toEqual(["-- a comment", "foo"]);
    expect(adds).toEqual(["-- a comment changed", "bar"]);
  });
});
