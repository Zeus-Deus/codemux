import { useSyncExternalStore } from "react";

/**
 * The root font size in px — what `1rem` is right now.
 *
 * The interface-size setting (`applyTypography`) rescales the app by writing
 * the root element's inline `font-size`, so every rem-sized control grows with
 * it. Geometry that is computed in JS (title-bar insets, the details card's
 * corner) has to scale by the same factor, and re-run when it changes.
 */
export function useRootFontPx(): number {
  return useSyncExternalStore(subscribe, read, () => 16);
}

function read(): number {
  const size = parseFloat(getComputedStyle(document.documentElement).fontSize);
  return Number.isFinite(size) && size > 0 ? size : 16;
}

function subscribe(onChange: () => void): () => void {
  const observer = new MutationObserver(onChange);
  observer.observe(document.documentElement, {
    attributes: true,
    attributeFilter: ["style", "class"],
  });
  // Browser-level font scaling changes the root size without touching it.
  window.addEventListener("resize", onChange);
  return () => {
    observer.disconnect();
    window.removeEventListener("resize", onChange);
  };
}
