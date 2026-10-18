import { Eye } from "lucide-react";

import { usePrWatch } from "@/hooks/use-pr-watch";
import { PR_WATCH_TONE_CLASS, describePrWatch } from "@/lib/pr-watch";
import { cn } from "@/lib/utils";

/**
 * Context Row chip beside the PR chip, for a chat that watches its pull
 * request. Shown only while a watch exists (watching, stopped or ended) so
 * an unwatched chat's row stays as it was; arming lives in the workspace
 * details card. Clicking stops an active watch or re-arms a stopped one.
 */
export function PrWatchChip({ threadId }: { threadId: string }) {
  const { watch, busy, toggle } = usePrWatch(threadId);
  if (!watch) return null;
  const view = describePrWatch(watch);
  return (
    <button
      type="button"
      data-testid="pr-watch-chip"
      onClick={() => toggle()}
      disabled={busy}
      aria-pressed={view.active}
      aria-label={view.active ? `Stop watching #${watch.pr_number}` : `Watch #${watch.pr_number} again`}
      title={view.title}
      className={cn(
        "inline-flex h-[26px] shrink-0 items-center gap-1 rounded-md px-1.5 font-mono text-label font-semibold tabular-nums transition-colors duration-150 hover:bg-surface-2 disabled:opacity-60",
        PR_WATCH_TONE_CLASS[view.tone],
      )}
    >
      <Eye className="size-3.5" />
      {watch.status === "active" && !watch.last_error
        ? watch.attempts > 0 && `${watch.attempts}/${watch.max_attempts}`
        : view.label}
    </button>
  );
}
