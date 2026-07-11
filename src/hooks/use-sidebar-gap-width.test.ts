/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, expect, it } from "vitest";
import { act, cleanup, renderHook, waitFor } from "@testing-library/react";

import { useSidebarGapWidth } from "./use-sidebar-gap-width";

afterEach(() => {
  cleanup();
  document.body.innerHTML = "";
});

describe("useSidebarGapWidth", () => {
  it("falls back to SidebarProvider's own default width (256px) when no sidebar-gap node exists", () => {
    // No `[data-slot="sidebar-gap"]` in the tree — mirrors any render
    // where `TitleBar` mounts without (or before) the app's sidebar, e.g.
    // this exact unit test, or `title-bar.test.tsx`'s jsdom tree which
    // never mounts a real `SidebarProvider`.
    const { result } = renderHook(() => useSidebarGapWidth());
    expect(result.current).toBe(256);
  });

  it("measures the live sidebar-gap element on mount and tracks ResizeObserver updates", () => {
    const gap = document.createElement("div");
    gap.setAttribute("data-slot", "sidebar-gap");
    gap.getBoundingClientRect = () => ({ width: 300 }) as DOMRect;
    document.body.appendChild(gap);

    let observedCallback: ResizeObserverCallback | null = null;
    let observedTarget: Element | null = null;
    class FakeResizeObserver {
      constructor(cb: ResizeObserverCallback) {
        observedCallback = cb;
      }
      observe(el: Element) {
        observedTarget = el;
      }
      unobserve() {}
      disconnect() {}
    }
    const originalRO = globalThis.ResizeObserver;
    globalThis.ResizeObserver =
      FakeResizeObserver as unknown as typeof ResizeObserver;

    try {
      const { result } = renderHook(() => useSidebarGapWidth());
      // Initial synchronous measurement (useLayoutEffect) picks up the
      // real rendered box before paint, not the 256px fallback.
      expect(result.current).toBe(300);
      expect(observedTarget).toBe(gap);

      // Simulate the sidebar collapsing to its icon rail, or a drag-resize
      // — either way, the box's width changes and the observer fires.
      act(() => {
        observedCallback?.(
          [{ contentRect: { width: 52 } } as ResizeObserverEntry],
          {} as ResizeObserver,
        );
      });
      expect(result.current).toBe(52);
    } finally {
      globalThis.ResizeObserver = originalRO;
    }
  });

  it("falls back cleanly if the sidebar-gap node is later removed (e.g. a mobile Sheet swap)", async () => {
    const gap = document.createElement("div");
    gap.setAttribute("data-slot", "sidebar-gap");
    gap.getBoundingClientRect = () => ({ width: 300 }) as DOMRect;
    document.body.appendChild(gap);

    class FakeResizeObserver {
      observe() {}
      unobserve() {}
      disconnect() {}
    }
    const originalRO = globalThis.ResizeObserver;
    globalThis.ResizeObserver =
      FakeResizeObserver as unknown as typeof ResizeObserver;

    try {
      const { result } = renderHook(() => useSidebarGapWidth());
      expect(result.current).toBe(300);

      // `Sidebar()` unmounts `sidebar-gap` entirely (rather than resizing
      // it) when it swaps to the mobile Sheet variant — the MutationObserver
      // must notice the node is gone and reset to the fallback instead of
      // freezing on the last measurement.
      act(() => {
        gap.remove();
      });
      await waitFor(() => expect(result.current).toBe(256));
    } finally {
      globalThis.ResizeObserver = originalRO;
    }
  });
});
