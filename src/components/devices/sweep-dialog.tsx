import { useRef, useState } from "react";

import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { toast } from "@/lib/toast";
import { formatBytes } from "@/lib/format-bytes";
import { cn } from "@/lib/utils";
import { closeWorkspaceWithWorktree } from "@/tauri/commands";

import { evictWorktreeSizes, type SweepCandidate } from "./use-sweep-candidates";

export interface SweepOutcome {
  closed: number;
  /** Refused by the backend because the worktree still holds work. */
  skipped: number;
  /** Rejected for any other reason (teardown script, git, IO). */
  failed: number;
  freedBytes: number;
  /** "<title>: <error>" per failure, for the toast body. */
  failures: string[];
}

export interface SweepProgress {
  /** Candidates completed, 1-based. */
  index: number;
  /** `candidates.length`, captured once at entry. */
  total: number;
  /** The workspace just attempted. */
  title: string;
  /** Running totals so far. */
  outcome: SweepOutcome;
}

/**
 * The backend's dirty/unpushed refusal. Other "use force" rejections (a
 * failing teardown script, say) are real failures, not the sweep working
 * as intended, and are reported as such.
 */
const KEEPS_WORK_PATTERN = /uncommitted change|unpushed commit/i;

/**
 * Remove every candidate's worktree without force and without touching
 * its branch — the dialog only promises to free disk. The backend refuses
 * a dirty or unpushed worktree; that refusal is the feature here (a sweep
 * must never be the thing that loses work), so it counts as skipped.
 *
 * `signal` is checked between candidates only: an aborted sweep finishes
 * the removal already in flight, and whatever was removed stays removed.
 */
export async function runSweep(
  candidates: readonly SweepCandidate[],
  onProgress?: (p: SweepProgress) => void,
  signal?: AbortSignal,
): Promise<SweepOutcome> {
  const total = candidates.length;
  const outcome: SweepOutcome = {
    closed: 0,
    skipped: 0,
    failed: 0,
    freedBytes: 0,
    failures: [],
  };
  const swept: string[] = [];
  for (let i = 0; i < total; i++) {
    if (signal?.aborted) break;
    const ws = candidates[i];
    try {
      await closeWorkspaceWithWorktree(ws.id, true, false, false);
      outcome.closed += 1;
      outcome.freedBytes += ws.bytes ?? 0;
      swept.push(ws.id);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      if (KEEPS_WORK_PATTERN.test(message)) {
        outcome.skipped += 1;
      } else {
        outcome.failed += 1;
        outcome.failures.push(`${ws.title}: ${message}`);
      }
    }
    onProgress?.({
      index: i + 1,
      total,
      title: ws.title,
      outcome: { ...outcome, failures: [...outcome.failures] },
    });
  }
  evictWorktreeSizes(swept);
  return outcome;
}

export function sweepSummary(outcome: SweepOutcome): string {
  const parts: string[] = [];
  if (outcome.freedBytes > 0) {
    parts.push(`Freed ~${formatBytes(outcome.freedBytes)}`);
  } else if (outcome.closed > 0) {
    parts.push(
      `Removed ${outcome.closed} ${outcome.closed === 1 ? "worktree" : "worktrees"}`,
    );
  } else {
    parts.push("Nothing removed");
  }
  if (outcome.skipped > 0) {
    parts.push(`${outcome.skipped} skipped (uncommitted work)`);
  }
  if (outcome.failed > 0) {
    parts.push(`${outcome.failed} failed`);
  }
  return parts.join(" · ");
}

const EMPTY_OUTCOME: SweepOutcome = {
  closed: 0,
  skipped: 0,
  failed: 0,
  freedBytes: 0,
  failures: [],
};

/** One sweep, from the Sweep click until the dialog closes. */
interface SweepRun {
  /** The candidates as they were at the click. The live prop shrinks as
   *  worktrees go, so nothing on the progress surface reads it. */
  queue: readonly SweepCandidate[];
  progress: SweepProgress | null;
  settled: boolean;
  stopping: boolean;
}

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  candidates: readonly SweepCandidate[];
  /** Sum of the measured candidates; null while nothing is known. */
  knownBytes: number | null;
}

