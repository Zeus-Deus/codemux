import { describe, expect, it } from "vitest";
import { indexDiffRows } from "./pr-anchor";
import {
  applySuggestion,
  hasSuggestion,
  originalLines,
  splitSuggestions,
} from "./pr-suggestion";

describe("splitSuggestions", () => {
  it("keeps prose and suggestions in order", () => {
    const body = [
      "Use the constant here.",
      "",
      "```suggestion",
      "  const recent = entries.slice(0, LIMIT);",
      "```",
      "Otherwise **fine**.",
    ].join("\n");
    expect(splitSuggestions(body)).toEqual([
      { kind: "markdown", text: "Use the constant here.\n" },
      { kind: "suggestion", lines: ["  const recent = entries.slice(0, LIMIT);"] },
      { kind: "markdown", text: "Otherwise **fine**." },
    ]);
  });

  it("leaves ordinary code fences to the markdown", () => {
    const body = "```ts\nconst a = 1;\n```";
    expect(splitSuggestions(body)).toEqual([{ kind: "markdown", text: body }]);
    expect(hasSuggestion(body)).toBe(false);
  });

  it("treats an empty suggestion as a deletion and an unclosed one as running to the end", () => {
    expect(splitSuggestions("```suggestion\n```")).toEqual([
      { kind: "suggestion", lines: [] },
    ]);
    expect(splitSuggestions("~~~~suggestion\r\na\r\n```\r\nb")).toEqual([
      { kind: "suggestion", lines: ["a", "```", "b"] },
    ]);
  });
});

describe("originalLines", () => {
  const diff = [
    "diff --git a/a.ts b/a.ts",
    "--- a/a.ts",
    "+++ b/a.ts",
    "@@ -1,3 +1,3 @@",
    " one",
    "-two",
    "+TWO",
    " three",
  ].join("\n");
  const index = indexDiffRows(diff);

  it("reads a range off the requested side", () => {
    expect(originalLines(index, "a.ts", "RIGHT", 1, 2)).toEqual(["one", "TWO"]);
    expect(originalLines(index, "a.ts", "LEFT", 2, 2)).toEqual(["two"]);
  });

  it("is null when any line is outside the diff", () => {
    expect(originalLines(index, "a.ts", "RIGHT", 3, 9)).toBeNull();
    expect(originalLines(index, "b.ts", "RIGHT", 1, 1)).toBeNull();
  });
});

describe("applySuggestion", () => {
  it("replaces the anchored range and keeps the line ending", () => {
    expect(applySuggestion("a\nb\nc\n", 2, 2, ["b"], ["B1", "B2"])).toBe("a\nB1\nB2\nc\n");
    expect(applySuggestion("a\r\nb\r\nc", 1, 2, ["a", "b"], [])).toBe("c");
  });

  it("refuses when the checkout no longer has those lines", () => {
    expect(() => applySuggestion("a\nedited\nc", 2, 2, ["b"], ["B"])).toThrow(
      /changed in your checkout/,
    );
  });
});
