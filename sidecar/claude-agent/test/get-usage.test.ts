import { describe, expect, test } from "bun:test";

import { isUnsupportedSubtypeRejection } from "../src/methods/get-usage.ts";

describe("get-usage CLI rejection", () => {
  test("recognizes the CLI's unknown-subtype rejection", () => {
    expect(
      isUnsupportedSubtypeRejection(
        new Error("Unsupported control request subtype: get_usage"),
      ),
    ).toBe(true);
    expect(
      isUnsupportedSubtypeRejection(new Error("get_usage is not supported in this context")),
    ).toBe(true);
  });

  test("leaves real failures alone, even when they name the subtype", () => {
    expect(isUnsupportedSubtypeRejection(new Error("get_usage request timed out"))).toBe(false);
    expect(isUnsupportedSubtypeRejection(new Error("ECONNRESET"))).toBe(false);
  });
});
