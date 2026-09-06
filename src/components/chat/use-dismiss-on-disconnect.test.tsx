import { renderHook } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { useDismissOnDisconnect } from "./use-dismiss-on-disconnect";

it("keeps an active dialog open through rerenders and dismisses only its latest committed owner", () => {
  const old = vi.fn();
  const current = vi.fn();
  const view = renderHook(({ open, change }) => useDismissOnDisconnect(open, change), {
    initialProps: { open: false, change: old },
  });
  view.rerender({ open: true, change: old });
  view.rerender({ open: true, change: current });
  expect(old).not.toHaveBeenCalled();
  expect(current).not.toHaveBeenCalled();
  view.unmount();
  expect(old).not.toHaveBeenCalled();
  expect(current).toHaveBeenCalledExactlyOnceWith(false);
});

it("does not notify an already closed dialog on disconnection", () => {
  const change = vi.fn();
  const view = renderHook(() => useDismissOnDisconnect(false, change));
  view.unmount();
  expect(change).not.toHaveBeenCalled();
});
