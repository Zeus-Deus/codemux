import { useEffect, useRef } from "react";
import { useAppStore, useHomeDir } from "@/stores/app-store";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { hasAnyPane } from "@/lib/pane-tree";
import { agentChatCreatePane } from "@/tauri/commands";

/**
 * Primitive-summary selector for the bits of `appState` this hook
 * actually depends on. Returning a string fingerprint means the effect
 * below only re-runs when one of the four relevant fields changes —
 * not on every backend `app-state-changed` tick (agent tokens, git
 * polls, hook events). Without this, the effect body would walk the
 * surface tree on every tick under heavy load.
 */
function selectEmptyWorkspaceFingerprint(
  s: { appState: ReturnType<typeof useAppStore.getState>["appState"] },
): string {
  const app = s.appState;
  if (!app) return "no-app-state";
  const wsId = app.active_workspace_id;
  if (!wsId) return "no-active-ws";
  const ws = app.workspaces.find((w) => w.workspace_id === wsId);
  if (!ws) return `ws-missing:${wsId}`;
  // Whether the active surface has any panes — this is the only
  // structural fact the effect cares about.
  const activeSurface = ws.surfaces.find(
    (sf) => sf.surface_id === ws.active_surface_id,
  );
  const hasPane = activeSurface ? hasAnyPane(activeSurface.root) : false;
  // Project_root vs cwd drives the Home-draft-vs-pane branch.
  const projectRoot = ws.project_root ?? ws.cwd;
  return `${wsId}|${hasPane ? "1" : "0"}|${projectRoot}`;
}

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
  // Subscribe to a primitive string fingerprint of the four fields
  // this effect actually depends on, NOT the whole appState. Without
  // this, the effect body re-ran on every backend `app-state-changed`
  // tick (many per second under load) walking surface trees and doing
  // workspace lookups for nothing. The audit pass found this pattern;
  // see `selectEmptyWorkspaceFingerprint` above.
  const fingerprint = useAppStore(selectEmptyWorkspaceFingerprint);
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
    if (!flagsLoaded) return;
    if (!enableAgentChat || !enableLazyWorkspaceCreation) return;
    if (activeDraftId) return;

    // Re-read the live snapshot at effect-fire time. The fingerprint
    // dep above ensures we only get here when the relevant slice
    // changed, but we still need the full workspace object to read
    // surfaces / surface ids.
    const appState = useAppStore.getState().appState;
    if (!appState) return;

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
    fingerprint,
    homeDir,
    enableAgentChat,
    enableLazyWorkspaceCreation,
    flagsLoaded,
    activeDraftId,
  ]);
}
