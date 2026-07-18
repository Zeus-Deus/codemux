/// <reference types="@testing-library/jest-dom/vitest" />
/**
 * Scroll-anchoring shim behavior:
 *
 *  1. On an engine with native `overflow-anchor` (and no override) the
 *     component renders null and installs NOTHING — the native path is
 *     byte-identical.
 *  2. Forced on, a content resize while the reader is away from the bottom
 *     (`data-scrollable` contains "end") corrects `scrollTop` by exactly
 *     how far the captured anchor row shifted.
 *  3/4. The same resize while pinned at the bottom (no "end") or during an
 *     engine-driven autoscroll (`data-autoscrolling`) leaves `scrollTop`
 *     untouched — the scroller engine owns the tail there.
 *  5. The localStorage "off" override disables the shim even where native
 *     anchoring is missing.
 *
 * jsdom does no layout, so geometry is mocked per element (assigned
 * `getBoundingClientRect`, same approach as other scroller-adjacent
 * tests), `ResizeObserver` is replaced with a recording stub (the
 * vitest.setup default is a no-op class), and `requestAnimationFrame`
 * runs callbacks synchronously so the rAF-throttled capture is immediate.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render } from "@testing-library/react";

import {
  SCROLL_ANCHORING_SHIM_STORAGE_KEY,
  ScrollAnchoringShim,
  decideScrollAnchoringShim,
  findAnchorRow,
  resetScrollAnchoringShimCacheForTests,
} from "./scroll-anchoring-shim";

// ---------------------------------------------------------------------------
// Environment stubs
// ---------------------------------------------------------------------------

type ROCallback = (
  entries: ResizeObserverEntry[],
  observer: ResizeObserver,
) => void;

/** Recording ResizeObserver: lets a test fire the content-resize callback
 *  by hand and assert whether the shim installed an observer at all. */
class RecordingResizeObserver {
  static instances: RecordingResizeObserver[] = [];
  observed: Element[] = [];
  constructor(public callback: ROCallback) {
    RecordingResizeObserver.instances.push(this);
  }
  observe(el: Element) {
    this.observed.push(el);
  }
  unobserve() {}
  disconnect() {}
}

beforeEach(() => {
  RecordingResizeObserver.instances = [];
  vi.stubGlobal("ResizeObserver", RecordingResizeObserver);
  // Synchronous rAF: the shim's throttled capture runs inside the dispatch.
  vi.stubGlobal(
    "requestAnimationFrame",
    (cb: FrameRequestCallback): number => {
      cb(0);
      return 1;
    },
  );
  vi.stubGlobal("cancelAnimationFrame", () => {});
  resetScrollAnchoringShimCacheForTests();
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  localStorage.removeItem(SCROLL_ANCHORING_SHIM_STORAGE_KEY);
  resetScrollAnchoringShimCacheForTests();
});

// ---------------------------------------------------------------------------
// Scaffold: the scroller anatomy the shim resolves against
// ---------------------------------------------------------------------------

const ROW_H = 100;

function domRect(top: number, height: number): DOMRect {
  return {
    top,
    bottom: top + height,
    height,
    left: 0,
    right: 0,
    width: 0,
    x: 0,
    y: top,
    toJSON: () => ({}),
  } as DOMRect;
}

/** Render the shim inside a minimal scroller anatomy with three rows and
 *  mutable mocked geometry. Row tops are viewport-relative (viewport top
 *  is 0): m0 fully above the viewport, m1 the topmost visible row. */
