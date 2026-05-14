import { cn } from "@/lib/utils";
import type { UsageSeverity } from "./types";

interface UsageSeverityBadgeProps {
  severity: UsageSeverity;
}

/** A small colored dot shown next to rows under elevated/high resource use. */
export function UsageSeverityBadge({ severity }: UsageSeverityBadgeProps) {
  if (severity === "normal") return null;

  return (
    <span
      role="img"
      aria-label={severity === "high" ? "High usage" : "Elevated usage"}
      className={cn(
        "h-1.5 w-1.5 shrink-0 rounded-full",
        severity === "high" ? "bg-danger" : "bg-warning",
      )}
    />
  );
}
