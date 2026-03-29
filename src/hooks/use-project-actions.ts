import { useCallback } from "react";
import {
  pickFolderDialog,
  checkIsGitRepo,
  initGitRepo,
  dbAddRecentProject,
  gitCloneRepo,
  createEmptyWorkspace,
  activateWorkspace,
} from "@/tauri/commands";
import { useUIStore } from "@/stores/ui-store";

interface OpenProjectResult {
  success: boolean;
  path?: string;
  name?: string;
}

export function useProjectActions() {
  const setShowCloneDialog = useUIStore((s) => s.setShowCloneDialog);

  const openProject = useCallback(async (): Promise<OpenProjectResult> => {
    const folder = await pickFolderDialog("Open project");
    if (!folder) return { success: false };

    const name = folder.split("/").filter(Boolean).pop() || folder;
    const isGit = await checkIsGitRepo(folder);

    if (!isGit) {
      const confirmed = window.confirm(
        `"${name}" is not a git repository. Initialize one?`,
      );
      if (!confirmed) return { success: false };
      await initGitRepo(folder);
    }

    await dbAddRecentProject(folder, name);

    // Create a temporary workspace so the project appears in the sidebar,
    // then show the onboarding wizard in the content area.
    const wsId = await createEmptyWorkspace(folder);
    await activateWorkspace(wsId);
    useUIStore.getState().setOnboardingProjectDir(folder);

    return { success: true, path: folder, name };
  }, []);

  const openCloneDialog = useCallback(() => {
    setShowCloneDialog(true);
  }, [setShowCloneDialog]);

  const cloneProject = useCallback(
    async (url: string, targetDir: string) => {
      const clonedPath = await gitCloneRepo(url, targetDir);
      const name = clonedPath.split("/").filter(Boolean).pop() || clonedPath;
      await dbAddRecentProject(clonedPath, name);

      // Create a temporary workspace so the project appears in the sidebar,
      // then show the onboarding wizard in the content area.
      const wsId = await createEmptyWorkspace(clonedPath);
      await activateWorkspace(wsId);
      useUIStore.getState().setOnboardingProjectDir(clonedPath);

      return { path: clonedPath, name };
    },
    [],
  );

  return { openProject, openCloneDialog, cloneProject };
}
