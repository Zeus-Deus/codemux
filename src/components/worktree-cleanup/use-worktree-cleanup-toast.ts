import { useCallback } from "react";

import { useTauriEvent } from "@/hooks/use-tauri-event";
import { toast } from "@/lib/toast";
import { onWorktreeCleanup, type WorktreeCleanupEvent } from "@/tauri/events";

import { listTitles, openWorktreeCleanup, pluralWorktrees } from "./worktree-cleanup-store";

/** How long the "can be removed" offer stays up: long enough to notice
 *  after coming back to the window, and Settings → Archive keeps the door. */
const OFFER_DURATION_MS = 15_000;

/**
 * Turns the backend's background worktree sweep into a toast: a receipt
 * when automatic cleanup removed something, otherwise an offer to review
 * the newly stale worktrees. The backend only reports a removable set once,
 * so this never nags about the same worktrees again.
 */
export function useWorktreeCleanupToast() {
  const handle = useCallback((event: WorktreeCleanupEvent) => {
    if (event.removed.length > 0) {
      toast.success(`Cleaned up ${pluralWorktrees(event.removed.length)}`, {
        description: `Removed ${listTitles(event.removed)}. Their branches and commits were kept.`,
      });
      return;
    }
    if (event.available.length > 0) {
      const n = event.available.length;
      toast.info(`${pluralWorktrees(n)} can be cleaned up`, {
        description: `${listTitles(event.available)} ${n === 1 ? "is" : "are"} merged or long unused, with no uncommitted or unpushed work.`,
        duration: OFFER_DURATION_MS,
        action: { label: "Review", onClick: openWorktreeCleanup },
      });
    }
  }, []);

  useTauriEvent(onWorktreeCleanup, handle, [handle]);
}
