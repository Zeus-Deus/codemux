/**
 * Putting a pull request's branch in front of you.
 *
 * Two surfaces offer this — the incoming list's row action and the
 * detail column's header button — and they must agree, because they can
 * be looking at the same pull request at the same time. One switches to
 * the workspace that already has the branch; the other cuts a worktree.
 * Which of the two happens is never a choice the caller makes.
 */

import { createWorktreeWorkspace } from "@/tauri/commands";
import { activateWorkspaceInteraction } from "@/lib/perf/instrumented-activate";

export interface CheckOutRequest {
  projectRoot: string;
  headBranch: string | null;
  prNumber: number;
  /** Workspace already standing on the branch, when there is one. */
  existingWorkspaceId?: string | null;
}

export async function checkOutPr(req: CheckOutRequest): Promise<void> {
  if (req.existingWorkspaceId) {
    await activateWorkspaceInteraction(req.existingWorkspaceId);
    return;
  }
  if (!req.headBranch) {
    throw new Error("This pull request has no head branch to check out.");
  }
  await createWorktreeWorkspace(
    req.projectRoot,
    req.headBranch,
    false,
    "single",
    null,
    null,
    null,
    req.prNumber,
  );
}