/**
 * Confirm for the "This device" sweep chip. Once confirmed, the same dialog
 * becomes the progress surface and then the receipt.
 */
export function SweepDialog({ open, onOpenChange, candidates, knownBytes }: Props) {
  const [run, setRun] = useState<SweepRun | null>(null);
  const abortRef = useRef<AbortController | null>(null);
  const running = run !== null && !run.settled;
  const count = candidates.length;

  const handleConfirm = async () => {
    const queue = [...candidates];
    const controller = new AbortController();
    abortRef.current = controller;
    setRun({ queue, progress: null, settled: false, stopping: false });
    try {
      const outcome = await runSweep(
        queue,
        (progress) => setRun((r) => r && { ...r, progress }),
        controller.signal,
      );
      if (outcome.failed > 0) {
        toast.warning(sweepSummary(outcome), {
          description: outcome.failures.join("\n"),
        });
      }
    } finally {
      abortRef.current = null;
      setRun((r) => r && { ...r, settled: true });
    }
  };

  const handleStop = () => {
    abortRef.current?.abort();
    setRun((r) => r && { ...r, stopping: true });
  };

  return (
    <Dialog open={open} onOpenChange={(next) => !running && onOpenChange(next)}>
      <DialogContent
        className={cn("sm:max-w-[380px]", run && "gap-0 overflow-hidden p-0")}
        showCloseButton={false}
        // Back to the confirm only once the close animation is over, so the
        // receipt doesn't flash into the confirm on its way out.
        onCloseAutoFocus={() => {
          if (run?.settled) setRun(null);
        }}
      >
        {run ? (
          <SweepProgressSurface
            run={run}
            onStop={handleStop}
            onDone={() => onOpenChange(false)}
          />
        ) : (
          <>
            <DialogHeader>
              <DialogTitle className="text-body-lg">
                Sweep {count} settled {count === 1 ? "workspace" : "workspaces"}
                {knownBytes !== null && knownBytes > 0 && (
                  <span className="ml-1.5 font-mono text-label font-normal text-muted-foreground">
                    ~{formatBytes(knownBytes)}
                  </span>
                )}
              </DialogTitle>
              <DialogDescription className="text-body leading-relaxed">
                Removes their worktrees from disk; branches are kept. Worktrees
                with uncommitted or unpushed work are skipped.
              </DialogDescription>
            </DialogHeader>
            <DialogFooter className="gap-2 sm:gap-2">
              <Button
                type="button"
                variant="ghost"
                size="sm"
                onClick={() => onOpenChange(false)}
              >
                Cancel
              </Button>
              <Button
                type="button"
                size="sm"
                className="bg-status-working/[0.14] text-status-working hover:bg-status-working/[0.22]"
                disabled={count === 0}
                onClick={() => void handleConfirm()}
              >
                Sweep
              </Button>
            </DialogFooter>
          </>
        )}
      </DialogContent>
    </Dialog>
  );
}

/**
 * Job line, hero pair, rail, now line. Every region has a fixed height and
 * keeps it from the first tick into the receipt, so the dialog never
 * resizes while you watch it.
 */
