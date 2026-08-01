import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import {
  MAX_PARKED_TEARDOWNS,
  flushAllTeardowns,
  flushTeardown,
  parkTeardown,
  pendingTeardownSessionIds,
  setTeardownScheduler,
  type TeardownHandle,
} from "./deferred-teardown";

/**
 * The queue tests drive the rAF → rAF → idle chain through an injected
 * scheduler that captures callbacks, so a test decides exactly when a frame or
 * an idle slot happens. The default (feature-detected) scheduler gets its own
 * block at the bottom, driven through stubbed globals.
 */
function setup() {
  const frames = new Map<number, () => void>();
  const idles = new Map<number, () => void>();
  let nextHandle = 1;

  setTeardownScheduler({
    requestFrame: (cb) => {
      const handle = nextHandle++;
      frames.set(handle, cb);
      return handle;
    },
    cancelFrame: (handle: TeardownHandle) => {
      frames.delete(handle as number);
    },
    requestIdle: (cb) => {
      const handle = nextHandle++;
      idles.set(handle, cb);
      return handle;
    },
    cancelIdle: (handle: TeardownHandle) => {
      idles.delete(handle as number);
    },
  });

  const drain = (map: Map<number, () => void>) => {
    const pending = [...map.values()];
    map.clear();
    for (const cb of pending) cb();
  };

  return {
    frame: () => drain(frames),
    idle: () => drain(idles),
    pendingFrames: () => frames.size,
    pendingIdles: () => idles.size,
  };
}

describe("deferred-teardown", () => {
  beforeEach(() => {
    flushAllTeardowns();
  });

  afterEach(() => {
    flushAllTeardowns();
    setTeardownScheduler(null);
  });

  it("runs a parked job only after two frames and an idle slot", () => {
    const s = setup();
    const run = vi.fn();

    parkTeardown({ sessionId: "a", run });
    expect(run).not.toHaveBeenCalled();
    expect(pendingTeardownSessionIds()).toEqual(["a"]);

    // First frame: still inside the commit that is about to paint.
    s.frame();
    expect(run).not.toHaveBeenCalled();
    // Second frame: the incoming pane has painted; now we wait for idle.
    s.frame();
    expect(run).not.toHaveBeenCalled();
    expect(s.pendingIdles()).toBe(1);

    s.idle();
    expect(run).toHaveBeenCalledTimes(1);
    expect(pendingTeardownSessionIds()).toEqual([]);
  });

  it("flushTeardown runs the job now and cancels the pending idle slot", () => {
    const s = setup();
    const run = vi.fn();

    parkTeardown({ sessionId: "a", run });
    s.frame();
    s.frame();
    expect(s.pendingIdles()).toBe(1);

    flushTeardown("a");
    expect(run).toHaveBeenCalledTimes(1);
    expect(s.pendingIdles()).toBe(0);
    expect(pendingTeardownSessionIds()).toEqual([]);
  });

  it("runs a job exactly once when a flush races the idle chain", () => {
    const s = setup();
    const run = vi.fn();

    parkTeardown({ sessionId: "a", run });
    s.frame();
    s.frame();
    flushTeardown("a");
    // The idle callback would still fire if the scheduler ignored the cancel
    // (a real rIC cancel can lose the race); the job's own guard must hold.
    s.idle();
    flushTeardown("a");
    flushAllTeardowns();

    expect(run).toHaveBeenCalledTimes(1);
  });

  it("flushTeardown is a no-op for a session with nothing parked", () => {
    setup();
    const run = vi.fn();

    parkTeardown({ sessionId: "a", run });
    flushTeardown("b");

    expect(run).not.toHaveBeenCalled();
    expect(pendingTeardownSessionIds()).toEqual(["a"]);
  });

  it("parks at most two jobs, evicting the oldest synchronously", () => {
    const s = setup();
    const order: string[] = [];
    const jobs = ["a", "b", "c"].map((sessionId) => ({
      sessionId,
      run: vi.fn(() => order.push(sessionId)),
    }));

    parkTeardown(jobs[0]);
    parkTeardown(jobs[1]);
    expect(order).toEqual([]);
    expect(pendingTeardownSessionIds()).toEqual(["a", "b"]);

    // Third park: the oldest runs NOW so the number of live-but-abandoned
    // terminals stays bounded.
    parkTeardown(jobs[2]);
    expect(order).toEqual(["a"]);
    expect(pendingTeardownSessionIds()).toEqual(["b", "c"]);
    expect(pendingTeardownSessionIds().length).toBe(MAX_PARKED_TEARDOWNS);

    s.frame();
    s.frame();
    s.idle();
    expect(order).toEqual(["a", "b", "c"]);
  });

  it("finishes an earlier job for the same session before parking a new one", () => {
    setup();
    const first = vi.fn();
    const second = vi.fn();

    parkTeardown({ sessionId: "a", run: first });
    parkTeardown({ sessionId: "a", run: second });

    expect(first).toHaveBeenCalledTimes(1);
    expect(second).not.toHaveBeenCalled();
    expect(pendingTeardownSessionIds()).toEqual(["a"]);
  });

  it("flushAllTeardowns drains every job once, in park order", () => {
    setup();
    const order: string[] = [];
    parkTeardown({ sessionId: "a", run: () => order.push("a") });
    parkTeardown({ sessionId: "b", run: () => order.push("b") });

    flushAllTeardowns();
    flushAllTeardowns();

    expect(order).toEqual(["a", "b"]);
    expect(pendingTeardownSessionIds()).toEqual([]);
  });

  it("keeps draining when a job throws", () => {
    setup();
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    const after = vi.fn();

    parkTeardown({
      sessionId: "a",
      run: () => {
        throw new Error("dispose blew up");
      },
    });
    parkTeardown({ sessionId: "b", run: after });
    flushAllTeardowns();

    expect(after).toHaveBeenCalledTimes(1);
    expect(pendingTeardownSessionIds()).toEqual([]);
    expect(consoleError).toHaveBeenCalled();
    consoleError.mockRestore();
  });
});

