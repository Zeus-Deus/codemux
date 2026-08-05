import { describe, expect, it } from "vitest";

import {
  getAnchoredTurnMetrics,
  getRowBottom,
  realContentOverflowsViewport,
  resolveSendAnchorIndex,
  SEND_ANCHOR_OFFSET,
  type TranscriptMeasurementState,
} from "./send-scroll-state";

/** A measured LegendList snapshot: rows are laid out top-to-bottom with the
 *  given heights, so positions are their running sum. */
function measured({
  heights,
  scroll = 0,
  scrollLength = 500,
}: {
  heights: number[];
  scroll?: number;
  scrollLength?: number;
}): TranscriptMeasurementState {
  const tops: number[] = [];
  let running = 0;
  for (const h of heights) {
    tops.push(running);
    running += h;
  }
  return {
    data: heights.map((_, i) => ({ i })),
    scroll,
    scrollLength,
    positionAtIndex: (index) => tops[index],
    sizeAtIndex: (index) => heights[index],
  };
}

describe("resolveSendAnchorIndex", () => {
  const rows = [
    { nonce: null },
    { nonce: "n-1" },
    { nonce: null },
    { nonce: "n-2" },
    { nonce: null },
  ];
  const get = (row: { nonce: string | null }) => row.nonce;

  it("finds the row carrying the send's client nonce", () => {
    expect(resolveSendAnchorIndex(rows, "n-1", get)).toBe(1);
  });

  it("does not fall back to the last index when rows follow the prompt", () => {
    // The regression this guards: a queued follow-up bubble or a control row
    // landing after the prompt would make "last index" anchor the wrong row.
    expect(resolveSendAnchorIndex(rows, "n-2", get)).toBe(3);
    expect(resolveSendAnchorIndex(rows, "n-2", get)).not.toBe(rows.length - 1);
  });

  it("returns the newest match when a nonce appears twice", () => {
    // A hydrate replay can reintroduce an older bubble carrying the same
    // token; the one the reader just sent is the later one.
    const duplicated = [{ nonce: "dup" }, { nonce: null }, { nonce: "dup" }];
    expect(resolveSendAnchorIndex(duplicated, "dup", get)).toBe(2);
  });

  it("returns null with no anchor, or when the row is not in the list yet", () => {
    expect(resolveSendAnchorIndex(rows, null, get)).toBeNull();
    expect(resolveSendAnchorIndex(rows, "missing", get)).toBeNull();
    expect(resolveSendAnchorIndex([], "n-1", get)).toBeNull();
  });
});

describe("getRowBottom", () => {
  it("adds the measured height to the row's position", () => {
    expect(getRowBottom(measured({ heights: [100, 60] }), 1)).toBe(160);
  });

  it("floors an unmeasured (zero-height) row at 1px", () => {
    // A row that has not laid out yet must not read as "no row at all".
    expect(getRowBottom(measured({ heights: [100, 0] }), 1)).toBe(101);
  });

  it("returns null when the virtualizer has no finite measurement", () => {
    const state: TranscriptMeasurementState = {
      ...measured({ heights: [10] }),
      positionAtIndex: () => Number.NaN,
    };
    expect(getRowBottom(state, 0)).toBeNull();
  });
});

describe("getAnchoredTurnMetrics", () => {
  it("does not move while the turn still fits the viewport", () => {
    // Prompt at index 2 (top 200) + a short answer: everything from the
    // prompt down fits, so the prompt stays parked near the top.
    const state = measured({
      heights: [100, 100, 40, 120],
      scroll: 200 - SEND_ANCHOR_OFFSET,
      scrollLength: 500,
    });
    const metrics = getAnchoredTurnMetrics({ state, anchorIndex: 2 });
    expect(metrics).not.toBeNull();
    expect(metrics!.turnHeight).toBe(160);
    expect(metrics!.overflowsUsableViewport).toBe(false);
    expect(metrics!.scrollDeltaToRevealEnd).toBe(0);
  });

  it("advances only far enough to reveal the tail once the turn overflows", () => {
    const state = measured({
      heights: [100, 100, 40, 900],
      scroll: 200 - SEND_ANCHOR_OFFSET,
      scrollLength: 500,
    });
    const metrics = getAnchoredTurnMetrics({ state, anchorIndex: 2 })!;
    expect(metrics.overflowsUsableViewport).toBe(true);
    // lastBottom 1140 - usable (500 - 16) = 656 target; from scroll 184.
    expect(metrics.targetScrollToRevealEnd).toBe(656);
    expect(metrics.scrollDeltaToRevealEnd).toBe(656 - 184);
  });

  it("never scrolls backwards", () => {
    // The reader (or a previous advance) is already past the tail target.
    const state = measured({
      heights: [100, 100, 40, 900],
      scroll: 5000,
      scrollLength: 500,
    });
    expect(
      getAnchoredTurnMetrics({ state, anchorIndex: 2 })!.scrollDeltaToRevealEnd,
    ).toBe(0);
  });

  it("clamps an out-of-range anchor index instead of reading past the data", () => {
    const state = measured({ heights: [100, 100] });
    expect(getAnchoredTurnMetrics({ state, anchorIndex: 99 })).not.toBeNull();
    expect(getAnchoredTurnMetrics({ state, anchorIndex: -5 })!.anchorTop).toBe(0);
  });

  it("returns null for an empty list or an unmeasured tail", () => {
    expect(
      getAnchoredTurnMetrics({ state: measured({ heights: [] }), anchorIndex: 0 }),
    ).toBeNull();
    const unmeasured: TranscriptMeasurementState = {
      ...measured({ heights: [100, 100] }),
      sizeAtIndex: () => Number.NaN,
    };
    expect(getAnchoredTurnMetrics({ state: unmeasured, anchorIndex: 0 })).toBeNull();
  });
});

describe("realContentOverflowsViewport", () => {
  it("is false while the real rows fit, so following never targets blank space", () => {
    // The anchored end space is reserved emptiness below the last row;
    // following-end must not treat it as somewhere to scroll to.
    expect(
      realContentOverflowsViewport(
        measured({ heights: [100, 100], scrollLength: 500 }),
      ),
    ).toBe(false);
  });

  it("is true once the real rows exceed the usable viewport", () => {
    expect(
      realContentOverflowsViewport(
        measured({ heights: [300, 300], scrollLength: 500 }),
      ),
    ).toBe(true);
  });

  it("is false for an empty transcript", () => {
    expect(realContentOverflowsViewport(measured({ heights: [] }))).toBe(false);
  });
});
