import { useEffect, useState } from "react";

/**
 * Whole seconds since `startedAt`, for the "12s" pill beside a push/pull
 * spinner. Stays null for the first ~2s so quick operations show only the
 * spinner instead of a jittering counter, and null whenever nothing is in
 * flight. Ticks once a second only while an operation runs; idle renders
 * mount no interval.
 */
export function useElapsedSeconds(startedAt: number | null): number | null {
  const [elapsedSec, setElapsedSec] = useState<number | null>(null);
  useEffect(() => {
    if (startedAt === null) {
      setElapsedSec(null);
      return;
    }
    const tick = () => {
      const ms = Date.now() - startedAt;
      setElapsedSec(ms < 2_000 ? null : Math.floor(ms / 1_000));
    };
    tick();
    const id = window.setInterval(tick, 1_000);
    return () => window.clearInterval(id);
  }, [startedAt]);
  return elapsedSec;
}
