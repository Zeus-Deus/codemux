import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { createTailGlide } from "./tail-glide";

let frames: Array<FrameRequestCallback | null> = [];
let now = 0;
function flushFrame() {
  now += 1000 / 60;
  const pending = frames;
  frames = [];
  for (const cb of pending) cb?.(now);
}

beforeEach(() => {
  frames = [];
  now = 0;
  vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => frames.push(cb));
  vi.stubGlobal("cancelAnimationFrame", (id: number) => {
    frames[id - 1] = null;
  });
});
afterEach(() => vi.unstubAllGlobals());

function viewport(scrollTop = 0) {
  return { scrollTop } as HTMLElement;
}

describe("createTailGlide", () => {
  it("eases toward the target over several frames and lands on it", () => {
    const node = viewport();
    const glide = createTailGlide(() => node);
    glide.follow(() => 200);
    flushFrame();
    // One frame closes a share of the gap, not all of it.
    expect(node.scrollTop).toBeGreaterThan(0);
    expect(node.scrollTop).toBeLessThan(100);
    for (let i = 0; i < 40 && glide.active(); i++) flushFrame();
    expect(glide.active()).toBe(false);
    expect(node.scrollTop).toBe(200);
  });

  it("retargets an active glide instead of restarting it", () => {
    const node = viewport();
    let target = 100;
    const glide = createTailGlide(() => node);
    glide.follow(() => target);
    flushFrame();
    target = 300;
    glide.follow(() => target);
    expect(frames.filter(Boolean)).toHaveLength(1);
    for (let i = 0; i < 60 && glide.active(); i++) flushFrame();
    expect(node.scrollTop).toBe(300);
  });

  it("stops when the target is withdrawn or the glide is canceled", () => {
    const node = viewport();
    let owned = true;
    const glide = createTailGlide(() => node);
    glide.follow(() => (owned ? 500 : null));
    flushFrame();
    owned = false;
    const reached = node.scrollTop;
    flushFrame();
    expect(glide.active()).toBe(false);
    expect(node.scrollTop).toBe(reached);

    glide.follow(() => 500);
    glide.cancel();
    flushFrame();
    expect(node.scrollTop).toBe(reached);
  });

  it("stops without moving when content shrank below the offset", () => {
    const node = viewport(400);
    const glide = createTailGlide(() => node);
    glide.follow(() => 300);
    flushFrame();
    expect(glide.active()).toBe(false);
    expect(node.scrollTop).toBe(400);
  });
});
