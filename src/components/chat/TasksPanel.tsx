import { Check, CircleCheck, Copy, LoaderCircle } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";

import { ScrollArea } from "@/components/ui/scroll-area";
import { cn } from "@/lib/utils";
import type { TaskSnapshotItem, TasksSnapshot } from "@/tauri/events";

/** Overall run state, derived from the rows: every row done → complete;
 *  any row in progress → working; otherwise the plan is still queued. */
type RunState = "queued" | "working" | "complete";

function runStateOf(snapshot: TasksSnapshot): RunState {
  const total = snapshot.tasks.length;
  const completed = snapshot.tasks.filter(
    (task) => task.status === "completed",
  ).length;
  if (total > 0 && completed === total) return "complete";
  if (snapshot.tasks.some((task) => task.status === "in_progress"))
    return "working";
  return "queued";
}

const RUN_STATE_LABEL: Record<RunState, string> = {
  queued: "Queued",
  working: "Working",
  complete: "Complete",
};

const BADGE_TONE: Record<RunState, string> = {
  queued: "bg-muted text-muted-foreground",
  working: "bg-status-working/14 text-status-working",
  complete: "bg-status-open/14 text-status-open",
};

const BAR_TONE: Record<RunState, string> = {
  queued: "bg-muted-foreground/60",
  working: "bg-status-working",
  complete: "bg-status-open",
};

function StatusGlyph({ task }: { task: TaskSnapshotItem }) {
  if (task.status === "completed") {
    return (
      <CircleCheck
        className="size-4 text-status-open"
        strokeWidth={1.8}
        aria-hidden
      />
    );
  }
  if (task.status === "in_progress") {
    return (
      <LoaderCircle
        className="size-[15px] animate-spin text-status-working"
        strokeWidth={1.9}
        aria-hidden
      />
    );
  }
  return (
    <span
      className="size-[9px] rounded-full border-[1.6px] border-foreground/25"
      aria-hidden
    />
  );
}

/** Snapshot → markdown checklist, for the footer's Copy action. */
export function tasksToMarkdown(snapshot: TasksSnapshot): string {
  const lines = snapshot.tasks.map(
    (task) => `- [${task.status === "completed" ? "x" : " "}] ${task.title}`,
  );
  return (snapshot.explanation ? [snapshot.explanation, ""] : [])
    .concat(lines)
    .join("\n");
}

