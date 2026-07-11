import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import {
  createIdleScrollbackSerializer,
  type IdleHandle,
} from "./scrollback-idle-serializer";

/**
 * These tests drive the serializer deterministically with:
 *  - an injected `now()` backed by a manual `clock`,
 *  - vitest fake timers for the settle-check `setTimeout`, and
 *  - an injected `requestIdle`/`cancelIdle` that capture the idle callback so a
 *    test flushes it explicitly.
 *
 * `advance(ms)` bumps the manual clock AND the fake-timer clock in lockstep, so
 * a settle timer that fires during the advance sees a consistent `now()`.
 */
const SETTLE_MS = 1000;
const MIN_INTERVAL_MS = 5000;

function setup(overrides: { settleMs?: number; minIntervalMs?: number } = {}) {
  let clock = 0;
  let alt = false;
  let enabled = true;
  let pendingIdle: ((didTimeout: boolean) => void) | null = null;
  let idleHandles = 0;

  // Param typed as the source's IdleHandle (number | ReturnType<typeof
  // setTimeout>) rather than the narrower `number`: the injected scheduler
  // must satisfy `(handle: IdleHandle) => void`, and in a Node type
  // environment `ReturnType<typeof setTimeout>` widens IdleHandle beyond
  // `number`, so a `number`-only param is not assignable there.
  const cancelIdle = vi.fn((_handle: IdleHandle) => {
    pendingIdle = null;
  });
  const requestIdle = vi.fn((cb: (didTimeout: boolean) => void) => {
    pendingIdle = cb;
    return ++idleHandles;
  });
  // Each successful serialize returns a distinct string so tests can tell a
  // fresh serialization from a stale cached one.
  const serialize = vi.fn(() => `DATA${serialize.mock.calls.length}`);

  const s = createIdleScrollbackSerializer({
    serialize,
    isAlternateBuffer: () => alt,
    isEnabled: () => enabled,
    settleMs: overrides.settleMs ?? SETTLE_MS,
    minIntervalMs: overrides.minIntervalMs ?? MIN_INTERVAL_MS,
    now: () => clock,
    requestIdle,
    cancelIdle,
  });

  return {
    s,
    serialize,
    requestIdle,
    cancelIdle,
    setAlt: (v: boolean) => {
      alt = v;
    },
    setEnabled: (v: boolean) => {
      enabled = v;
    },
    hasIdle: () => pendingIdle !== null,
    // `didTimeout=true` simulates an idle slot granted only because the
    // scheduler's timeout elapsed (busy main thread).
    flushIdle: (didTimeout = false) => {
      const cb = pendingIdle;
      pendingIdle = null;
      if (cb) cb(didTimeout);
    },
    advance: async (ms: number) => {
      clock += ms;
      await vi.advanceTimersByTimeAsync(ms);
    },
  };
}

