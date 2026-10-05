import { afterEach, describe, expect, it, vi } from "vitest";
import { createWheelScrollAnimation, installWheelScrolling, tryAnimatedWheelScroll } from "./wheel-scrolling";

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

describe("wheel input ownership", () => {
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
