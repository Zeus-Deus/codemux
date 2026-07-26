import { useCallback, useEffect, useRef } from "react";

/** Approximate line height used to convert line-mode wheel deltas to px.
 *  Matches the browsers' own default step for `DOM_DELTA_LINE`. */
const LINE_HEIGHT_PX = 16;
/** Fallback page height when there's no window to measure (SSR / tests). */
const PAGE_HEIGHT_PX = 800;

/** Convert a raw wheel delta into pixels.
 *
 *  `WheelEvent.deltaY` is only in pixels when `deltaMode` is
 *  `DOM_DELTA_PIXEL`. Classic notched mice report `DOM_DELTA_LINE` (and some
 *  configurations `DOM_DELTA_PAGE`), where a full notch arrives as a delta of
 *  `1` — applying that verbatim to `scrollTop`/`scrollLeft` scrolls a single
 *  pixel and the surface feels stuck. */
export function normalizeWheelDelta(delta: number, deltaMode: number): number {
  switch (deltaMode) {
    case 1 /* WheelEvent.DOM_DELTA_LINE */:
      return delta * LINE_HEIGHT_PX;
    case 2 /* WheelEvent.DOM_DELTA_PAGE */:
      return (
        delta *
        (typeof window !== "undefined" && window.innerHeight
          ? window.innerHeight
          : PAGE_HEIGHT_PX)
      );
    default:
      return delta;
  }
}

/** Let a horizontally-overflowing strip (tab bar, preset bar) be panned with a
 *  plain vertical mouse wheel.
 *
 *  `overflow-x: auto` only reacts to native horizontal wheel / trackpad input
 *  on its own — a vertical wheel over the strip moves `scrollLeft` by 0 — so
 *  the vertical delta is translated manually. `deltaX` is left to the native
 *  horizontal-scroll path.
 *
 *  The listener is attached natively rather than through React's `onWheel`
 *  prop because React registers wheel listeners passively at the root, where
 *  `preventDefault()` is a no-op. Without it the ancestor scroller consumes
 *  the same gesture and the page scrolls at the same time as the strip.
 *
 *  The event is only consumed when the strip actually moved, so a wheel at
 *  either end still chains to the ancestor scroller as usual.
 *
 *  Returns a stable callback ref to place on the scrolling element, so the
 *  listener follows the element itself — hosts that mount the strip
 *  conditionally (the preset bar returns `null` until its store loads) still
 *  get it wired up the moment the node appears.
 */
export function useHorizontalWheelScroll<T extends HTMLElement>(): (
  node: T | null,
) => void {
  const detachRef = useRef<(() => void) | null>(null);

  // Detach on unmount too — the callback ref is only invoked with `null` when
  // the element is swapped, not when the whole tree goes away.
  useEffect(() => () => detachRef.current?.(), []);

  return useCallback((node: T | null) => {
    detachRef.current?.();
    detachRef.current = null;
    if (!node) return;

    const onWheel = (e: WheelEvent) => {
      if (e.deltaY === 0) return;
      if (node.scrollWidth <= node.clientWidth) return;

      const delta = normalizeWheelDelta(e.deltaY, e.deltaMode);
      if (delta === 0) return;

      const before = node.scrollLeft;
      const max = node.scrollWidth - node.clientWidth;
      node.scrollLeft = Math.max(0, Math.min(max, before + delta));
      if (node.scrollLeft !== before) e.preventDefault();
    };

    node.addEventListener("wheel", onWheel, { passive: false });
    detachRef.current = () => node.removeEventListener("wheel", onWheel);
  }, []);
}
