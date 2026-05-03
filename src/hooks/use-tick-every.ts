// Force a component to re-render on a fixed cadence.
//
// Used by the Stage 5 sync status display to refresh the
// "Last synced N minutes ago" label without re-fetching the
// underlying timestamp. The sync timestamp itself is stable in
// state; only its rendered string needs to age.
//
// Caller picks the interval; 30 seconds is the right default for
// the relative-time labels (boundaries land at 60s/3600s/86400s,
// so 30s catches every transition with one rerender of slack).

import { useEffect, useState } from "react";

export function useTickEvery(intervalMs: number): void {
  const [, force] = useState(0);
  useEffect(() => {
    if (intervalMs <= 0) return;
    const id = window.setInterval(() => force((c) => c + 1), intervalMs);
    return () => window.clearInterval(id);
  }, [intervalMs]);
}
