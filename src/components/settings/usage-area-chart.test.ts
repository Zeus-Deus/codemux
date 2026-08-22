import { describe, expect, it } from "vitest";

import { monotonePath, tickIndices } from "./usage-area-chart";

describe("monotonePath", () => {
  it("handles the degenerate sizes", () => {
    expect(monotonePath([])).toBe("");
    expect(monotonePath([{ x: 5, y: 7 }])).toBe("M5,7");
  });

  it("keeps a flat run on the baseline instead of overshooting", () => {
    // A spike followed by zeros: a Catmull-Rom curve would dip below the
    // baseline (y > 100 here) on the way down; a monotone one must not.
    const pts = [
      { x: 0, y: 100 },
      { x: 10, y: 0 },
      { x: 20, y: 100 },
      { x: 30, y: 100 },
      { x: 40, y: 100 },
    ];
    const d = monotonePath(pts);
    const ys = [...d.matchAll(/[\d.]+,([\d.]+)/g)].map((m) => Number(m[1]));
    expect(Math.max(...ys)).toBeLessThanOrEqual(100);
    expect(Math.min(...ys)).toBeGreaterThanOrEqual(0);
    expect(d.startsWith("M0,100 C")).toBe(true);
    expect(d.endsWith("40,100")).toBe(true);
  });
});

describe("tickIndices", () => {
  it("labels every bucket when there is room", () => {
    expect(tickIndices(7, 640)).toEqual([0, 1, 2, 3, 4, 5, 6]);
  });

  it("thins dense axes but always keeps the first and last bucket", () => {
    const ticks = tickIndices(90, 640);
    expect(ticks[0]).toBe(0);
    expect(ticks[ticks.length - 1]).toBe(89);
    expect(ticks.length).toBeLessThanOrEqual(Math.floor(640 / 72));
    // Evenly stepped apart from the final tick.
    const steps = new Set(ticks.slice(1, -1).map((t, i) => t - ticks[i]));
    expect(steps.size).toBe(1);
  });

  it("never crowds the final tick", () => {
    for (const n of [2, 3, 8, 9, 24, 30, 31, 90]) {
      const ticks = tickIndices(n, 500);
      const gaps = ticks.slice(1).map((t, i) => t - ticks[i]);
      const step = gaps[0];
      expect(gaps.every((g) => g >= step / 2)).toBe(true);
    }
  });

  it("handles empty and single-bucket inputs", () => {
    expect(tickIndices(0, 640)).toEqual([]);
    expect(tickIndices(1, 640)).toEqual([0]);
  });
});
