import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { SidebarProvider, useSidebar } from "./sidebar";

const STORAGE_KEY = "codemux.sidebar.width";

let setWidth: ((width: number) => void) | null = null;

function WidthProbe() {
  const { sidebarWidth, setSidebarWidth } = useSidebar();
  setWidth = setSidebarWidth;
  return <span data-testid="width">{sidebarWidth}</span>;
}

function renderSidebar() {
  return render(
    <SidebarProvider>
      <WidthProbe />
    </SidebarProvider>,
  );
}

describe("SidebarProvider width", () => {
  beforeEach(() => {
    window.localStorage.removeItem(STORAGE_KEY);
    setWidth = null;
    // jsdom has no matchMedia; `useIsMobile` only needs a desktop answer.
    vi.stubGlobal(
      "matchMedia",
      vi.fn(() => ({
        matches: false,
        addEventListener: vi.fn(),
        removeEventListener: vi.fn(),
      })),
    );
  });

  afterEach(() => {
    cleanup();
    window.localStorage.removeItem(STORAGE_KEY);
    vi.unstubAllGlobals();
  });

  it("starts at the default width when nothing is stored", () => {
    renderSidebar();
    expect(screen.getByTestId("width").textContent).toBe("288px");
  });

  it("keeps a dragged width across a remount", () => {
    const first = renderSidebar();
    act(() => setWidth?.(340));
    expect(screen.getByTestId("width").textContent).toBe("340px");
    expect(window.localStorage.getItem(STORAGE_KEY)).toBe("340");
    first.unmount();

    renderSidebar();
    expect(screen.getByTestId("width").textContent).toBe("340px");
  });

  it("clamps stored widths and ignores garbage", () => {
    window.localStorage.setItem(STORAGE_KEY, "9000");
    const clamped = renderSidebar();
    expect(screen.getByTestId("width").textContent).toBe("400px");
    clamped.unmount();

    window.localStorage.setItem(STORAGE_KEY, "wide");
    renderSidebar();
    expect(screen.getByTestId("width").textContent).toBe("288px");
  });
});
