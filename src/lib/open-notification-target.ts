import { activateWorkspaceInteraction } from "@/lib/perf/instrumented-activate";
import { toast } from "@/lib/toast";
import { useAppStore } from "@/stores/app-store";
import { useMobileNavigationStore } from "@/stores/mobile-navigation-store";
import { activatePane } from "@/tauri/commands";

/** Where an agent notification leads. Snake_case to match the backend
 *  payloads in `src-tauri/src/notifications.rs`. */
export interface NotificationTargetIds {
  workspace_id: string;
  pane_id: string;
}

/**
 * Open the workspace and pane an agent notification is about, the same way a
 * sidebar click would. Shared by the native notification click (desktop) and
 * the toast / Web Notification on a remote client.
 */
export async function openNotificationTarget({
  workspace_id,
  pane_id,
}: NotificationTargetIds): Promise<void> {
  const exists = useAppStore
    .getState()
    .appState?.workspaces.some((w) => w.workspace_id === workspace_id);
  if (!exists) {
    toast.info("This workspace is no longer open.");
    return;
  }
  try {
    await activateWorkspaceInteraction(workspace_id);
    if (pane_id) await activatePane(pane_id);
    // Mobile shows the workspace list until something is opened.
    useMobileNavigationStore.getState().setHome(false);
  } catch (error) {
    toast.error("Could not open the agent", { description: String(error) });
  }
}
