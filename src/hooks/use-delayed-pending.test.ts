import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, renderHook } from "@testing-library/react";

import { SPINNER_DELAY_MS } from "@/components/ui/spinner";
import { SPINNER_MIN_VISIBLE_MS, useDelayedPending } from "./use-delayed-pending";

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

const render = () =>
  renderHook(({ pending }) => useDelayedPending(pending), { initialProps: { pending: false } });

describe("useDelayedPending", () => {
  it("never shows for a wait shorter than the delay", () => {
    const { result, rerender } = render();
    rerender({ pending: true });
    act(() => vi.advanceTimersByTime(SPINNER_DELAY_MS - 1));
    rerender({ pending: false });
    act(() => vi.advanceTimersByTime(SPINNER_DELAY_MS));
    expect(result.current).toBe(false);
  });

  it("shows after the delay and holds for the minimum once shown", () => {
    const { result, rerender } = render();
    rerender({ pending: true });
    act(() => vi.advanceTimersByTime(SPINNER_DELAY_MS));
    expect(result.current).toBe(true);

    rerender({ pending: false });
    act(() => vi.advanceTimersByTime(SPINNER_MIN_VISIBLE_MS - 1));
    expect(result.current).toBe(true);
    act(() => vi.advanceTimersByTime(1));
    expect(result.current).toBe(false);
  });

  it("hides at once when the wait outlasted the minimum", () => {
    const { result, rerender } = render();
    rerender({ pending: true });
    act(() => vi.advanceTimersByTime(SPINNER_DELAY_MS + SPINNER_MIN_VISIBLE_MS));
    rerender({ pending: false });
    act(() => vi.advanceTimersByTime(0));
    expect(result.current).toBe(false);
  });

  it("stays shown when a new wait starts during the hold", () => {
    const { result, rerender } = render();
    rerender({ pending: true });
    act(() => vi.advanceTimersByTime(SPINNER_DELAY_MS));
    rerender({ pending: false });
    act(() => vi.advanceTimersByTime(SPINNER_MIN_VISIBLE_MS / 2));
    rerender({ pending: true });
    act(() => vi.advanceTimersByTime(SPINNER_MIN_VISIBLE_MS));
    expect(result.current).toBe(true);
  });
});
