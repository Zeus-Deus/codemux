import { cn } from "@/lib/utils";
import type { ActivePaneStatus } from "@/tauri/types";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";

const STATUS_CONFIG = {
  permission: {
    pingColor: "bg-status-attention",
    dotColor: "bg-status-attention",
    pulse: true,
    tooltip: "Needs input",
  },
  working: {
    pingColor: "bg-status-working",
    dotColor: "bg-status-working",
    pulse: true,
    tooltip: "Agent working",
  },
  review: {
    pingColor: "",
    dotColor: "bg-status-open",
    pulse: false,
    tooltip: "Ready for review",
  },
} as const satisfies Record<ActivePaneStatus, {
  pingColor: string;
  dotColor: string;
  pulse: boolean;
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
    <span className={cn("relative inline-flex size-2", className)}>
      {config.pulse && (
        <span
          className={cn(
            "absolute inline-flex h-full w-full animate-ping rounded-full opacity-75",
            config.pingColor,
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
      <TooltipContent side="right" className="text-xs">
        {config.tooltip}
      </TooltipContent>
    </Tooltip>
  );
}
