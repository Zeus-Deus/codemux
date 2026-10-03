import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";

const stylesheet = readFileSync("src/globals.css", "utf8");

import {
  COMPOSER_OVERLAY_CARD,
  READING_BACK_THRESHOLD_PX,
  isReadingBack,
} from "./composer-overlay";

const geometry = (scrollTop: number) => ({
  scrollHeight: 10_000,
  clientHeight: 800,
  scrollTop,
});

/** Distance from the live edge is `scrollHeight - scrollTop - clientHeight`,
 *  so the tail sits at scrollTop 9200 for this geometry. */
const AT_EDGE = 9_200;

describe("isReadingBack", () => {
  it("is false at the live edge, so a settled transcript never sits dimmed", () => {
    expect(isReadingBack(geometry(AT_EDGE))).toBe(false);
  });

  it("tolerates the sub-pixel and end-pin slack just above the edge", () => {
    expect(isReadingBack(geometry(AT_EDGE - READING_BACK_THRESHOLD_PX))).toBe(
      false,
    );
  });

  it("dims once the reader is past the threshold", () => {
    expect(
      isReadingBack(geometry(AT_EDGE - READING_BACK_THRESHOLD_PX - 1)),
    ).toBe(true);
  });

  it("stays true far back in history", () => {
    expect(isReadingBack(geometry(0))).toBe(true);
  });

  it("treats an overscrolled position as the live edge rather than reading back", () => {
    expect(isReadingBack(geometry(AT_EDGE + 40))).toBe(false);
  });
});

describe("COMPOSER_OVERLAY_CARD", () => {
  it("dissolves the transcript before it reaches the floating title band", () => {
    // The viewport runs under the band, but the mask keeps a clear zone over
    // the controls, so they need no plates; jump and send targets add the
    // band height (see transcript-top-inset.ts).
    const bandRule = stylesheet.match(/\[data-under-titlebar="true"\] \[data-transcript-edge-fade\]\s*\{([^}]+)\}/)?.[1];
    expect(bandRule).not.toContain("padding-top");
    expect(bandRule).toContain("--transcript-top-clear: 32px");
    expect(bandRule).toContain("--transcript-top-fade: 24px");
  });
  it("keeps docked controls opaque while reading history, including without a mask", () => {
    const cardRule = stylesheet.match(/\.composer-overlay-card\s*\{([^}]+)\}/)?.[1];
    expect(cardRule).toContain("background-color: var(--background)");
    expect(stylesheet).not.toMatch(/\[data-reading-back\][^{]*\.composer-overlay-card/);
  });
  // The dim/restore rules are hand-written CSS in `globals.css` keyed off
  // this class under `[data-reading-back]`. Utilities cannot express it:
  // Tailwind wraps group selectors in `:where()`, which contributes no
  // specificity, so a stacked `:hover` restore silently lost to the dim.
  it("carries the class the stylesheet keys the fade off", () => {
    expect(COMPOSER_OVERLAY_CARD).toContain("composer-overlay-card");
  });

  // The region is `pointer-events-none` so the empty column gutters stay
  // transparent to clicks and selection on the transcript rows behind it.
  it("re-enables pointer events on the opaque docked controls", () => {
    expect(COMPOSER_OVERLAY_CARD).toContain("pointer-events-auto");
  });
});
