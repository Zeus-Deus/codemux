import { afterEach, describe, expect, it, vi } from "vitest";
import { createWheelScrollAnimation, installWheelScrolling, isWebKitMouseWheel, tryAnimatedWheelScroll } from "./wheel-scrolling";

function harness(period = 5, quantized = false) {
  let now = 0;
  let position = 0;
  let maximum = 100_000;
  let id = 0;
  const pending = new Map<number, { at: number; callback: (time: number) => void }>();
  const writes: number[] = [];
  const animation = createWheelScrollAnimation({
    read: () => position,
    write: (next) => { position = quantized ? Math.floor(next) : next; writes.push(next); },
    maximum: () => maximum,
  }, {
    now: () => now,
    request: (callback) => { pending.set(++id, { at: now + period, callback }); return id; },
    cancel: (frame) => { pending.delete(frame); },
  });
  return {
    animation, writes,
    get position() { return position; },
    get pending() { return pending.size; },
    moveExternally(next: number) { position = next; },
    resize(next: number) { maximum = next; },
    advance(ms: number) {
      const until = now + ms;
      while (true) {
        const next = [...pending.entries()].sort((a, b) => a[1].at - b[1].at)[0];
        if (!next || next[1].at > until) break;
        now = next[1].at;
        pending.delete(next[0]);
        next[1].callback(now);
      }
      now = until;
    },
  };
}

