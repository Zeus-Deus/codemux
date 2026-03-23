import { useEffect } from "react";
import { splitPane, closePane, createTab, closeTab } from "@/tauri/commands";
import { useAppStore } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";

export function useKeyboardShortcuts() {
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      // Escape closes settings
      if (e.key === "Escape" && useUIStore.getState().showSettings) {
        e.preventDefault();
        useUIStore.getState().setShowSettings(false);
        return;
      }

      const appState = useAppStore.getState().appState;
      if (!appState) return;

      const ws = appState.workspaces.find(
        (w) => w.workspace_id === appState.active_workspace_id,
      );
      if (!ws) return;

      const surface = ws.surfaces.find(
        (s) => s.surface_id === ws.active_surface_id,
      );
      const activePaneId = surface?.active_pane_id;

      // Ctrl+Shift+D — split active pane right
      if (e.ctrlKey && e.shiftKey && e.key === "D") {
        e.preventDefault();
        if (activePaneId)
          splitPane(activePaneId, "horizontal").catch(console.error);
      }

      // Ctrl+Shift+W — close active pane
      if (e.ctrlKey && e.shiftKey && e.key === "W") {
        e.preventDefault();
        if (activePaneId) closePane(activePaneId).catch(console.error);
      }

      // Ctrl+T — new terminal tab
      if (e.ctrlKey && !e.shiftKey && e.key === "t") {
        e.preventDefault();
        createTab(ws.workspace_id, "terminal").catch(console.error);
      }

      // Ctrl+W — close active tab (only if more than one tab)
      if (e.ctrlKey && !e.shiftKey && e.key === "w") {
        e.preventDefault();
        if (ws.tabs.length > 1)
          closeTab(ws.workspace_id, ws.active_tab_id).catch(console.error);
      }
    };

    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);
}
