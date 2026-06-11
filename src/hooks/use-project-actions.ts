import { useCallback } from "react";
import {
  checkIsGitRepo,
  initGitRepo,
  dbAddRecentProject,
  gitCloneRepo,
  createEmptyWorkspace,
  activateWorkspace,
} from "@/tauri/commands";
import { pickFolder } from "@/lib/file-dialog";
import { useAppStore } from "@/stores/app-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { useUIStore } from "@/stores/ui-store";
import { basename } from "@/lib/path";

/** True when the first-project legacy onboarding wizard should be
 *  suppressed. Two reasons fire it:
 *
 *  1. The user already saw and dismissed onboarding (`hasSeenOnboarding`
 *     is sticky on `useUIStore`). Closing every workspace or opening a
 *     second project must NOT re-arm the wizard — that's the re-trap
 *     fix from main.
 *  2. The user opted into the chat-agent + lazy-workspace-creation
 *     flow, in which case `useEnsureDraftWhenEmpty` auto-spawns an
 *     agent_chat pane on the freshly created empty project workspace
 *     instead, giving the user the chat UI directly.
 *
 *  Both reads are synchronous via `getState()` so action callbacks
 *  don't need to subscribe.
 */
function shouldSkipOnboarding(): boolean {
  if (useUIStore.getState().hasSeenOnboarding) return true;
  const flags = useFeatureFlags.getState();
  return flags.enableAgentChat && flags.enableLazyWorkspaceCreation;
}

interface OpenProjectResult {
  success: boolean;
  path?: string;
  name?: string;
}

export function useProjectActions() {
  const setShowCloneDialog = useUIStore((s) => s.setShowCloneDialog);

  const openProject = useCallback(async (): Promise<OpenProjectResult> => {
    const folder = await pickFolder("Open project");
    if (!folder) return { success: false };

    const name = basename(folder);
    const isGit = await checkIsGitRepo(folder);

    if (!isGit) {
      const confirmed = window.confirm(
        `"${name}" is not a git repository. Initialize one?`,
      );
      if (!confirmed) return { success: false };
      await initGitRepo(folder);
    }

    await dbAddRecentProject(folder, name);

    // Only show the onboarding wizard for truly first-time users — no existing
    // workspaces AND they have never seen/dismissed onboarding before. The
    // `hasSeenOnboarding` flag persists so that closing all workspaces or
    // opening a second project doesn't re-arm the wizard.
    const hasWorkspaces = (useAppStore.getState().appState?.workspaces.length ?? 0) > 0;
    const wsId = await createEmptyWorkspace(folder);
    await activateWorkspace(wsId);
    if (!hasWorkspaces && !shouldSkipOnboarding()) {
      useUIStore.getState().setOnboardingProjectDir(folder);
    }

    return { success: true, path: folder, name };
  }, []);

  const openCloneDialog = useCallback(() => {
    setShowCloneDialog(true);
  }, [setShowCloneDialog]);

  const cloneProject = useCallback(
    async (url: string, targetDir: string) => {
      const clonedPath = await gitCloneRepo(url, targetDir);
      const name = basename(clonedPath);
      await dbAddRecentProject(clonedPath, name);

      const hasWorkspaces = (useAppStore.getState().appState?.workspaces.length ?? 0) > 0;
      const wsId = await createEmptyWorkspace(clonedPath);
      await activateWorkspace(wsId);
      if (!hasWorkspaces && !shouldSkipOnboarding()) {
        useUIStore.getState().setOnboardingProjectDir(clonedPath);
      }

      return { path: clonedPath, name };
    },
    [],
  );

  return { openProject, openCloneDialog, cloneProject };
}