describe("scrollback-idle-serializer", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => {
    vi.clearAllTimers();
    vi.useRealTimers();
  });

  it("does not serialize while output keeps arriving (chunks < settleMs apart)", async () => {
    const h = setup();

    // A steady drip of output, each chunk half a settle window apart.
    h.s.notifyOutput();
    for (let i = 0; i < 6; i++) {
      await h.advance(SETTLE_MS / 2);
      h.s.notifyOutput();
    }
    // The settle timer keeps re-arming for the remaining quiet window and never
    // reaches the idle stage.
    expect(h.requestIdle).not.toHaveBeenCalled();
    expect(h.hasIdle()).toBe(false);
    expect(h.serialize).not.toHaveBeenCalled();
    expect(h.s.getFreshData()).toBeNull();
  });

  it("serializes once after output settles and getFreshData returns the data", async () => {
    const h = setup();

    h.s.notifyOutput();
    // Before settling: nothing serialized, no fresh data.
    expect(h.s.getFreshData()).toBeNull();

    await h.advance(SETTLE_MS); // settle timer fires → schedules idle
    expect(h.requestIdle).toHaveBeenCalledTimes(1);
    // Idle not yet run → still dirty.
    expect(h.s.getFreshData()).toBeNull();

    h.flushIdle(); // idle callback runs the serialize
    expect(h.serialize).toHaveBeenCalledTimes(1);
    expect(h.s.getFreshData()).toEqual({ data: "DATA1" });
  });

  it("notifyOutput after a serialize invalidates the cache and re-serializes, respecting minIntervalMs", async () => {
    const h = setup();

    // First serialize at t = SETTLE_MS.
    h.s.notifyOutput();
    await h.advance(SETTLE_MS);
    h.flushIdle();
    expect(h.serialize).toHaveBeenCalledTimes(1);
    expect(h.s.getFreshData()).toEqual({ data: "DATA1" });

    // New output invalidates the cache immediately.
    h.s.notifyOutput();
    expect(h.s.getFreshData()).toBeNull();

    // Output settles, but we're still inside minIntervalMs of the last
    // serialize → the settle timer re-arms instead of scheduling idle.
    await h.advance(SETTLE_MS);
    expect(h.requestIdle).toHaveBeenCalledTimes(1); // no new idle yet

    // Advance past the remaining min-interval; now it's allowed.
    await h.advance(MIN_INTERVAL_MS - SETTLE_MS);
    expect(h.requestIdle).toHaveBeenCalledTimes(2);
    h.flushIdle();
    expect(h.serialize).toHaveBeenCalledTimes(2);
    expect(h.s.getFreshData()).toEqual({ data: "DATA2" });
  });

  it("notifyResize invalidates the cache", async () => {
    const h = setup();

    h.s.notifyOutput();
    await h.advance(SETTLE_MS);
    h.flushIdle();
    expect(h.s.getFreshData()).toEqual({ data: "DATA1" });

    h.s.notifyResize();
    expect(h.s.getFreshData()).toBeNull();

    // A resize past the min-interval re-serializes after settling.
    await h.advance(MIN_INTERVAL_MS);
    h.flushIdle();
    expect(h.serialize).toHaveBeenCalledTimes(2);
    expect(h.s.getFreshData()).toEqual({ data: "DATA2" });
  });

  it("skips serialize on the alt screen at idle time (stays dirty), then serializes after leaving alt + new output", async () => {
    const h = setup();
    h.setAlt(true);

    h.s.notifyOutput();
    await h.advance(SETTLE_MS);
    expect(h.requestIdle).toHaveBeenCalledTimes(1);
    h.flushIdle(); // idle runs but sees alt screen → no serialize
    expect(h.serialize).not.toHaveBeenCalled();
    expect(h.s.getFreshData()).toBeNull(); // still dirty

    // Leaving the alt screen emits output on the primary buffer.
    h.setAlt(false);
    h.s.notifyOutput();
    await h.advance(SETTLE_MS);
    h.flushIdle();
    expect(h.serialize).toHaveBeenCalledTimes(1);
    expect(h.s.getFreshData()).toEqual({ data: "DATA1" });
  });

  it("getFreshData returns null when isAlternateBuffer() is true even with a clean cache", async () => {
    const h = setup();

    h.s.notifyOutput();
    await h.advance(SETTLE_MS);
    h.flushIdle();
    expect(h.s.getFreshData()).toEqual({ data: "DATA1" });

    // Cache is clean, but we're now on the alt screen — must not hand back the
    // primary-screen cache (an alt-screen unmount persists with alt semantics).
    h.setAlt(true);
    expect(h.s.getFreshData()).toBeNull();

    // Back on the primary screen, the still-clean cache is usable again.
    h.setAlt(false);
    expect(h.s.getFreshData()).toEqual({ data: "DATA1" });
  });

  it("stays dirty when serialize throws and retries on a later settle (does not wedge)", async () => {
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});
    const h = setup();
    // Throw on the first call, succeed on the second.
    h.serialize.mockImplementationOnce(() => {
      throw new Error("boom");
    });

    h.s.notifyOutput();
    await h.advance(SETTLE_MS);
    h.flushIdle();
    expect(h.serialize).toHaveBeenCalledTimes(1);
    expect(errorSpy).toHaveBeenCalledTimes(1);
    expect(h.s.getFreshData()).toBeNull(); // stayed dirty

    // The scheduler is not wedged: new output arms a fresh settle and retries.
    h.s.notifyOutput();
    await h.advance(SETTLE_MS);
    h.flushIdle();
    expect(h.serialize).toHaveBeenCalledTimes(2);
    expect(h.s.getFreshData()).toEqual({ data: "DATA2" });

    errorSpy.mockRestore();
  });

  it("skips serialize when isEnabled() is false at idle time, without rescheduling; re-enable + output self-heals", async () => {
    const h = setup();
    h.setEnabled(false);

    h.s.notifyOutput();
    await h.advance(SETTLE_MS);
    expect(h.requestIdle).toHaveBeenCalledTimes(1);
    h.flushIdle(); // idle runs but session restore is off → skip entirely
    expect(h.serialize).not.toHaveBeenCalled();
    expect(h.s.getFreshData()).toBeNull(); // stayed dirty

    // No reschedule while disabled: no settle timer was re-armed, and time
    // passing schedules nothing new (zero timer churn for opt-outs).
    expect(vi.getTimerCount()).toBe(0);
    await h.advance(MIN_INTERVAL_MS);
    expect(h.requestIdle).toHaveBeenCalledTimes(1);

    // Re-enabling self-heals: the next output re-arms and serializes.
    h.setEnabled(true);
    h.s.notifyOutput();
    await h.advance(SETTLE_MS);
    h.flushIdle();
    expect(h.serialize).toHaveBeenCalledTimes(1);
    expect(h.s.getFreshData()).toEqual({ data: "DATA1" });
  });

  it("defers a didTimeout idle slot at most twice, then serializes on the third", async () => {
    const h = setup();

    h.s.notifyOutput();
    await h.advance(SETTLE_MS);
    expect(h.requestIdle).toHaveBeenCalledTimes(1);

    // 1st timed-out slot → defer: no serialize, settle timer re-armed.
    h.flushIdle(true);
    expect(h.serialize).not.toHaveBeenCalled();
    expect(vi.getTimerCount()).toBe(1);
    await h.advance(SETTLE_MS);
    expect(h.requestIdle).toHaveBeenCalledTimes(2);

    // 2nd timed-out slot → defer again.
    h.flushIdle(true);
    expect(h.serialize).not.toHaveBeenCalled();
    await h.advance(SETTLE_MS);
    expect(h.requestIdle).toHaveBeenCalledTimes(3);

    // 3rd timed-out slot → cap reached: take the bounded hit and serialize
    // despite the contention (unconditional deferral would starve under
    // sustained load and push the cost back onto the switch path).
    h.flushIdle(true);
    expect(h.serialize).toHaveBeenCalledTimes(1);
    expect(h.s.getFreshData()).toEqual({ data: "DATA1" });
  });

  it("a genuine (non-timeout) idle slot resets the defer counter", async () => {
    const h = setup();

    // Burn one deferral, then serialize via a genuine idle slot.
    h.s.notifyOutput();
    await h.advance(SETTLE_MS);
    h.flushIdle(true); // defer #1
    await h.advance(SETTLE_MS);
    h.flushIdle(); // genuine slot → serializes, counter resets
    expect(h.serialize).toHaveBeenCalledTimes(1);

    // A later cycle gets its full two deferrals again.
    h.s.notifyOutput();
    await h.advance(MIN_INTERVAL_MS);
    h.flushIdle(true); // defer #1
    expect(h.serialize).toHaveBeenCalledTimes(1);
    await h.advance(SETTLE_MS);
    h.flushIdle(true); // defer #2
    expect(h.serialize).toHaveBeenCalledTimes(1);
    await h.advance(SETTLE_MS);
    h.flushIdle(true); // cap reached → serialize
    expect(h.serialize).toHaveBeenCalledTimes(2);
  });

  it("notifyOutput does not stack settle timers (only one pending at a time)", () => {
    const h = setup();

    h.s.notifyOutput();
    h.s.notifyOutput();
    h.s.notifyOutput();
    h.s.notifyOutput();
    // Exactly one settle-check timer is armed regardless of chunk count.
    expect(vi.getTimerCount()).toBe(1);
  });

  it("dispose cancels a pending settle timer and idle callback (idempotent)", async () => {
    const h = setup();

    // Case 1: dispose while a settle timer is pending.
    h.s.notifyOutput();
    expect(vi.getTimerCount()).toBe(1);
    h.s.dispose();
    expect(vi.getTimerCount()).toBe(0);
    await h.advance(SETTLE_MS);
    expect(h.serialize).not.toHaveBeenCalled();

    // Case 2: dispose while an idle callback is pending.
    const h2 = setup();
    h2.s.notifyOutput();
    await h2.advance(SETTLE_MS);
    expect(h2.hasIdle()).toBe(true);
    h2.s.dispose();
    expect(h2.cancelIdle).toHaveBeenCalledTimes(1);
    h2.flushIdle(); // no-op: cancelIdle cleared it
    expect(h2.serialize).not.toHaveBeenCalled();

    // Idempotent.
    expect(() => {
      h2.s.dispose();
      h2.s.dispose();
    }).not.toThrow();
  });

  it("works via the requestIdleCallback→setTimeout fallback when requestIdle isn't injected", async () => {
    // No requestIdle/cancelIdle injected → the module's default scheduler is
    // used. jsdom lacks requestIdleCallback, so it falls back to
    // setTimeout(fn, 200), which fake timers control. `now()` is kept in sync
    // with the fake-timer clock via `clock`.
    let clock = 0;
    let alt = false;
    const serialize = vi.fn(() => "FALLBACK");
    const s = createIdleScrollbackSerializer({
      serialize,
      isAlternateBuffer: () => alt,
      settleMs: SETTLE_MS,
      minIntervalMs: MIN_INTERVAL_MS,
      now: () => clock,
    });
    const advance = async (ms: number) => {
      clock += ms;
      await vi.advanceTimersByTimeAsync(ms);
    };

    s.notifyOutput();
    await advance(SETTLE_MS); // settle timer fires → schedules the fallback idle
    expect(serialize).not.toHaveBeenCalled(); // idle setTimeout not yet fired
    await advance(200); // the 200ms setTimeout fallback fires → serialize
    expect(serialize).toHaveBeenCalledTimes(1);
    expect(s.getFreshData()).toEqual({ data: "FALLBACK" });

    s.dispose();
    void alt; // silence unused-var lint; kept for parity with setup()
  });
});
