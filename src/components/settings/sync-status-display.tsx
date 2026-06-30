// Visible sync-status surface (Stage 5 of Step 10).
//
// Stages 1-4 shipped fully-working sync underneath the surface;
// before this component the only way a user knew sync was
// happening was watching `~/.codemux/skills/` change on disk.
// SyncStatusDisplay is the dashboard.
//
// Layout:
//
//   [icon] Sync ready              [Sync now]
//          Last synced 3 minutes ago
//
//   [error banner only when state="error"]
//
// The icon + label react to the engine's state machine via
// `useSkillsSyncStatus`. The "Last synced X ago" string ages on
// its own via `useTickEvery(30s)` so the user sees the relative
// label drift forward without a manual refresh.

import { AlertCircle, CheckCircle2, Loader2, RefreshCw } from "lucide-react";

import { useSkillsSyncStatus } from "@/hooks/use-skills-sync-status";
import { useTickEvery } from "@/hooks/use-tick-every";
import { relativeTime } from "@/lib/relative-time";

export type SyncStateKind = "idle" | "syncing" | "error";

/// State icon as a small inline element. Kept here rather than a
/// separate file because there's nothing else that uses it; if
/// Stage 6 adds a top-bar status badge it can lift this into its
/// own component then.
export function SyncStateIcon({ state }: { state: SyncStateKind }) {
  switch (state) {
    case "idle":
      return (
        <CheckCircle2
          className="h-4 w-4 shrink-0 text-status-open"
          aria-label="Sync ready"
        />
      );
    case "syncing":
      return (
        <Loader2
          className="h-4 w-4 shrink-0 animate-spin text-foreground"
          aria-label="Syncing"
        />
      );
    case "error":
      return (
        <AlertCircle
          className="h-4 w-4 shrink-0 text-destructive"
          aria-label="Sync error"
        />
      );
  }
}

function stateLabel(state: SyncStateKind): string {
  switch (state) {
    case "idle":
      return "Sync ready";
    case "syncing":
      return "Syncing…";
    case "error":
      return "Sync error";
  }
}

export function SyncStatusDisplay() {
  const { status, syncNow, isSyncing } = useSkillsSyncStatus();
  // Re-render every 30s so "Last synced N minutes ago" advances
  // without re-fetching the underlying timestamp. 30s is enough
  // resolution for the buckets — the smallest one is "just now"
  // → "1 minute ago" at the 60s line.
  useTickEvery(30_000);

  if (!status) {
    return <SyncStatusSkeleton />;
  }

  const buttonLabel = status.state === "error" ? "Retry" : "Sync now";

  return (
    <div className="space-y-2">
      <div className="flex items-center gap-3">
        <SyncStateIcon state={status.state} />
        <div className="min-w-0 flex-1">
          <div className="text-sm font-medium">{stateLabel(status.state)}</div>
          {status.lastSyncAt && (
            <div className="text-xs text-muted-foreground">
              Last synced {relativeTime(status.lastSyncAt)}
            </div>
          )}
          {status.state === "syncing" && status.startedAt && (
            <div className="text-xs text-muted-foreground">
              Started {relativeTime(status.startedAt)}
            </div>
          )}
        </div>
        <button
          type="button"
          onClick={syncNow}
          disabled={isSyncing}
          className="inline-flex items-center gap-1.5 rounded-md border border-border bg-background px-2.5 py-1 text-xs font-medium hover:bg-muted disabled:cursor-not-allowed disabled:opacity-40"
        >
          {isSyncing ? (
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
          ) : (
            <RefreshCw className="h-3.5 w-3.5" />
          )}
          {buttonLabel}
        </button>
      </div>

      {status.state === "error" && status.lastError && (
        <div
          role="alert"
          className="flex items-start gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-2 text-xs text-foreground"
        >
          <AlertCircle className="mt-0.5 h-3.5 w-3.5 shrink-0 text-destructive" />
          <span className="break-words">{status.lastError}</span>
        </div>
      )}
    </div>
  );
}

/// Loader shown for the brief window between mount and the first
/// `skillsSyncStatus()` resolution. Same shape as the loaded
/// state so the layout doesn't shift.
function SyncStatusSkeleton() {
  return (
    <div className="flex items-center gap-3">
      <div className="h-4 w-4 animate-pulse rounded-full bg-muted" />
      <div className="min-w-0 flex-1 space-y-1">
        <div className="h-3.5 w-24 animate-pulse rounded bg-muted" />
        <div className="h-3 w-32 animate-pulse rounded bg-muted/60" />
      </div>
    </div>
  );
}
