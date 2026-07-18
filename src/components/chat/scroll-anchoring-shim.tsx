import { useEffect, useRef } from "react";

/**
 * Scroll-anchoring shim for the Agent Chat transcript.
 *
 * The transcript keeps thousands of rows cheap with
 * `[content-visibility:auto]` + per-row `contain-intrinsic-size` estimates
 * (see `MessageList.tsx`). Rows above the viewport carry ESTIMATED heights
 * that settle to REAL heights when first revealed, and the design delegates
 * absorbing those settles to native CSS scroll anchoring: the viewport is
 * `[overflow-anchor:none]` by default and
 * `data-[scrollable~=end]:[overflow-anchor:auto]` — anchoring is ON exactly
 * while the reader is scrolled away from the bottom.
 *
 * WebKit engines never implemented the `overflow-anchor` property AND
 * (verified empirically on WebKitGTK 2.52.5, the Linux webview this app
 * ships in) perform no implicit scroll anchoring at all: a content-height
 * change above the viewport shifts the visual content with `scrollTop`
 * unchanged. `content-visibility` IS supported there, so the settles do
 * happen — and scrolling up through cold history visibly jumps ("bugs down
 * then corrects") on Linux (WebKitGTK) and macOS (WKWebView), while
 * Chromium-family engines are fine.
 *
 * `<ScrollAnchoringShim />` manually reproduces what native scroll
 * anchoring does, ONLY on engines that lack it (feature-detected via
 * `CSS.supports` on `overflow-anchor`; a `localStorage` override mirrors
 * `transcript-fade.ts` for manual A/B testing). Mechanism:
 *
 *  - ANCHOR CAPTURE — on viewport scroll (passive, rAF-throttled to one
 *    capture per frame), remember the first row intersecting the viewport
 *    top and its viewport-relative top offset.
 *  - CORRECTION — a ResizeObserver on the content element fires when
 *    height settles change total content height. If the reader is away
 *    from the bottom (the viewport's `data-scrollable` contains `"end"` —
 *    the exact gate the CSS design uses; while pinned/following the
 *    scroller engine owns the tail and the shim must stay out) and the
 *    engine is not mid-autoscroll, nudge `scrollTop` by however far the
 *    anchor row moved, pinning it visually — exactly like native anchoring.
 *
 * On engines with native `overflow-anchor` the component renders null and
 * installs nothing: zero behavior change on the Chromium/WebView2 path.
 */

/**
 * `localStorage` key for the manual shim override. Values: `"on"` forces
 * the shim active even where `overflow-anchor` is supported (lets a
 * Chromium dev session exercise the correction path), `"off"` forces it
 * inactive; anything else is ignored. Mirrors
 * `TRANSCRIPT_FADE_STORAGE_KEY` in `transcript-fade.ts` — a live escape
 * hatch togglable from devtools without a rebuild.
 */
export const SCROLL_ANCHORING_SHIM_STORAGE_KEY = "codemux:anchoring-shim";

/** How close (px) the anchor row must stay to its captured offset before a
 *  correction is applied. Sub-pixel jitter from rounding is left alone. */
const CORRECTION_EPSILON_PX = 0.5;

/** Read the manual override, tolerating unavailable storage (private mode,
 *  sandbox) — no override then. */
export function readScrollAnchoringShimOverride(): "on" | "off" | null {
  try {
    const value = globalThis.localStorage?.getItem(
      SCROLL_ANCHORING_SHIM_STORAGE_KEY,
    );
    return value === "on" || value === "off" ? value : null;
  } catch {
    return null; // storage unavailable — no override
  }
}

/** Does this engine implement CSS scroll anchoring (`overflow-anchor`)?
 *  Guarded for environments without a `CSS` global (jsdom variants). */
export function detectOverflowAnchorSupport(): boolean {
  try {
    return (
      typeof CSS !== "undefined" && !!CSS.supports?.("overflow-anchor", "none")
    );
  } catch {
    return false;
  }
}

/**
 * Pure shim decision. Override wins first (both directions); otherwise the
 * shim runs exactly where native anchoring is missing. Injectable inputs
 * with production defaults so it is unit-testable (mirrors
 * `decideTranscriptFade`).
 */
