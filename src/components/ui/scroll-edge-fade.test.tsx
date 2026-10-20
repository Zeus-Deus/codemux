/// <reference types="@testing-library/jest-dom/vitest" />
import { useRef } from "react";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, it, vi } from "vitest";

import { ScrollEdgeFade, useScrollEdges } from "./scroll-edge-fade";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

function Example({ open = true }: { open?: boolean }) {
  const ref = useRef<HTMLDivElement>(null);
  const edges = useScrollEdges(ref, open);
  return open ? (
    <div>
      <div ref={ref} data-testid="viewport"><div data-testid="content">Rows</div></div>
      <ScrollEdgeFade edge="top" visible={edges.top} data-testid="top" />
      <ScrollEdgeFade edge="bottom" visible={edges.bottom} surface="rgb(20, 30, 40)" data-testid="bottom" />
    </div>
  ) : null;
}

it("shows only edges with more content, including on a delayed mount", () => {
  vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(100);
  vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockReturnValue(300);
  const { rerender } = render(<Example open={false} />);
  rerender(<Example />);
  const viewport = screen.getByTestId("viewport");
  expect(screen.getByTestId("top")).toHaveAttribute("data-visible", "false");
  expect(screen.getByTestId("bottom")).toHaveAttribute("data-visible", "true");
  viewport.scrollTop = 100;
  fireEvent.scroll(viewport);
  expect(screen.getByTestId("top")).toHaveAttribute("data-visible", "true");
  expect(screen.getByTestId("bottom")).toHaveAttribute("data-visible", "true");
  viewport.scrollTop = 200;
  fireEvent.scroll(viewport);
  expect(screen.getByTestId("bottom")).toHaveAttribute("data-visible", "false");
  expect(screen.getByTestId("bottom")).toHaveAttribute("aria-hidden", "true");
  expect(screen.getByTestId("bottom")).toHaveClass("pointer-events-none", "motion-reduce:transition-none");
  expect(screen.getByTestId("bottom").style.backgroundImage).toContain("rgb(20, 30, 40)");
  expect(screen.getByTestId("bottom").style.maskImage).toBe("");
});

it("updates when content is filtered or resized, and disconnects on unmount", async () => {
  vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(100);
  const height = vi.spyOn(HTMLElement.prototype, "scrollHeight", "get").mockReturnValue(300);
  let resize = () => {};
  const observe = vi.fn();
  const disconnect = vi.fn();
  vi.stubGlobal("ResizeObserver", class {
    constructor(callback: () => void) { resize = callback; }
    observe = observe;
    disconnect = disconnect;
  });
  const { unmount } = render(<Example />);
  expect(observe).toHaveBeenCalledWith(screen.getByTestId("viewport"));
  expect(observe).toHaveBeenCalledWith(screen.getByTestId("content"));
  height.mockReturnValue(100);
  screen.getByTestId("content").hidden = true;
  await waitFor(() => expect(screen.getByTestId("bottom")).toHaveAttribute("data-visible", "false"));
  height.mockReturnValue(300);
  act(() => resize());
  expect(screen.getByTestId("bottom")).toHaveAttribute("data-visible", "true");
  disconnect.mockClear();
  unmount();
  expect(disconnect).toHaveBeenCalledOnce();
});
