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

/** Returns `Date.now()` and forces a coarse (~30s) re-render while `active`
 *  is true. When inactive it mounts no interval, so idle rows stay static. */
export function useCoarseClock(active: boolean): number {
  const [, tick] = useReducer((n: number) => n + 1, 0);
  useEffect(() => {
    if (!active) return;
    return subscribe(tick);
  }, [active]);
  return Date.now();
}
