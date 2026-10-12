import { useEffect } from "react";
import { getWorkspaceStatus } from "@/lib/pane-status";
import { useAppStore } from "@/stores/app-store";
import { useSidebarDensityStore } from "@/stores/sidebar-density-store";

/**
 * Stamp when every workspace entered its current agent status, whatever is on
 * screen. The sidebar rows record these marks as they render, but they unmount
 * when the sidebar collapses to its rail or a full-screen page takes over, and
 * "jump to the workspace that needs you" ranks blocked workspaces by these
 * marks. Without an always-on observer, a workspace that blocked while the
 * inbox was hidden would rank as just blocked, or keep an old mark from a
 * previous block.
 *
 * Subscribes outside React so status churn never re-renders the caller.
 * `observeStatus` ignores unchanged statuses, so this agrees with (and never
 * resets) the marks the mounted rows write.
 */
export function useWorkspaceStatusTracking(): void {
  useEffect(() => {
    let seenWorkspaces: unknown = null;
    let seenStatuses: unknown = null;
    const sync = (state: ReturnType<typeof useAppStore.getState>) => {
      const app = state.appState;
      if (!app) return;
      if (app.workspaces === seenWorkspaces && app.pane_statuses === seenStatuses) return;
      seenWorkspaces = app.workspaces;
      seenStatuses = app.pane_statuses;
      const { observeStatus } = useSidebarDensityStore.getState();
      for (const ws of app.workspaces) {
        observeStatus(ws.workspace_id, getWorkspaceStatus(ws.surfaces, app.pane_statuses));
      }
    };
    sync(useAppStore.getState());
    return useAppStore.subscribe(sync);
  }, []);
}