export function decideScrollAnchoringShim(
  supported: boolean = detectOverflowAnchorSupport(),
  override: "on" | "off" | null = readScrollAnchoringShimOverride(),
): boolean {
  if (override === "on") return true;
  if (override === "off") return false;
  return !supported;
}

let cachedEnabled: boolean | null = null;

/**
 * Cached production entry point: decide once per app session (the webview
 * engine doesn't change at runtime). Logs the verdict once — but only when
 * the shim is active or overridden, so the native-anchoring default path
 * stays quiet. Suppressed under vitest (jsdom reports `overflow-anchor`
 * unsupported, which would log on every test run).
 */
export function scrollAnchoringShimEnabled(): boolean {
  if (cachedEnabled !== null) return cachedEnabled;
  const override = readScrollAnchoringShimOverride();
  cachedEnabled = decideScrollAnchoringShim(
    detectOverflowAnchorSupport(),
    override,
  );
  if ((cachedEnabled || override !== null) && import.meta.env?.MODE !== "test") {
    console.info(
      `[codemux::transcript] scroll-anchoring shim ${
        cachedEnabled ? "active" : "inactive"
      } — ${
        override !== null
          ? `forced by localStorage["${SCROLL_ANCHORING_SHIM_STORAGE_KEY}"]="${override}"`
          : `this engine has no native overflow-anchor scroll anchoring; ` +
            `set localStorage["${SCROLL_ANCHORING_SHIM_STORAGE_KEY}"]="off" to disable`
      }`,
    );
  }
  return cachedEnabled;
}

/** Test hook: clear the module-level decision cache. */
export function resetScrollAnchoringShimCacheForTests(): void {
  cachedEnabled = null;
}

// ---------------------------------------------------------------------------
// Pure anchor search
// ---------------------------------------------------------------------------

/**
 * Find the ANCHOR row: the first row (rows are document order = visual
 * top-to-bottom) whose bottom edge is below the viewport's top edge — i.e.
 * the topmost row still (partially) visible. The +1px slop ignores rows
 * whose bottom coincides with the viewport edge.
 *
 * `startIndex` is the previously captured anchor's index: a typical scroll
 * moves the anchor only a few rows, so the search walks outward from there
 * (the predicate is monotone over the ordered rows — false above, true from
 * the anchor down — so a directional walk from any start converges).
 * Without a usable `startIndex` it falls back to a full forward scan.
 */
export function findAnchorRow(
  rows: ArrayLike<HTMLElement>,
  viewportTop: number,
  startIndex?: number,
): { el: HTMLElement; index: number } | null {
  const len = rows.length;
  if (len === 0) return null;
  const intersects = (i: number) =>
    rows[i].getBoundingClientRect().bottom > viewportTop + 1;

  if (startIndex !== undefined && startIndex >= 0 && startIndex < len) {
    let i = startIndex;
    if (intersects(i)) {
      // Walk backward to the FIRST intersecting row (anchor moved up).
      while (i > 0 && intersects(i - 1)) i--;
      return { el: rows[i], index: i };
    }
    // Walk forward to the first intersecting row (anchor moved down).
    for (i = startIndex + 1; i < len; i++) {
      if (intersects(i)) return { el: rows[i], index: i };
    }
    return null;
  }

  for (let i = 0; i < len; i++) {
    if (intersects(i)) return { el: rows[i], index: i };
  }
  return null;
}

// ---------------------------------------------------------------------------
// Imperative shim
// ---------------------------------------------------------------------------

interface CapturedAnchor {
  el: HTMLElement;
  /** Row index at capture time — the next capture's search hint only;
   *  geometry always re-reads the element itself. */
  index: number;
  /** Viewport-relative top of the anchor row at capture time. The
   *  correction restores exactly this offset. */
  topOffset: number;
}

/**
 * Install the shim on a resolved viewport/content pair. Returns a cleanup
 * that removes the scroll listener, disconnects the observer, and cancels
 * any in-flight capture frame. Exported for direct unit testing; the
 * component below is the production mount path.
 */
