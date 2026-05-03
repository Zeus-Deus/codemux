import { useEffect, useRef } from "react";
import { useAppStore, useHomeDir } from "@/stores/app-store";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { hasAnyPane } from "@/lib/pane-tree";
import { agentChatCreatePane } from "@/tauri/commands";

/**
 * Ensure something is showing for the active workspace whenever the
 * user has nothing else to look at.
 *
 * Three-way dispatch when no draft is active and the active workspace
 * (or lack thereof) has no real panes to resume:
 *   - No active workspace → auto-create a Home draft.
 *   - Active workspace is Home-rooted (`project_root === homeDir`) →
 *     auto-create a Home draft (so "What should we do today?" replaces
 *     the empty-workspace splash).
 *   - Active workspace is a project workspace → auto-spawn an
 *     `agent_chat` pane instead. Dropping the user onto a Home draft
 *     that doesn't belong to this workspace would be confusing and
 *     breaks "messages go to the workspace I clicked."
 *
 * Runs only when `enable_agent_chat` + `enable_lazy_workspace_creation`
 * are both on (and flags have loaded). Idempotent:
 *  - Once `activeDraftId` becomes non-null, the early return up top
 *    stops the effect from running again until the draft clears.
 *  - The pane-spawn branch guards with an in-flight ref so repeated
 *    effect fires while the Tauri call is pending don't double-spawn.
 */
export function useEnsureDraftWhenEmpty() {
  const appState = useAppStore((s) => s.appState);
  const homeDir = useHomeDir();
  const enableAgentChat = useFeatureFlags((s) => s.enableAgentChat);
  const enableLazyWorkspaceCreation = useFeatureFlags(
    (s) => s.enableLazyWorkspaceCreation,
  );
  const flagsLoaded = useFeatureFlags((s) => s.loaded);
  const activeDraftId = useChatDraftStore((s) => s.activeDraftId);

  // Tracks the workspace_id we most recently kicked off an
  // `agentChatCreatePane` call for, so rapid state updates don't
  // fan out into multiple pane-spawn requests for the same empty
  // workspace. Cleared on failure so a retry can fire.
  const inFlightSpawnRef = useRef<string | null>(null);

  useEffect(() => {
    if (!appState || !flagsLoaded) return;
    if (!enableAgentChat || !enableLazyWorkspaceCreation) return;
    if (activeDraftId) return;

    const activeWs = appState.workspaces.find(
      (w) => w.workspace_id === appState.active_workspace_id,
    );
    if (activeWs) {
      const activeSurface = activeWs.surfaces.find(
        (s) => s.surface_id === activeWs.active_surface_id,
      );
      if (activeSurface && hasAnyPane(activeSurface.root)) return;

      // Empty workspace. If we can confidently say this is a project
      // workspace (not Home-rooted), inject an agent_chat pane rather
      // than falling back to the Home draft — the user clicked a
      // specific workspace, their next message should land on it.
      // When `homeDir` is still null (boot hasn't resolved it yet) we
      // defer to the legacy Home-draft path for safety.
      const isProjectWorkspace =
        homeDir !== null &&
        (activeWs.project_root ?? activeWs.cwd) !== homeDir;
      if (isProjectWorkspace) {
        if (inFlightSpawnRef.current === activeWs.workspace_id) return;
        inFlightSpawnRef.current = activeWs.workspace_id;
        agentChatCreatePane(activeWs.workspace_id, "claude", null).catch(
          (err) => {
            console.error(
              "[ensure-draft] failed to auto-spawn agent_chat pane:",
              err,
            );
            inFlightSpawnRef.current = null;
          },
        );
        return;
      }
    }

    const store = useChatDraftStore.getState();
    const draft = store.getOrCreateHomeDraft();
    store.setActiveDraft(draft.draftId);
  }, [
    appState,
    homeDir,
    enableAgentChat,
    enableLazyWorkspaceCreation,
    flagsLoaded,
    activeDraftId,
  ]);
}
