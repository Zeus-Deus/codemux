import { afterEach, describe, expect, it, vi } from "vitest";
import { scheduleSequentialIdlePrefetch } from "./idle-prefetch";

afterEach(() => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("scheduleSequentialIdlePrefetch", () => {
  it("waits for idle and loads chunks sequentially", async () => {
    let idleCallback: (() => void) | null = null;
    vi.stubGlobal(
      "requestIdleCallback",
      vi.fn((callback: () => void) => {
        idleCallback = callback;
        return 41;
      }),
    );
    vi.stubGlobal("cancelIdleCallback", vi.fn());

    let finishFirst!: () => void;
    const first = vi.fn(
      () => new Promise<void>((resolve) => {
        finishFirst = resolve;
      }),
    );
    const second = vi.fn(async () => {});

    scheduleSequentialIdlePrefetch([first, second]);
    expect(first).not.toHaveBeenCalled();

    expect(idleCallback).not.toBeNull();
    (idleCallback as unknown as () => void)();
    expect(first).toHaveBeenCalledTimes(1);
    expect(second).not.toHaveBeenCalled();

    finishFirst();
    await Promise.resolve();
    await Promise.resolve();
    expect(second).toHaveBeenCalledTimes(1);
  });

  it("cancels before the idle callback starts any import", () => {
    const cancelIdle = vi.fn();
    vi.stubGlobal("requestIdleCallback", vi.fn(() => 17));
    vi.stubGlobal("cancelIdleCallback", cancelIdle);
    const load = vi.fn(async () => {});

    const cancel = scheduleSequentialIdlePrefetch([load]);
    cancel();

    expect(cancelIdle).toHaveBeenCalledWith(17);
    expect(load).not.toHaveBeenCalled();
  });

  it("uses a cancellable timer fallback when idle callbacks are unavailable", () => {
    vi.useFakeTimers();
    vi.stubGlobal("requestIdleCallback", undefined);
    const load = vi.fn(async () => {});

    const cancel = scheduleSequentialIdlePrefetch([load]);
    vi.advanceTimersByTime(999);
    expect(load).not.toHaveBeenCalled();
    cancel();
    vi.runAllTimers();
    expect(load).not.toHaveBeenCalled();
  });
});
