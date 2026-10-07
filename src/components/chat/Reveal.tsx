import { useLayoutEffect, useRef, useState, type ReactNode } from "react";

import { cn } from "@/lib/utils";

/**
 * Height disclosure for transcript rows (tool cards, work-log steps). It eases
 * a `0fr → 1fr` grid row, so nothing has to be measured, and keeps the
 * transcript from jumping by a whole body height in one frame.
 *
 * Children mount on the first open and stay mounted afterwards, so closing
 * can animate. A row that is never opened never renders its body, which
 * matters for the dense, mostly collapsed transcript. Closed content is
 * `inert`, so its links and buttons leave the tab order.
 */
export function Reveal({
  open,
  id,
  className,
  children,
}: {
  open: boolean;
  id?: string;
  className?: string;
  children: ReactNode;
}) {
  const [mounted, setMounted] = useState(open);
  // Lags `open` by one layout pass on the way in, so a freshly mounted body
  // starts from the collapsed row and transitions open.
  const [shown, setShown] = useState(open);
  const ref = useRef<HTMLDivElement>(null);

  useLayoutEffect(() => {
    if (open && !mounted) {
      setMounted(true);
      return;
    }
    if (open && !shown) {
      // Commit the collapsed style before flipping it, or the browser
      // coalesces both into one style pass and skips the transition.
      void ref.current?.offsetHeight;
      setShown(true);
    } else if (!open && shown) {
      setShown(false);
    }
  }, [mounted, open, shown]);

  if (!mounted) return null;
  return (
    <div
      ref={ref}
      id={id}
      data-state={shown ? "open" : "closed"}
      inert={!shown}
      aria-hidden={!shown || undefined}
      className={cn(
        "grid transition-[grid-template-rows,opacity] duration-150 ease-out motion-reduce:transition-none",
        shown ? "grid-rows-[1fr]" : "grid-rows-[0fr] opacity-0",
      )}
    >
      <div className={cn("min-h-0 overflow-hidden", className)}>{children}</div>
    </div>
  );
}
