import { create } from "zustand";

import type { StaleWorktree } from "@/tauri/commands";

/** Whether the stale-worktree review dialog is open. App-level so the
 *  cleanup toast and Settings → Archive open the same dialog. */
export const useWorktreeCleanupStore = create<{ open: boolean }>(() => ({ open: false }));

export function openWorktreeCleanup(): void {
  useWorktreeCleanupStore.setState({ open: true });
}

export function closeWorktreeCleanup(): void {
  useWorktreeCleanupStore.setState({ open: false });
}

/** One line saying why a worktree is stale. */
export function staleReasonLabel(worktree: StaleWorktree): string {
  const { reason } = worktree;
  switch (reason.kind) {
    case "branch_merged":
      return `Merged into ${reason.base}`;
    case "pr_merged":
      return `PR #${reason.number} merged`;
    case "idle": {
      const verb = worktree.source === "archived" ? "Archived" : "Closed";
      return `${verb} ${reason.days} days ago`;
    }
  }
}

export function pluralWorktrees(n: number): string {
  return `${n} ${n === 1 ? "worktree" : "worktrees"}`;
}

/** "a, b and 3 more" for toast bodies. */
export function listTitles(worktrees: readonly StaleWorktree[], max = 2): string {
  const titles = worktrees.map((w) => w.title);
  if (titles.length <= max) return titles.join(" and ");
  return `${titles.slice(0, max).join(", ")} and ${titles.length - max} more`;
}
