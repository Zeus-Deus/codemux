import { cn } from "@/lib/utils";
import type { GitCloneProgress } from "@/tauri/events";

interface CloneProgressRowProps {
  /** Latest parsed clone progress, or `null` before the first event. */
  progress: GitCloneProgress | null;
  className?: string;
}

/**
 * Progress readout for an in-flight `git clone`: a phase label + percent,
 * a slim determinate bar (indeterminate pulse when percent is unknown),
 * and the raw throughput detail ("12.00 MiB | 1.20 MiB/s") in muted text.
 *
 * Before any event arrives (`progress === null`) it shows a reassuring
 * "Cloning…" pulse so a slow connection never looks hung.
 */
export function CloneProgressRow({ progress, className }: CloneProgressRowProps) {
  const phase = progress?.phase ?? "Cloning";
  const percent = progress?.percent ?? null;
  const detail = progress?.detail ?? "";
  const determinate = percent !== null;

  return (
    <div className={cn("space-y-1.5", className)}>
      <div className="flex items-center justify-between gap-2 text-xs">
        <span className="font-medium text-foreground">{phase}…</span>
        {determinate && (
          <span className="font-mono text-muted-foreground">{percent}%</span>
        )}
      </div>

      <div className="h-1.5 w-full overflow-hidden rounded-full bg-foreground/10">
        {determinate ? (
          <div
            className="h-full rounded-full bg-foreground transition-[width] duration-300 ease-out"
            style={{ width: `${percent}%` }}
          />
        ) : (
          <div className="h-full w-full animate-pulse rounded-full bg-foreground/40" />
        )}
      </div>

      <p className="min-h-4 truncate text-[11px] text-muted-foreground">
        {detail || "This can take a while on slow connections…"}
      </p>
    </div>
  );
}
