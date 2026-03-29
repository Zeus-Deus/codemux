import { cn } from "@/lib/utils";
import type { ActivePaneStatus } from "@/tauri/types";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";

const STATUS_CONFIG = {
  permission: {
    pingColor: "bg-red-400",
    dotColor: "bg-red-500",
    pulse: true,
    tooltip: "Needs input",
  },
  working: {
    pingColor: "bg-amber-400",
    dotColor: "bg-amber-500",
    pulse: true,
    tooltip: "Agent working",
  },
  review: {
    pingColor: "",
    dotColor: "bg-emerald-500",
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
}

export function StatusIndicator({ status, className }: StatusIndicatorProps) {
  const config = STATUS_CONFIG[status];

  return (
    <Tooltip>
      <TooltipTrigger asChild>
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
      </TooltipTrigger>
      <TooltipContent side="right" className="text-xs">
        {config.tooltip}
      </TooltipContent>
    </Tooltip>
  );
}
