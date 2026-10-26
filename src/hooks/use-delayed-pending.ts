import { useEffect, useRef, useState } from "react";

import { SPINNER_DELAY_MS } from "@/components/ui/spinner";

/** Once shown, a spinner stays at least this long so a wait that ends just
 *  after the delay does not blink it on and straight off again. */
export const SPINNER_MIN_VISIBLE_MS = 400;

/** Whether to show a spinner for `pending`: only once it has lasted `delay`,
 *  then for at least `minVisible`. For spinners that replace a label or
 *  icon, where a quick wait should leave the original in place. Render the
 *  spinner with `immediate`, since this already waited. */
export function useDelayedPending(
  pending: boolean,
  { delay = SPINNER_DELAY_MS, minVisible = SPINNER_MIN_VISIBLE_MS } = {},
): boolean {
  const [shown, setShown] = useState(false);
  const shownAt = useRef(0);

  useEffect(() => {
    if (pending === shown) return;
    const wait = pending ? delay : Math.max(0, minVisible - (Date.now() - shownAt.current));
    const timer = setTimeout(() => {
      if (pending) shownAt.current = Date.now();
      setShown(pending);
    }, wait);
    return () => clearTimeout(timer);
  }, [pending, shown, delay, minVisible]);

  return shown;
}