describe("responsive wheel animation", () => {
  it.each([5, 16])("keeps up with rapid input at a %i ms frame interval and settles exactly", (period) => {
    const h = harness(period);
    for (let i = 0; i < 250; i++) {
      h.animation.scrollBy(25.4);
      h.advance(2);
    }
    expect(h.position).toBeGreaterThan(6350 * 0.85);
    h.advance(250);
    expect(h.position).toBeCloseTo(6350, 6);
    expect(h.pending).toBe(0);
  });

  it("coalesces input without creating one animation per event", () => {
    const h = harness();
    for (let i = 0; i < 1000; i++) h.animation.scrollBy(0.2);
    expect(h.pending).toBe(1);
    h.advance(250);
    expect(h.position).toBeCloseTo(200, 6);
  });

  it("retains subpixel input between gestures in a whole-pixel webview", () => {
    const h = harness(5, true);
    for (let i = 0; i < 4; i++) {
      h.animation.scrollBy(0.25);
      h.advance(250);
    }
    expect(h.position).toBe(1);
  });

  it("glides through an isolated tick instead of applying most of it in one frame", () => {
    const h = harness(16);
    h.animation.scrollBy(120);
    h.advance(16);
    expect(h.position).toBeGreaterThan(0);
    expect(h.position).toBeLessThan(12);
    h.advance(80);
    expect(h.position).toBeGreaterThan(40);
    expect(h.position).toBeLessThan(80);
    h.advance(120);
    expect(h.position).toBe(120);
    expect(h.pending).toBe(0);
  });

  it.each([25.4, -25.4])("preserves a small %s px tick without an initial push", (delta) => {
    const h = harness(5, true);
    h.moveExternally(5000);
    h.animation.scrollBy(delta);
    h.advance(10);
    const moved = Math.abs(h.position - 5000);
    expect(moved).toBeLessThanOrEqual(1);
    expect(moved).toBeLessThan(Math.abs(delta) * 0.2);
    expect(h.pending).toBe(1);
    h.advance(250);
    expect(Math.abs(h.position - (5000 + delta))).toBeLessThan(1);
    expect(h.pending).toBe(0);
  });

  it("scales isolated small movements in proportion to the input", () => {
    const tiny = harness();
    const larger = harness();
    tiny.animation.scrollBy(1.875);
    larger.animation.scrollBy(15);
    for (let i = 0; i < 40; i++) {
      tiny.advance(5);
      larger.advance(5);
      expect(larger.position).toBeCloseTo(tiny.position * 8, 8);
    }
    expect(tiny.position).toBe(1.875);
    expect(larger.position).toBe(15);
  });

  it("keeps fine wheel movement continuous between samples 80 ms apart", () => {
    const h = harness(5, true);
    for (let i = 0; i < 6; i++) {
      h.animation.scrollBy(15.5, "fine-wheel");
      h.advance(60);
      const before = h.position;
      h.advance(20);
      expect(h.position).toBeGreaterThan(before);
      expect(h.pending).toBe(1);
    }
    h.advance(250);
    expect(h.position).toBe(93);
    expect(h.pending).toBe(0);
  });

  it.each([5, 16])("preserves rapid fractional wheel distance at %i ms frames", (period) => {
    const h = harness(period, true);
    for (let i = 0; i < 64; i++) {
      h.animation.scrollBy(15.5, "fine-wheel");
      h.advance(5);
    }
    expect(h.position).toBeGreaterThan(992 * 0.9);
    h.advance(250);
    expect(h.position).toBe(992);
    expect(h.pending).toBe(0);
  });

  it("retains the short filter for continuous non-mouse input", () => {
    const h = harness();
    h.animation.scrollBy(5, "continuous");
    h.advance(30);
    expect(h.position).toBe(5);
    expect(h.pending).toBe(0);
  });

  it("does not stop and surge when fine samples alternate across 30 ms", () => {
    const h = harness();
    const events = new Set<number>();
    let end = 0;
    for (let i = 0; i < 40; i++) {
      if (i) end += i % 2 ? 29 : 31;
      events.add(end);
    }
    let previousPosition = 0;
    let previousSpeed = 0;
    let maximumSpeedChange = 0;
    for (let time = 0; time <= end + 250; time++) {
      if (events.has(time)) h.animation.scrollBy(15.5, "fine-wheel");
      if (time % 5 === 0) {
        const speed = (h.position - previousPosition) / 5;
        maximumSpeedChange = Math.max(maximumSpeedChange, Math.abs(speed - previousSpeed));
        previousPosition = h.position;
        previousSpeed = speed;
      }
      h.advance(1);
    }
    expect(maximumSpeedChange).toBeLessThan(0.1);
    expect(h.position).toBeCloseTo(620, 6);
    expect(h.pending).toBe(0);
  });

  it("does not accumulate a startup backlog and surge when fine input starts fast", () => {
    const h = harness();
    let previous = 0;
    let peakSpeed = 0;
    for (let i = 0; i < 64; i++) {
      h.animation.scrollBy(15.5, "fine-wheel");
      h.advance(5);
      peakSpeed = Math.max(peakSpeed, (h.position - previous) / 5);
      previous = h.position;
    }
    expect(peakSpeed).toBeLessThan(3.5);
    expect(h.position).toBeGreaterThan(992 * 0.9);
    h.advance(250);
    expect(h.position).toBe(992);
    const before = h.position;
    h.animation.scrollBy(15.5, "fine-wheel");
    h.advance(10);
    expect(h.position - before).toBeLessThan(1);
    h.advance(250);
    expect(h.position).toBe(1007.5);
  });

  it("preserves the moving glide's velocity when another tick arrives", () => {
    const h = harness(5);
    h.animation.scrollBy(120);
    h.advance(75);
    const previous = h.position;
    h.advance(5);
    const before = h.position;
    h.animation.scrollBy(25.4);
    h.advance(5);
    // Retargeting should continue the current speed rather than reseeding a
    // slow start or jumping to the speed of a fresh accumulated destination.
    const ratio = (h.position - before) / (before - previous);
    expect(ratio).toBeGreaterThan(0.8);
    expect(ratio).toBeLessThan(1.2);
    h.advance(250);
    expect(h.position).toBeCloseTo(145.4, 6);
  });

  it("keeps gliding between ordinary wheel ticks 150 ms apart", () => {
    const h = harness();
    for (let i = 0; i < 5; i++) {
      h.animation.scrollBy(25.4);
      h.advance(120);
      const before = h.position;
      h.advance(30);
      expect(h.position - before).toBeGreaterThan(1);
      expect(h.pending).toBe(1);
    }
    h.advance(250);
    expect(h.position).toBeCloseTo(127, 6);
  });

  it("retains velocity on reversal and reconciles the signed wheel distance", () => {
    const h = harness();
    h.animation.scrollBy(120);
    h.advance(100);
    const before = h.position;
    h.animation.scrollBy(-120);
    h.advance(5);
    expect(h.position).toBeGreaterThan(before);
    h.advance(250);
    expect(h.position).toBeCloseTo(0, 6);
    expect(h.pending).toBe(0);
  });

  it("yields to programmatic scrolling and starts the next gesture at its new position", () => {
    const h = harness();
    h.animation.scrollBy(1000);
    h.advance(5);
    h.moveExternally(5000);
    h.advance(250);
    expect(h.position).toBe(5000);
    expect(h.pending).toBe(0);
    h.animation.scrollBy(20);
    h.advance(250);
    expect(h.position).toBe(5020);
  });

  it("clamps the destination when content shrinks and passes input at a boundary", () => {
    const h = harness();
    h.animation.scrollBy(1000);
    h.resize(100);
    h.advance(250);
    expect(h.position).toBe(100);
    expect(h.animation.scrollBy(1)).toBe(false);
  });

  it("cancels pending movement without a late write", () => {
    const h = harness();
    h.animation.scrollBy(1000);
    h.animation.cancel();
    h.advance(250);
    expect(h.writes).toEqual([]);
    expect(h.animation.scrollBy(Infinity)).toBe(false);
  });
});

afterEach(() => vi.restoreAllMocks());

