import { useEffect, useLayoutEffect, useRef } from "react";

/** Wall-clock cadence for elapsed-time labels. */
const DEFAULT_INTERVAL_MS = 1000;

/**
 * A text span that re-derives itself on an interval by writing
 * `textContent` straight to its own DOM node.
 *
 * Elapsed-time labels ("2m 41s") used to be driven by a 1 Hz
 * `setNow(Date.now())` in the owning card. That is a React state update, so
 * every second the whole card — and, for the docked activity bar, the whole
 * pane subtree below it — re-entered the commit phase to change a handful of
 * characters. Here the tick never reaches React at all: only this node's
 * text node changes, and the memoized transcript rows around it are
 * untouched.
 *
 * `compute` is read through a ref and re-run on every commit, so a
 * prop-driven change (a new tool count, a settled duration) still lands with
 * the render that carried it.
 *
 * Only use this for output that is a pure function of `now` plus already
 * rendered props. Anything the ticking value FEEDS — layout, branching,
 * anything React must see — has to stay on a real React tick.
 */
export function TickingText({
  compute,
  active = true,
  intervalMs = DEFAULT_INTERVAL_MS,
  className,
  testId,
}: {
  /** Text for a given wall-clock instant. */
  compute: (now: number) => string;
  /** `false` freezes the label at its last painted value (nothing running). */
  active?: boolean;
  intervalMs?: number;
  className?: string;
  testId?: string;
}) {
  const nodeRef = useRef<HTMLSpanElement | null>(null);
  const computeRef = useRef(compute);
  const paintRef = useRef<() => void>(() => undefined);

  // Layout effect, no dep array: paint before the browser sees the frame so a
  // commit never flashes stale text, and re-bind `compute` each render so the
  // interval below always calls the current closure.
  useLayoutEffect(() => {
    computeRef.current = compute;
    paintRef.current = () => {
      const node = nodeRef.current;
      if (!node) return;
      const next = computeRef.current(Date.now());
      if (node.textContent !== next) node.textContent = next;
    };
    paintRef.current();
  });

  useEffect(() => {
    if (!active) return;
    const id = window.setInterval(() => paintRef.current(), intervalMs);
    return () => window.clearInterval(id);
  }, [active, intervalMs]);

  // Rendered empty: the text is owned imperatively by the effects above, so
  // React must never claim knowledge of these children.
  return <span ref={nodeRef} className={className} data-testid={testId} />;
}
