import { useLayoutEffect, useState } from "react";

/**
 * Matches `SidebarProvider`'s own default (`useState(256)` in
 * `src/components/ui/sidebar.tsx`) so the very first paint — before this
 * hook has measured anything — lines up with the sidebar's actual initial
 * width instead of flashing a guess.
 */
const FALLBACK_WIDTH_PX = 256;

/** The layout box whose width always equals the sidebar's live, effective
 *  width: full width when expanded, `--sidebar-width-icon` in the collapsed
 *  icon rail, and the live drag value while `SidebarRail` is being dragged.
 *  See `Sidebar()` in `src/components/ui/sidebar.tsx`. On mobile, `Sidebar`
 *  swaps to a Radix `Sheet` instead and this node doesn't exist at all. */
const SIDEBAR_GAP_SELECTOR = '[data-slot="sidebar-gap"]';

/**
 * Tracks the app sidebar's live rendered width in pixels from *outside*
 * the `SidebarProvider` tree.
 *
 * `TitleBar` renders as a sibling above `SidebarProvider` in `AppShell`
 * (see `src/components/layout/app-shell.tsx`), not as a descendant, so it
 * can't read the provider's `--sidebar-width` / `--sidebar-width-icon` CSS
 * variables (custom properties only cascade to descendants) or call
 * `useSidebar()` (throws outside the provider). Rather than duplicating the
 * sidebar's width/collapse state in a second store, this hook measures the
 * one DOM node whose box already encodes the answer — the sidebar's
 * layout-reserving `sidebar-gap` div — via `ResizeObserver`, so it stays in
 * sync with expand/collapse, icon-rail mode, and live drag-resize without
 * touching `src/components/ui/sidebar.tsx`.
 *
 * A `MutationObserver` on `<body>` re-syncs the target node itself (not
 * just its size): `Sidebar()` unmounts `sidebar-gap` entirely rather than
 * resizing it when it swaps to the mobile `Sheet` variant, and a stale
 * `ResizeObserver` on a detached node would otherwise leave this hook
 * frozen at its last measurement instead of falling back cleanly.
 */
export function useSidebarGapWidth(): number {
  const [width, setWidth] = useState(FALLBACK_WIDTH_PX);

  useLayoutEffect(() => {
    if (typeof ResizeObserver === "undefined" || typeof MutationObserver === "undefined") {
      return;
    }

    let resizeObserver: ResizeObserver | null = null;
    let observedEl: HTMLElement | null = null;

    const detach = () => {
      resizeObserver?.disconnect();
      resizeObserver = null;
      observedEl = null;
    };

    const sync = () => {
      const el = document.querySelector<HTMLElement>(SIDEBAR_GAP_SELECTOR);
      if (el === observedEl) return;
      detach();
      if (!el) {
        // No sidebar rendered right now (e.g. mid-swap to the mobile
        // Sheet variant) — fall back rather than show a stale width.
        setWidth(FALLBACK_WIDTH_PX);
        return;
      }
      observedEl = el;
      setWidth(el.getBoundingClientRect().width);
      resizeObserver = new ResizeObserver((entries) => {
        const entry = entries[0];
        if (entry) setWidth(entry.contentRect.width);
      });
      resizeObserver.observe(el);
    };

    sync();

    const mutationObserver = new MutationObserver(sync);
    mutationObserver.observe(document.body, { childList: true, subtree: true });

    return () => {
      mutationObserver.disconnect();
      detach();
    };
  }, []);

  return width;
}
