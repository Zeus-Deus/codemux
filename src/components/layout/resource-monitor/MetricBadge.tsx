import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { Eyebrow } from "@/components/ui/eyebrow";

interface MetricBadgeProps {
  label: string;
  value: string;
  tooltip?: string;
}

/** One of the three header readouts (CPU / Memory / RAM Share). */
export function MetricBadge({ label, value, tooltip }: MetricBadgeProps) {
  const content = (
    <div className="min-w-0 px-3 first:pl-0 last:pr-0">
      <Eyebrow>
        {label}
      </Eyebrow>
      <div className="mt-1.5 text-body-lg leading-none font-medium tabular-nums tracking-tight text-foreground whitespace-nowrap">
        {value}
      </div>
    </div>
  );

  if (!tooltip) return content;

  return (
    <Tooltip delayDuration={150}>
      <TooltipTrigger asChild>{content}</TooltipTrigger>
      <TooltipContent side="top" sideOffset={6}>
        {tooltip}
      </TooltipContent>
    </Tooltip>
  );
}