export function TasksPanel({
  snapshot,
  updatedAt = null,
}: {
  snapshot: TasksSnapshot;
  /** Clock time (ms) the snapshot was last applied; null hides the caption. */
  updatedAt?: number | null;
}) {
  const total = snapshot.tasks.length;
  const completed = snapshot.tasks.filter(
    (task) => task.status === "completed",
  ).length;
  const progress = total === 0 ? 0 : Math.round((completed / total) * 100);
  const runState = runStateOf(snapshot);
  const titleById = new Map(
    snapshot.tasks.map((task) => [task.task_id, task.title]),
  );

  const [copied, setCopied] = useState(false);
  const copyTimer = useRef<number | null>(null);
  useEffect(
    () => () => {
      if (copyTimer.current != null) window.clearTimeout(copyTimer.current);
    },
    [],
  );
  const handleCopy = useCallback(() => {
    void navigator.clipboard
      ?.writeText(tasksToMarkdown(snapshot))
      .then(() => {
        setCopied(true);
        if (copyTimer.current != null) window.clearTimeout(copyTimer.current);
        copyTimer.current = window.setTimeout(() => setCopied(false), 1500);
      })
      .catch(() => {
        /* best-effort */
      });
  }, [snapshot]);

  return (
    <div
      className="flex h-full min-h-0 flex-col bg-background"
      data-testid="tasks-panel"
    >
      {/* Header earns its space: run state + counts + freshness on one
          line over a 3px tone-colored bar. The tab already says "Tasks",
          so the header doesn't restate it. */}
      <div className="shrink-0 border-b border-border/60 px-3.5 pb-3 pt-3">
        <div className="mb-2 flex items-center gap-2">
          <span
            data-testid="tasks-status-badge"
            className={cn(
              "inline-flex items-center rounded-[5px] px-1.5 py-px font-mono text-[10px] font-semibold uppercase tracking-[0.09em]",
              BADGE_TONE[runState],
            )}
          >
            {RUN_STATE_LABEL[runState]}
          </span>
          <span className="min-w-0 flex-1 font-mono text-[10.5px] tabular-nums text-muted-foreground">
            {completed}/{total} done
          </span>
          {updatedAt != null && (
            <span className="shrink-0 font-mono text-[10.5px] tabular-nums text-muted-foreground">
              {new Date(updatedAt).toLocaleTimeString([], {
                hour: "numeric",
                minute: "2-digit",
                second: "2-digit",
              })}
            </span>
          )}
        </div>
        <div
          className="h-[3px] overflow-hidden rounded-full bg-foreground/12"
          role="progressbar"
          aria-label="Task progress"
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={progress}
        >
          <div
            className={cn(
              "h-full rounded-full transition-[width] duration-300",
              BAR_TONE[runState],
            )}
            style={{ width: `${progress}%` }}
          />
        </div>
      </div>

      <ScrollArea className="min-h-0 flex-1">
        <div className="px-2.5 py-3">
          {/* One-line intent in the agent's words — what makes an old
              list legible when scrolling back. */}
          {snapshot.explanation && (
            <p className="mx-1.5 mb-3 text-xs leading-relaxed text-muted-foreground">
              {snapshot.explanation}
            </p>
          )}
          <ol className="flex flex-col gap-0.5" aria-label="Agent task list">
            {snapshot.tasks.map((task, index) => {
              const blockers = task.blocked_by
                .map((id) => titleById.get(id) ?? id)
                .filter(Boolean);
              return (
                <li
                  key={task.task_id}
                  className={cn(
                    "flex items-start gap-2 rounded-lg px-2 py-1.5 transition-colors",
                    task.status === "in_progress" && "bg-status-working/8",
                    task.status === "completed" && "bg-status-open/8",
                    task.status === "pending" && "hover:bg-foreground/4",
                  )}
                >
                  <span className="mt-[3px] flex size-4 shrink-0 items-center justify-center">
                    <StatusGlyph task={task} />
                  </span>
                  <span className="mt-0.5 w-4 shrink-0 text-right font-mono text-[10px] tabular-nums leading-4 text-muted-foreground/70">
                    {index + 1}
                  </span>
                  <div className="min-w-0 flex-1">
                    <p
                      className={cn(
                        "text-[13px] leading-5",
                        task.status === "completed" &&
                          "text-muted-foreground line-through decoration-border",
                        task.status === "in_progress" && "text-foreground",
                        task.status === "pending" && "text-foreground/75",
                      )}
                    >
                      {task.title}
                    </p>
                    {task.detail && task.detail !== task.title && (
                      <p className="mt-0.5 text-xs leading-relaxed text-muted-foreground">
                        {task.detail}
                      </p>
                    )}
                    {blockers.length > 0 && (
                      <p className="mt-1 text-[11px] leading-relaxed text-status-working">
                        Blocked by {blockers.join(", ")}
                      </p>
                    )}
                  </div>
                </li>
              );
            })}
          </ol>
        </div>
      </ScrollArea>

      <div className="flex shrink-0 items-center gap-2 border-t border-border/60 px-3 py-2">
        <span className="min-w-0 flex-1 font-mono text-[10px] text-muted-foreground">
          agent plan · updates live
        </span>
        <button
          type="button"
          onClick={handleCopy}
          data-testid="tasks-copy"
          className="inline-flex h-[23px] items-center gap-1.5 rounded-[7px] border border-border/60 px-2 text-[11px] font-semibold text-muted-foreground transition-colors hover:border-border hover:bg-foreground/6 hover:text-foreground"
        >
          {copied ? (
            <Check className="size-3" aria-hidden />
          ) : (
            <Copy className="size-3" aria-hidden />
          )}
          {copied ? "Copied" : "Copy"}
        </button>
      </div>
    </div>
  );
}
