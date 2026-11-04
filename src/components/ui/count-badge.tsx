import type { ComponentProps } from "react";
import { cn } from "@/lib/utils";

/**
 * The corner count on an icon button (open PRs, active ports). Quiet on
 * purpose: a count is information, not an alarm, so it sits on the neutral
 * active surface instead of a bright primary disc that outshone the rail's
 * real attention states. The ring cuts it cleanly out of the glyph beneath.
 */
export function CountBadge({ className, ...props }: ComponentProps<"span">) {
  return (
    <span
      className={cn(
        "pointer-events-none absolute -right-1 -top-1 h-[15px] min-w-[15px] rounded-full px-1",
        "text-center text-micro font-semibold leading-[15px] tabular-nums text-foreground",
        // An opaque neutral (the surface ladder is translucent and would let
        // the glyph show through), cut out of the icon by a sidebar ring.
        "bg-[color-mix(in_oklch,var(--foreground)_12%,var(--sidebar))] ring-2 ring-sidebar",
        className,
      )}
      {...props}
    />
  );
}
