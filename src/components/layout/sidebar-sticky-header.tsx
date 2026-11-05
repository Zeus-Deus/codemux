import { useLayoutEffect, useRef, useState, type ReactNode } from "react";

import { ScrollEdgeFade } from "@/components/ui/scroll-edge-fade";

/** The sidebar's sticky header block (filter row + Needs-you strip), shared by
 *  both views.
 *
 *  Besides pinning its children, it keeps programmatic and focus scrolls clear
 *  of itself: its height is published on the sidebar scroller as
 *  `--sidebar-sticky-h` and the scroller's `scroll-padding` reserves that
 *  height (plus a small gap), so every `scrollIntoView({ block: "nearest" })`
 *  — the active card on activation, the Needs-you jump, Tab focus — stops
 *  below the header instead of under it, and a little short of the bottom
 *  edge. The scroller belongs to the sidebar primitive, so the padding is set
 *  on it here rather than in its className.
 *
 *  While the list is scrolled, a short fade under the header lets text that
 *  passes beneath it dissolve instead of being sliced in half. */
export function SidebarStickyHeader({ children }: { children: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  const [scrolled, setScrolled] = useState(false);

  useLayoutEffect(() => {
    const header = ref.current;
    const scroller = header?.closest<HTMLElement>('[data-sidebar="content"]');
    if (!header || !scroller) return;

    const publish = () =>
      scroller.style.setProperty("--sidebar-sticky-h", `${header.offsetHeight}px`);
    const sync = () => setScrolled(scroller.scrollTop > 0);
    publish();
    sync();
    scroller.style.scrollPaddingTop = "calc(var(--sidebar-sticky-h) + 8px)";
    scroller.style.scrollPaddingBottom = "8px";

    const resizeObserver =
      typeof ResizeObserver === "undefined" ? null : new ResizeObserver(publish);
    resizeObserver?.observe(header);
    scroller.addEventListener("scroll", sync, { passive: true });
    return () => {
      resizeObserver?.disconnect();
      scroller.removeEventListener("scroll", sync);
      scroller.style.removeProperty("--sidebar-sticky-h");
      scroller.style.scrollPaddingTop = "";
      scroller.style.scrollPaddingBottom = "";
    };
  }, []);

  return (
    <div ref={ref} data-sidebar-sticky-header className="sticky top-0 z-10 bg-sidebar">
      {children}
      <ScrollEdgeFade
        edge="top"
        visible={scrolled}
        surface="var(--sidebar)"
        className="top-full h-3"
      />
    </div>
  );
}
