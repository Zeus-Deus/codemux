import { describe, it, expect } from "vitest";
import { normalizeWheelDelta } from "./wheel";

describe("normalizeWheelDelta", () => {
  it("passes pixel-mode deltas through untouched", () => {
    expect(normalizeWheelDelta(120, 0 /* DOM_DELTA_PIXEL */)).toBe(120);
    expect(normalizeWheelDelta(-42.5, 0)).toBe(-42.5);
    expect(normalizeWheelDelta(0, 0)).toBe(0);
  });

  it("scales line-mode deltas to pixels", () => {
    // Notched mice report one notch as a delta of ~1 line; applied raw that
    // scrolls a single pixel and the surface reads as stuck.
    expect(normalizeWheelDelta(3, 1 /* DOM_DELTA_LINE */)).toBe(48);
    expect(normalizeWheelDelta(-1, 1)).toBe(-16);
  });

  it("scales page-mode deltas by the viewport height", () => {
    expect(normalizeWheelDelta(1, 2 /* DOM_DELTA_PAGE */)).toBe(
      window.innerHeight,
    );
    expect(normalizeWheelDelta(-2, 2)).toBe(-2 * window.innerHeight);
  });
});
