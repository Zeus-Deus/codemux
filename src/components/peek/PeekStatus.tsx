import { cn } from "@/lib/utils";
import type { PeekStatusKind } from "@/stores/browser-peek-store";

const statuses: Record<PeekStatusKind, { className: string; label: string }> = {
  live: { className: "bg-status-open", label: "Live" },
  connecting: { className: "bg-muted-foreground/60 motion-safe:animate-pulse", label: "Connecting" },
  stale: { className: "bg-warning", label: "Stale" },
  error: { className: "bg-destructive", label: "Error" },
};

/** A status dot with an accessible label; it never adds a row or visible text. */
export function PeekStatus({ status, label }: { status: PeekStatusKind; label?: string }) {
  const dot = statuses[status];
  return <span role="img" aria-label={label ?? dot.label} title={label ?? dot.label}
    className={cn("h-[7px] w-[7px] shrink-0 rounded-full", dot.className)} />;
}