describe("deferred-teardown default scheduler", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.resetModules();
    vi.useRealTimers();
  });

  it("uses requestIdleCallback with a timeout when it is available", async () => {
    const frameCbs: FrameRequestCallback[] = [];
    let idleCb: (() => void) | null = null;
    let idleOptions: IdleRequestOptions | undefined;

    vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => {
      frameCbs.push(cb);
      return frameCbs.length;
    });
    vi.stubGlobal("cancelAnimationFrame", () => {});
    vi.stubGlobal(
      "requestIdleCallback",
      (cb: () => void, options?: IdleRequestOptions) => {
        idleCb = cb;
        idleOptions = options;
        return 1;
      },
    );
    vi.stubGlobal("cancelIdleCallback", () => {});

    vi.resetModules();
    const mod = await import("./deferred-teardown");
    const run = vi.fn();
    mod.parkTeardown({ sessionId: "a", run });

    frameCbs.shift()?.(0);
    expect(idleCb).toBeNull();
    frameCbs.shift()?.(0);
    expect(idleOptions).toEqual({ timeout: 500 });

    expect(run).not.toHaveBeenCalled();
    (idleCb as unknown as () => void)();
    expect(run).toHaveBeenCalledTimes(1);
  });

  it("falls back to timers where rAF/rIC are unavailable", async () => {
    vi.stubGlobal("requestAnimationFrame", undefined);
    vi.stubGlobal("cancelAnimationFrame", undefined);
    vi.stubGlobal("requestIdleCallback", undefined);
    vi.stubGlobal("cancelIdleCallback", undefined);
    vi.useFakeTimers();

    vi.resetModules();
    const mod = await import("./deferred-teardown");
    const run = vi.fn();
    mod.parkTeardown({ sessionId: "a", run });

    // Three timer stages stand in for frame → frame → idle; each
    // `runOnlyPendingTimers` fires exactly the stage queued by the previous.
    expect(run).not.toHaveBeenCalled();
    await vi.runOnlyPendingTimersAsync();
    expect(run).not.toHaveBeenCalled();
    await vi.runOnlyPendingTimersAsync();
    expect(run).not.toHaveBeenCalled();
    await vi.runOnlyPendingTimersAsync();
    expect(run).toHaveBeenCalledTimes(1);
  });
});