function SweepProgressSurface({
  run,
  onStop,
  onDone,
}: {
  run: SweepRun;
  onStop: () => void;
  onDone: () => void;
}) {
  const { queue, progress, settled, stopping } = run;
  const total = queue.length;
  const index = progress?.index ?? 0;
  const outcome = progress?.outcome ?? EMPTY_OUTCOME;
  const stopped = settled && index < total;
  // A receipt with anything other than clean removals keeps its proportions.
  const partial = stopped || outcome.failed > 0;
  // The worktree being removed right now: the one after the last reported.
  const current = queue[Math.min(index, total - 1)]?.title ?? "";

  const nowLine = settled
    ? [
        outcome.skipped > 0
          ? `${outcome.skipped} held uncommitted or unpushed work`
          : "Nothing was skipped",
        outcome.failed > 0 ? `${outcome.failed} failed` : null,
      ]
        .filter(Boolean)
        .join(" · ")
    : `removing ${current}`;

  return (
    <>
      <div className="px-4 pt-3.5">
        <div className="flex h-[18px] items-center gap-2.5">
          <DialogTitle
            className={cn(
              "truncate text-body leading-[18px] tracking-[-0.005em]",
              settled && !stopped ? "text-status-open" : "text-foreground",
            )}
          >
            {!settled
              ? "Sweeping settled workspaces"
              : stopped
                ? "Sweep stopped"
                : "Sweep complete"}
          </DialogTitle>
          {!settled && outcome.skipped > 0 && (
            <span className="ml-auto shrink-0 whitespace-nowrap rounded-sm bg-status-working/[0.14] px-1.5 py-0.5 font-mono text-caption leading-[13px] text-status-working">
              {outcome.skipped} skipped
            </span>
          )}
        </div>
        <DialogDescription className="sr-only">
          Removes worktrees one at a time; branches are kept and worktrees with
          uncommitted or unpushed work are skipped.
        </DialogDescription>
      </div>

      <div className="flex items-end gap-6 px-4 pt-2.5 pb-4">
        <div className="flex flex-col gap-[3px]">
          <span className="font-mono text-[2rem] leading-none font-medium tracking-[-0.02em] text-foreground tabular-nums">
            {total - index}
          </span>
          <span className="text-label leading-[14px] text-muted-foreground">
            of {total} left
          </span>
        </div>
        <div className="flex flex-col gap-[3px] pb-0.5">
          <span className="font-mono text-[1.25rem] leading-[21px] text-status-working tabular-nums">
            {formatBytes(outcome.freedBytes)}
          </span>
          <span className="text-label leading-[14px] text-muted-foreground">
            {settled ? "freed" : "freed so far"}
          </span>
        </div>
      </div>

      <div
        role="progressbar"
        aria-label="Sweep progress"
        aria-valuemin={0}
        aria-valuemax={total}
        aria-valuenow={index}
        className="relative flex h-0.5 bg-surface-2"
      >
        {!settled ? (
          <div
            className="absolute inset-y-0 left-0 bg-status-working shadow-[0_0_10px_color-mix(in_oklch,var(--status-working)_55%,transparent)] transition-[width] duration-300 ease-out"
            style={{ width: `${total > 0 ? (index / total) * 100 : 0}%` }}
          />
        ) : partial ? (
          <>
            <div className="bg-status-open" style={{ flex: outcome.closed }} />
            <div
              className="bg-status-working"
              style={{ flex: outcome.skipped + outcome.failed }}
            />
            <div style={{ flex: total - index }} />
          </>
        ) : (
          <div className="flex-1 bg-status-open" />
        )}
      </div>

      <div className="flex h-[39px] items-center gap-2 px-4">
        <span
          aria-hidden
          className={cn(
            "size-[5px] shrink-0 rounded-full",
            settled ? "bg-status-open" : "cm-breathe bg-status-working",
          )}
        />
        <span className="min-w-0 truncate font-mono text-label text-muted-foreground">
          {nowLine}
        </span>
      </div>

      <DialogFooter className="mx-0 mb-0 flex-row items-center gap-3 py-2.5 pr-3 pl-4 sm:justify-between">
        <span className="min-w-0 truncate text-label text-muted-foreground">
          Branches kept · uncommitted work skipped
        </span>
        {settled ? (
          <Button type="button" variant="secondary" size="sm" onClick={onDone}>
            Done
          </Button>
        ) : (
          <Button
            type="button"
            variant="ghost"
            size="sm"
            disabled={stopping}
            onClick={onStop}
          >
            {stopping ? "Stopping" : "Stop"}
          </Button>
        )}
      </DialogFooter>
    </>
  );
}
