import {
  activateWorkspace,
  agentChatCreatePane,
  getHomeDir,
  getOrCreateHomeWorkspace,
} from "@/tauri/commands";
import { useAppStore } from "@/stores/app-store";
import { findChatPane } from "./pane-tree";

/**
 * Activate the singleton Home workspace, creating it if absent, and
 * ensure a chat pane exists on its active surface (spawns one if the
 * active surface has no chat pane yet — caller-side idempotency).
 *
 * Shared between sidebar-header "+" and the pinned sidebar-home-row.
 */
export async function openHomeChat(): Promise<string> {
  const wsId = await getOrCreateHomeWorkspace();
  await activateWorkspace(wsId);

  const snapshot = useAppStore.getState().appState;
  const ws = snapshot?.workspaces.find((w) => w.workspace_id === wsId);
  const activeSurface = ws?.surfaces.find(
    (s) => s.surface_id === ws.active_surface_id,
  );
  const existing = activeSurface ? findChatPane(activeSurface.root) : null;

  if (!existing) {
    const homeDir = await getHomeDir();
    await agentChatCreatePane(wsId, null, homeDir);
  }

  return wsId;
}
