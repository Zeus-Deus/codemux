import { describe, expect, it } from "vitest";

import { clampPeekSize, MIN_PEEK_SIZE } from "./browser-peek-store";

describe("clampPeekSize", () => {
  const viewport = { width: 2000, height: 1000 };

  it("keeps a size inside the bounds", () => {
    expect(clampPeekSize({ width: 600, height: 400 }, viewport)).toEqual({ width: 600, height: 400 });
  });

  it("never shrinks below the readable minimum", () => {
    expect(clampPeekSize({ width: 100, height: 50 }, viewport)).toEqual(MIN_PEEK_SIZE);
  });

  it("never grows past 60% of the window", () => {
    expect(clampPeekSize({ width: 5000, height: 5000 }, viewport)).toEqual({ width: 1200, height: 600 });
  });

  it("lets the minimum win on a window too small for both", () => {
    expect(clampPeekSize({ width: 500, height: 500 }, { width: 400, height: 300 })).toEqual(MIN_PEEK_SIZE);
  });
});