function renderScaffold(opts?: {
  scrollable?: string;
  autoscrolling?: boolean;
}) {
  const { scrollable = "start end", autoscrolling = false } = opts ?? {};
  const { container } = render(
    <div data-slot="message-scroller">
      <ScrollAnchoringShim />
      <div
        data-slot="message-scroller-viewport"
        data-scrollable={scrollable}
        {...(autoscrolling ? { "data-autoscrolling": "" } : {})}
      >
        <div data-slot="message-scroller-content">
          <div data-message-id="m0" />
          <div data-message-id="m1" />
          <div data-message-id="m2" />
        </div>
      </div>
    </div>,
  );

  const viewport = container.querySelector<HTMLElement>(
    '[data-slot="message-scroller-viewport"]',
  )!;
  const content = container.querySelector<HTMLElement>(
    '[data-slot="message-scroller-content"]',
  )!;

  // Mutable geometry: tests shift a row's top to simulate a
  // content-visibility height settle above it.
  const rowTops = new Map<string, number>([
    ["m0", -120],
    ["m1", 30],
    ["m2", 30 + ROW_H],
  ]);
  viewport.getBoundingClientRect = () => domRect(0, 600);
  for (const row of Array.from(
    content.querySelectorAll<HTMLElement>("[data-message-id]"),
  )) {
    const id = row.dataset.messageId!;
    row.getBoundingClientRect = () => domRect(rowTops.get(id)!, ROW_H);
  }

  // jsdom's scrollTop is layout-coupled; replace it with a plain settable
  // property so the shim's `scrollTop += delta` is observable.
  let scrollTop = 1000;
  Object.defineProperty(viewport, "scrollTop", {
    configurable: true,
    get: () => scrollTop,
    set: (v: number) => {
      scrollTop = v;
    },
  });

  /** The observer the shim installed on the content element (if any). */
  const contentObserver = () =>
    RecordingResizeObserver.instances.find((ro) =>
      ro.observed.includes(content),
    ) ?? null;

  /** Capture an anchor: a user scroll (rAF-synchronous → immediate). */
  const scrollOnce = () => viewport.dispatchEvent(new Event("scroll"));

  /** Simulate a content-height settle notification. */
  const fireContentResize = () => {
    const ro = contentObserver();
    expect(ro).not.toBeNull();
    ro!.callback([], ro as unknown as ResizeObserver);
  };

  return { viewport, content, rowTops, contentObserver, scrollOnce, fireContentResize };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe("ScrollAnchoringShim activation", () => {
  it("installs nothing on an engine with native overflow-anchor", () => {
    vi.stubGlobal("CSS", { supports: () => true });
    resetScrollAnchoringShimCacheForTests();

    const { viewport, contentObserver, scrollOnce } = renderScaffold();
    // Renders null: no observer, and a scroll + nothing else must not move
    // scrollTop.
    expect(contentObserver()).toBeNull();
    expect(RecordingResizeObserver.instances.length).toBe(0);
    scrollOnce();
    expect(viewport.scrollTop).toBe(1000);
  });

  it("localStorage 'off' disables the shim even where anchoring is unsupported", () => {
    vi.stubGlobal("CSS", { supports: () => false });
    localStorage.setItem(SCROLL_ANCHORING_SHIM_STORAGE_KEY, "off");
    resetScrollAnchoringShimCacheForTests();

    const { contentObserver } = renderScaffold();
    expect(contentObserver()).toBeNull();
    expect(RecordingResizeObserver.instances.length).toBe(0);
  });
});

describe("ScrollAnchoringShim correction (forced on)", () => {
  beforeEach(() => {
    localStorage.setItem(SCROLL_ANCHORING_SHIM_STORAGE_KEY, "on");
    resetScrollAnchoringShimCacheForTests();
  });

  it("pins the anchor row: a +200px shift while away from the bottom scrolls by +200", () => {
    const { viewport, rowTops, scrollOnce, fireContentResize } =
      renderScaffold({ scrollable: "start end" });

    // Capture: first row intersecting the viewport top is m1 (m0's bottom
    // sits above the edge), at topOffset 30.
    scrollOnce();

    // A height settle above the viewport pushes everything down 200px.
    rowTops.set("m1", 230);
    rowTops.set("m2", 230 + ROW_H);
    fireContentResize();
    expect(viewport.scrollTop).toBe(1200);

    // The stored offset was refreshed from a live measurement, so a second
    // notification with no further shift applies no further correction.
    fireContentResize();
    expect(viewport.scrollTop).toBe(1200);
  });

  it("leaves scrollTop untouched while pinned at the bottom (no 'end' in data-scrollable)", () => {
    const { viewport, rowTops, scrollOnce, fireContentResize } =
      renderScaffold({ scrollable: "start" });

    scrollOnce();
    rowTops.set("m1", 230);
    fireContentResize();
    expect(viewport.scrollTop).toBe(1000);
  });

  it("leaves scrollTop untouched during an engine-driven autoscroll", () => {
    const { viewport, rowTops, scrollOnce, fireContentResize } =
      renderScaffold({ scrollable: "start end", autoscrolling: true });

    scrollOnce();
    rowTops.set("m1", 230);
    fireContentResize();
    expect(viewport.scrollTop).toBe(1000);
  });

  it("skips a sub-epsilon shift and recaptures instead of scrolling when the anchor is gone", () => {
    const { viewport, content, rowTops, scrollOnce, fireContentResize } =
      renderScaffold({ scrollable: "start end" });

    scrollOnce();
    // Sub-pixel jitter is left alone.
    rowTops.set("m1", 30.3);
    fireContentResize();
    expect(viewport.scrollTop).toBe(1000);

    // Anchor row unmounts (e.g. transcript rebuild): the correction round
    // recaptures and bails without touching scrollTop.
    content.querySelector('[data-message-id="m1"]')!.remove();
    rowTops.set("m2", 400);
    fireContentResize();
    expect(viewport.scrollTop).toBe(1000);
  });
});

describe("decideScrollAnchoringShim", () => {
  it("runs exactly where native anchoring is missing (no override)", () => {
    expect(decideScrollAnchoringShim(true, null)).toBe(false);
    expect(decideScrollAnchoringShim(false, null)).toBe(true);
  });

  it("override wins in both directions", () => {
    expect(decideScrollAnchoringShim(true, "on")).toBe(true);
    expect(decideScrollAnchoringShim(false, "off")).toBe(false);
  });
});

describe("findAnchorRow", () => {
  function fakeRow(top: number): HTMLElement {
    return {
      getBoundingClientRect: () => domRect(top, ROW_H),
    } as unknown as HTMLElement;
  }

  it("full scan finds the first row crossing the viewport top", () => {
    const rows = [fakeRow(-300), fakeRow(-150), fakeRow(-50), fakeRow(50)];
    // -50 + 100 = 50 > 1 → index 2 is the topmost visible row.
    expect(findAnchorRow(rows, 0)?.index).toBe(2);
  });

  it("incremental search converges from a stale hint in both directions", () => {
    const rows = [fakeRow(-300), fakeRow(-150), fakeRow(-50), fakeRow(50)];
    expect(findAnchorRow(rows, 0, 0)?.index).toBe(2); // walk forward
    expect(findAnchorRow(rows, 0, 3)?.index).toBe(2); // walk backward
    expect(findAnchorRow(rows, 0, 2)?.index).toBe(2); // already right
  });

  it("returns null when every row is above the viewport top", () => {
    const rows = [fakeRow(-300), fakeRow(-150)];
    expect(findAnchorRow(rows, 0)).toBeNull();
    expect(findAnchorRow(rows, 0, 1)).toBeNull();
    expect(findAnchorRow([], 0)).toBeNull();
  });
});
