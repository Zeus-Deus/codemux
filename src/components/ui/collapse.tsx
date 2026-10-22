import {
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";

import { cn } from "@/lib/utils";

/** Slightly past the `base` duration: settles the phase when no
 *  `transitionend` arrives (reduced motion, a hidden tab, jsdom). */
const SETTLE_FALLBACK_MS = 250;

type Phase = "closed" | "opening" | "open" | "closing";

/**
 * Height disclosure: eases a `0fr → 1fr` grid row with a fade, so nothing has
 * to be measured and the content below glides instead of jumping a whole
 * body height in one frame. Opening takes `base` on the drawer curve; closing
 * takes `quick`. Content is clipped only while it moves, so focus rings and
 * hover fills can overhang once it has settled open.
 *
 * Children mount on the first open. By default they unmount again once the
 * close has played, which suits long lists (a folded sidebar project).
 * `keepMounted` keeps them after the first open instead — right for the
 * dense, mostly collapsed transcript, where re-mounting a heavy tool body on
 * every toggle costs more than holding it. Closed content is `inert`, so its
 * links and buttons leave the tab order.
 */
export function Collapse({
  open,
  id,
  keepMounted = false,
  className,
  children,
}: {
  open: boolean;
  id?: string;
  keepMounted?: boolean;
  className?: string;
  children: ReactNode;
}) {
  const [mounted, setMounted] = useState(open);
  // Lags `open` by one layout pass on the way in, so a freshly mounted body
  // starts from the collapsed row and transitions open.
  const [phase, setPhase] = useState<Phase>(open ? "open" : "closed");
  const ref = useRef<HTMLDivElement>(null);

  useLayoutEffect(() => {
    if (open && !mounted) {
      setMounted(true);
      return;
    }
    if (open && (phase === "closed" || phase === "closing")) {
      // Commit the collapsed style before flipping it, or the browser
      // coalesces both into one style pass and skips the transition.
      void ref.current?.offsetHeight;
      setPhase("opening");
    } else if (!open && (phase === "open" || phase === "opening")) {
      setPhase("closing");
    }
  }, [mounted, open, phase]);

  const settle = () => {
    if (phase === "opening") setPhase("open");
    else if (phase === "closing") {
      setPhase("closed");
      if (!keepMounted) setMounted(false);
    }
  };
  const settleRef = useRef(settle);
  settleRef.current = settle;
  const moving = phase === "opening" || phase === "closing";
  useEffect(() => {
    if (!moving) return;
    // Nothing to wait for when the transition is switched off: settle now
    // rather than on a timer. Reduced motion either drops the property
    // (`motion-reduce:transition-none`) or clamps every duration to 0.01ms
    // (the global rule), so anything under 1ms counts as off.
    const element = ref.current;
    const style = element ? getComputedStyle(element) : null;
    if (
      style &&
      (style.transitionProperty === "none" ||
        !(parseFloat(style.transitionDuration) >= 0.001))
    ) {
      settleRef.current();
      return;
    }
    const timer = window.setTimeout(() => settleRef.current(), SETTLE_FALLBACK_MS);
    return () => window.clearTimeout(timer);
  }, [moving, phase]);

  if (!mounted) return null;
  const expanded = phase === "opening" || phase === "open";
  return (
    <div
      ref={ref}
      id={id}
      data-slot="collapse"
      data-state={expanded ? "open" : "closed"}
      inert={!expanded}
      aria-hidden={!expanded || undefined}
      onTransitionEnd={(event) => {
        if (event.target === event.currentTarget) settle();
      }}
      className={cn(
        "grid transition-[grid-template-rows,opacity] duration-150 ease-drawer motion-reduce:transition-none",
        expanded ? "grid-rows-[1fr]" : "grid-rows-[0fr] opacity-0 duration-100 ease-out",
      )}
    >
      <div
        className={cn(
          // min-w-0: a grid item's automatic minimum is its content width,
          // so long rows would otherwise widen the whole column.
          "min-h-0 min-w-0",
          phase === "open" ? "overflow-visible" : "overflow-hidden",
          className,
        )}
      >
        {children}
      </div>
    </div>
  );
}
