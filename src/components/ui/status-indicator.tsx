import { cn } from "@/lib/utils";
import type { ActivePaneStatus } from "@/tauri/types";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";

// The ping is reserved for `permission`: it is this app's "look at me"
// signal, and only a blocked agent needs the user. Working agents breathe
// instead, so five busy dots never drown out the one that is waiting. The
// permission dot also carries a ring, so it still reads as different from a
// working dot in greyscale and with motion reduced.
const STATUS_CONFIG = {
  permission: {
    dotColor: "bg-status-attention ring-2 ring-status-attention/35",
    ping: "bg-status-attention",
    tooltip: "Needs input",
  },
  working: {
    dotColor: "bg-status-working cm-breathe",
    ping: null,
    tooltip: "Agent working",
  },
  // Steady on purpose: a background watch loop is presence, not progress.
  monitoring: {
    dotColor: "bg-status-monitoring",
    ping: null,
    tooltip: "Monitoring in the background",
  },
  review: {
    dotColor: "bg-status-open",
    ping: null,
    tooltip: "Ready for review",
  },
} as const satisfies Record<ActivePaneStatus, {
  dotColor: string;
  ping: string | null;
  tooltip: string;
}>;

interface StatusIndicatorProps {
  status: ActivePaneStatus;
  className?: string;
  /**
   * When false, renders just the dot without the Tooltip wrapper. Used where
   * an ancestor already owns the hover affordance — e.g. the collapsed sidebar
   * rail's project avatars, which open a HoverCard flyout on hover, so a
   * competing per-dot tooltip would fight it.
   */
  withTooltip?: boolean;
}

export function StatusIndicator({
  status,
  className,
  withTooltip = true,
}: StatusIndicatorProps) {
  const config = STATUS_CONFIG[status];

  const dot = (
    <span
      data-status={status}
      className={cn("relative inline-flex size-2", className)}
    >
      {config.ping && (
        <span
          data-status-ping
          className={cn(
            "absolute inline-flex h-full w-full motion-safe:animate-ping rounded-full opacity-75",
            config.ping,
          )}
        />
      )}
      <span
        className={cn(
          "relative inline-flex size-2 rounded-full",
          config.dotColor,
        )}
      />
    </span>
  );

  if (!withTooltip) return dot;

  return (
    <Tooltip>
      <TooltipTrigger asChild>{dot}</TooltipTrigger>
      <TooltipContent side="right" className="text-label">
        {config.tooltip}
      </TooltipContent>
    </Tooltip>
  );
}
