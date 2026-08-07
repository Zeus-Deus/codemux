import { describe, expect, it } from "vitest";

import {
  MIN_CONTENT_WIDTH,
  RIGHT_PANEL_MIN_WIDTH,
  clampRightPanelWidth,
  maxRightPanelWidth,
} from "./right-panel-width";

describe("maxRightPanelWidth", () => {
  // The headline requirement: the panel has to be draggable wide enough to
  // be a real workspace (a browser lives in it now), not the old 500px lip.
  it("allows three quarters of the row on a normal window", () => {
    expect(maxRightPanelWidth(1600)).toBe(1200);
  });

  it("scales with the row rather than capping at a pixel number", () => {
    expect(maxRightPanelWidth(2560)).toBe(1920);
    expect(maxRightPanelWidth(3840)).toBe(2880);
  });

  // The fraction is what binds at any realistic window: a 1280px window
  // with the sidebar open leaves a ~990px row, and 75% of that still
  // clears the content floor.
  it("lets the fraction rule bind at a normal laptop width", () => {
    expect(maxRightPanelWidth(992)).toBe(744);
    expect(maxRightPanelWidth(744)).toBe(744 - MIN_CONTENT_WIDTH);
  });

  // Only once the row gets genuinely small does the floor take over, so
  // dragging can never squeeze the chat out of existence.
  it("keeps room for the chat side once the row gets tight", () => {
    expect(maxRightPanelWidth(800)).toBe(800 - MIN_CONTENT_WIDTH);
    expect(maxRightPanelWidth(600)).toBe(600 - MIN_CONTENT_WIDTH);
  });

  it("never returns less than the panel's own minimum", () => {
    expect(maxRightPanelWidth(300)).toBe(RIGHT_PANEL_MIN_WIDTH);
    expect(maxRightPanelWidth(0)).toBeGreaterThan(RIGHT_PANEL_MIN_WIDTH);
  });
});

describe("clampRightPanelWidth", () => {
  it("passes through a width that fits", () => {
    expect(clampRightPanelWidth(900, 1600)).toBe(900);
  });

  it("clamps a width the row can't give", () => {
    expect(clampRightPanelWidth(1500, 1600)).toBe(1200);
    expect(clampRightPanelWidth(900, 992)).toBe(744);
  });

  it("holds the floor", () => {
    expect(clampRightPanelWidth(10, 1600)).toBe(RIGHT_PANEL_MIN_WIDTH);
  });

  it("survives an unmeasured row instead of collapsing to the minimum", () => {
    // `maxRightPanelWidth(0)` is the sanity ceiling, so an un-laid-out row
    // never silently shrinks a panel the user sized deliberately.
    expect(clampRightPanelWidth(900, 0)).toBe(900);
  });
});
