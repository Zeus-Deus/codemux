/**
 * Eased tail-follow for the transcript viewport.
 *
 * While the reader is pinned to the bottom of a live turn, new content should
 * slide up underneath them rather than arrive as hard per-frame steps. `follow`
 * starts (or retargets) one rAF loop that moves `scrollTop` a fixed fraction of
 * the remaining gap each frame, reading the target fresh every frame, so a
 * burst of deltas extends the same glide instead of restarting it. The loop
 * stops on its own once it lands, when the target reports `null` (the caller
 * no longer owns the viewport), or on `cancel` (a reader gesture).
 *
 * The step is frame-rate independent: at 60fps it closes ~25% of the gap per
 * frame, which settles a typical streamed line in roughly 150-250ms.
 */

/** Share of the remaining gap closed per 60fps frame. */
export const TAIL_GLIDE_RATE = 0.25;

const FRAME_MS = 1000 / 60;

/** Below this the glide snaps to the target and stops. */
const LAND_EPSILON_PX = 0.5;

export interface TailGlide {
  /** Glide toward `target()` (an absolute `scrollTop`), re-read each frame. */
  follow(target: () => number | null): void;
  cancel(): void;
  active(): boolean;
}

export function createTailGlide(
  getNode: () => HTMLElement | null | undefined,
): TailGlide {
  let frame: number | null = null;
  let target: (() => number | null) | null = null;
  let lastTime: number | null = null;

  const stop = () => {
    if (frame !== null) cancelAnimationFrame(frame);
    frame = null;
    target = null;
    lastTime = null;
  };

  const tick = (time: number) => {
    frame = null;
    const node = getNode();
    const goal = target?.();
    if (!node || goal == null) {
      stop();
      return;
    }
    const gap = goal - node.scrollTop;
    // Content shrank under the glide; the browser already clamped the offset.
    if (gap <= LAND_EPSILON_PX) {
      if (gap > 0) node.scrollTop = goal;
      stop();
      return;
    }
    const dt = lastTime === null ? FRAME_MS : Math.min(time - lastTime, 4 * FRAME_MS);
    lastTime = time;
    const share = 1 - Math.pow(1 - TAIL_GLIDE_RATE, dt / FRAME_MS);
    node.scrollTop += Math.min(gap, Math.max(1, gap * share));
    frame = requestAnimationFrame(tick);
  };

  return {
    follow(next) {
      target = next;
      if (frame === null) frame = requestAnimationFrame(tick);
    },
    cancel: stop,
    active: () => frame !== null,
  };
}
