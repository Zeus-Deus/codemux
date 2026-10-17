import { useCallback, useEffect, useState } from "react";

import { reportFailure } from "@/lib/toast";
import { selectActiveWorkspaceId, useAppStore } from "@/stores/app-store";
import { useDetectedEditors } from "@/stores/editor-discovery-store";
import {
  selectDefaultEditor,
  useSyncedSettingsStore,
} from "@/stores/synced-settings-store";
import { openInEditor } from "@/tauri/commands";

/**
 * "Open the active workspace in an editor": the detected editors, the
 * user's default (seeded with the first detected one), and an `open` that
 * also makes the chosen editor the new default. Shared by the legacy title
 * bar's launcher and the Workspace details card.
 */
export function useEditorLauncher() {
  const editors = useDetectedEditors();
  const [isLoading, setIsLoading] = useState(false);
  const persistedEditor = useSyncedSettingsStore(selectDefaultEditor);
  const workspacePath = useAppStore(
    (s) =>
      s.appState?.workspaces.find(
        (w) => w.workspace_id === selectActiveWorkspaceId(s),
      )?.cwd,
  );

  useEffect(() => {
    if (
      editors.length > 0 &&
      !persistedEditor &&
      !useSyncedSettingsStore.getState().isLoading
    ) {
      void useSyncedSettingsStore
        .getState()
        .updateSetting("editor", "default_ide", editors[0].id);
    }
  }, [editors, persistedEditor]);

  const defaultEditorId =
    persistedEditor || (editors.length > 0 ? editors[0].id : null);
  const defaultEditor = editors.find((e) => e.id === defaultEditorId);

  const open = useCallback(
    (editorId: string) => {
      if (!workspacePath || isLoading) return;
      setIsLoading(true);
      useSyncedSettingsStore
        .getState()
        .updateSetting("editor", "default_ide", editorId);
      const editorName =
        editors.find((e) => e.id === editorId)?.name ?? "the editor";
      openInEditor(editorId, workspacePath)
        .catch(reportFailure(`Couldn't open in ${editorName}`))
        .finally(() => setIsLoading(false));
    },
    [workspacePath, isLoading, editors],
  );

  return {
    editors,
    defaultEditor,
    defaultEditorId,
    workspacePath: workspacePath ?? null,
    isLoading,
    open,
    /** Nothing to open, or nothing to open it with. */
    unavailable: editors.length === 0 || !workspacePath,
  };
}
