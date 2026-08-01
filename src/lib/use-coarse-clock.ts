import { useEffect, useReducer } from "react";

const TICK_MS = 30_000;

// One shared interval backs every subscriber so the sidebar never runs a
// per-row per-second timer — coarse (~30s) re-renders are enough to keep
// elapsed labels and the settled-✓ fade current.
const listeners = new Set<() => void>();
let timer: ReturnType<typeof setInterval> | null = null;

function subscribe(fn: () => void): () => void {
  listeners.add(fn);
  if (timer === null) {
    timer = setInterval(() => {
      for (const l of listeners) l();
    }, TICK_MS);
  }
  return () => {
    listeners.delete(fn);
    if (listeners.size === 0 && timer !== null) {
      clearInterval(timer);
      timer = null;
    }
  };
}

/** Returns a coarse (~30s) timestamp and forces a re-render when it advances,
 *  while `active` is true. When inactive it mounts no interval, so idle rows
 *  stay static.
 *
 *  The timestamp is held in state rather than read as `Date.now()` per render.
 *  A per-render read looks harmless but hands every consumer a fresh number on
 *  any unrelated re-render, so passing it down as a prop defeats every
 *  `React.memo` boundary underneath — which is the whole reason the sidebar
 *  shares one clock in the first place. Resolution is unchanged: the value is
 *  never more than one tick stale, which is what "coarse" means here. */
export function useCoarseClock(active: boolean): number {
  const [now, tick] = useReducer(() => Date.now(), 0, () => Date.now());
  useEffect(() => {
    if (!active) return;
    return subscribe(tick);
  }, [active]);
  return now;
}
