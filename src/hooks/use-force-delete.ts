import { useState } from "react";

import { toast } from "@/lib/toast";

/**
 * Force-escalation for destructive deletes.
 *
 * The backend is honest about force: a delete request with
 * `forceDelete: false` REFUSES (before touching the workspace) when the
 * worktree is dirty or a teardown script fails, rejecting with a message
 * that matches {@link USE_FORCE_PATTERN}. Every delete surface therefore
 * runs the same two-step state machine:
 *
 *   1. issue the delete non-forced;
 *   2. on a rejection matching the pattern, keep the surface alive and
 *      offer an explicit "Force delete" that reissues with
 *      `forceDelete: true`. Any other rejection is a plain error.
 *
 * `useForceDelete` is the dialog-shaped variant (delete dialogs render
 * `forceMessage` in place and relabel their confirm button);
 * `runDeleteWithForceToast` is the toast-shaped variant for surfaces
 * whose confirm UI is a bare `window.confirm` + toast.
 */

/** Single source of truth for detecting the backend's "dirty — retry
 *  with force" rejections. The wording is pinned by a backend test; the
 *  UI only relies on this fragment. */
export const USE_FORCE_PATTERN = /use force/i;

export interface UseForceDeleteOptions {
  /** Perform the delete. Called with `force: false` first; called again
   *  with `force: true` when the user confirms the escalated state. */
  run: (force: boolean) => Promise<void>;
  /** Success path — e.g. close the dialog (and toast). */
  onDone?: () => void;
  /** Non-escalating failure path — e.g. error toast + close. Receives
   *  the normalized rejection message. */
  onError?: (message: string) => void;
}

export interface UseForceDeleteResult {
  /** Backend dirty/teardown message; non-null = escalated state. The
   *  dialog shows it verbatim and relabels its button "Force delete". */
  forceMessage: string | null;
  /** Run the delete. Non-forced on the first attempt; forced once the
   *  hook is in the escalated state. */
  confirm: () => Promise<void>;
  /** Clear the escalated state (call when the dialog closes so a
   *  re-open starts un-escalated). */
  reset: () => void;
}

export function useForceDelete({
  run,
  onDone,
  onError,
}: UseForceDeleteOptions): UseForceDeleteResult {
  const [forceMessage, setForceMessage] = useState<string | null>(null);

  const confirm = async () => {
    const force = forceMessage !== null;
    try {
      await run(force);
      onDone?.();
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      if (!force && USE_FORCE_PATTERN.test(message)) {
        // Dirty worktree — escalate in place instead of failing.
        setForceMessage(message);
        return;
      }
      onError?.(message);
    }
  };

  const reset = () => setForceMessage(null);

  return { forceMessage, confirm, reset };
}

export interface RunDeleteWithForceToastOptions {
  /** Perform the delete; same contract as `UseForceDeleteOptions.run`. */
  run: (force: boolean) => Promise<void>;
  /** Headline for the error toasts. Default: "Delete failed". */
  errorTitle?: string;
}

/** How long the escalation toast (with its "Force delete" action) stays
 *  up. Longer than the default error toast so the user has time to read
 *  the dirty-worktree message and decide. */
const FORCE_TOAST_DURATION_MS = 15_000;

/**
 * Toast-shaped force escalation for surfaces without a dialog (workspaces
 * overview row, empty-workspace state). The non-forced delete runs
 * immediately; a rejection matching {@link USE_FORCE_PATTERN} surfaces an
 * error toast whose destructive "Force delete" action reissues the same
 * delete with `forceDelete: true` (same pattern as `toast.undoable`'s
 * action button). Any other rejection is a plain error toast.
 */
export async function runDeleteWithForceToast({
  run,
  errorTitle = "Delete failed",
}: RunDeleteWithForceToastOptions): Promise<void> {
  try {
    await run(false);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    if (USE_FORCE_PATTERN.test(message)) {
      toast.error(errorTitle, {
        description: message,
        duration: FORCE_TOAST_DURATION_MS,
        action: {
          label: "Force delete",
          onClick: () => {
            run(true).catch((forceErr) => {
              toast.error(errorTitle, {
                description:
                  forceErr instanceof Error
                    ? forceErr.message
                    : String(forceErr),
              });
            });
          },
        },
      });
      return;
    }
    toast.error(errorTitle, { description: message });
  }
}
