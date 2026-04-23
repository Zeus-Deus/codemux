import { describe, expect, it } from "vitest";

import { deriveTitleFromFirstMessage } from "./derive-title";

describe("deriveTitleFromFirstMessage", () => {
  it("returns null for an empty string", () => {
    expect(deriveTitleFromFirstMessage("")).toBeNull();
  });

  it("returns null for whitespace only", () => {
    expect(deriveTitleFromFirstMessage("   \n\t  ")).toBeNull();
  });

  it("returns a short message as-is", () => {
    expect(deriveTitleFromFirstMessage("hi there")).toBe("hi there");
  });

  it("returns a message of exactly 40 chars as-is", () => {
    const text = "x".repeat(40);
    expect(deriveTitleFromFirstMessage(text)).toBe(text);
    expect(deriveTitleFromFirstMessage(text)).toHaveLength(40);
  });

  it("trims leading and trailing whitespace before anything else", () => {
    expect(deriveTitleFromFirstMessage("  hello  ")).toBe("hello");
  });

  it("trims a long message back to a word boundary within the lookback window (no ellipsis)", () => {
    // 47 chars total — the last space within the first 40 chars is
    // at index 30 ("Can you help me understand the"), well inside
    // the 25-40 lookback window, so we cut there with no ellipsis.
    const text = "Can you help me understand the architecture of my project";
    const result = deriveTitleFromFirstMessage(text);
    expect(result).toBe("Can you help me understand the");
    expect(result!.endsWith("…")).toBe(false);
  });

  it("hard-truncates with an ellipsis when no word boundary sits in the lookback window", () => {
    // 50 chars of letters + one early space — the space is far
    // outside the lookback window, so the function falls back to a
    // hard truncate with ellipsis.
    const text = "a" + " " + "b".repeat(48);
    // First 40 chars: "a bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" — the
    // only space is at index 1, well before MAX_TITLE - LOOKBACK (25).
    const result = deriveTitleFromFirstMessage(text);
    expect(result).toMatch(/…$/);
    expect(result!.length).toBeLessThanOrEqual(41); // 40 + "…"
  });

  it("handles a message of exactly 41 chars by trimming to the lookback word boundary when possible", () => {
    // "aaaa bbbbbbbb cccccccc dddddddd eeeeeeee ff"
    // index positions: "aaaa" (0-3), space(4), "bbbbbbbb" (5-12),
    // space(13), "cccccccc" (14-21), space(22), "dddddddd" (23-30),
    // space(31), "eeeeeeee" (32-39), space(40), "ff" (41-42)
    // First 40 chars end right at index 39 (last "e").
    // lastIndexOf(" ") in first 40 chars is index 31.
    // 31 >= 40 - 15 = 25 → cut at 31, return "aaaa bbbbbbbb cccccccc dddddddd".
    const text = "aaaa bbbbbbbb cccccccc dddddddd eeeeeeee ff";
    const result = deriveTitleFromFirstMessage(text);
    expect(result).toBe("aaaa bbbbbbbb cccccccc dddddddd");
  });

  it("trims trailing whitespace from the word-boundary cut", () => {
    // If the word boundary lands on a run of spaces, we don't want
    // trailing spaces in the final title.
    const text = "word1 word2 word3                       suffix-far-away";
    const result = deriveTitleFromFirstMessage(text);
    expect(result!.endsWith(" ")).toBe(false);
  });
});
