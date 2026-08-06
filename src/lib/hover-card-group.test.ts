import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import {
  __resetHoverCardGroupForTests,
  isHoverCardGroupActive,
  registerOpenHoverCard,
} from "./hover-card-group";

const GROUP_TIMEOUT_MS = 400;

beforeEach(() => {
  vi.useFakeTimers();
  __resetHoverCardGroupForTests();
});

afterEach(() => {
  __resetHoverCardGroupForTests();
  vi.useRealTimers();
});

describe("hover card group phase", () => {
  it("is inactive until a card actually opens", () => {
    expect(isHoverCardGroupActive()).toBe(false);
    registerOpenHoverCard(() => {});
    expect(isHoverCardGroupActive()).toBe(true);
  });

  it("outlives a closed card by the grace window, so crossing a gap between two rows still opens instantly", () => {
    const release = registerOpenHoverCard(() => {});
    release();

    vi.advanceTimersByTime(GROUP_TIMEOUT_MS - 1);
    expect(isHoverCardGroupActive()).toBe(true);

    vi.advanceTimersByTime(1);
    expect(isHoverCardGroupActive()).toBe(false);
  });

  it("does not expire while another card is still open", () => {
    const releaseA = registerOpenHoverCard(() => {});
    registerOpenHoverCard(() => {});
    releaseA();

    vi.advanceTimersByTime(GROUP_TIMEOUT_MS * 3);
    expect(isHoverCardGroupActive()).toBe(true);
  });

  it("restarts the grace window on each fresh open", () => {
    registerOpenHoverCard(() => {})();
    vi.advanceTimersByTime(GROUP_TIMEOUT_MS - 50);

    registerOpenHoverCard(() => {})();
    vi.advanceTimersByTime(GROUP_TIMEOUT_MS - 50);
    expect(isHoverCardGroupActive()).toBe(true);

    vi.advanceTimersByTime(50);
    expect(isHoverCardGroupActive()).toBe(false);
  });

  it("supersedes the previous card when a new one opens, so the old one starts leaving in the same frame", () => {
    const closeA = vi.fn();
    registerOpenHoverCard(closeA);
    expect(closeA).not.toHaveBeenCalled();

    registerOpenHoverCard(vi.fn());
    expect(closeA).toHaveBeenCalledTimes(1);
  });

  it("keeps the phase active across a supersede hand-off", () => {
    // The superseded card releases from inside its own close, which must not
    // momentarily drop the group to zero and schedule an expiry mid-sweep.
    let releaseA: (() => void) | null = null;
    releaseA = registerOpenHoverCard(() => releaseA?.());
    registerOpenHoverCard(() => {});

    vi.advanceTimersByTime(GROUP_TIMEOUT_MS * 2);
    expect(isHoverCardGroupActive()).toBe(true);
  });

  it("does not resurrect an already-superseded card", () => {
    const closeA = vi.fn();
    registerOpenHoverCard(closeA);
    const releaseB = registerOpenHoverCard(vi.fn());
    releaseB();

    registerOpenHoverCard(vi.fn());
    expect(closeA).toHaveBeenCalledTimes(1);
  });

  it("ignores a repeated release, so an unmount after a close cannot unbalance the count", () => {
    const release = registerOpenHoverCard(() => {});
    registerOpenHoverCard(() => {});
    release();
    release();

    vi.advanceTimersByTime(GROUP_TIMEOUT_MS * 3);
    expect(isHoverCardGroupActive()).toBe(true);
  });
});
