import { useCallback, useState } from "react";
import { initGitRepoNoCommit, refreshWorkspaceGitInfo } from "@/tauri/commands";
import { toast } from "@/lib/toast";
import type { WorkspaceSnapshot } from "@/tauri/types";

/**
 * Non-git projects are first-class in Codemux: any folder can be opened
 * as a project and runs in plain-directory mode (no worktrees, diffs, or
 * checkpoints). These helpers power the explicit, opt-in "Initialize
 * Git" affordance that replaces the old silent degradation — we never
 * `git init` a user's folder without a click.
 */

/** Whether the "no git" state + "Initialize Git" affordance applies to
 *  this workspace. True only for local standard project workspaces whose
 *  directory is known to not be a git repo:
 *
 *  - `is_git === false` exactly — `undefined` (older snapshots persisted
 *    before the field existed) stays optimistic-true, matching the Rust
 *    serde default, so real repos never flash the affordance.
 *  - standard workspaces only — Home ($HOME) is not a project and should
 *    not be nudged toward `git init`.
 *  - local only — for attach-in-place (`attach_only`) and host-backed
 *    workspaces the cwd is a host path; the local `is_git` probe is
 *    meaningless there.
 */
export function showNoGitState(
  workspace: Pick<
    WorkspaceSnapshot,
    | "is_git"
    | "workspace_type"
    | "attach_only"
    | "host_id"
    | "project_root"
    | "cwd"
  >,
  homeDir: string | null,
): boolean {
  const projectRoot = workspace.project_root ?? workspace.cwd;

  return (
    homeDir !== null &&
    projectRoot !== homeDir &&
    workspace.is_git === false &&
    workspace.workspace_type === "standard" &&
    !workspace.attach_only &&
    (workspace.host_id === null || workspace.host_id === undefined)
  );
}

/** Run `git init` on the workspace's project folder, then refresh its
 *  git info so `is_git` flips (and the git UI lights up) immediately
 *  instead of waiting for the polling loop. */
export function useInitializeGit(workspace: WorkspaceSnapshot | null) {
  const [initializing, setInitializing] = useState(false);

  const initialize = useCallback(async () => {
    if (!workspace || initializing) return;
    setInitializing(true);
    try {
      await initGitRepoNoCommit(workspace.project_root ?? workspace.cwd);
      await refreshWorkspaceGitInfo(workspace.workspace_id);
    } catch (err) {
      toast.error(`Git initialization failed: ${err}`);
    } finally {
      setInitializing(false);
    }
  }, [workspace, initializing]);

  return { initialize, initializing };
}
