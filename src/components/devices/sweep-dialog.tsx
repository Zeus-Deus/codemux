import { useState } from "react";

import { Loader2 } from "lucide-react";

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
 */
export async function runSweep(
  candidates: readonly SweepCandidate[],
): Promise<SweepOutcome> {
  const outcome: SweepOutcome = {
    closed: 0,
    skipped: 0,
    failed: 0,
    freedBytes: 0,
    failures: [],
  };
  const swept: string[] = [];
  for (const ws of candidates) {
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

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  candidates: readonly SweepCandidate[];
  /** Sum of the measured candidates; null while nothing is known. */
  knownBytes: number | null;
}

/** Minimal confirm for the "This device" sweep chip. */
export function SweepDialog({ open, onOpenChange, candidates, knownBytes }: Props) {
  const [running, setRunning] = useState(false);
  const count = candidates.length;

  const handleConfirm = async () => {
    setRunning(true);
    try {
      const outcome = await runSweep(candidates);
      const summary = sweepSummary(outcome);
      if (outcome.failed > 0) {
        toast.warning(summary, { description: outcome.failures.join("\n") });
      } else {
        toast.success(summary);
      }
      onOpenChange(false);
    } finally {
      setRunning(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={(next) => !running && onOpenChange(next)}>
      <DialogContent className="sm:max-w-[380px]" showCloseButton={false}>
        <DialogHeader>
          <DialogTitle className="text-[14px]">
            Sweep {count} settled {count === 1 ? "workspace" : "workspaces"}
            {knownBytes !== null && knownBytes > 0 && (
              <span className="ml-1.5 font-mono text-[11px] font-normal text-muted-foreground">
                ~{formatBytes(knownBytes)}
              </span>
            )}
          </DialogTitle>
          <DialogDescription className="text-[12.5px] leading-relaxed">
            Removes their worktrees from disk; branches are kept. Worktrees
            with uncommitted or unpushed work are skipped.
          </DialogDescription>
        </DialogHeader>
        <DialogFooter className="gap-2 sm:gap-2">
          <Button
            type="button"
            variant="ghost"
            size="sm"
            disabled={running}
            onClick={() => onOpenChange(false)}
          >
            Cancel
          </Button>
          <Button
            type="button"
            size="sm"
            className="bg-status-working/[0.14] text-status-working hover:bg-status-working/[0.22]"
            disabled={running}
            onClick={() => void handleConfirm()}
          >
            {running && <Loader2 className="size-3.5 animate-spin" aria-hidden />}
            Sweep
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
