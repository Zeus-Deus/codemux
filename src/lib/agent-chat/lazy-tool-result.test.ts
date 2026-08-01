import { describe, expect, it } from "vitest";

import {
  formatLazyBytes,
  isLazyToolResultStub,
  lazyToolResultStub,
  LAZY_TOOL_RESULT_KEY,
  toolResultContentFromPayload,
} from "./lazy-tool-result";

const stub = {
  [LAZY_TOOL_RESULT_KEY]: {
    row_id: 12,
    bytes: 65_536,
    preview: "first lines",
    line_count: 900,
    has_images: false,
  },
};

describe("isLazyToolResultStub", () => {
  it("recognizes a well-formed stub", () => {
    expect(isLazyToolResultStub(stub)).toBe(true);
    expect(lazyToolResultStub(stub)?.row_id).toBe(12);
  });

  it("rejects real bodies — a stub must never be inferred from content", () => {
    // A false positive here would hide a genuine tool result behind a
    // fetch affordance pointing at a row that does not exist.
    for (const content of [
      null,
      "plain text",
      ["a", "b"],
      { text: "an object body" },
      { [LAZY_TOOL_RESULT_KEY]: "not an object" },
      { [LAZY_TOOL_RESULT_KEY]: { row_id: "12", bytes: 1, preview: "", line_count: 1 } },
      { [LAZY_TOOL_RESULT_KEY]: { row_id: 12, preview: "", line_count: 1 } },
    ]) {
      expect(isLazyToolResultStub(content)).toBe(false);
      expect(lazyToolResultStub(content)).toBeNull();
    }
  });
});

describe("toolResultContentFromPayload", () => {
  it("extracts the content of an item_completed tool_result", () => {
    const payload = JSON.stringify({
      type: "item_completed",
      thread_id: "t",
      turn_id: "turn-1",
      item: { kind: "tool_result", tool_use_id: "tu-1", content: "body", is_error: false },
    });
    expect(toolResultContentFromPayload(payload)).toBe("body");
  });

  it("returns null for anything else so the caller can offer a retry", () => {
    expect(toolResultContentFromPayload("not json")).toBeNull();
    expect(toolResultContentFromPayload(JSON.stringify({ type: "turn_completed" }))).toBeNull();
    expect(
      toolResultContentFromPayload(
        JSON.stringify({ type: "item_completed", item: { kind: "assistant_text", text: "x" } }),
      ),
    ).toBeNull();
    expect(
      toolResultContentFromPayload(
        JSON.stringify({ type: "item_completed", item: { kind: "tool_result" } }),
      ),
    ).toBeNull();
  });
});

describe("formatLazyBytes", () => {
  it("scales the unit", () => {
    expect(formatLazyBytes(512)).toBe("512 B");
    expect(formatLazyBytes(64 * 1024)).toBe("64 KB");
    expect(formatLazyBytes(3 * 1024 * 1024)).toBe("3.0 MB");
  });
});
