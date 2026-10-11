import { AlertCircle, Loader2, X } from "lucide-react";
import { cn } from "@/lib/utils";
import { useUIStore } from "@/stores/ui-store";
import type { PendingWorkspace } from "@/tauri/types";

/** Optimistic sidebar row for a workspace that is still being created, or
 *  that failed. A failed row that carries the dialog's draft stays put
 *  until the user retries from the reopened dialog or dismisses it. */
export function PendingWorkspaceRow({
  pending,
  className,
}: {
  pending: PendingWorkspace;
  /** Padding and gap, which differ between the sidebar layouts. */
  className?: string;
}) {
  const reopenPendingWorkspace = useUIStore((s) => s.reopenPendingWorkspace);
  const removePendingWorkspace = useUIStore((s) => s.removePendingWorkspace);
  const failed = pending.status === "failed";
  const label = failed ? pending.errorMessage || "Failed" : pending.name;
  const icon = failed ? (
    <AlertCircle className="size-3.5 shrink-0 text-destructive" />
  ) : (
    <Loader2 className="size-3.5 shrink-0 animate-spin text-muted-foreground" />
  );

  if (!failed || !pending.draft) {
    return (
      <div
        className={cn(
          "flex items-center text-body",
          className,
          failed ? "opacity-60" : "opacity-70 motion-safe:animate-pulse",
        )}
      >
        {icon}
        <span className="truncate text-label text-muted-foreground">{label}</span>
      </div>
    );
  }

  return (
    <div
      data-pending-failed
      className={cn(
        "group/pending flex items-center rounded-md text-body transition-colors duration-150 hover:bg-surface-2",
        className,
      )}
    >
      <button
        type="button"
        onClick={() => reopenPendingWorkspace(pending.id)}
        title={`${label}\nReopen to edit and retry`}
        className="flex min-w-0 flex-1 items-center gap-2 text-left"
      >
        {icon}
        <span className="min-w-0 flex-1 truncate text-label text-muted-foreground">
          {label}
        </span>
        <span className="shrink-0 text-label font-medium text-foreground">
          Reopen
        </span>
      </button>
      <button
        type="button"
        aria-label="Dismiss failed workspace"
        onClick={() => removePendingWorkspace(pending.id)}
        className="inline-flex size-5 shrink-0 items-center justify-center rounded-sm text-muted-foreground transition-colors duration-100 hover:bg-surface-3 hover:text-foreground"
      >
        <X className="size-3" />
      </button>
    </div>
  );
}