export function installScrollAnchoringShim(
  viewport: HTMLElement,
  content: HTMLElement,
): () => void {
  // Without ResizeObserver there is nothing to correct against — no-op.
  if (typeof ResizeObserver === "undefined") return () => {};

  let anchor: CapturedAnchor | null = null;
  let framePending = false;
  let frameId = 0;

  const captureAnchor = () => {
    const rows = content.querySelectorAll<HTMLElement>("[data-message-id]");
    const viewportTop = viewport.getBoundingClientRect().top;
    const found = findAnchorRow(rows, viewportTop, anchor?.index);
    anchor = found
      ? {
          el: found.el,
          index: found.index,
          topOffset: found.el.getBoundingClientRect().top - viewportTop,
        }
      : null;
  };

  // Passive + rAF-throttled: at most one capture per frame, off the scroll
  // hot path.
  const onScroll = () => {
    if (framePending) return;
    framePending = true;
    frameId = requestAnimationFrame(() => {
      framePending = false;
      captureAnchor();
    });
  };

  const observer = new ResizeObserver(() => {
    // Gate (a): only while the reader is away from the bottom — the exact
    // condition under which the CSS design enables native anchoring (the
    // engine keeps "end" in `data-scrollable` while unpinned in history).
    // While pinned/following the engine owns the tail snap and the shim
    // must stay out.
    const scrollable = viewport.getAttribute("data-scrollable") ?? "";
    if (!scrollable.split(/\s+/).includes("end")) return;
    // Gate (b): never fight an engine-driven programmatic scroll.
    if (viewport.hasAttribute("data-autoscrolling")) return;
    // Gate (c): need a live captured anchor; a torn-down one (row
    // unmounted) recaptures and sits this round out.
    if (!anchor || !anchor.el.isConnected) {
      captureAnchor();
      return;
    }
    const viewportTop = viewport.getBoundingClientRect().top;
    const delta =
      anchor.el.getBoundingClientRect().top - viewportTop - anchor.topOffset;
    if (Math.abs(delta) <= CORRECTION_EPSILON_PX) return;
    // The settle pushed the anchor row by `delta`; scroll by the same
    // amount so it stays visually pinned, then re-measure the offset (the
    // write may clamp at the scroll range edges).
    viewport.scrollTop += delta;
    anchor.topOffset =
      anchor.el.getBoundingClientRect().top -
      viewport.getBoundingClientRect().top;
  });

  viewport.addEventListener("scroll", onScroll, { passive: true });
  observer.observe(content);

  return () => {
    viewport.removeEventListener("scroll", onScroll);
    observer.disconnect();
    if (framePending) cancelAnimationFrame(frameId);
    framePending = false;
  };
}

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------

/**
 * Mount point for the shim. Rendered inside `MessageList`'s
 * `<MessageScroller>` (a sibling of the viewport, like `MessageTrail`).
 * On engines with native anchoring it renders null and installs nothing;
 * when active it renders only an inert hidden probe span, used to resolve
 * the viewport/content elements from the shared scroller root the same way
 * `MessageTrail` does (`closest('[data-slot="message-scroller"]')` →
 * `querySelector` for the slots) — no ref plumbing through the scroller.
 */
export function ScrollAnchoringShim() {
  if (!scrollAnchoringShimEnabled()) return null;
  return <ActiveScrollAnchoringShim />;
}

/** Active branch split out so the inert path pays for no hooks. */
function ActiveScrollAnchoringShim() {
  const probeRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    const root = probeRef.current?.closest('[data-slot="message-scroller"]');
    const viewport =
      (root as HTMLElement | null)?.querySelector<HTMLElement>(
        '[data-slot="message-scroller-viewport"]',
      ) ?? null;
    const contentEl =
      (root as HTMLElement | null)?.querySelector<HTMLElement>(
        '[data-slot="message-scroller-content"]',
      ) ?? null;
    if (!viewport || !contentEl) return;
    return installScrollAnchoringShim(viewport, contentEl);
  }, []);

  return <span ref={probeRef} aria-hidden style={{ display: "none" }} />;
}