describe("WebKit mouse source recognition", () => {
  it.each([
    [15.5, -15, true], [-15.5, 15, true], [31, -30, true],
    [5, -15, false], [-5, 15, false], [5.2, -15, false],
    [40, -120, false], [0.2, 0, false], [5, 15, false],
    [15.5, -15.5, false], [15.5, NaN, false],
  ])("classifies delta=%s ticks=%s conservatively", (delta, ticks, expected) => {
    const event = new WheelEvent("wheel", { deltaY: delta });
    Object.defineProperty(event, "wheelDeltaY", { value: ticks });
    expect(isWebKitMouseWheel(event)).toBe(expected);
  });
  it("keeps missing legacy ticks and line-unit events unclassified", () => {
    expect(isWebKitMouseWheel(new WheelEvent("wheel", { deltaY: 15.5 }))).toBe(false);
    const event = new WheelEvent("wheel", { deltaY: 1, deltaMode: 1 });
    Object.defineProperty(event, "wheelDeltaY", { value: -120 });
    expect(isWebKitMouseWheel(event)).toBe(false);
  });
});

describe("wheel input ownership", () => {
  it("keeps coalesced full ticks on a fine mouse gesture's timing and resets after idle", () => {
    const element = document.createElement("div");
    document.body.append(element);
    Object.defineProperties(element, { scrollHeight: { value: 100_000 }, clientHeight: { value: 1000 } });
    let now = 0;
    let nextId = 0;
    const frames = new Map<number, FrameRequestCallback>();
    vi.spyOn(window.performance, "now").mockImplementation(() => now);
    vi.spyOn(window, "requestAnimationFrame").mockImplementation((callback) => {
      frames.set(++nextId, callback);
      return nextId;
    });
    vi.spyOn(window, "cancelAnimationFrame").mockImplementation((id) => { frames.delete(id); });
    const advance = (ms: number) => {
      for (let elapsed = 0; elapsed < ms; elapsed += 5) {
        now += 5;
        const callbacks = [...frames.values()];
        frames.clear();
        for (const callback of callbacks) callback(now);
      }
    };
    const wheel = (delta: number, ticks: number) => {
      const event = new WheelEvent("wheel", { deltaY: delta });
      Object.defineProperty(event, "wheelDeltaY", { value: ticks });
      return event;
    };
    const reference = harness();
    const stop = installWheelScrolling(document, () => true);
    try {
      for (let i = 0; i < 40; i++) {
        const full = i % 8 === 7;
        const delta = full ? 124 : 15.5;
        expect(tryAnimatedWheelScroll(element, delta, wheel(delta, full ? -120 : -15))).toBe(true);
        reference.animation.scrollBy(delta, "fine-wheel");
        advance(20);
        reference.advance(20);
        expect(element.scrollTop).toBeCloseTo(reference.position, 6);
      }
      advance(250);
      reference.advance(250);
      expect(element.scrollTop).toBeCloseTo(1162.5, 6);
      const start = element.scrollTop;
      const ordinary = harness();
      expect(tryAnimatedWheelScroll(element, 124, wheel(124, -120))).toBe(true);
      ordinary.animation.scrollBy(124);
      advance(20);
      ordinary.advance(20);
      expect(element.scrollTop - start).toBeCloseTo(ordinary.position, 6);
    } finally { stop(); element.remove(); }
  });

  it("keeps the parent's pending distance when a nested horizontal scroller cannot consume input", () => {
    const parent = document.createElement("div");
    const horizontal = document.createElement("div");
    parent.append(horizontal);
    document.body.append(parent);
    Object.defineProperties(parent, { scrollHeight: { value: 10_000 }, clientHeight: { value: 1000 } });
    Object.defineProperties(horizontal, { scrollHeight: { value: 100 }, clientHeight: { value: 100 } });
    const request = vi.spyOn(window, "requestAnimationFrame").mockReturnValue(1);
    vi.spyOn(window, "cancelAnimationFrame").mockImplementation(() => {});
    const stop = installWheelScrolling();
    try {
      expect(tryAnimatedWheelScroll(parent, 100)).toBe(true);
      expect(tryAnimatedWheelScroll(horizontal, 100)).toBe(false);
      expect(tryAnimatedWheelScroll(parent, 100)).toBe(true);
      expect(request).toHaveBeenCalledTimes(1);
      request.mock.calls[0][0](performance.now() + 250);
      expect(parent.scrollTop).toBe(200);
    } finally { stop(); parent.remove(); }
  });

  it("leaves untrusted DOM wheels on their original path", () => {
    const stop = installWheelScrolling();
    const event = new WheelEvent("wheel", { deltaY: 100, bubbles: true, cancelable: true });
    document.body.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(false);
    stop();
    expect(tryAnimatedWheelScroll(document.body, 100)).toBe(false);
  });

  it("does not animate forwarded rail input with reduced motion enabled", () => {
    vi.spyOn(window, "matchMedia").mockReturnValue({
      matches: true,
      addEventListener: vi.fn(), removeEventListener: vi.fn(),
    } as unknown as MediaQueryList);
    const stop = installWheelScrolling();
    expect(tryAnimatedWheelScroll(document.body, 100)).toBe(false);
    stop();
  });
});
